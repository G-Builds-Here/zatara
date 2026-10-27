use std::collections::HashMap;
use std::io::{self, BufReader};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use serde_json::{json, Value};
use zatara_types::{
    launch_mode_from_env, LaunchConfig, LaunchMode, NETCOREDBG_ARG, RELEASE_WARNING_ENV,
    WORK_DIR_ENV,
};
use crate::framing::{read_frame, write_frame};
use crate::logging::{self, ProxyLog};
use crate::session::{RelaySink, Session};
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
pub struct AdapterSpec {
    program: PathBuf,
    args: Vec<String>,
    missing_reason: Option<String>,
    proxy_log_path: Option<PathBuf>,
}
impl AdapterSpec {
    pub fn from_env() -> Self {
        let work_dir = std::env::var_os(WORK_DIR_ENV);
        let engine_gate = std::env::var(logging::ENGINE_LOGGING_ENV).ok();
        let proxy_gate = std::env::var(logging::PROXY_LOG_ENV).ok();
        if logging::usable_work_dir(work_dir.as_deref()).is_none() {
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
                Self::from_parts(
                    &[],
                    Some(found.path.as_os_str()),
                    work_dir.as_deref(),
                    engine_gate.as_deref(),
                    proxy_gate.as_deref(),
                )
            }
            crate::macos_netcoredbg::ProbeOutcome::NotFound => {
                AdapterSpec {
                    program: PathBuf::from("netcoredbg"),
                    args: interpreter_args(),
                    missing_reason: Some(crate::macos_netcoredbg::missing_binary_message()),
                    proxy_log_path: None,
                }
            }
        }
    }
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
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        cmd.spawn()
    }
}
fn interpreter_args() -> Vec<String> {
    vec!["--interpreter=vscode".to_string()]
}
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
        let s = spec(&["--netcoredbg", "C:/cache/ndbg.exe", "--extra"], Some("C:/env/ndbg.exe"));
        assert_eq!(s.program, PathBuf::from("C:/cache/ndbg.exe"));
        assert_eq!(s.args, vec!["--interpreter=vscode".to_string()]);
    }
    #[test]
    fn the_env_seam_still_resolves_when_no_argv_flag_is_present() {
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
struct InFlight {
    client_seq: i64,
    command: String,
}
pub(crate) struct Tunnel {
    out: Mutex<io::Stdout>,
    child_in: Mutex<ChildStdin>,
    child: Arc<Mutex<Child>>,
    sink: Mutex<Box<dyn RelaySink>>,
    pending: Mutex<HashMap<i64, InFlight>>,
    init_wait: Mutex<Option<i64>>,
    init_answered: AtomicBool,
    step_announced: AtomicBool,
    configuration_done: AtomicBool,
    pending_config_done: Mutex<Option<i64>>,
    test_settle: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    out_seq: AtomicI64,
    child_seq: AtomicI64,
    runner: Option<crate::runner::RunnerConfig>,
    pub(crate) tree: crate::runner::TreeRegistry,
    program_session: AtomicBool,
    runner_launch_pending: AtomicBool,
    adapter_tree: crate::runner::TreeRegistry,
    log: Option<ProxyLog>,
}
impl Tunnel {
    fn next_out_seq(&self) -> i64 {
        self.out_seq.fetch_add(1, Ordering::Relaxed) + 1
    }
    fn next_child_seq(&self) -> i64 {
        self.child_seq.fetch_add(1, Ordering::Relaxed) + 1
    }
    pub(crate) fn send_to_client(&self, mut message: Value) {
        message["seq"] = json!(self.next_out_seq());
        let body = message.to_string();
        let mut out = self.out.lock().expect("client stdout lock poisoned");
        if write_frame(&mut *out, body.as_bytes()).is_err() {
            eprintln!("zatara-proxy: client write failed (client gone)");
        }
    }
    pub(crate) fn error_response(&self, request_seq: i64, command: &str, message: &str) {
        self.send_to_client(error_response_frame(request_seq, command, message));
    }
    pub(crate) fn kill_adapter(&self) {
        let _ = self.child.lock().expect("child lock poisoned").kill();
    }
    pub(crate) fn begin_program_session(&self) {
        self.program_session.store(true, Ordering::SeqCst);
        let pid = self.child.lock().expect("child lock poisoned").id();
        self.adapter_tree.adopt(Some(pid));
    }
    pub(crate) fn note_runner_launch_pending(&self) {
        self.runner_launch_pending.store(true, Ordering::SeqCst);
    }
    pub(crate) fn runner_lane_decided(&self, mtp_lane: bool) {
        if !mtp_lane && cfg!(target_os = "macos") {
            return;
        }
        self.runner_launch_pending.store(false, Ordering::SeqCst);
        if mtp_lane {
            return;
        }
        let stashed = self
            .pending_config_done
            .lock()
            .expect("pending configDone lock poisoned")
            .take();
        if let Some(client_seq) = stashed {
            if let Some(log) = &self.log {
                log.note("relay: stashed configurationDone forwarded (rendezvous lane, pre-attach)");
            }
            let _ = self.forward_to_child(&json!({
                "type": "request",
                "seq": client_seq,
                "command": "configurationDone",
                "arguments": {},
            }));
        }
    }
    fn announce_program_exit_error(&self, code: i64) {
        self.send_to_client(exit_error_event(code));
    }
    pub(crate) fn arm_test_settle(&self, settle: std::sync::mpsc::Receiver<()>) {
        *self.test_settle.lock().expect("settle gate lock poisoned") = Some(settle);
    }
    fn wait_for_test_settle(&self) {
        const SETTLE_BUDGET: std::time::Duration = std::time::Duration::from_secs(15);
        let gate = self.test_settle.lock().expect("settle gate lock poisoned").take();
        if let Some(settle) = gate {
            let _ = settle.recv_timeout(SETTLE_BUDGET);
        }
    }
    fn wait_for_initialize_answered(&self) {
        const INIT_ORDER_BUDGET: std::time::Duration = std::time::Duration::from_secs(5);
        let deadline = std::time::Instant::now() + INIT_ORDER_BUDGET;
        while !self.init_answered.load(Ordering::SeqCst) {
            if std::time::Instant::now() >= deadline {
                eprintln!("zatara-proxy: initialize unanswered past the ordering budget - proceeding");
                break;
            }
            thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    pub(crate) fn attach_process(
        &self,
        pid: u64,
        client_seq: i64,
        client_command: &str,
    ) -> io::Result<()> {
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
            .insert(
                child_seq,
                InFlight { client_seq, command: client_command.to_string() },
            );
        let body = outbound.to_string();
        let mut child_in = self.child_in.lock().expect("child stdin lock poisoned");
        write_frame(&mut *child_in, body.as_bytes())
    }
    pub(crate) fn launch_program(
        &self,
        program: &str,
        args: &[String],
        cwd: &str,
        client_seq: i64,
    ) -> io::Result<()> {
        let child_seq = self.next_child_seq();
        let outbound = json!({
            "seq": child_seq,
            "type": "request",
            "command": "launch",
            "arguments": {
                "type": "coreclr",
                "name": "zatara: debug test",
                "request": "launch",
                "stopAtEntry": false,
                "program": program,
                "args": args,
                "cwd": cwd,
            },
        });
        self.pending
            .lock()
            .expect("pending map lock poisoned")
            .insert(
                child_seq,
                InFlight { client_seq, command: "launch".to_string() },
            );
        let body = outbound.to_string();
        let mut child_in = self.child_in.lock().expect("child stdin lock poisoned");
        write_frame(&mut *child_in, body.as_bytes())
    }
    pub(crate) fn forward_to_child(&self, request: &Value) -> io::Result<()> {
        let client_seq = request.get("seq").and_then(Value::as_i64).unwrap_or(0);
        let command = request.get("command").and_then(Value::as_str).unwrap_or("").to_string();
        self.forward_to_child_tracked(request, Some((client_seq, command)))
    }
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
fn pump_child(tunnel: Arc<Tunnel>, child_out: ChildStdout) {
    let mut reader = BufReader::new(child_out);
    while let Ok(Some(bytes)) = read_frame(&mut reader) {
        let Ok(mut message) = serde_json::from_slice::<Value>(&bytes) else {
            eprintln!("zatara-proxy: dropped malformed frame from the adapter");
            continue;
        };
        tunnel.sink.lock().expect("sink lock poisoned").on_child_message(&message);
        match message.get("type").and_then(Value::as_str) {
            Some("event") => {
                if message.get("event").and_then(Value::as_str) == Some("exited")
                    && tunnel.program_session.load(Ordering::SeqCst)
                {
                    if let Some(code) = message
                        .get("body")
                        .and_then(|body| body.get("exitCode"))
                        .and_then(Value::as_i64)
                    {
                        if code != 0 {
                            tunnel.announce_program_exit_error(code);
                        }
                    }
                }
                if matches!(
                    message.get("event").and_then(Value::as_str),
                    Some("exited" | "terminated")
                ) {
                    tunnel.wait_for_test_settle();
                }
                tunnel.send_to_client(message)
            }
            Some("response") => {
                if message.get("command").and_then(Value::as_str) == Some("initialize") {
                    if let Some(seq) = tunnel.init_wait.lock().expect("init_wait lock poisoned").take() {
                        if message.get("success").and_then(Value::as_bool) == Some(true) {
                            let mut body = message
                                .get("body")
                                .and_then(|body| body.get("capabilities"))
                                .cloned()
                                .unwrap_or_else(|| json!({}));
                            if let Some(map) = body.as_object_mut() {
                                map.insert("supportsConfigurationDoneRequest".to_string(), json!(true));
                                map.insert("supportsTerminateRequest".to_string(), json!(true));
                            }
                            tunnel.send_to_client(json!({
                                "type": "response",
                                "request_seq": seq,
                                "command": "initialize",
                                "success": true,
                                "body": body,
                            }));
                        } else {
                            let reason = message
                                .get("message")
                                .and_then(Value::as_str)
                                .unwrap_or("adapter rejected initialize");
                            tunnel.error_response(seq, "initialize", reason);
                        }
                        tunnel.init_answered.store(true, Ordering::SeqCst);
                    }
                    continue;
                }
                let child_seq = message.get("request_seq").and_then(Value::as_i64);
                let correlated = child_seq
                    .and_then(|seq| tunnel.pending.lock().expect("pending map lock poisoned").remove(&seq));
                if let Some(in_flight) = correlated {
                    let attach_answered =
                        message.get("command").and_then(Value::as_str) == Some("attach")
                            && matches!(in_flight.command.as_str(), "launch" | "attach");
                    if attach_answered {
                        tunnel.runner_launch_pending.store(false, Ordering::SeqCst);
                    }
                    let resumed_now = attach_answered
                        && message.get("success").and_then(Value::as_bool) == Some(true);
                    if resumed_now {
                        tunnel.configuration_done.store(true, Ordering::SeqCst);
                        let stashed = tunnel
                            .pending_config_done
                            .lock()
                            .expect("pending configDone lock poisoned")
                            .take();
                        match stashed {
                            Some(client_seq) => {
                                if let Some(log) = &tunnel.log {
                                    log.note("relay: stashed configurationDone forwarded (attach answered, post-attach rendezvous)");
                                }
                                let _ = tunnel.forward_to_child(&json!({
                                    "type": "request",
                                    "seq": client_seq,
                                    "command": "configurationDone",
                                    "arguments": {},
                                }));
                            }
                            None => {
                                let _ = tunnel.forward_to_child_untracked(&json!({
                                    "type": "request",
                                    "command": "configurationDone",
                                    "arguments": {},
                                }));
                            }
                        }
                    }
                    let program_launch_answer = matches!(in_flight.command.as_str(), "launch")
                        && message.get("command").and_then(Value::as_str) == Some("launch")
                        && tunnel.program_session.load(Ordering::SeqCst);
                    let stashed_at_launch = if program_launch_answer
                        && message.get("success").and_then(Value::as_bool) == Some(true)
                    {
                        let stashed = tunnel
                            .pending_config_done
                            .lock()
                            .expect("pending configDone lock poisoned")
                            .take();
                        tunnel.configuration_done.store(true, Ordering::SeqCst);
                        let _ = tunnel.forward_to_child_untracked(&json!({
                            "type": "request",
                            "command": "configurationDone",
                            "arguments": {},
                        }));
                        stashed
                    } else {
                        None
                    };
                    let step_cmd = matches!(in_flight.command.as_str(), "next" | "stepIn" | "stepOut");
                    let answered = message.get("success").and_then(Value::as_bool);
                    let refusal_reason = if step_cmd && answered == Some(false) {
                        Some(
                            message
                                .get("message")
                                .and_then(Value::as_str)
                                .unwrap_or("the adapter refused the step")
                                .to_string(),
                        )
                    } else {
                        None
                    };
                    let client_command = in_flight.command.clone();
                    if answer_benign_attach_config_done(
                        &mut message,
                        &client_command,
                        tunnel.program_session.load(Ordering::SeqCst),
                    ) {
                        if let Some(log) = &tunnel.log {
                            log.note("relay: benign attach configurationDone E_FAIL answered as success");
                        }
                    }
                    message["request_seq"] = json!(in_flight.client_seq);
                    message["command"] = json!(in_flight.command);
                    tunnel.send_to_client(message);
                    if let Some(stashed) = stashed_at_launch {
                        tunnel.send_to_client(json!({
                            "type": "response",
                            "request_seq": stashed,
                            "command": "configurationDone",
                            "success": true,
                            "body": {},
                        }));
                    } else if program_launch_answer && answered != Some(true) {
                        if let Some(stashed) = tunnel
                            .pending_config_done
                            .lock()
                            .expect("pending configDone lock poisoned")
                            .take()
                        {
                            tunnel.error_response(
                                stashed,
                                "configurationDone",
                                "the session launch failed",
                            );
                        }
                    }
                    if let Some(reason) = refusal_reason {
                        if !tunnel.step_announced.swap(true, Ordering::SeqCst) {
                            tunnel.send_to_client(step_refusal_event(&client_command, &reason));
                        }
                    } else if (step_cmd || client_command == "continue") && answered == Some(true) {
                        tunnel.step_announced.store(false, Ordering::SeqCst);
                    }
                }
            }
            _ => {}
        }
    }
    end_session(&tunnel);
}
fn end_session(tunnel: &Tunnel) -> ! {
    tunnel.tree.kill_all();
    tunnel.adapter_tree.kill_all();
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
    if let Some(seq) = tunnel.init_wait.lock().expect("init_wait lock poisoned").take() {
        tunnel.error_response(seq, "initialize", "adapter exited before responding");
        tunnel.init_answered.store(true, Ordering::SeqCst);
    }
    if let Some(seq) = tunnel
        .pending_config_done
        .lock()
        .expect("pending configDone lock poisoned")
        .take()
    {
        tunnel.error_response(seq, "configurationDone", "the session ended before the launch answered");
    }
    let reported_exit_code = exit_code.unwrap_or(-1);
    if tunnel.program_session.load(Ordering::SeqCst) && reported_exit_code != 0 {
        tunnel.announce_program_exit_error(i64::from(reported_exit_code));
    }
    tunnel.send_to_client(json!({
        "type": "event",
        "event": "exited",
        "body": { "exitCode": reported_exit_code },
    }));
    tunnel.send_to_client(json!({"type": "event", "event": "terminated"}));
    std::process::exit(0);
}
fn exit_error_event(code: i64) -> Value {
    json!({
        "type": "event",
        "event": "output",
        "body": {
            "category": "stderr",
            "output": format!("zatara: program exited with code {code}\n"),
        },
    })
}
fn step_refusal_event(command: &str, reason: &str) -> Value {
    json!({
        "type": "event",
        "event": "output",
        "body": {
            "category": "stderr",
            "output": format!(
                "zatara: the debugger refused '{command}' ({reason}). An on-demand pause can leave it on code it cannot step; press Continue and stop again on a breakpoint. The refusal came from the adapter, relayed untouched.\n"
            ),
        },
    })
}
fn error_response_frame(request_seq: i64, command: &str, message: &str) -> Value {
    json!({
        "type": "response",
        "request_seq": request_seq,
        "command": command,
        "success": false,
        "body": { "error": { "id": 0, "format": message } },
    })
}
fn answer_benign_attach_config_done(
    message: &mut Value,
    client_command: &str,
    program_session: bool,
) -> bool {
    if client_command != "configurationDone" || program_session {
        return false;
    }
    if message.get("success").and_then(Value::as_bool) != Some(false) {
        return false;
    }
    if !message
        .get("message")
        .and_then(Value::as_str)
        .is_some_and(|m| m.contains("0x80004005"))
    {
        return false;
    }
    message["success"] = json!(true);
    message["body"] = json!({});
    if let Some(obj) = message.as_object_mut() {
        obj.remove("message");
    }
    true
}
pub fn serve(spec: AdapterSpec, sink: Box<dyn RelaySink>) -> io::Result<()> {
    if let Some(guidance) = &spec.missing_reason {
        let mut frame = error_response_frame(0, "", guidance);
        frame["seq"] = json!(1);
        write_frame(&mut io::stdout(), frame.to_string().as_bytes())?;
        return Err(io::Error::other(guidance.clone()));
    }
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
        init_wait: Mutex::new(None),
        init_answered: AtomicBool::new(false),
        step_announced: AtomicBool::new(false),
        configuration_done: AtomicBool::new(false),
        pending_config_done: Mutex::new(None),
        test_settle: Mutex::new(None),
        out_seq: AtomicI64::new(0),
        child_seq: AtomicI64::new(0),
        runner: crate::runner::RunnerConfig::from_env(),
        tree: crate::runner::TreeRegistry::default(),
        program_session: AtomicBool::new(false),
        runner_launch_pending: AtomicBool::new(false),
        adapter_tree: crate::runner::TreeRegistry::default(),
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
            Ok(None) => break,
            Ok(Some(bytes)) => {
                let Ok(message) = serde_json::from_slice::<Value>(&bytes) else {
                    tunnel.error_response(0, "", "malformed DAP frame: body is not valid JSON");
                    continue;
                };
                tunnel.sink.lock().expect("sink lock poisoned").on_client_message(&message);
                if message.get("type").and_then(Value::as_str) != Some("request") {
                    continue;
                }
                let command = message.get("command").and_then(Value::as_str).unwrap_or("").to_string();
                let client_seq = message.get("seq").and_then(Value::as_i64).unwrap_or(0);
                session.advance_on_client_message(&message);
                match command.as_str() {
                    "initialize" => {
                        *tunnel.init_wait.lock().expect("init_wait lock poisoned") = Some(client_seq);
                        if tunnel.forward_to_child_untracked(&message).is_err() {
                            adapter_lost();
                        }
                        tunnel.wait_for_initialize_answered();
                        if let Some(warning) = release_warning() {
                            eprintln!("zatara-proxy: note (degraded session): {warning}");
                        }
                    }
                    "launch" => {
                        let arguments = message.get("arguments").cloned().unwrap_or(Value::Null);
                        match serde_json::from_value::<LaunchConfig>(arguments) {
                            Ok(config)
                                if config.request
                                    == Some(zatara_types::LaunchRequest::Attach) =>
                            {
                                tunnel.error_response(
                                    client_seq,
                                    "launch",
                                    "launch config rejected: an attach config must \
                                     arrive as an attach request — this session's \
                                     extension answered Launch for it; update or \
                                     reinstall the zatara extension",
                                );
                            }
                            Ok(config) => {
                                session.accept_launch();
                                match crate::program::dispatch_mode(&config, launch_mode_from_env()) {
                                    LaunchMode::Project => {
                                        crate::program::start(
                                            Arc::clone(&tunnel),
                                            &config,
                                            &message,
                                            client_seq,
                                        );
                                    }
                                    LaunchMode::Test => match tunnel.runner {
                                        Some(runner) => {
                                            tunnel.note_runner_launch_pending();
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
                                    },
                                }
                            }
                            Err(e) => tunnel.error_response(
                                client_seq,
                                "launch",
                                &format!("launch config rejected: {e}"),
                            ),
                        }
                    }
                    "attach" => {
                        let arguments = message.get("arguments").cloned().unwrap_or(Value::Null);
                        match serde_json::from_value::<LaunchConfig>(arguments) {
                            Ok(config) => match config.process_id {
                                None => tunnel.error_response(
                                    client_seq,
                                    "attach",
                                    "attach config rejected: missing field `process_id`",
                                ),
                                Some(pid) => {
                                    session.accept_launch();
                                    if !crate::runner::process_exists(pid) {
                                        tunnel.error_response(
                                            client_seq,
                                            "attach",
                                            &format!(
                                                "attach to pid {pid} failed: no process {pid} is running"
                                            ),
                                        );
                                    } else if tunnel
                                        .attach_process(pid, client_seq, "attach")
                                        .is_err()
                                    {
                                        adapter_lost();
                                    }
                                }
                            },
                            Err(e) => tunnel.error_response(
                                client_seq,
                                "attach",
                                &format!("attach config rejected: {e}"),
                            ),
                        }
                    }
                    "configurationDone" => {
                        if tunnel.configuration_done.load(Ordering::SeqCst) {
                            tunnel.send_to_client(json!({
                                "type": "response",
                                "request_seq": client_seq,
                                "command": "configurationDone",
                                "success": true,
                                "body": {},
                            }));
                        } else if tunnel.program_session.load(Ordering::SeqCst)
                            || tunnel.runner_launch_pending.load(Ordering::SeqCst)
                        {
                            *tunnel
                                .pending_config_done
                                .lock()
                                .expect("pending configDone lock poisoned") = Some(client_seq);
                            if let Some(log) = &tunnel.log {
                                log.note("relay: client configurationDone stashed (program session, pre-launch)");
                            }
                        } else if tunnel.forward_to_child(&message).is_err() {
                            adapter_lost();
                        }
                    }
                    "disconnect" => {
                        tunnel.tree.kill_all();
                        if tunnel.forward_to_child(&message).is_err() {
                            adapter_lost();
                        }
                        const DISCONNECT_GRACE: std::time::Duration =
                            std::time::Duration::from_secs(2);
                        let grace = std::time::Instant::now() + DISCONNECT_GRACE;
                        loop {
                            let gone = tunnel
                                .child
                                .lock()
                                .expect("child lock poisoned")
                                .try_wait()
                                .is_ok_and(|status| status.is_some());
                            if gone || std::time::Instant::now() >= grace {
                                break;
                            }
                            std::thread::sleep(std::time::Duration::from_millis(10));
                        }
                        tunnel.adapter_tree.kill_all();
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
                tunnel.error_response(0, "", &format!("framing error: {e}"));
                break;
            }
        }
    }
    tunnel.tree.kill_all();
    tunnel.adapter_tree.kill_all();
    let mut child = tunnel.child.lock().expect("child lock poisoned");
    let _ = child.kill();
    let _ = child.wait();
    Ok(())
}
pub(crate) fn adapter_lost() -> ! {
    eprintln!("zatara-proxy: adapter write failed — the adapter is gone");
    std::process::exit(1);
}
#[cfg(test)]
mod exit_law_tests {
    use super::*;
    #[test]
    fn a_nonzero_exit_is_announced_at_error_level_naming_the_code() {
        let event = exit_error_event(3);
        assert_eq!(event["type"], "event");
        assert_eq!(event["event"], "output");
        assert_eq!(event["body"]["category"], "stderr");
        let output = event["body"]["output"].as_str().expect("output text");
        assert!(output.contains('3'), "the code must be named: {output}");
        assert!(output.ends_with('\n'), "a DAP output chunk is line-terminated");
    }
}
#[cfg(test)]
mod benign_config_done_tests {
    use super::*;
    fn refused_configuration_done() -> Value {
        json!({
            "type": "response",
            "seq": 42,
            "request_seq": 7,
            "command": "configurationDone",
            "success": false,
            "message": "Failed command 'configurationDone' : 0x80004005",
            "body": {},
        })
    }
    #[test]
    fn the_attach_lane_refusal_is_answered_as_success_with_the_engine_log_message() {
        let mut frame = refused_configuration_done();
        assert!(
            answer_benign_attach_config_done(&mut frame, "configurationDone", false),
            "the exact engine refusal on the attach/test lane must be rewritten"
        );
        assert_eq!(frame["success"], json!(true));
        assert_eq!(frame["body"], json!({}));
        assert!(frame.get("message").is_none(), "a success carries no refusal text");
        assert_eq!(frame["type"], "response");
        assert_eq!(frame["request_seq"], json!(7));
        assert_eq!(frame["command"], "configurationDone");
    }
    #[test]
    fn a_program_session_configurationDone_failure_is_never_rewritten() {
        let mut frame = refused_configuration_done();
        assert!(
            !answer_benign_attach_config_done(&mut frame, "configurationDone", true),
            "the program lane's failure must reach the client untouched"
        );
        assert_eq!(frame, refused_configuration_done());
    }
    #[test]
    fn other_failed_responses_and_other_failures_of_the_same_command_pass_through() {
        let mut step = refused_configuration_done();
        step["command"] = json!("next");
        let step_before = step.clone();
        assert!(
            !answer_benign_attach_config_done(&mut step, "next", false),
            "a refused step is the client's truth, not a benign resume refusal"
        );
        assert_eq!(step, step_before);
        let mut other = refused_configuration_done();
        other["message"] = json!("the session launch failed");
        let other_before = other.clone();
        assert!(
            !answer_benign_attach_config_done(&mut other, "configurationDone", false),
            "only the 0x80004005 resume-on-running signature is rewritten"
        );
        assert_eq!(other, other_before);
        let mut ok = refused_configuration_done();
        ok["success"] = json!(true);
        let ok_before = ok.clone();
        assert!(!answer_benign_attach_config_done(&mut ok, "configurationDone", false));
        assert_eq!(ok, ok_before);
    }
}
