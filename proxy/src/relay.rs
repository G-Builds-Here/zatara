//! The relay core: a client (Zed) on the proxy's own stdio, a netcoredbg
//! child on piped stdio, and a seq-correlating pipe between them.
//!
//! The proxy is a DAP endpoint on both sides: every message it sends to the
//! client carries the proxy's own `seq`, and responses carry the
//! `request_seq` of the *client's* request (the child's numbering stays
//! child-side). Bodies are relayed as opaque JSON — only envelope numbers
//! are rewritten — so fields zatara-types has never seen survive the hop
//! intact (the forward-compatibility contract of the types crate).

use std::collections::HashMap;
use std::io::{self, BufReader};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use serde_json::{json, Value};
use zatara_types::{LaunchConfig, NETCOREDBG_ARG, RELEASE_WARNING_ENV, WORK_DIR_ENV};

use crate::framing::{read_frame, write_frame};
use crate::logging::{self, ProxyLog};
use crate::session::{RelaySink, Session};

/// Windows: never let the spawned adapter allocate a console window.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Which adapter binary the proxy spawns as its child, what the shipped
/// diagnostics gates (LOC-0117) resolved to, and — on the macOS discovery
/// path (LOC-0116) — why there may be no binary to spawn at all.
pub struct AdapterSpec {
    program: PathBuf,
    args: Vec<String>,
    /// `Some` when discovery proved no netcoredbg exists: `serve` answers
    /// the client with this guidance instead of spawning, and nothing else
    /// in this struct is consulted.
    missing_reason: Option<String>,
    /// The proxy-log gate's single already-gated sink (`None` = no sink).
    /// The child's argv is the engine gate's only effect (netcoredbg writes
    /// its own wire traffic); nothing else is a sink.
    proxy_log_path: Option<PathBuf>,
}

impl AdapterSpec {
    /// The spec as the process presents it: argv first (the extension hands
    /// the resolved path there), `ZATARA_NETCOREDBG` next (the seam tests
    /// and CI inject through), and when nothing names the binary explicitly
    /// the platform decides — elsewhere that is plain PATH lookup by the OS
    /// (`netcoredbg` as program), on the macOS discovery branch the
    /// settings→PATH→Homebrew probe of [`crate::macos_netcoredbg`]. The work
    /// dir and the two default-off gates are read here too (LOC-0117) —
    /// `from_parts` stays pure.
    pub fn from_env() -> Self {
        let work_dir = std::env::var_os(WORK_DIR_ENV);
        let engine_gate = std::env::var(logging::ENGINE_LOGGING_ENV).ok();
        let proxy_gate = std::env::var(logging::PROXY_LOG_ENV).ok();
        // Same usable-work-dir predicate the resolver applies: an empty
        // value is no location, so its armed gate gets the same staying-off
        // note as a missing one.
        if logging::usable_work_dir(work_dir.as_deref()).is_none() {
            // An armed gate with no sanctioned location stays off — say so
            // on stderr (never a file the user didn't ask for), and name the
            // carrier so an operator sees the one-line fix.
            for (name, gate) in [
                (logging::ENGINE_LOGGING_ENV, engine_gate.as_deref()),
                (logging::PROXY_LOG_ENV, proxy_gate.as_deref()),
            ] {
                if gate == Some("1") {
                    eprintln!(
                        "zatara-proxy: {name} is on but no {WORK_DIR_ENV} is known — staying off; diagnostics live only in the extension work dir"
                    );
                }
            }
        }
        let args: Vec<String> = std::env::args().skip(1).collect();
        let ndbg_env = std::env::var_os("ZATARA_NETCOREDBG");
        let named = args.iter().any(|arg| arg == NETCOREDBG_ARG) || ndbg_env.is_some();
        if named || !probe_armed() {
            return Self::from_parts(
                &args,
                ndbg_env.as_deref(),
                work_dir.as_deref(),
                engine_gate.as_deref(),
                proxy_gate.as_deref(),
            );
        }
        // Nothing explicitly named the debugger, and this host walks for
        // it: an explicit source is a voucher and skips the walk entirely.
        let prefixes = brew_prefixes();
        let outcome = crate::macos_netcoredbg::discover(
            None,
            std::env::var_os("PATH").as_deref(),
            &prefixes,
            &crate::macos_netcoredbg::host_is_candidate,
        );
        match outcome {
            crate::macos_netcoredbg::ProbeOutcome::Found(found) => {
                eprintln!("[zatara-proxy] netcoredbg via {:?}: {}", found.source, found.path.display());
                // The walked path enters through the same pure constructor
                // the named sources use — walked or named, it is the same
                // netcoredbg, so the gates add their effects here too.
                Self::from_parts(
                    &[],
                    Some(found.path.as_os_str()),
                    work_dir.as_deref(),
                    engine_gate.as_deref(),
                    proxy_gate.as_deref(),
                )
            }
            crate::macos_netcoredbg::ProbeOutcome::NotFound => {
                // Guidance instead of a spawn: serve answers and returns,
                // consulting nothing else in this spec — with no child to
                // flag and no session to log, the gates have no effect.
                AdapterSpec {
                    program: PathBuf::from("netcoredbg"),
                    args: interpreter_args(),
                    missing_reason: Some(crate::macos_netcoredbg::missing_binary_message()),
                    proxy_log_path: None,
                }
            }
        }
    }

    /// Interpreter mode is non-negotiable: it is the DAP-over-stdio contract
    /// this relay speaks. The `--netcoredbg <path>` pair is consumed here
    /// and never forwarded — it names *which* adapter to run, not a request
    /// to it. The engine-logging flag is the one argv the gates add: AC1's
    /// wire-traffic file lives under the work dir and nowhere else.
    pub(crate) fn from_parts(
        args: &[String],
        netcoredbg_env: Option<&std::ffi::OsStr>,
        work_dir_env: Option<&std::ffi::OsStr>,
        engine_logging_gate: Option<&str>,
        proxy_log_gate: Option<&str>,
    ) -> Self {
        let from_argv = args
            .iter()
            .position(|arg| arg == NETCOREDBG_ARG)
            .and_then(|i| args.get(i + 1))
            .map(PathBuf::from);
        let mut child_args = interpreter_args();
        if let Some(target) =
            logging::gated_log_target(engine_logging_gate, work_dir_env, logging::ENGINE_LOG_NAME)
        {
            child_args.push(format!("--engineLogging={}", target.display()));
        }
        AdapterSpec {
            program: from_argv
                .or_else(|| netcoredbg_env.map(PathBuf::from))
                .unwrap_or_else(|| PathBuf::from("netcoredbg")),
            args: child_args,
            missing_reason: None,
            proxy_log_path: logging::gated_log_target(
                proxy_log_gate,
                work_dir_env,
                logging::PROXY_LOG_NAME,
            ),
        }
    }

    fn spawn(&self) -> io::Result<Child> {
        // A resolved asset may be an archive (every download lane ships
        // netcoredbg as one: linux a tar.gz, Windows a zip of exe +
        // DLLs, macOS arm64 since D28 a zip of the unix binary):
        // materialize names the program the exec may actually
        // run. Non-archives pass through untouched (AC2).
        let program = crate::asset::materialize(&self.program)?;
        let mut cmd = Command::new(&program);
        cmd.args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        cmd.spawn()
    }
}

/// The DAP interpreter-mode arguments, spelled once (every spec source —
/// argv, env, default, discovery — runs the same child).
fn interpreter_args() -> Vec<String> {
    vec!["--interpreter=vscode".to_string()]
}

/// Whether this host may run the macOS discovery walk: always on a Mac;
/// elsewhere only when the documented test/CI seam arms it (the probe is
/// host-agnostic logic, so the suite exercises this branch on every OS).
/// Arming is not walking: since D28 an Apple-Silicon session normally
/// arrives with netcoredbg NAMED (the release serves osx-arm64), and the
/// `named` check in [`AdapterSpec::from_env`] skips the walk outright —
/// the walk's live shapes here are Intel Macs and degraded sessions.
/// Why a session runs degraded, IF anyone names it. The extension stopped
/// filling this seam the same day the ruling landed (2026-10-13): the
/// degraded note lives in logs, never in the session UI, and the extension
/// logs its own copy. The seam stays so an older extension's note still
/// reaches the log instead of vanishing — and specifically the LOG: the
/// consumer below is an eprintln, never a Console event.
fn release_warning() -> Option<String> {
    std::env::var(RELEASE_WARNING_ENV)
        .ok()
        .filter(|value| !value.is_empty())
}

fn probe_armed() -> bool {
    #[cfg(target_os = "macos")]
    {
        true
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::env::var_os(crate::macos_netcoredbg::PROBE_ARM_ENV).is_some()
    }
}

/// The Homebrew prefixes to walk: the seam override when set (suites pin
/// it away from the developer's machine), the canonical two otherwise.
fn brew_prefixes() -> Vec<std::path::PathBuf> {
    if let Some(list) = std::env::var_os(crate::macos_netcoredbg::BREW_PREFIXES_ENV) {
        let pinned: Vec<PathBuf> = std::env::split_paths(&list).collect();
        if !pinned.is_empty() {
            return pinned;
        }
    }
    crate::macos_netcoredbg::DEFAULT_BREW_PREFIXES.iter().map(std::path::PathBuf::from).collect()
}

#[cfg(test)]
mod adapter_spec_tests {
    use super::*;

    fn spec(argv: &[&str], env: Option<&str>) -> AdapterSpec {
        spec_gated(argv, env, None, None, None)
    }

    /// The same pure constructor with the LOC-0117 inputs exposed: work dir,
    /// engine-logging gate, proxy-log gate.
    fn spec_gated(
        argv: &[&str],
        env: Option<&str>,
        work_dir: Option<&str>,
        engine_gate: Option<&str>,
        proxy_gate: Option<&str>,
    ) -> AdapterSpec {
        AdapterSpec::from_parts(
            &argv.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            env.map(std::ffi::OsStr::new),
            work_dir.map(std::ffi::OsStr::new),
            engine_gate,
            proxy_gate,
        )
    }

    #[test]
    fn the_documented_argv_flag_carries_the_resolved_netcoredbg_path() {
        // The contract of the extension's get_dap_binary answer (LOC-0114):
        // `--netcoredbg <resolved path>` in the proxy's argv, and it beats
        // every other source.
        let s = spec(&["--netcoredbg", "C:/cache/ndbg.exe", "--extra"], Some("C:/env/ndbg.exe"));
        assert_eq!(s.program, PathBuf::from("C:/cache/ndbg.exe"));
        assert_eq!(s.args, vec!["--interpreter=vscode".to_string()]);
    }

    #[test]
    fn the_env_seam_still_resolves_when_no_argv_flag_is_present() {
        // The merged LOC-0107/0109 suites inject through ZATARA_NETCOREDBG;
        // the flag must not retire that seam.
        let s = spec(&[], Some("C:/env/ndbg.exe"));
        assert_eq!(s.program, PathBuf::from("C:/env/ndbg.exe"));
    }

    #[test]
    fn with_neither_source_the_path_netcoredbg_is_the_default() {
        let s = spec(&["--unrelated", "x"], None);
        assert_eq!(s.program, PathBuf::from("netcoredbg"));
        assert_eq!(s.args, vec!["--interpreter=vscode".to_string()]);
    }

    #[test]
    fn the_engine_logging_gate_adds_the_wire_traffic_flag_pointed_at_the_work_dir() {
        // AC1's enabled leg at the argv seam: the flag is netcoredbg's own,
        // carries the work-dir path, and rides *after* the non-negotiable
        // interpreter mode — nothing else changes.
        let s = spec_gated(&[], None, Some("C:/wd"), Some("1"), None);
        assert_eq!(
            s.args,
            vec![
                "--interpreter=vscode".to_string(),
                format!("--engineLogging={}", PathBuf::from("C:/wd").join(logging::ENGINE_LOG_NAME).display()),
            ]
        );
        assert!(s.proxy_log_path.is_none(), "AC1's gate must not open AC2's sink");
    }

    #[test]
    fn the_engine_logging_gate_stays_off_when_disarmed_or_with_no_work_dir() {
        // The default leg (fresh install) and the no-location leg: the child
        // argv is exactly what it was before any gate existed.
        for (work_dir, gate) in [
            (Some("C:/wd"), None),
            (Some("C:/wd"), Some("0")),
            (None, Some("1")),
            (Some(""), Some("1")),
        ] {
            let s = spec_gated(&[], None, work_dir, gate, None);
            assert_eq!(
                s.args,
                vec!["--interpreter=vscode".to_string()],
                "engine gate disarmed (work_dir {work_dir:?}, gate {gate:?}) must leave argv untouched"
            );
        }
    }

    #[test]
    fn the_proxy_log_gate_resolves_one_sink_inside_the_work_dir() {
        // AC2 at the spec seam: the sink is the single named file under the
        // work dir when armed, nothing when not, and never a second path.
        let armed = spec_gated(&[], None, Some("C:/wd"), None, Some("1"));
        assert_eq!(
            armed.proxy_log_path,
            Some(PathBuf::from("C:/wd").join(logging::PROXY_LOG_NAME))
        );
        let off = spec_gated(&[], None, Some("C:/wd"), None, None);
        assert_eq!(off.proxy_log_path, None, "default settings open no sink");
        let no_dir = spec_gated(&[], None, None, None, Some("1"));
        assert_eq!(no_dir.proxy_log_path, None, "no work dir, no sink");
        let empty_dir = spec_gated(&[], None, Some(""), None, Some("1"));
        assert_eq!(
            empty_dir.proxy_log_path, None,
            "an empty work dir names no directory — the sink must not land in the CWD"
        );
    }
}

/// A client request forwarded to the child, waiting for its response.
struct InFlight {
    client_seq: i64,
    command: String,
}

/// Shared relay state: both the client-reading loop and the child-pumping
/// thread speak through one Tunnel. The build-orchestration runner (when
/// armed) drives its children through the same surface.
pub(crate) struct Tunnel {
    out: Mutex<io::Stdout>,
    child_in: Mutex<ChildStdin>,
    child: Arc<Mutex<Child>>,
    sink: Mutex<Box<dyn RelaySink>>,
    pending: Mutex<HashMap<i64, InFlight>>,
    /// Set the instant the proxy's attach answer is relayed — the adapter
    /// has been told to resume (the synthesized `configurationDone` on the
    /// same write) and a late client `configurationDone` must not reach it
    /// a second time (netcoredbg answers the duplicate with 0x80004005,
    /// which Zed treats as a failed request). Live deadlock 2026-10-13:
    /// Zed polls `threads` on the launch answer, netcoredbg refuses until
    /// resumed, and Zed aborts before its `configurationDone` is ever sent.
    configuration_done: AtomicBool,
    /// The one-shot settle gate the build-orchestration runner arms while
    /// it still owns the test step: the relay holds the adapter's
    /// session-end events (`exited`, then `terminated`) behind it, so the
    /// step's console — the vstest runner, which prints its `Passed!`
    /// summary only after the debuggee is gone — drains onto the client
    /// pipe BEFORE the client's console closes. Live defect 2026-10-13:
    /// the end pair won that race in every real run and the summary
    /// landed in an already-closed session — the user's verdict line,
    /// gone. `None` (plain relay, failure paths): the events flow through
    /// unheld.
    test_settle: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    out_seq: AtomicI64,
    child_seq: AtomicI64,
    /// `Some` when the environment arms the build-orchestration runner: the
    /// relay then hands validated launches over instead of forwarding them.
    runner: Option<crate::runner::RunnerConfig>,
    /// The runner's adopted process trees, killable from every session-end
    /// path this process owns. Empty while the runner is unarmed — the plain
    /// relay's teardown stays exactly the adapter kill it always was.
    pub(crate) tree: crate::runner::TreeRegistry,
    /// AC2's single sink, already gated before this process started: `None`
    /// (the default) writes nothing anywhere. Lifecycle lines only — the log
    /// content is the story's declared non-goal, the one-file rule is the
    /// deliverable.
    log: Option<ProxyLog>,
}

impl Tunnel {
    fn next_out_seq(&self) -> i64 {
        self.out_seq.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn next_child_seq(&self) -> i64 {
        self.child_seq.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Send a message to the client under the stdout lock, carrying the
    /// proxy's own seq. `write_frame` flushes per message.
    pub(crate) fn send_to_client(&self, mut message: Value) {
        message["seq"] = json!(self.next_out_seq());
        let body = message.to_string();
        let mut out = self.out.lock().expect("client stdout lock poisoned");
        if write_frame(&mut *out, body.as_bytes()).is_err() {
            eprintln!("zatara-proxy: client write failed (client gone)");
        }
    }

    /// DAP error response — a `success: false` answer carrying an
    /// explanatory `body.error.format`, the shape clients render.
    pub(crate) fn error_response(&self, request_seq: i64, command: &str, message: &str) {
        self.send_to_client(error_response_frame(request_seq, command, message));
    }

    /// End the session by adapter death: the pump sees the closed pipe and
    /// runs the session-end sequence (`exited`, `terminated`, stdout EOF).
    pub(crate) fn kill_adapter(&self) {
        let _ = self.child.lock().expect("child lock poisoned").kill();
    }

    /// Install the settle gate's receiving end (see the field doc): the
    /// runner calls this the moment the attach landed, handing over the
    /// receiver its step-drainer signals on. Arming *before* any session-
    /// end event can exist is what makes the hold race-free.
    pub(crate) fn arm_test_settle(&self, settle: std::sync::mpsc::Receiver<()>) {
        *self.test_settle.lock().expect("settle gate lock poisoned") = Some(settle);
    }

    /// The relay's side of the gate, taken once: the first held event
    /// waits for the test step to drain, the second finds the gate spent
    /// and flows — keeping `exited`/`terminated` in the adapter's own
    /// order behind the flushed console. Every teardown path wakes the
    /// waiter with the same kill it already performs (the waited-on child
    /// is this session's), so no end ever idles on the gate; the budget
    /// only bounds a survivor that refuses to die holding the step's pty.
    fn wait_for_test_settle(&self) {
        const SETTLE_BUDGET: std::time::Duration = std::time::Duration::from_secs(15);
        let gate = self.test_settle.lock().expect("settle gate lock poisoned").take();
        if let Some(settle) = gate {
            let _ = settle.recv_timeout(SETTLE_BUDGET);
        }
    }

    /// Send a proxy-authored `attach{processId}` to the adapter on the
    /// runner's behalf: the client never spawned that debugger target, so
    /// the adapter's attach response is routed back as the answer to the
    /// client's original launch request (the launch seq is what the
    /// in-flight entry carries, under the `launch` command name).
    pub(crate) fn attach_process(&self, pid: u64, launch_seq: i64) -> io::Result<()> {
        let child_seq = self.next_child_seq();
        let outbound = json!({
            "seq": child_seq,
            "type": "request",
            "command": "attach",
            "arguments": { "processId": pid },
        });
        self.pending
            .lock()
            .expect("pending map lock poisoned")
            .insert(child_seq, InFlight { client_seq: launch_seq, command: "launch".into() });
        let body = outbound.to_string();
        let mut child_in = self.child_in.lock().expect("child stdin lock poisoned");
        write_frame(&mut *child_in, body.as_bytes())
    }

    /// Forward a client request to the child under the proxy's child-side
    /// seq, remembering how to correlate the response back.
    fn forward_to_child(&self, request: &Value) -> io::Result<()> {
        let client_seq = request.get("seq").and_then(Value::as_i64).unwrap_or(0);
        let command = request.get("command").and_then(Value::as_str).unwrap_or("").to_string();
        self.forward_to_child_tracked(request, Some((client_seq, command)))
    }

    /// Forward without in-flight tracking — for requests whose child-side
    /// response the proxy swallows itself (e.g. `initialize`, which the
    /// proxy answers with its own capabilities).
    fn forward_to_child_untracked(&self, request: &Value) -> io::Result<()> {
        self.forward_to_child_tracked(request, None)
    }

    fn forward_to_child_tracked(&self, request: &Value, track: Option<(i64, String)>) -> io::Result<()> {
        let child_seq = self.next_child_seq();
        let mut outbound = request.clone();
        outbound["seq"] = json!(child_seq);
        if let Some((client_seq, command)) = track {
            self.pending
                .lock()
                .expect("pending map lock poisoned")
                .insert(child_seq, InFlight { client_seq, command });
        }
        let body = outbound.to_string();
        let mut child_in = self.child_in.lock().expect("child stdin lock poisoned");
        write_frame(&mut *child_in, body.as_bytes())
    }
}

/// Child stdout -> client: relay child responses (with `request_seq`
/// translated back to the client's numbering) and drop responses for
/// requests the proxy answered itself (e.g. `initialize`).
fn pump_child(tunnel: Arc<Tunnel>, child_out: ChildStdout) {
    let mut reader = BufReader::new(child_out);
    while let Ok(Some(bytes)) = read_frame(&mut reader) {
        let Ok(mut message) = serde_json::from_slice::<Value>(&bytes) else {
            eprintln!("zatara-proxy: dropped malformed frame from the adapter");
            continue;
        };
        tunnel.sink.lock().expect("sink lock poisoned").on_child_message(&message);
        match message.get("type").and_then(Value::as_str) {
            // Events are pure downstream traffic: relayed verbatim under the
            // proxy's own seq. The body survives untouched — that is what
            // "forwarded intact, field for field" means to the client.
            // One exception, timing-only: the debuggee-done pair waits
            // behind the settle gate, because the client closes its
            // console on `terminated` and the vstest summary — printed by
            // the runner's test step AFTER the debuggee died — must be on
            // the pipe before that close (owner's lost `Passed!`, live
            // defect 2026-10-13). Envelope and body untouched either way.
            Some("event") => {
                if matches!(
                    message.get("event").and_then(Value::as_str),
                    Some("exited" | "terminated")
                ) {
                    tunnel.wait_for_test_settle();
                }
                tunnel.send_to_client(message)
            }
            Some("response") => {
                let child_seq = message.get("request_seq").and_then(Value::as_i64);
                let correlated = child_seq
                    .and_then(|seq| tunnel.pending.lock().expect("pending map lock poisoned").remove(&seq));
                // Only relay responses we can route back to a client request.
                // The child's answer to `initialize` (which the proxy answers
                // itself) is untracked, so it stays child-side and is dropped.
                if let Some(in_flight) = correlated {
                    // The proxy-authored attach is the one pairing where the
                    // adapter's word ("attach") is not the client's word
                    // ("launch"). Two spec repairs ride this moment, in this
                    // order, live defects 2026-10-13:
                    // 1) resume the adapter BEFORE the answer leaves: a
                    //    client that polls `threads` on the launch answer
                    //    (Zed does) gets 0x80004005 from a netcoredbg that
                    //    refuses `threads` until resumed, aborts its startup
                    //    chain, and never sends its own configurationDone —
                    //    deadlock. The synthesized resume goes out first so
                    //    the client's polls land on a running session.
                    // 2) the answer must carry the CLIENT's command name —
                    //    a response that says "attach" to a "launch" request
                    //    is treated as unanswered, the configurationDone
                    //    never comes, and netcoredbg waits forever.
                    let resumed_now = in_flight.command == "launch"
                        && message.get("command").and_then(Value::as_str) == Some("attach")
                        && message.get("success").and_then(Value::as_bool) == Some(true);
                    if resumed_now {
                        tunnel.configuration_done.store(true, Ordering::SeqCst);
                        let _ = tunnel.forward_to_child_untracked(&json!({
                            "type": "request",
                            "command": "configurationDone",
                            "arguments": {},
                        }));
                    }
                    message["request_seq"] = json!(in_flight.client_seq);
                    message["command"] = json!(in_flight.command);
                    tunnel.send_to_client(message);
                }
            }
            _ => {}
        }
    }
    end_session(&tunnel);
}

/// The adapter's stdout closed: the child is gone. Fail every request still
/// waiting on it, hand the client the session-end sequence (`exited` carrying
/// the child's code, then `terminated`), and let stdout close so the editor
/// tears the session down rather than hanging on a dead pipe.
fn end_session(tunnel: &Tunnel) -> ! {
    // The adapter is gone and this path ends with `process::exit` — any
    // runner tree still alive past here is an orphan the moment the
    // process is. Kill first, grieve second.
    tunnel.tree.kill_all();
    // ... but grieve before answering: the kill above woke the settle
    // waiter, and its drain puts the test step's final bytes — the run
    // summary among them — on the client pipe before this function's own
    // session-end pair closes the console behind them.
    tunnel.wait_for_test_settle();
    let exit_code = tunnel
        .child
        .lock()
        .expect("child lock poisoned")
        .wait()
        .ok()
        .and_then(|status| status.code());
    if let Some(log) = &tunnel.log {
        log.note(&format!(
            "adapter exited (code: {})",
            exit_code.map(|c| c.to_string()).unwrap_or_else(|| "signal".to_string())
        ));
    }
    tunnel.sink.lock().expect("sink lock poisoned").on_child_exit(exit_code);

    // Requests the child died before answering: answer them ourselves so no
    // client request hangs waiting for a response that can never arrive.
    let waiting: Vec<InFlight> = tunnel
        .pending
        .lock()
        .expect("pending map lock poisoned")
        .drain()
        .map(|(_, flight)| flight)
        .collect();
    for flight in waiting {
        tunnel.error_response(
            flight.client_seq,
            &flight.command,
            "adapter exited before responding",
        );
    }

    tunnel.send_to_client(json!({
        "type": "event",
        "event": "exited",
        "body": { "exitCode": exit_code.unwrap_or(-1) },
    }));
    tunnel.send_to_client(json!({"type": "event", "event": "terminated"}));

    // Closing the process's stdout — which exiting does — is the teardown
    // signal: the client's read side sees EOF and closes the session UI.
    std::process::exit(0);
}

/// The one shape of a DAP error response in this file: a `success: false`
/// answer carrying an explanatory `body.error.format`. No `seq` —
/// [`Tunnel::send_to_client`] stamps it for relay-path answers, and the
/// pre-spawn guidance caller stamps its own (it answers before the
/// tunnel's counter exists).
fn error_response_frame(request_seq: i64, command: &str, message: &str) -> Value {
    json!({
        "type": "response",
        "request_seq": request_seq,
        "command": command,
        "success": false,
        "body": { "error": { "id": 0, "format": message } },
    })
}

/// Run the relay on the process's own stdio until the client hangs up.
pub fn serve(spec: AdapterSpec, sink: Box<dyn RelaySink>) -> io::Result<()> {
    if let Some(guidance) = &spec.missing_reason {
        // Discovery proved there is no debugger to spawn (macOS branch,
        // LOC-0116): the session is answered with the install guidance —
        // the same `body.error.format` shape every other relay error uses,
        // at request_seq 0 for the requests that never got to arrive —
        // then the proxy ends without ever spawning a child. The stdout
        // close is the teardown signal the client reads as session end.
        // This answer runs before any sink is opened: with no child there
        // is no session to log, so no gate writes on this path.
        let mut frame = error_response_frame(0, "", guidance);
        frame["seq"] = json!(1); // this process's only message, pre-tunnel
        write_frame(&mut io::stdout(), frame.to_string().as_bytes())?;
        return Err(io::Error::other(guidance.clone()));
    }
    // The proxy-log gate already resolved (or refused) its one sink. A
    // requested-but-unopenable log degrades to no log — losing the log must
    // never lose the debug session.
    let proxy_log = spec.proxy_log_path.as_deref().and_then(|path| {
        ProxyLog::open(path)
            .map_err(|e| {
                eprintln!("zatara-proxy: cannot open proxy log {}: {e} — continuing without it", path.display())
            })
            .ok()
    });
    let mut child = match spec.spawn() {
        Ok(child) => child,
        Err(e) => {
            if let Some(log) = &proxy_log {
                log.note(&format!("adapter spawn failed: {e}"));
            }
            return Err(e);
        }
    };
    if let Some(log) = &proxy_log {
        log.note(&format!("adapter spawned: {}", spec.program.display()));
    }
    let child_in = child.stdin.take().expect("child stdin piped");
    let child_out = child.stdout.take().expect("child stdout piped");
    let tunnel = Arc::new(Tunnel {
        out: Mutex::new(io::stdout()),
        child_in: Mutex::new(child_in),
        child: Arc::new(Mutex::new(child)),
        sink: Mutex::new(sink),
        pending: Mutex::new(HashMap::new()),
        configuration_done: AtomicBool::new(false),
        test_settle: Mutex::new(None),
        out_seq: AtomicI64::new(0),
        child_seq: AtomicI64::new(0),
        runner: crate::runner::RunnerConfig::from_env(),
        tree: crate::runner::TreeRegistry::default(),
        log: proxy_log,
    });
    thread::spawn({
        let tunnel = Arc::clone(&tunnel);
        move || pump_child(tunnel, child_out)
    });

    let stdin = io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let mut session = Session::new();
    loop {
        match read_frame(&mut reader) {
            Ok(None) => break, // clean client EOF
            Ok(Some(bytes)) => {
                let Ok(message) = serde_json::from_slice::<Value>(&bytes) else {
                    // A framed body that is not JSON is answerable and the
                    // frame boundary is still intact: the pipe keeps serving.
                    tunnel.error_response(0, "", "malformed DAP frame: body is not valid JSON");
                    continue;
                };
                tunnel.sink.lock().expect("sink lock poisoned").on_client_message(&message);
                if message.get("type").and_then(Value::as_str) != Some("request") {
                    continue; // clients do not send responses/events today
                }
                let command = message.get("command").and_then(Value::as_str).unwrap_or("").to_string();
                let client_seq = message.get("seq").and_then(Value::as_i64).unwrap_or(0);
                session.advance_on_client_message(&message);
                match command.as_str() {
                    "initialize" => {
                        // The proxy is the client's adapter: it answers for
                        // its own capabilities. The answer goes out *before*
                        // the request is forwarded, so the handshake order is
                        // the proxy's guarantee and cannot be overtaken by a
                        // fast child event reacting to being initialized. The
                        // child is initialized too (DAP requires it before any
                        // other request), but its answer is proxy-internal —
                        // untracked, so the pump drops it.
                        tunnel.send_to_client(json!({
                            "type": "response",
                            "request_seq": client_seq,
                            "command": "initialize",
                            "success": true,
                            "body": { "capabilities": {
                                "supportsConfigurationDoneRequest": true,
                                "supportsTerminateRequest": true,
                            } },
                        }));
                        if tunnel.forward_to_child_untracked(&message).is_err() {
                            adapter_lost();
                        }
                        // The degraded-session note (owner ruling
                        // 2026-10-13, sharpened same day): an unreachable
                        // release neither blocks nor takes Console billing —
                        // it goes to the log, where the adapter's stderr
                        // already lands. The session itself must not look
                        // broken when it is not.
                        if let Some(warning) = release_warning() {
                            eprintln!("zatara-proxy: note (degraded session): {warning}");
                        }
                    }
                    "launch" => {
                        // Validate against the single-definition contract…
                        let arguments = message.get("arguments").cloned().unwrap_or(Value::Null);
                        match serde_json::from_value::<LaunchConfig>(arguments) {
                            // …but forward the raw body: re-serializing the
                            // parsed LaunchConfig would silently drop fields
                            // the proxy has never seen.
                            Ok(config) => {
                                // The phase enters `Launched` only on this
                                // verdict — a launch answered with a
                                // validation error leaves it where it stands.
                                session.accept_launch();
                                match tunnel.runner {
                                    // An armed runner owns the validated
                                    // launch from here: build and test are
                                    // orchestrated in-proxy and the debugger
                                    // receives an `attach`, so the raw
                                    // launch never reaches the adapter.
                                    Some(runner) => {
                                        crate::runner::start(
                                            Arc::clone(&tunnel),
                                            runner,
                                            config,
                                            client_seq,
                                        );
                                    }
                                    None => {
                                        if tunnel.forward_to_child(&message).is_err() {
                                            adapter_lost();
                                        }
                                    }
                                }
                            }
                            Err(e) => tunnel.error_response(
                                client_seq,
                                "launch",
                                &format!("launch config rejected: {e}"),
                            ),
                        }
                    }
                    "configurationDone" => {
                        // Before the attach answer: forward — netcoredbg
                        // buffers a pre-attach configurationDone and resumes
                        // on attach (verified against the real adapter).
                        // After it: the session was already configured and
                        // resumed by the proxy's synthesized copy; forwarding
                        // the client's would earn netcoredbg's 0x80004005
                        // duplicate error, while the client only needs the
                        // success the configuration genuinely has.
                        if tunnel.configuration_done.load(Ordering::SeqCst) {
                            tunnel.send_to_client(json!({
                                "type": "response",
                                "request_seq": client_seq,
                                "command": "configurationDone",
                                "success": true,
                                "body": {},
                            }));
                        } else if tunnel.forward_to_child(&message).is_err() {
                            adapter_lost();
                        }
                    }
                    "disconnect" => {
                        // The runner's tree goes first, synchronously: the
                        // adapter may answer the disconnect and exit on its
                        // own the instant it is forwarded, and the pump's
                        // session-end path ends in `process::exit` — a kill
                        // issued after the forward races that exit and can
                        // lose the tree. The client asked to stop; nothing
                        // it owns is allowed to outlive the request.
                        tunnel.tree.kill_all();
                        // Forward so the adapter can answer, then the proxy
                        // owns teardown: it shuts the child down itself rather
                        // than trusting the adapter to self-terminate, so a
                        // wedged netcoredbg can never keep the proxy alive
                        // past the client's disconnect. Killing the child
                        // closes its stdout; the pump then runs the session-end
                        // sequence and the proxy exits.
                        if tunnel.forward_to_child(&message).is_err() {
                            adapter_lost();
                        }
                        let _ = tunnel.child.lock().expect("child lock poisoned").kill();
                    }
                    _ => {
                        if tunnel.forward_to_child(&message).is_err() {
                            adapter_lost();
                        }
                    }
                }
            }
            Err(e) => {
                // A broken frame boundary cannot be re-synchronised: answer
                // once, then end.
                tunnel.error_response(0, "", &format!("framing error: {e}"));
                break;
            }
        }
    }
    // The client hung up: never orphan the adapter (research findings §9),
    // and never orphan the runner's trees either — a hung-up stdin is a stop
    // like any other, and the editor is not coming back for them.
    tunnel.tree.kill_all();
    let mut child = tunnel.child.lock().expect("child lock poisoned");
    let _ = child.kill();
    let _ = child.wait();
    Ok(())
}

/// The adapter pipe broke mid-session.
fn adapter_lost() -> ! {
    eprintln!("zatara-proxy: adapter write failed — the adapter is gone");
    std::process::exit(1);
}
