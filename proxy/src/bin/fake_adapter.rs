use std::env;
use std::io::{self, BufRead, BufReader, Write};
use std::sync::atomic::{AtomicI64, Ordering};
use serde_json::{json, Value};
const SET_BREAKPOINTS_BODY: &str =
    include_str!("../../../../fixtures/dap/set_breakpoints_response_body_unknown_field.json");
const OUTPUT_BODY: &str = include_str!("../../../../fixtures/dap/output_event_body.json");
const BREAKPOINT_BODY: &str = include_str!("../../../../fixtures/dap/breakpoint_event_body.json");
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
static SEQ: AtomicI64 = AtomicI64::new(0);
static WIRE: std::sync::OnceLock<std::sync::Mutex<Wire>> = std::sync::OnceLock::new();
static PENDING_LAUNCH: std::sync::Mutex<Option<i64>> = std::sync::Mutex::new(None);
struct Wire {
    engine_log: Option<std::fs::File>,
}
impl Wire {
    fn emit(&mut self, message: &Value) {
        let stdout = io::stdout();
        let mut out = stdout.lock();
        write_frame(&mut out, message);
        if let Some(log) = &mut self.engine_log {
            writeln!(log, "-> {message}").expect("fake adapter: engine log write");
        }
    }
    fn log_incoming(&mut self, message: &Value) {
        if let Some(log) = &mut self.engine_log {
            writeln!(log, "<- {message}").expect("fake adapter: engine log write");
        }
    }
}
fn wire_emit(message: &mut Value) {
    let mut wire = WIRE.get_or_init(|| std::sync::Mutex::new(Wire { engine_log: None })).lock().expect("fake adapter: wire lock poisoned");
    message["seq"] = json!(SEQ.fetch_add(1, Ordering::Relaxed) + 1);
    wire.emit(message);
}
fn wire_log_incoming(message: &Value) {
    let mut wire = WIRE.get_or_init(|| std::sync::Mutex::new(Wire { engine_log: None })).lock().expect("fake adapter: wire lock poisoned");
    wire.log_incoming(message);
}
#[cfg(windows)]
fn death_code(status: std::process::ExitStatus) -> i64 {
    status.code().map(|code| code as i64).unwrap_or(1)
}
#[cfg(not(windows))]
fn death_code(status: std::process::ExitStatus) -> i64 {
    use std::os::unix::process::ExitStatusExt;
    status.code().map(|code| code as i64).unwrap_or_else(|| 128 + status.signal().unwrap_or(0) as i64)
}
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
        return;
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
    let kills_target = !env::var("FAKE_ATTACHED_KILLS_TARGET").unwrap_or_default().is_empty();
    let refuse_step = env::var("FAKE_REFUSE_STEP").unwrap_or_default();
    let startup_ms = env::var("FAKE_DEBUGGEE_STARTUP_MS").unwrap_or_default();
    let mut attached_pid: Option<u32> = None;
    while let Some(msg) = read_frame(&mut reader).expect("fake adapter: framing error") {
        wire_log_incoming(&msg);
        let command = msg.get("command").and_then(Value::as_str).unwrap_or("").to_string();
        let request_seq = msg.get("seq").and_then(Value::as_i64).unwrap_or(0);
        if command == "attach" {
            attached_pid = msg
                .get("arguments")
                .and_then(|a| a.get("processId"))
                .and_then(Value::as_u64)
                .map(|pid| pid as u32);
        }
        if !exit_on.is_empty() && exit_on == command {
            if kills_target {
                if let Some(pid) = attached_pid {
                    kill_process(pid);
                }
            }
            std::process::exit(exit_code);
        }
        if !refuse_step.is_empty() && matches!(command.as_str(), "next" | "stepIn" | "stepOut") {
            wire_emit(&mut json!({
                "type": "response", "request_seq": request_seq,
                "command": command, "success": false,
                "message": format!("Failed command '{command}' : 0x80004005"),
            }));
            continue;
        }
        if !startup_ms.is_empty() {
            if command == "launch" {
                let window: u64 = startup_ms
                    .parse()
                    .expect("FAKE_DEBUGGEE_STARTUP_MS must be milliseconds");
                *PENDING_LAUNCH
                    .lock()
                    .expect("fake adapter: pending-launch lock poisoned") = Some(request_seq);
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(window));
                    if let Some(seq) = PENDING_LAUNCH
                        .lock()
                        .expect("fake adapter: pending-launch lock poisoned")
                        .take()
                    {
                        wire_emit(&mut json!({
                            "type": "response", "request_seq": seq,
                            "command": "launch", "success": true, "body": {},
                        }));
                    }
                });
                continue;
            }
            if command == "configurationDone" {
                if let Some(seq) = PENDING_LAUNCH
                    .lock()
                    .expect("fake adapter: pending-launch lock poisoned")
                    .take()
                {
                    wire_emit(&mut json!({
                        "type": "response", "request_seq": seq,
                        "command": "launch", "success": false,
                        "message": "Command execution timed out.",
                    }));
                }
            }
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
            std::thread::sleep(std::time::Duration::from_millis(200));
            wire_emit(&mut json!({ "type": "event", "event": "exited",
                                   "body": { "exitCode": ended_code } }));
            wire_emit(&mut json!({ "type": "event", "event": "terminated" }));
        }
        if !spawn_child_on.is_empty() && spawn_child_on == command {
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
                    std::process::exit(0);
                }
                wire_emit(&mut json!({ "type": "event", "event": "exited",
                                       "body": { "exitCode": code } }));
                wire_emit(&mut json!({ "type": "event", "event": "terminated" }));
            });
        }
        if command == "disconnect" {
            if kills_target {
                if let Some(pid) = attached_pid {
                    kill_process(pid);
                }
            }
            std::process::exit(0);
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
