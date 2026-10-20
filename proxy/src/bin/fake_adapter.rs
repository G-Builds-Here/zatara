//! Scripted stand-in for a netcoredbg adapter child (test infrastructure —
//! never built for shipping targets, see `required-features` in Cargo.toml).
//!
//! Speaks the same Content-Length framed DAP over stdio that netcoredbg
//! speaks in `--interpreter=vscode` mode, replaying the spec-modelled bodies
//! from `fixtures/dap/` so the relay can be asserted field-for-field in CI
//! with no netcoredbg binary present (fixture provenance: that directory's
//! README — these are DAP-spec models, not live captures).
//!
//! Script controls (env, read once at startup):
//! - `FAKE_EXIT_ON_CMD=<command>`   — exit before answering that request
//!   (child dies with a request in flight).
//! - `FAKE_EXIT_AFTER_CMD=<command>` — exit right after answering it.
//! - `FAKE_EXIT_CODE=<n>`           — exit code for both (default 0).
//! - `FAKE_ECHO_ARGS=<command>`     — answer that request with its own
//!   arguments as the response body (child-side echo: proves what actually
//!   arrived through the relay, after envelope rewriting).
//! - `FAKE_STOPPED_REASON=<reason>` — the `stopped` event's reason (default
//!   `step`; a gutter breakpoint session scripts `breakpoint`).
//! - `FAKE_ENDED_AFTER_CMD=<command>` — 200 ms after answering that request,
//!   emit the debuggee-done pair (`exited` then `terminated`) and KEEP
//!   SERVING: what a real netcoredbg does when its debuggee dies (repro6
//!   probe 2026-10-13), scripted so the settle gate's ordering — summary
//!   before session-end — is assertable without a real testhost.
//! - `FAKE_ENDED_EXIT_CODE=<n>`       — the code the ended pair's `exited`
//!   carries (default 0 — a clean debuggee death).
//! - `FAKE_SPAWN_CHILD_ON_CMD=<command>` — right after answering that
//!   request, spawn a real child: another instance of this binary with
//!   `FAKE_CHILD=1` — a naive sleeper with NO parent-death watchdog, the
//!   shape of a debuggee that stays alive and observable until somebody
//!   actually kills it (the orphan oracle a self-cleaning fixture would
//!   fake green). `FAKE_CHILD_PID_FILE=<file>` records its pid.
//! - `FAKE_CHILD_EXIT_FILE=<file>`    — when that child dies (whatever
//!   killed it), record its exit code (one integer, written BEFORE any
//!   death announcement) — the child-side truth for code-preservation
//!   claims across the relay.
//! - `FAKE_SILENT_ON_CHILD_DEATH=1`   — when that child dies, exit with no
//!   debuggee-done pair at all: the wedged-crasher whose silence the
//!   relay's D11 synthesis must cover. Default (unset): announce
//!   `exited(recorded code)` then `terminated` and keep serving — the
//!   netcoredbg shape of a debuggee death.
//! - `FAKE_ARGV_LOG=<file>`         — record argv (one arg per line) before
//!   serving: the child-side truth of what the relay actually spawned with.
//! - `FAKE_PID_LOG=<file>`          — write this process's pid (one line) at
//!   startup: the oracle for "no adapter child left running" claims.
//! - `FAKE_ATTACHED_KILLS_TARGET=1` — remember the pid an `attach` request
//!   names, and kill it when the debugger goes: on the scripted
//!   `disconnect` exit and on `FAKE_EXIT_ON_CMD`/`FAKE_EXIT_AFTER_CMD`
//!   death. Models the recorded debuggee disposition of the LOC-0134 probe
//!   (rows 10–12, pinned netcoredbg build): disconnect kills the
//!   attached-to process, and a debugger that dies mid-session takes it
//!   down with it.
//! - `FAKE_EMIT_OUTPUT_AFTER_CMD=<command>` + `FAKE_OUTPUT_LINES_FILE=<file>`
//!   — after answering that request, stream the file's bytes as one `output`
//!   event per newline-terminated line — each event's `output` carries its
//!   trailing newline, byte-for-byte — then keep serving: the shape of a
//!   debuggee's raw stdout arriving over the wire as DAP output events
//!   (LOC-0133's measured event sequence).
//!
//! Adapter argv honored (netcoredbg fidelity — LOC-0117):
//! - `--engineLogging=<file>`       — real netcoredbg dumps the full DAP
//!   wire traffic to that file as `<-`/`->` JSONL when given this flag
//!   (research findings §4); the fake reproduces exactly that, so an enabled
//!   engine-logging gate can be asserted on the file's existence and shape
//!   without a netcoredbg binary. Unhonored flags are still ignored, as with
//!   every other argv the proxy today forwards.
//!
//! Scripted event stream (mirrors what a real netcoredbg session emits):
//! `setBreakpoints` → `breakpoint` event; `configurationDone` → `stopped`
//! event; `continue` → `continued` event; `next` → `output` event. A
//! `disconnect` request is answered, then the fake exits — real netcoredbg
//! terminates its interpreter after processing disconnect.
//!
//! The framing codec here is deliberately local: test-harness code stays
//! self-contained so a production-codec bug can never mask itself with an
//! identical bug in the harness. The production codec lives in the proxy lib.

use std::env;
use std::io::{self, BufRead, BufReader, Write};
use std::sync::atomic::{AtomicI64, Ordering};
use serde_json::{json, Value};

const SET_BREAKPOINTS_BODY: &str =
    include_str!("../../../../fixtures/dap/set_breakpoints_response_body_unknown_field.json");
const OUTPUT_BODY: &str = include_str!("../../../../fixtures/dap/output_event_body.json");
const BREAKPOINT_BODY: &str = include_str!("../../../../fixtures/dap/breakpoint_event_body.json");

/// Read one `Content-Length` framed body and parse it as JSON.
fn read_frame(reader: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut header: Vec<u8> = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let n = reader.read(&mut byte)?;
        if n == 0 {
            return Ok(None);
        }
        header.push(byte[0]);
        if header.ends_with(b"\r\n\r\n") || header.ends_with(b"\n\n") {
            break;
        }
        if header.len() > 8192 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "header block too long"));
        }
    }
    let mut length: Option<usize> = None;
    for line in String::from_utf8_lossy(&header).split('\n') {
        if let Some((key, value)) = line.trim().split_once(':') {
            if key.trim().eq_ignore_ascii_case("content-length") {
                length = value.trim().parse().ok();
            }
        }
    }
    let length = length.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "no Content-Length"))?;
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body)?;
    Ok(Some(serde_json::from_slice(&body).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?))
}

fn write_frame(out: &mut impl Write, message: &Value) {
    let body = message.to_string();
    write!(out, "Content-Length: {}\r\n\r\n{}", body.len(), body).unwrap();
    out.flush().unwrap();
}

/// Every outgoing message of this fake: one seq counter and one wire,
/// shared by the request loop and (when a scripted debuggee is planted)
/// the thread that announces its death. A death frame and a response
/// frame must never interleave mid-write, so every frame — and its
/// engine-log twin — goes out under the one `WIRE` lock, and every seq
/// is stamped under that same lock (wire order and seq order agree, as
/// a single-threaded fake's always did).
static SEQ: AtomicI64 = AtomicI64::new(0);
static WIRE: std::sync::OnceLock<std::sync::Mutex<Wire>> = std::sync::OnceLock::new();

struct Wire {
    engine_log: Option<std::fs::File>,
}

impl Wire {
    /// Send a frame and mirror it into the engine log (the `<-`/`->` JSONL
    /// real netcoredbg writes for `--engineLogging`; the fake owes the same
    /// shape — Mock Fidelity Ladder). Callers hold the `WIRE` lock.
    fn emit(&mut self, message: &Value) {
        let stdout = io::stdout();
        let mut out = stdout.lock();
        write_frame(&mut out, message);
        if let Some(log) = &mut self.engine_log {
            writeln!(log, "-> {message}").expect("fake adapter: engine log write");
        }
    }

    /// Mirror an incoming frame into the engine log (the `<-` half).
    fn log_incoming(&mut self, message: &Value) {
        if let Some(log) = &mut self.engine_log {
            writeln!(log, "<- {message}").expect("fake adapter: engine log write");
        }
    }
}

/// The one wire: the seq is stamped under the same lock the bytes leave
/// under, so wire order and seq order always agree (a single-threaded
/// fake's guarantee, kept past the death-announcement thread).
fn wire_emit(message: &mut Value) {
    let mut wire = WIRE.get_or_init(|| std::sync::Mutex::new(Wire { engine_log: None })).lock().expect("fake adapter: wire lock poisoned");
    message["seq"] = json!(SEQ.fetch_add(1, Ordering::Relaxed) + 1);
    wire.emit(message);
}

fn wire_log_incoming(message: &Value) {
    let mut wire = WIRE.get_or_init(|| std::sync::Mutex::new(Wire { engine_log: None })).lock().expect("fake adapter: wire lock poisoned");
    wire.log_incoming(message);
}

/// The exit code a dead scripted debuggee carries: the kernel's answer
/// where the OS records one; the shell convention (128 + signal) for a
/// signal death, which is the shape netcoredbg reports for those too.
#[cfg(windows)]
fn death_code(status: std::process::ExitStatus) -> i64 {
    status.code().map(|code| code as i64).unwrap_or(1)
}

#[cfg(not(windows))]
fn death_code(status: std::process::ExitStatus) -> i64 {
    use std::os::unix::process::ExitStatusExt;
    status.code().map(|code| code as i64).unwrap_or_else(|| 128 + status.signal().unwrap_or(0) as i64)
}

/// Kill `pid` outright — the fake's model of what the pinned debug stack
/// does to an attached-to process when its debugger goes (LOC-0134 probe,
/// rows 10–12). The kernel32 binding is hand-written for the same reason
/// the proxy's own liveness probe avoids a windows-sys dependency: the GNU
/// toolchains get nothing to dlltool.
#[cfg(windows)]
fn kill_process(pid: u32) {
    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(desired_access: u32, inherit_handle: i32, process_id: u32) -> *mut u8;
        fn TerminateProcess(process: *mut u8, exit_code: u32) -> i32;
        fn CloseHandle(handle: *mut u8) -> i32;
    }
    const PROCESS_TERMINATE: u32 = 0x0001;
    let handle = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) };
    if handle.is_null() {
        return; // already gone — the disposition this models already happened
    }
    unsafe {
        TerminateProcess(handle, 1);
        CloseHandle(handle);
    }
}

#[cfg(not(windows))]
fn kill_process(pid: u32) {
    let _ = unsafe { libc::kill(pid as i32, libc::SIGKILL) };
}

fn main() {
    // The naive debuggee (a second instance of this binary planted by
    // FAKE_SPAWN_CHILD_ON_CMD): nothing but a sleep until someone kills
    // it. No watchdog, no witness, no serving — an orphan the proxy
    // failed to contain stays alive here, observable to a process scan.
    if env::var_os("FAKE_CHILD").is_some() {
        std::thread::sleep(std::time::Duration::from_secs(3600));
        return;
    }

    let stdin = io::stdin();
    let mut reader = BufReader::new(stdin.lock());

    let argv: Vec<String> = std::env::args().skip(1).collect();
    if let Ok(spy) = env::var("FAKE_ARGV_LOG") {
        let mut file = std::fs::File::create(&spy).expect("fake adapter: argv spy creatable");
        for arg in &argv {
            writeln!(file, "{arg}").expect("fake adapter: argv spy writable");
        }
    }
    if let Ok(path) = env::var("FAKE_PID_LOG") {
        let mut file = std::fs::File::create(&path).expect("fake adapter: pid log creatable");
        writeln!(file, "{}", std::process::id()).expect("fake adapter: pid log writable");
    }
    // Netcoredbg fidelity (LOC-0117): the engine-logging flag means wire
    // traffic starts landing in that file this instant (create+truncate, as
    // the real adapter opens its engine log per run). The file joins the
    // shared wire — a death announcement written by the debuggee-watcher
    // thread owes the same `->` mirror as a loop-emitted response.
    let engine_log: Option<std::fs::File> = argv
        .iter()
        .find_map(|arg| arg.strip_prefix("--engineLogging="))
        .map(|path| {
            std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(path)
                .expect("fake adapter: engine log creatable")
        });
    if engine_log.is_some() {
        WIRE.get_or_init(|| std::sync::Mutex::new(Wire { engine_log: None }))
            .lock()
            .expect("fake adapter: wire lock poisoned")
            .engine_log = engine_log;
    }

    let exit_on = env::var("FAKE_EXIT_ON_CMD").unwrap_or_default();
    let exit_after = env::var("FAKE_EXIT_AFTER_CMD").unwrap_or_default();
    let exit_code: i32 = env::var("FAKE_EXIT_CODE").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let echo_args = env::var("FAKE_ECHO_ARGS").unwrap_or_default();
    let stopped_reason = env::var("FAKE_STOPPED_REASON").unwrap_or_else(|_| "step".into());
    let ended_after = env::var("FAKE_ENDED_AFTER_CMD").unwrap_or_default();
    let output_after = env::var("FAKE_EMIT_OUTPUT_AFTER_CMD").unwrap_or_default();
    let output_lines_file = env::var("FAKE_OUTPUT_LINES_FILE").unwrap_or_default();
    if output_after.is_empty() != output_lines_file.is_empty() {
        panic!("fake adapter: FAKE_EMIT_OUTPUT_AFTER_CMD and FAKE_OUTPUT_LINES_FILE are used together");
    }
    let ended_code: i64 = env::var("FAKE_ENDED_EXIT_CODE").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let spawn_child_on = env::var("FAKE_SPAWN_CHILD_ON_CMD").unwrap_or_default();
    let child_pid_file = env::var("FAKE_CHILD_PID_FILE").ok().filter(|path| !path.is_empty());
    let child_exit_file = env::var("FAKE_CHILD_EXIT_FILE").ok().filter(|path| !path.is_empty());
    let silent_on_child_death = env::var("FAKE_SILENT_ON_CHILD_DEATH").map(|value| value == "1").unwrap_or(false);
    // The recorded disposition (LOC-0134 probe, rows 10–12): the pid the
    // last `attach` aimed this debugger at, and whether the fake models
    // the debugger taking the debuggee down with it wherever the debugger
    // goes.
    let kills_target = !env::var("FAKE_ATTACHED_KILLS_TARGET").unwrap_or_default().is_empty();
    // Refusal mode (live finding 2026-10-16): netcoredbg answers
    // `next`/`step*` with E_FAIL when an on-demand pause left it on an
    // unsteppable frame. Modeled so the relay's refusal announcement has
    // an adapter to announce about.
    let refuse_step = env::var("FAKE_REFUSE_STEP").unwrap_or_default();
    let mut attached_pid: Option<u32> = None;

    while let Some(msg) = read_frame(&mut reader).expect("fake adapter: framing error") {
        wire_log_incoming(&msg);
        let command = msg.get("command").and_then(Value::as_str).unwrap_or("").to_string();
        let request_seq = msg.get("seq").and_then(Value::as_i64).unwrap_or(0);

        if command == "attach" {
            // Remember the debuggee this debugger is now aimed at — the
            // disposition is graded against it wherever the debugger dies.
            attached_pid = msg
                .get("arguments")
                .and_then(|a| a.get("processId"))
                .and_then(Value::as_u64)
                .map(|pid| pid as u32);
        }

        if !exit_on.is_empty() && exit_on == command {
            if kills_target {
                if let Some(pid) = attached_pid {
                    kill_process(pid); // dying debugger: the debuggee goes with it
                }
            }
            std::process::exit(exit_code); // die mid-request: no response, no events
        }

        if !refuse_step.is_empty() && matches!(command.as_str(), "next" | "stepIn" | "stepOut") {
            wire_emit(&mut json!({
                "type": "response", "request_seq": request_seq,
                "command": command, "success": false,
                "message": format!("Failed command '{command}' : 0x80004005"),
            }));
            continue;
        }

        let body: Value = if !echo_args.is_empty() && echo_args == command {
            msg.get("arguments").cloned().unwrap_or(Value::Null)
        } else {
            match command.as_str() {
                "initialize" => json!({ "capabilities": { "supportsConfigurationDoneRequest": true } }),
                "setBreakpoints" => {
                    serde_json::from_str(SET_BREAKPOINTS_BODY).expect("setBreakpoints fixture body")
                }
                _ => json!({}),
            }
        };
        wire_emit(&mut json!({
            "type": "response", "request_seq": request_seq,
            "command": command, "success": true, "body": body,
        }));

        match command.as_str() {
            "setBreakpoints" => {
                // A real netcoredbg announces binding with a `breakpoint`
                // event (reason new/bound) carrying the Breakpoint body —
                // the forward-compat fixture body proves undeclared fields
                // survive the relay hop on the event path too.
                wire_emit(&mut json!({ "type": "event", "event": "breakpoint",
                             "body": serde_json::from_str::<Value>(BREAKPOINT_BODY)
                                 .expect("breakpoint fixture body") }));
            }
            "configurationDone" => {
                wire_emit(&mut json!({ "type": "event", "event": "stopped",
                             "body": { "reason": stopped_reason, "threadId": 1 } }));
            }
            "continue" => {
                wire_emit(&mut json!({ "type": "event", "event": "continued",
                             "body": { "threadId": 1 } }));
            }
            "next" => {
                wire_emit(&mut json!({ "type": "event", "event": "output",
                             "body": serde_json::from_str::<Value>(OUTPUT_BODY)
                                 .expect("output fixture body") }));
            }
            _ => {}
        }

        if !output_after.is_empty() && output_after == command {
            // The raw-stdout shape (LOC-0133's measured sequence): one
            // `output` event per newline-terminated line of the file, each
            // carrying its own trailing newline. The bytes between newlines
            // are the event's `output` verbatim — no classification, no
            // filtering, no CRLF substitution: whatever interpretation the
            // relay might be tempted to add is observable here as drift.
            let raw = std::fs::read(&output_lines_file)
                .unwrap_or_else(|e| panic!("fake adapter: output lines file {output_lines_file} unreadable: {e}"));
            let mut start = 0usize;
            let mut chunks: Vec<&[u8]> = Vec::new();
            for (index, byte) in raw.iter().enumerate() {
                if *byte == b'\n' {
                    chunks.push(&raw[start..=index]);
                    start = index + 1;
                }
            }
            if start < raw.len() {
                chunks.push(&raw[start..]);
            }
            for chunk in chunks {
                wire_emit(&mut json!({ "type": "event", "event": "output",
                             "body": { "category": "stdout",
                                       "output": String::from_utf8_lossy(chunk) } }));
            }
        }

        if !ended_after.is_empty() && ended_after == command {
            // The debuggee dying under a live interpreter: netcoredbg
            // announces `exited` then `terminated` and keeps serving
            // (repro6 probe 2026-10-13) — the pair leaves here while the
            // runner's test step may still be printing its summary. The
            // pause makes the pair arrive FIRST on a proxy without the
            // settle gate, so the regression test fails without the fix.
            std::thread::sleep(std::time::Duration::from_millis(200));
            wire_emit(&mut json!({ "type": "event", "event": "exited",
                                   "body": { "exitCode": ended_code } }));
            wire_emit(&mut json!({ "type": "event", "event": "terminated" }));
        }

        if !spawn_child_on.is_empty() && spawn_child_on == command {
            // Plant a naive debuggee under this interpreter: another
            // instance of this binary (env cleared — it inherits neither
            // this session's controls nor its witnesses), its pid
            // recorded, and a thread that waits for it. Whatever kills
            // the child, its exit code is recorded FIRST, then the death
            // is announced the way netcoredbg announces one — or not at
            // all, the wedged-crasher shape D11 synthesis must cover.
            let exe = env::current_exe().expect("fake adapter: self exe known");
            let mut child = std::process::Command::new(exe)
                .env_clear()
                .env("FAKE_CHILD", "1")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .expect("fake adapter: scripted debuggee spawns");
            if let Some(path) = child_pid_file.as_ref() {
                std::fs::write(path, format!("{}\n", child.id())).expect("fake adapter: child pid recorded");
            }
            let exit_file = child_exit_file.clone();
            std::thread::spawn(move || {
                let status = child.wait().expect("fake adapter: scripted debuggee waited");
                let code = death_code(status);
                if let Some(path) = exit_file.as_ref() {
                    std::fs::write(path, format!("{code}\n")).expect("fake adapter: child exit recorded");
                }
                if silent_on_child_death {
                    std::process::exit(0); // wedged crasher: D11's synthesis takes over
                }
                wire_emit(&mut json!({ "type": "event", "event": "exited",
                                       "body": { "exitCode": code } }));
                wire_emit(&mut json!({ "type": "event", "event": "terminated" }));
            });
        }

        if command == "disconnect" {
            if kills_target {
                if let Some(pid) = attached_pid {
                    // Recorded stop disposition: on this pin the
                    // attached-to process does not outlive the disconnect,
                    // whatever terminateDebuggee said.
                    kill_process(pid);
                }
            }
            std::process::exit(0); // real netcoredbg terminates after disconnect
        }
        if !exit_after.is_empty() && exit_after == command {
            if kills_target {
                if let Some(pid) = attached_pid {
                    kill_process(pid);
                }
            }
            std::process::exit(exit_code);
        }
    }
}
