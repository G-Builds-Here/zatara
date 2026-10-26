use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use portable_pty::{native_pty_system, CommandBuilder, ExitStatus, PtySize};
use serde_json::{json, Value};
use zatara_types::LaunchConfig;
use crate::relay::Tunnel;
const SPAWN_MARKER: &str = "zatara: spawn ";
const TREE_MARKER: &str = "zatara: tree pid ";
const DEFAULT_BANNER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const PTY_ROWS: u16 = 50;
const PTY_COLS: u16 = 220;
const PTY_READ_CHUNK_BYTES: usize = 8192;
const PUMP_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const PUMP_QUIET_WINDOW: std::time::Duration = std::time::Duration::from_millis(250);
const PUMP_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(25);
const TEST_SETTLE_GRACE: std::time::Duration = std::time::Duration::from_millis(500);
#[derive(Clone, Copy)]
pub(crate) struct RunnerConfig {
    pub(crate) banner_timeout: std::time::Duration,
}
impl RunnerConfig {
    pub(crate) fn from_env() -> Option<Self> {
        runner_armed(std::env::var("ZATARA_RUNNER").ok().as_deref()).then(|| RunnerConfig {
            banner_timeout: banner_timeout(
                std::env::var("ZATARA_RUNNER_BANNER_TIMEOUT_SECS").ok().as_deref(),
            ),
        })
    }
}
fn runner_armed(raw: Option<&str>) -> bool {
    raw != Some("0")
}
fn banner_timeout(raw: Option<&str>) -> std::time::Duration {
    raw.and_then(|s| s.parse::<u64>().ok())
        .map_or(DEFAULT_BANNER_TIMEOUT, std::time::Duration::from_secs)
}
const PAUSE_MARKER: &[u8] = b"Process Id: ";
pub(crate) fn start(
    tunnel: Arc<Tunnel>,
    runner: RunnerConfig,
    config: LaunchConfig,
    launch_seq: i64,
) {
    std::thread::spawn(move || run(tunnel, runner, config, launch_seq));
}
fn build_argv(config: &LaunchConfig) -> Vec<String> {
    vec!["dotnet".into(), "build".into(), config.project_path.clone()]
}
pub fn build_filter(config: &LaunchConfig) -> String {
    let fqn = &config.fully_qualified_name;
    match config.scope.as_deref().unwrap_or("method") {
        "case" => {
            let args: Vec<&str> = config.case_arguments.iter().map(String::as_str).map(str::trim).collect();
            match config.framework.as_deref().unwrap_or("xunit") {
                "nunit" => format!("FullyQualifiedName~{fqn}&FullyQualifiedName~{}", args.join(",")),
                "mstest" => format!("FullyQualifiedName~{fqn}&Name~{}", args.join(",")),
                _ => {
                    let derived: Vec<u32>;
                    let string_slots: &[u32] = if !config.case_string_slots.is_empty() {
                        &config.case_string_slots
                    } else {
                        derived = config
                            .case_raw_arguments
                            .iter()
                            .enumerate()
                            .filter(|(_, raw)| raw.starts_with('\u{22}'))
                            .map(|(i, _)| (i + 1) as u32)
                            .collect();
                        &derived
                    };
                    let positional_params = config.case_string_slots.is_empty();
                    if string_slots.is_empty() {
                        let display: Vec<String> = if config.method_parameters.is_empty() {
                            args.iter().map(|a| a.to_string()).collect()
                        } else {
                            config
                                .method_parameters
                                .iter()
                                .zip(args.iter())
                                .map(|(param, arg)| format!("{param}: {arg}"))
                                .collect()
                        };
                        format!("FullyQualifiedName~{fqn}&DisplayName~{}", display.join(", "))
                    } else {
                        let mut params = config.method_parameters.iter();
                        let parts: Vec<String> = args
                            .iter()
                            .enumerate()
                            .map(|(i, arg)| {
                                if string_slots.contains(&((i + 1) as u32)) {
                                    format!("DisplayName~{arg}")
                                } else if positional_params {
                                    match config.method_parameters.get(i) {
                                        Some(param) => format!("DisplayName~{param}: {arg}"),
                                        None => format!("DisplayName~{arg}"),
                                    }
                                } else {
                                    match params.next() {
                                        Some(param) => format!("DisplayName~{param}: {arg}"),
                                        None => format!("DisplayName~{arg}"),
                                    }
                                }
                            })
                            .collect();
                        format!("FullyQualifiedName~{fqn}&{}", parts.join("&"))
                    }
                }
            }
        }
        "class" => format!("FullyQualifiedName~{}", dot_terminate(fqn)),
        "file" => fqn
            .split('|')
            .map(|class| format!("FullyQualifiedName~{}", dot_terminate(class)))
            .collect::<Vec<_>>()
            .join("|"),
        _ => format!("FullyQualifiedName={fqn}"),
    }
}
fn dot_terminate(class: &str) -> String {
    if class.ends_with('.') {
        class.to_string()
    } else {
        format!("{class}.")
    }
}
pub(crate) fn test_argv(config: &LaunchConfig) -> Vec<String> {
    let mut argv = vec![
        "dotnet".into(),
        "test".into(),
        config.project_path.clone(),
        "--filter".into(),
        build_filter(config),
    ];
    if config.results_log {
        argv.push("--logger".into());
        argv.push("trx".into());
    }
    argv
}
pub(crate) fn work_dir(config: &LaunchConfig) -> String {
    if !config.cwd.is_empty() {
        return config.cwd.clone();
    }
    config
        .project_path
        .rsplit_once(['/', '\\'])
        .map(|(dir, _)| dir.to_string())
        .unwrap_or_default()
}
struct Step {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    pump_done: Option<std::sync::mpsc::Receiver<()>>,
    output_ticks: Arc<AtomicU64>,
}
impl Step {
    fn drain_pump(&mut self) {
        if let Some(done) = self.pump_done.take() {
            drain_until_quiet(
                &done,
                &self.output_ticks,
                PUMP_QUIET_WINDOW,
                PUMP_DRAIN_TIMEOUT,
            );
        }
    }
}
fn drain_until_quiet(
    done: &std::sync::mpsc::Receiver<()>,
    ticks: &AtomicU64,
    quiet: std::time::Duration,
    cap: std::time::Duration,
) {
    let start = std::time::Instant::now();
    let mut last_ticks = ticks.load(Ordering::Relaxed);
    let mut quiet_since = start;
    loop {
        match done.recv_timeout(PUMP_POLL_INTERVAL) {
            Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        }
        if start.elapsed() >= cap {
            return;
        }
        let now_ticks = ticks.load(Ordering::Relaxed);
        if now_ticks != last_ticks {
            last_ticks = now_ticks;
            quiet_since = std::time::Instant::now();
        } else if quiet_since.elapsed() >= quiet {
            return;
        }
    }
}
const BUILD_CATEGORY: &str = "build";
const TEST_CATEGORY: &str = "test";
fn spawn_step(
    tunnel: &Arc<Tunnel>,
    argv: &[String],
    cwd: &str,
    extra_env: &[(String, String)],
    pids: Option<std::sync::mpsc::Sender<u64>>,
    category: &'static str,
) -> Result<Step, String> {
    announce(tunnel, argv);
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: PTY_ROWS,
            cols: PTY_COLS,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("{}: opening the pty failed: {e}", argv[0]))?;
    let mut cmd = CommandBuilder::new(&argv[0]);
    cmd.args(&argv[1..]);
    if !cwd.is_empty() {
        cmd.cwd(cwd);
    }
    if let Some(path) = std::env::var_os("PATH") {
        cmd.env("PATH", path);
    }
    cmd.env("TERM", "dumb");
    for (key, value) in extra_env {
        cmd.env(key, value);
    }
    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("{}: spawn failed: {e}", argv[0]))?;
    let pid = child.process_id();
    tunnel.tree.adopt(pid);
    if let Some(pid) = pid {
        send_output(tunnel, format!("{TREE_MARKER}{pid}\n"), "stdout");
    }
    let reader = pair.master.try_clone_reader().ok();
    let answerer = pair.master.take_writer().ok();
    let stream_tunnel = Arc::clone(tunnel);
    let (pump_done_tx, pump_done_rx) = std::sync::mpsc::channel();
    let output_ticks = Arc::new(AtomicU64::new(0));
    let pump_ticks = Arc::clone(&output_ticks);
    std::thread::spawn(move || {
        stream_pty_output(&stream_tunnel, reader, answerer, pids, category, pump_ticks);
        drop(pair.master);
        let _ = pump_done_tx.send(());
    });
    drop(pair.slave);
    Ok(Step {
        child,
        pump_done: Some(pump_done_rx),
        output_ticks,
    })
}
#[derive(Default)]
struct TerminalAnswerer {
    tail: Vec<u8>,
}
impl TerminalAnswerer {
    fn on_chunk(&mut self, chunk: &[u8], writer: &mut dyn Write) {
        let mut window = std::mem::take(&mut self.tail);
        window.extend_from_slice(chunk);
        let mut responses: Vec<u8> = Vec::new();
        let mut from = 0;
        while let Some(at) = find_sub(&window, b"\x1b[6n", from) {
            responses.extend_from_slice(b"\x1b[2;1R");
            from = at + 4;
        }
        self.tail.extend_from_slice(&window[window.len().saturating_sub(3)..]);
        if !responses.is_empty() {
            let _ = writer.write_all(&responses);
            let _ = writer.flush();
        }
    }
}
fn find_sub(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if from >= hay.len() {
        return None;
    }
    hay[from..].windows(needle.len()).position(|w| w == needle).map(|p| p + from)
}
#[derive(Default)]
struct PauseWatcher {
    matched: usize,
    digits: Option<Vec<u8>>,
    fired: bool,
}
impl PauseWatcher {
    fn on_chunk(&mut self, chunk: &[u8]) -> Option<u64> {
        for &b in chunk {
            if self.fired {
                return None;
            }
            match self.digits.take() {
                Some(mut digits) if b.is_ascii_digit() => {
                    digits.push(b);
                    self.digits = Some(digits);
                    continue;
                }
                Some(digits) => {
                    if !digits.is_empty() {
                        if let Some(pid) =
                            std::str::from_utf8(&digits).ok().and_then(|s| s.parse::<u64>().ok())
                        {
                            self.fired = true;
                            return Some(pid);
                        }
                    }
                }
                None => {}
            }
            if b == PAUSE_MARKER[self.matched] {
                self.matched += 1;
                if self.matched == PAUSE_MARKER.len() {
                    self.matched = 0;
                    self.digits = Some(Vec::new());
                }
            } else {
                self.matched = 0;
                if b == PAUSE_MARKER[0] {
                    self.matched = 1;
                }
            }
        }
        None
    }
}
#[cfg(test)]
fn parse_pause_pid(text: &[u8]) -> Option<u64> {
    PauseWatcher::default().on_chunk(text)
}
const LINE_PENDING_BUDGET: usize = 64 * 1024;
#[derive(Default)]
struct LineBatcher {
    pending: Vec<u8>,
}
impl LineBatcher {
    fn on_chunk(&mut self, chunk: &[u8]) -> Option<String> {
        self.pending.extend_from_slice(chunk);
        let through = self.pending.iter().rposition(|&b| b == b'\n').map_or(0, |at| at + 1);
        let flush_len = match through {
            0 if self.pending.len() < LINE_PENDING_BUDGET => return None,
            0 => self.pending.len(),
            n => n,
        };
        let whole: Vec<u8> = self.pending.drain(..flush_len).collect();
        Some(String::from_utf8_lossy(&whole).into_owned())
    }
    fn finish(&mut self) -> Option<String> {
        if self.pending.is_empty() {
            return None;
        }
        Some(String::from_utf8_lossy(&std::mem::take(&mut self.pending)).into_owned())
    }
}
fn stream_pty_output(
    tunnel: &Tunnel,
    mut reader: Option<Box<dyn Read + Send>>,
    mut writer: Option<Box<dyn Write + Send>>,
    pids: Option<std::sync::mpsc::Sender<u64>>,
    category: &'static str,
    ticks: Arc<AtomicU64>,
) {
    let Some(mut reader) = reader.take() else { return };
    let mut answerer = TerminalAnswerer::default();
    let mut watcher = pids.is_some().then(PauseWatcher::default);
    let mut batcher = LineBatcher::default();
    let mut chunk = [0u8; PTY_READ_CHUNK_BYTES];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                ticks.fetch_add(1, Ordering::Relaxed);
                if let Some(w) = writer.as_mut() {
                    answerer.on_chunk(&chunk[..n], w.as_mut());
                }
                if let (Some(w), Some(tx)) = (watcher.as_mut(), pids.as_ref()) {
                    if let Some(pid) = w.on_chunk(&chunk[..n]) {
                        let _ = tx.send(pid);
                    }
                }
                if let Some(text) = batcher.on_chunk(&chunk[..n]) {
                    send_output(tunnel, text, category);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    if let Some(text) = batcher.finish() {
        send_output(tunnel, text, category);
    }
}
fn wait_for_exit(step: &mut Step, program: &str) -> Result<ExitStatus, String> {
    step.child.wait().map_err(|e| format!("{program}: waiting for the child failed: {e}"))
}
fn announce(tunnel: &Tunnel, argv: &[String]) {
    let line = format!("{SPAWN_MARKER}{}\n", Value::Array(argv.iter().map(|a| json!(a)).collect()));
    send_output(tunnel, line, "stdout");
}
fn send_output(tunnel: &Tunnel, text: String, category: &str) {
    tunnel.send_to_client(json!({
        "type": "event",
        "event": "output",
        "body": { "output": text, "category": category },
    }));
}
fn fail_launch(tunnel: &Tunnel, launch_seq: i64, reason: &str) {
    tunnel.error_response(launch_seq, "launch", reason);
    tunnel.kill_adapter();
}
fn fail_and_kill_test(tunnel: &Tunnel, test: &mut Step, launch_seq: i64, reason: &str) {
    kill_test_tree(test);
    let _ = test.child.wait();
    test.drain_pump();
    fail_launch(tunnel, launch_seq, reason);
}
fn test_env(config: &LaunchConfig) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> =
        config.env.iter().flat_map(|map| map.clone().into_iter()).collect();
    env.push(("VSTEST_HOST_DEBUG".to_string(), "1".to_string()));
    env
}
fn run_mtp_debug(tunnel: Arc<Tunnel>, config: &LaunchConfig, launch_seq: i64) {
    let csproj = Path::new(&config.project_path);
    if config.results_log {
        send_output(
            &tunnel,
            "zatara: resultsLog is not wired on the MTP debug lane (no dotnet test runner involved)\n"
                .to_string(),
            "stderr",
        );
    }
    let program = match crate::discovery::locate_built_dll(csproj) {
        Ok(dll) => dll,
        Err(e) => return fail_launch(&tunnel, launch_seq, &e.to_string()),
    };
    let case_id = if config.scope.as_deref() == Some("case") {
        match crate::mtp_case::resolve_case_id(&program, config) {
            Ok(id) => Some(id),
            Err(reason) => return fail_launch(&tunnel, launch_seq, &reason),
        }
    } else {
        None
    };
    let args = match mtp_test_args(config, case_id.as_deref()) {
        Ok(args) => args,
        Err(reason) => return fail_launch(&tunnel, launch_seq, &reason),
    };
    let cwd = work_dir(config);
    let mut argv = vec![program.display().to_string()];
    argv.extend(args.iter().cloned());
    announce(&tunnel, &argv);
    tunnel.begin_program_session();
    if let Err(e) =
        tunnel.launch_program(&program.display().to_string(), &args, &cwd, launch_seq)
    {
        fail_launch(&tunnel, launch_seq, &format!("launch{{program}}: {e}"));
    }
}
fn mtp_test_args(config: &LaunchConfig, case_id: Option<&str>) -> Result<Vec<String>, String> {
    let fqn = &config.fully_qualified_name;
    match config.scope.as_deref().unwrap_or("method") {
        "case" => match case_id {
            Some(id) => Ok(crate::mtp_case::id_args(id)),
            None => Err("zatara: the xunit v3 / MTP debug lane could not resolve this \
                        theory case to a runner test-case id — the row's arguments matched \
                        no enumerated case; click the method's gutter arrow to debug every \
                        case of it"
                .to_string()),
        },
        "class" => Ok(vec!["-class".to_string(), trim_class_dot(fqn)]),
        "file" => Ok(fqn
            .split('|')
            .flat_map(|class| ["-class".to_string(), trim_class_dot(class)])
            .collect()),
        _ => Ok(vec!["-method".to_string(), fqn.clone()]),
    }
}
fn trim_class_dot(name: &str) -> String {
    name.strip_suffix('.').unwrap_or(name).to_string()
}
fn kill_tree_pid(pid: u32) {
    #[cfg(windows)]
    {
        let mut walked = false;
        if let Some(parents) = process_parent_map() {
            let mut children: std::collections::HashMap<u32, Vec<u32>> =
                std::collections::HashMap::new();
            for (&child, &parent) in &parents {
                children.entry(parent).or_default().push(child);
            }
            let mut stack = vec![pid];
            let mut doomed = Vec::new();
            while let Some(current) = stack.pop() {
                doomed.push(current);
                if let Some(kids) = children.get(&current) {
                    stack.extend(kids.iter().copied());
                }
            }
            walked = doomed.contains(&pid);
            for victim in doomed.iter().rev() {
                terminate_process(*victim);
            }
        }
        if !walked {
            terminate_process(pid);
        }
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
    #[cfg(unix)]
    {
        let rc = unsafe { libc::kill(-(pid as libc::pid_t), libc::SIGKILL) };
        if rc != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
        }
    }
}
#[cfg(windows)]
fn terminate_process(pid: u32) {
    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(desired_access: u32, inherit_handle: i32, process_id: u32) -> *mut u8;
        fn TerminateProcess(process: *mut u8, exit_code: u32) -> i32;
        fn CloseHandle(handle: *mut u8) -> i32;
    }
    const PROCESS_TERMINATE: u32 = 0x0001;
    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if handle.is_null() {
            return;
        }
        TerminateProcess(handle, 1);
        CloseHandle(handle);
    }
}
fn kill_test_tree(step: &mut Step) {
    if let Some(pid) = step.child.process_id() {
        kill_tree_pid(pid);
    }
}
#[derive(Clone, Default)]
pub(crate) struct TreeRegistry {
    state: std::sync::Arc<std::sync::Mutex<TreeState>>,
}
#[derive(Default)]
struct TreeState {
    pids: Vec<u32>,
    #[cfg(windows)]
    job: Option<JobHandle>,
}
impl TreeRegistry {
    pub(crate) fn adopt(&self, pid: Option<u32>) {
        let Some(pid) = pid else { return };
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.pids.push(pid);
        #[cfg(windows)]
        {
            if state.job.is_none() {
                state.job = create_kill_on_close_job();
            }
            if let Some(job) = state.job.as_ref() {
                assign_to_job(job.0, pid);
            }
        }
    }
    pub(crate) fn kill_all(&self) {
        #[cfg_attr(not(windows), allow(unused_mut))]
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        #[cfg(windows)]
        {
            state.job = None;
        }
        for &pid in &state.pids {
            kill_tree_pid(pid);
        }
    }
}
#[cfg(windows)]
struct JobHandle(*mut u8);
#[cfg(windows)]
unsafe impl Send for JobHandle {}
#[cfg(windows)]
impl Drop for JobHandle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}
#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn CreateJobObjectW(attributes: *const u8, name: *const u16) -> *mut u8;
    fn SetInformationJobObject(job: *mut u8, class: i32, info: *const u8, length: u32) -> i32;
    fn AssignProcessToJobObject(job: *mut u8, process: *mut u8) -> i32;
    fn OpenProcess(access: u32, inherit_handle: i32, process_id: u32) -> *mut u8;
    fn CloseHandle(handle: *mut u8) -> i32;
}
#[cfg(windows)]
#[repr(C)]
#[derive(Default)]
struct JobObjectBasicLimitInformation {
    per_process_user_time_limit: i64,
    per_job_user_time_limit: i64,
    limit_flags: u32,
    minimum_working_set_size: usize,
    maximum_working_set_size: usize,
    active_process_limit: u32,
    affinity: usize,
    priority_class: u32,
    scheduling_class: u32,
}
#[cfg(windows)]
fn create_kill_on_close_job() -> Option<JobHandle> {
    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000;
    const JOB_OBJECT_BASIC_LIMIT_INFORMATION: i32 = 2;
    let armed = std::env::var(crate::logging::PROXY_LOG_ENV).ok().as_deref() == Some("1")
        || std::env::var(crate::logging::ENGINE_LOGGING_ENV).ok().as_deref() == Some("1");
    let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if job.is_null() {
        if armed {
            eprintln!(
                "zatara-proxy: CreateJobObjectW failed: {}",
                std::io::Error::last_os_error()
            );
        }
        return None;
    }
    let limits = JobObjectBasicLimitInformation {
        limit_flags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        ..Default::default()
    };
    let ok = unsafe {
        SetInformationJobObject(
            job,
            JOB_OBJECT_BASIC_LIMIT_INFORMATION,
            &limits as *const _ as *const u8,
            std::mem::size_of::<JobObjectBasicLimitInformation>() as u32,
        )
    };
    if ok == 0 {
        if armed {
            eprintln!(
                "zatara-proxy: SetInformationJobObject(KILL_ON_JOB_CLOSE) failed: {}",
                std::io::Error::last_os_error()
            );
        }
        unsafe { CloseHandle(job) };
        return None;
    }
    Some(JobHandle(job))
}
#[cfg(windows)]
fn assign_to_job(job: *mut u8, pid: u32) -> bool {
    const PROCESS_TERMINATE: u32 = 0x0001;
    const PROCESS_SET_QUOTA: u32 = 0x0100;
    let handle = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid) };
    if handle.is_null() {
        return false;
    }
    let ok = unsafe { AssignProcessToJobObject(job, handle) } != 0;
    unsafe { CloseHandle(handle) };
    ok
}
pub(crate) fn process_exists(pid: u64) -> bool {
    if pid < 2 {
        return false;
    }
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        extern "system" {
            fn OpenProcess(desired_access: u32, inherit_handle: i32, process_id: u32) -> *mut u8;
            fn CloseHandle(handle: *mut u8) -> i32;
        }
        const QUERY_LIMITED_INFORMATION: u32 = 0x1000;
        let Some(pid32) = u32::try_from(pid).ok() else { return false };
        unsafe {
            let handle = OpenProcess(QUERY_LIMITED_INFORMATION, 0, pid32);
            if handle.is_null() {
                return false;
            }
            CloseHandle(handle);
            true
        }
    }
    #[cfg(unix)]
    {
        let Ok(pid32) = i32::try_from(pid) else { return false };
        let rc = unsafe { libc::kill(pid32, 0) };
        rc == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    }
}
fn pid_descends_from(pid: u64, ancestor: u64) -> bool {
    if pid == ancestor {
        return true;
    }
    let (Some(pid), Some(ancestor)) = (u32::try_from(pid).ok(), u32::try_from(ancestor).ok())
    else {
        return false;
    };
    if pid < 2 {
        return false;
    }
    #[cfg(windows)]
    let parents = process_parent_map();
    #[cfg(windows)]
    let lookup = |current: u32| parents.as_ref().and_then(|map| map.get(&current).copied());
    #[cfg(not(windows))]
    let lookup = parent_of;
    descends_via(pid, ancestor, lookup)
}
const TREE_WALK_BOUND: u32 = 64;
fn descends_via(pid: u32, ancestor: u32, mut parent: impl FnMut(u32) -> Option<u32>) -> bool {
    let mut current = pid;
    for _ in 0..TREE_WALK_BOUND {
        match parent(current) {
            Some(p) if p == ancestor => return true,
            Some(0) => return false,
            Some(p) if p == current => return false,
            Some(p) => current = p,
            None => return false,
        }
    }
    false
}
#[cfg(windows)]
fn process_parent_map() -> Option<std::collections::HashMap<u32, u32>> {
    #[repr(C)]
    struct ProcessEntry32W {
        dw_size: u32,
        cnt_usage: u32,
        th32_process_id: u32,
        th32_default_heap_id: usize,
        th32_module_id: u32,
        cnt_threads: u32,
        th32_parent_process_id: u32,
        pc_pri_class_base: i32,
        dw_flags: u32,
        sz_exe_file: [u16; 260],
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn CreateToolhelp32Snapshot(flags: u32, process_id: u32) -> *mut u8;
        fn Process32FirstW(snapshot: *mut u8, entry: *mut ProcessEntry32W) -> i32;
        fn Process32NextW(snapshot: *mut u8, entry: *mut ProcessEntry32W) -> i32;
        fn CloseHandle(handle: *mut u8) -> i32;
    }
    const SNAP_PROCESS: u32 = 0x2;
    let snapshot = unsafe { CreateToolhelp32Snapshot(SNAP_PROCESS, 0) };
    if snapshot.is_null() || snapshot == -1isize as *mut u8 {
        return None;
    }
    let mut parents = std::collections::HashMap::new();
    unsafe {
        let mut entry: ProcessEntry32W = std::mem::zeroed();
        entry.dw_size = std::mem::size_of::<ProcessEntry32W>() as u32;
        if Process32FirstW(snapshot, &mut entry) != 0 {
            parents.insert(entry.th32_process_id, entry.th32_parent_process_id);
            while Process32NextW(snapshot, &mut entry) != 0 {
                parents.insert(entry.th32_process_id, entry.th32_parent_process_id);
            }
        }
        CloseHandle(snapshot);
    }
    Some(parents)
}
#[cfg(all(unix, not(target_os = "macos")))]
fn parent_of(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_comm = stat.rsplit_once(')')?.1;
    after_comm.split_whitespace().nth(1).and_then(|field| field.parse().ok())
}
#[cfg(target_os = "macos")]
fn parent_of(pid: u32) -> Option<u32> {
    #[link(name = "proc")]
    extern "C" {
        fn proc_pidinfo(
            pid: libc::c_int,
            flavor: libc::c_int,
            arg: u64,
            buffer: *mut libc::c_void,
            buffersize: libc::c_int,
        ) -> libc::c_int;
    }
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let rc = unsafe {
        proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTBSDINFO,
            0,
            &mut info as *mut libc::proc_bsdinfo as *mut libc::c_void,
            std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int,
        )
    };
    if rc <= 0 || info.pbi_pid != pid || info.pbi_ppid < 1 {
        return None;
    }
    Some(info.pbi_ppid)
}
pub(crate) fn resolve_project_path(config: &mut LaunchConfig) {
    let fuzzy = fuzzy_armed(std::env::var("ZATARA_FUZZY_MATCH").ok().as_deref());
    resolve_project_path_with(config, fuzzy);
}
fn fuzzy_armed(raw: Option<&str>) -> bool {
    raw == Some("1")
}
fn resolve_project_path_with(config: &mut LaunchConfig, fuzzy: bool) {
    if let Some(file) = gutter_file_from_env(&config) {
        let claimed = Path::new(config.project_path.as_str());
        let mut cursor = file.parent();
        while let Some(ancestor) = cursor {
            if let Some(project) = unique_project(ancestor) {
                if project != claimed {
                    eprintln!(
                        "zatara: projectPath '{}' resolved to '{}' (walked up from the gutter file '{}')",
                        config.project_path,
                        project.display(),
                        file.display()
                    );
                    config.project_path = project.display().to_string();
                }
                break;
            }
            if has_project_file(ancestor) {
                break;
            }
            cursor = ancestor.parent();
        }
    }
    let path = Path::new(config.project_path.as_str());
    if path.is_file() {
        return;
    }
    if !path.is_dir() {
        return;
    }
    if let Some(project) = unique_project(path) {
        eprintln!(
            "zatara: projectPath '{}' resolved to '{}'",
            config.project_path,
            project.display()
        );
        config.project_path = project.display().to_string();
        return;
    }
    if !has_project_file(path) {
        let mut cursor = path.parent();
        while let Some(ancestor) = cursor {
            if let Some(project) = unique_project(ancestor) {
                eprintln!(
                    "zatara: projectPath '{}' resolved to '{}' (walked up from the test file's directory)",
                    config.project_path,
                    project.display()
                );
                config.project_path = project.display().to_string();
                return;
            }
            cursor = ancestor.parent();
        }
    }
    if let Some(file) = gutter_file_from_env(&config) {
        let mut cursor = file.parent();
        while let Some(ancestor) = cursor {
            if let Some(project) = unique_project(ancestor) {
                eprintln!(
                    "zatara: projectPath '{}' resolved to '{}' (walked up from the gutter file '{}')",
                    config.project_path,
                    project.display(),
                    file.display()
                );
                config.project_path = project.display().to_string();
                return;
            }
            cursor = ancestor.parent();
        }
    }
    if fuzzy {
        let mut cursor = Some(path);
        while let Some(dir) = cursor {
            if let Some(solution) = unique_solution(dir) {
                eprintln!(
                    "zatara: projectPath '{}' resolved to '{}' (fuzzy match: nearest solution above the test file's directory)",
                    config.project_path,
                    solution.display()
                );
                config.project_path = solution.display().to_string();
                return;
            }
            cursor = dir.parent();
        }
    }
}
fn gutter_file_from_env(config: &LaunchConfig) -> Option<PathBuf> {
    let raw = config.env.as_ref()?.get("DOTNET_FILE")?;
    let path = Path::new(raw.as_str());
    path.is_file().then(|| path.to_path_buf())
}
fn has_project_file(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries.flatten().any(|entry| {
                let path = entry.path();
                path.is_file() && path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("csproj"))
            })
        })
        .unwrap_or(false)
}
fn unique_project(dir: &Path) -> Option<PathBuf> {
    let mut projects: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file() && path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("csproj"))
        })
        .collect();
    projects.sort();
    match projects.as_slice() {
        [only] => Some(only.clone()),
        [] => None,
        many => {
            let stem = dir.file_name()?;
            many.iter().find(|p| p.file_stem().is_some_and(|name| name == stem)).cloned()
        }
    }
}
fn unique_solution(dir: &Path) -> Option<PathBuf> {
    let mut solutions: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file() && path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("sln"))
        })
        .collect();
    solutions.sort();
    match solutions.as_slice() {
        [only] => Some(only.clone()),
        _ => None,
    }
}
fn run(tunnel: Arc<Tunnel>, runner: RunnerConfig, mut config: LaunchConfig, launch_seq: i64) {
    resolve_project_path(&mut config);
    let cwd = work_dir(&config);
    let mut build = match spawn_step(&tunnel, &build_argv(&config), &cwd, &[], None, BUILD_CATEGORY) {
        Ok(step) => step,
        Err(e) => return fail_launch(&tunnel, launch_seq, &e),
    };
    match wait_for_exit(&mut build, "dotnet build") {
        Ok(status) if status.success() => {}
        Ok(status) => {
            build.drain_pump();
            return fail_launch(
                &tunnel,
                launch_seq,
                &format!("dotnet build failed with exit code {}", status.exit_code()),
            );
        }
        Err(e) => {
            build.drain_pump();
            return fail_launch(&tunnel, launch_seq, &e);
        }
    }
    let mtp_lane = crate::discovery::mtp_test_project(Path::new(&config.project_path));
    tunnel.runner_lane_decided(mtp_lane);
    if mtp_lane {
        return run_mtp_debug(tunnel, &config, launch_seq);
    }
    let (pid_tx, pid_rx) = std::sync::mpsc::channel();
    let mut test = match spawn_step(
        &tunnel,
        &test_argv(&config),
        &cwd,
        &test_env(&config),
        Some(pid_tx),
        TEST_CATEGORY,
    ) {
        Ok(step) => step,
        Err(e) => return fail_launch(&tunnel, launch_seq, &e),
    };
    match pid_rx.recv_timeout(runner.banner_timeout) {
        Ok(pid) => {
            send_output(&tunnel, format!("zatara: parsed testhost pid: {pid}\n"), "stdout");
            if !process_exists(pid) {
                fail_and_kill_test(
                    &tunnel,
                    &mut test,
                    launch_seq,
                    &format!(
                        "test host pid {pid} named in the pause banner is not a live process — refusing to attach"
                    ),
                );
                return;
            }
            let anchored_in_tree = test
                .child
                .process_id()
                .is_some_and(|anchor| pid_descends_from(pid, u64::from(anchor)));
            if !anchored_in_tree {
                fail_and_kill_test(
                    &tunnel,
                    &mut test,
                    launch_seq,
                    &format!(
                        "test host pid {pid} named in the pause banner is not a process inside the launched test tree — refusing to attach"
                    ),
                );
                return;
            }
            if let Err(e) = tunnel.attach_process(pid, launch_seq, "launch") {
                fail_and_kill_test(
                    &tunnel,
                    &mut test,
                    launch_seq,
                    &format!("attach{{processId: {pid}}}: {e}"),
                );
                return;
            }
            let (settle_tx, settle_rx) = std::sync::mpsc::channel();
            tunnel.arm_test_settle(settle_rx);
            std::thread::spawn(move || {
                let mut test = test;
                let _ = test.child.wait();
                if let Some(done) = test.pump_done.take() {
                    drain_until_quiet(
                        &done,
                        &test.output_ticks,
                        PUMP_QUIET_WINDOW,
                        TEST_SETTLE_GRACE,
                    );
                }
                let _ = settle_tx.send(());
            });
        }
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            fail_and_kill_test(
                &tunnel,
                &mut test,
                launch_seq,
                &format!(
                    "test host did not pause within {}s: no 'Process Id:' banner appeared",
                    runner.banner_timeout.as_secs()
                ),
            );
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            if test.child.try_wait().map(|status| status.is_none()).unwrap_or(true) {
                kill_test_tree(&mut test);
                let _ = test.child.wait();
            }
            test.drain_pump();
            fail_launch(
                &tunnel,
                launch_seq,
                "test host did not pause: it exited before printing a 'Process Id:' banner",
            );
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_runner_arms_by_default_and_only_the_exact_zero_opts_out() {
        assert!(runner_armed(None), "unset env must arm: default-off was the new-user trap");
        assert!(runner_armed(Some("1")), "the historical arm value still arms");
        assert!(runner_armed(Some("yes")), "a typo'd arm value arms rather than silently off-ing");
        assert!(!runner_armed(Some("0")), "the one documented opt-out still opts out");
        assert!(runner_armed(Some("0 ")), "only the exact `0` opts out — a trailing space is a typo, not an opt-out");
    }
    #[test]
    fn the_mtp_grammar_maps_every_gutter_scope_to_the_runner_s_own_flags() {
        let method = LaunchConfig {
            fully_qualified_name: "Ns.Calc.Adds".into(),
            ..Default::default()
        };
        assert_eq!(mtp_test_args(&method, None).unwrap(), ["-method", "Ns.Calc.Adds"]);
        let odd = LaunchConfig {
            fully_qualified_name: "Ns.Calc.Adds".into(),
            scope: Some("something-else".into()),
            ..Default::default()
        };
        assert_eq!(mtp_test_args(&odd, None).unwrap(), ["-method", "Ns.Calc.Adds"]);
        let class = LaunchConfig {
            fully_qualified_name: "Ns.CalcTests.".into(),
            scope: Some("class".into()),
            ..Default::default()
        };
        assert_eq!(mtp_test_args(&class, None).unwrap(), ["-class", "Ns.CalcTests"]);
        let file = LaunchConfig {
            fully_qualified_name: "Ns.A|Ns.B".into(),
            scope: Some("file".into()),
            ..Default::default()
        };
        assert_eq!(
            mtp_test_args(&file, None).unwrap(),
            ["-class", "Ns.A", "-class", "Ns.B"],
            "the file's pipe-joined classes each ride their own -class flag"
        );
    }
    #[test]
    fn a_single_theory_case_rides_its_row_id_and_only_an_unresolved_one_refuses() {
        let case = LaunchConfig {
            fully_qualified_name: "Ns.Calc.AddsCases".into(),
            scope: Some("case".into()),
            case_arguments: vec!["1".into(), "2".into()],
            ..Default::default()
        };
        assert_eq!(
            mtp_test_args(&case, Some("abc123")).unwrap(),
            ["-preEnumerateTheories", "-id", "abc123"]
        );
        let err = mtp_test_args(&case, None)
            .expect_err("an unresolved case must refuse, not widen");
        assert!(
            err.contains("method") && err.contains("case"),
            "the refusal must name both what it cannot do and what to click: {err}"
        );
    }
    #[test]
    fn a_silent_pump_blocked_on_a_survivor_is_walked_away_from_early() {
        let (_done_tx, done_rx) = std::sync::mpsc::channel();
        let ticks = AtomicU64::new(0);
        let start = std::time::Instant::now();
        drain_until_quiet(&done_rx, &ticks, PUMP_QUIET_WINDOW, PUMP_DRAIN_TIMEOUT);
        let waited = start.elapsed();
        assert!(
            waited >= PUMP_QUIET_WINDOW,
            "a silent stream cannot be declared quiet before its window elapses: {waited:?}"
        );
        assert!(
            waited < std::time::Duration::from_secs(1),
            "the drain rode toward the old 5 s cap instead of leaving on the quiet window: {waited:?}"
        );
    }
    #[test]
    fn a_finished_pump_ends_the_drain_without_waiting_the_window() {
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        done_tx.send(()).unwrap();
        let ticks = AtomicU64::new(0);
        let start = std::time::Instant::now();
        drain_until_quiet(&done_rx, &ticks, PUMP_QUIET_WINDOW, PUMP_DRAIN_TIMEOUT);
        let waited = start.elapsed();
        assert!(
            waited < PUMP_QUIET_WINDOW,
            "a delivered stream must leave on its done-signal, not ride the window: {waited:?}"
        );
    }
    #[test]
    fn a_stream_still_producing_bytes_is_streamed_to_its_cap_not_the_window() {
        let (_done_tx, done_rx) = std::sync::mpsc::channel();
        let ticks = Arc::new(AtomicU64::new(0));
        let chatter = Arc::clone(&ticks);
        std::thread::spawn(move || {
            for _ in 0..40 {
                std::thread::sleep(std::time::Duration::from_millis(50));
                chatter.fetch_add(1, Ordering::Relaxed);
            }
        });
        let cap = std::time::Duration::from_millis(400);
        let start = std::time::Instant::now();
        drain_until_quiet(&done_rx, &ticks, PUMP_QUIET_WINDOW, cap);
        let waited = start.elapsed();
        assert!(
            waited >= cap,
            "bytes still arriving are the session's own output: the drain must ride to the cap, got {waited:?}"
        );
        assert!(
            waited < cap + std::time::Duration::from_millis(150),
            "the drain overran its cap: {waited:?}"
        );
    }
    #[test]
    fn a_stream_that_goes_quiet_mid_drain_ends_the_drain_early() {
        let (_done_tx, done_rx) = std::sync::mpsc::channel();
        let ticks = Arc::new(AtomicU64::new(0));
        let chatter = Arc::clone(&ticks);
        std::thread::spawn(move || {
            for _ in 0..4 {
                std::thread::sleep(std::time::Duration::from_millis(50));
                chatter.fetch_add(1, Ordering::Relaxed);
            }
        });
        let start = std::time::Instant::now();
        drain_until_quiet(
            &done_rx,
            &ticks,
            PUMP_QUIET_WINDOW,
            std::time::Duration::from_secs(5),
        );
        let waited = start.elapsed();
        assert!(
            waited > std::time::Duration::from_millis(350)
                && waited < std::time::Duration::from_millis(1200),
            "last byte ~200 ms in plus the quiet window should land near 450 ms, never near the 5 s cap: {waited:?}"
        );
    }
    #[test]
    fn a_line_written_across_two_reads_reaches_the_client_whole() {
        let mut batcher = LineBatcher::default();
        assert_eq!(batcher.on_chunk(b"Passed!  - Fail"), None);
        assert_eq!(
            batcher.on_chunk(b"ed: 0, Passed: 3, Skipped: 0\r\n"),
            Some("Passed!  - Failed: 0, Passed: 3, Skipped: 0\r\n".to_string()),
            "the console line assembled across two pty reads must arrive whole, its \\r\\n intact"
        );
    }
    #[test]
    fn several_closed_lines_in_one_read_flush_together_in_order() {
        let mut batcher = LineBatcher::default();
        assert_eq!(batcher.on_chunk(b"a\nb\nc"), Some("a\nb\n".to_string()));
        assert_eq!(batcher.finish(), Some("c".to_string()));
        assert_eq!(batcher.finish(), None, "a drained batcher finishes empty");
    }
    #[test]
    fn a_read_splitting_a_multibyte_codepoint_corrupts_nothing() {
        let mut batcher = LineBatcher::default();
        assert_eq!(batcher.on_chunk(&[0xC3]), None);
        assert_eq!(batcher.on_chunk(&[0xA9, b'\n']), Some("é\n".to_string()));
    }
    #[test]
    fn an_unterminated_tail_is_flushed_at_stream_end_not_dropped() {
        let mut batcher = LineBatcher::default();
        assert_eq!(batcher.on_chunk(b""), None, "an empty read delivers nothing");
        assert_eq!(batcher.on_chunk(b"a final, unterminated line"), None);
        assert_eq!(
            batcher.finish(),
            Some("a final, unterminated line".to_string()),
            "the last bytes a child printed must reach the client even without a closing newline"
        );
    }
    #[test]
    fn an_unterminated_stream_at_the_hold_budget_flushes_rather_than_grows() {
        let mut batcher = LineBatcher::default();
        let blob = vec![b'-'; LINE_PENDING_BUDGET - 1];
        assert_eq!(batcher.on_chunk(&blob), None, "just under the budget still holds");
        let flushed = batcher.on_chunk(b"-").expect("reaching the budget must force delivery");
        assert_eq!(
            flushed.len(),
            LINE_PENDING_BUDGET,
            "the forced flush delivers every held byte — the bound moves bytes to the client, it never drops them"
        );
        assert_eq!(batcher.finish(), None, "the forced flush emptied the hold");
    }
    #[test]
    fn answers_a_cursor_position_query_with_a_position_report() {
        let mut sink: Vec<u8> = Vec::new();
        TerminalAnswerer::default().on_chunk(b"building\x1b[6n...", &mut sink);
        assert_eq!(sink, b"\x1b[2;1R", "a CPR query must be answered exactly once");
    }
    #[test]
    fn answers_a_query_straddling_chunk_boundaries_once() {
        let mut answerer = TerminalAnswerer::default();
        let mut sink: Vec<u8> = Vec::new();
        answerer.on_chunk(b"restore \x1b[6", &mut sink);
        answerer.on_chunk(b"n done", &mut sink);
        assert_eq!(sink, b"\x1b[2;1R", "the straddled query must be answered exactly once");
    }
    #[test]
    fn answers_a_query_fragmented_below_the_carry_width() {
        let mut answerer = TerminalAnswerer::default();
        let mut sink: Vec<u8> = Vec::new();
        for fragment in ["\x1b", "[", "6", "n"] {
            answerer.on_chunk(fragment.as_bytes(), &mut sink);
        }
        assert_eq!(
            sink,
            b"\x1b[2;1R",
            "a query fragmented below the 3-byte carry width must still be answered exactly once"
        );
    }
    #[test]
    fn a_query_fully_scanned_never_answers_twice() {
        let mut answerer = TerminalAnswerer::default();
        let mut sink: Vec<u8> = Vec::new();
        answerer.on_chunk(b"\x1b[6n", &mut sink);
        answerer.on_chunk(b"rest", &mut sink);
        assert_eq!(sink, b"\x1b[2;1R", "no double answer for one query");
    }
    #[test]
    fn silent_streams_answer_nothing() {
        let mut sink: Vec<u8> = Vec::new();
        TerminalAnswerer::default().on_chunk(b"Build succeeded. 0 Warning(s)\n", &mut sink);
        assert!(sink.is_empty(), "ordinary build text must not trigger answers");
    }
    #[test]
    fn test_argv_carries_the_filter_as_two_separate_entries() {
        let config = LaunchConfig {
            project_path: "C:/My Docs/p/p.csproj".into(),
            fully_qualified_name: "Basic.AddTests.AddsOneAndTwo".into(),
            cwd: String::new(),
            env: None,
            ..Default::default()
        };
        assert_eq!(
            test_argv(&config),
            vec![
                "dotnet",
                "test",
                "C:/My Docs/p/p.csproj",
                "--filter",
                "FullyQualifiedName=Basic.AddTests.AddsOneAndTwo"
            ]
            .iter().map(|s| s.to_string()).collect::<Vec<String>>(),
            "argv-direct: spaced paths and filter expressions are whole entries, never quoted shell text"
        );
    }
    #[test]
    fn results_log_extends_the_test_step_with_dotnet_s_own_trx_logger() {
        let config = LaunchConfig {
            project_path: "C:/w/p/p.csproj".into(),
            fully_qualified_name: "P.T.AddsOneAndTwo".into(),
            cwd: String::new(),
            env: None,
            results_log: true,
            ..Default::default()
        };
        assert_eq!(
            test_argv(&config),
            vec![
                "dotnet",
                "test",
                "C:/w/p/p.csproj",
                "--filter",
                "FullyQualifiedName=P.T.AddsOneAndTwo",
                "--logger",
                "trx"
            ]
            .iter().map(|s| s.to_string()).collect::<Vec<String>>(),
            "the trx capture rides the same argv-direct law: whole entries, never quoted shell text"
        );
    }
    fn fs_temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zatara-proj-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }
    fn config_naming(project_path: &Path) -> LaunchConfig {
        LaunchConfig {
            project_path: project_path.display().to_string(),
            ..scoped("f", None, None)
        }
    }
    #[test]
    fn an_existing_project_file_never_moves() {
        let dir = fs_temp("file");
        let real = dir.join("p.csproj");
        std::fs::write(&real, b"<Project/>").unwrap();
        std::fs::write(&dir.join("Other.csproj"), b"<Project/>").unwrap();
        let mut config = config_naming(&real);
        resolve_project_path(&mut config);
        assert_eq!(config.project_path, real.display().to_string());
    }
    #[test]
    fn a_nested_test_project_overrides_the_folder_twin_claim() {
        let root = fs_temp("nested-twin");
        let twin = root.join("Root.csproj");
        let tests = root.join("Root.Tests");
        std::fs::create_dir_all(&tests).unwrap();
        std::fs::write(&twin, b"<Project/>").unwrap();
        std::fs::write(&tests.join("Root.Tests.csproj"), b"<Project/>").unwrap();
        let opened = tests.join("T.cs");
        std::fs::write(&opened, b"//").unwrap();
        let mut config = config_naming(&twin);
        config.env = Some(std::collections::BTreeMap::from([(
            "DOTNET_FILE".to_string(),
            opened.display().to_string(),
        )]));
        resolve_project_path_with(&mut config, false);
        assert_eq!(
            config.project_path,
            tests.join("Root.Tests.csproj").display().to_string(),
            "the file-proven lane outranks the claim-untouched law: an \
             existing but wrong folder-twin must not survive the walk"
        );
    }
    #[test]
    fn a_single_project_shape_keeps_its_claim_under_the_new_lane() {
        let root = fs_temp("nested-single");
        let claim = root.join("Root.csproj");
        std::fs::write(&claim, b"<Project/>").unwrap();
        let nested = root.join("Tests");
        std::fs::create_dir_all(&nested).unwrap();
        let opened = nested.join("T.cs");
        std::fs::write(&opened, b"//").unwrap();
        let mut config = config_naming(&claim);
        config.env = Some(std::collections::BTreeMap::from([(
            "DOTNET_FILE".to_string(),
            opened.display().to_string(),
        )]));
        resolve_project_path_with(&mut config, false);
        assert_eq!(
            config.project_path,
            claim.display().to_string(),
            "the walk up from the gutter file names the claimed project \
             itself — nothing moves"
        );
    }
    #[test]
    fn a_tie_above_the_gutter_file_never_overrides_a_claim() {
        let root = fs_temp("nested-tie");
        let tied = root.join("tied");
        std::fs::create_dir_all(&tied).unwrap();
        std::fs::write(&tied.join("A.csproj"), b"<Project/>").unwrap();
        std::fs::write(&tied.join("B.csproj"), b"<Project/>").unwrap();
        std::fs::write(&root.join("Root.csproj"), b"<Project/>").unwrap();
        let opened = tied.join("T.cs");
        std::fs::write(&opened, b"//").unwrap();
        let claim = tied.join("A.csproj");
        let mut config = config_naming(&claim);
        config.env = Some(std::collections::BTreeMap::from([(
            "DOTNET_FILE".to_string(),
            opened.display().to_string(),
        )]));
        resolve_project_path_with(&mut config, false);
        assert_eq!(
            config.project_path,
            claim.display().to_string(),
            "a tie on the way up is a refusal, not a cue to keep walking \
             and adopt some ancestor's project over the held claim"
        );
    }
    #[test]
    fn a_directory_with_one_project_becomes_that_project() {
        let dir = fs_temp("single");
        std::fs::write(&dir.join("xunit.csproj"), b"<Project/>").unwrap();
        std::fs::write(&dir.join("readme.txt"), b"distractor".as_slice()).unwrap();
        let mut config = config_naming(&dir);
        resolve_project_path(&mut config);
        assert_eq!(
            config.project_path,
            dir.join("xunit.csproj").display().to_string(),
            "the directory's single project is the provable answer (live shape, owner 2026-10-13)"
        );
    }
    #[test]
    fn a_nested_test_folder_walks_up_to_its_project() {
        let root = fs_temp("walk");
        let project = root.join("eCommerceStarterCode.Tests");
        let nested = project.join("Controllers");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(&project.join("eCommerceStarterCode.Tests.csproj"), b"<Project/>").unwrap();
        std::fs::write(&nested.join("CartControllerTests.cs"), b"//").unwrap();
        let mut config = config_naming(&nested);
        resolve_project_path(&mut config);
        assert_eq!(
            config.project_path,
            project.join("eCommerceStarterCode.Tests.csproj").display().to_string(),
            "the nearest ancestor naming exactly one project is the provable answer"
        );
        let tied = root.join("tied");
        std::fs::create_dir_all(&tied).unwrap();
        std::fs::write(&tied.join("A.csproj"), b"<Project/>").unwrap();
        std::fs::write(&tied.join("B.csproj"), b"<Project/>").unwrap();
        let mut tie = config_naming(&tied);
        resolve_project_path(&mut tie);
        assert_eq!(
            tie.project_path,
            tied.display().to_string(),
            "ambiguity is dotnet's honest error to report, not ours to walk away from"
        );
    }
    #[test]
    fn a_solution_rooted_gutter_click_finds_its_project_below_the_root() {
        let root = fs_temp("slnroot");
        let tests = root.join("tests").join("V3Tests");
        let api = root.join("src").join("Api");
        std::fs::create_dir_all(&tests).unwrap();
        std::fs::create_dir_all(&api).unwrap();
        std::fs::write(&root.join("repo.sln"), b"# solution").unwrap();
        std::fs::write(&api.join("Api.csproj"), b"<Project/>").unwrap();
        std::fs::write(&tests.join("V3Tests.csproj"), b"<Project/>").unwrap();
        let opened = tests.join("CalcTests.cs");
        std::fs::write(&opened, b"//").unwrap();
        let mut config = config_naming(&root);
        config.env = Some(std::collections::BTreeMap::from([(
            "DOTNET_FILE".to_string(),
            opened.display().to_string(),
        )]));
        resolve_project_path_with(&mut config, true);
        assert_eq!(
            config.project_path,
            tests.join("V3Tests.csproj").display().to_string(),
            "the gutter file's own project beats even the fuzzy solution — \
             the file PROVES the aim, a solution builds the whole repo"
        );
        let mut literal = config_naming(&root);
        literal.env = Some(std::collections::BTreeMap::from([(
            "DOTNET_FILE".to_string(),
            "$ZED_FILE".to_string(),
        )]));
        resolve_project_path_with(&mut literal, true);
        assert_eq!(
            literal.project_path,
            root.join("repo.sln").display().to_string(),
            "a non-file DOTNET_FILE changes nothing: the fuzzy solution still answers"
        );
    }
    #[test]
    fn an_ambiguous_directory_resolves_only_by_the_naming_convention() {
        let named = fs_temp("namematch");
        let stem = named.file_name().unwrap().to_string_lossy().into_owned();
        std::fs::write(&named.join(format!("{stem}.csproj")), b"<Project/>").unwrap();
        std::fs::write(&named.join("Helpers.csproj"), b"<Project/>").unwrap();
        let mut config = config_naming(&named);
        resolve_project_path(&mut config);
        assert_eq!(
            config.project_path,
            named.join(format!("{stem}.csproj")).display().to_string(),
            "a project named after its directory is the .NET convention locator.rs encodes"
        );
        let tie = fs_temp("tie");
        std::fs::write(&tie.join("Alpha.csproj"), b"<Project/>").unwrap();
        std::fs::write(&tie.join("Beta.csproj"), b"<Project/>").unwrap();
        let mut config = config_naming(&tie);
        resolve_project_path(&mut config);
        assert_eq!(
            config.project_path,
            tie.display().to_string(),
            "an unresolved tie stays the caller's — dotnet names the ambiguity, not us"
        );
    }
    #[test]
    fn a_missing_project_path_is_never_rewritten() {
        let dir = fs_temp("missing");
        std::fs::write(&dir.join("Main.csproj"), b"<Project/>").unwrap();
        let missing = dir.join("Main.typo.csproj");
        let mut config = config_naming(&missing);
        resolve_project_path(&mut config);
        assert_eq!(config.project_path, missing.display().to_string());
    }
    #[test]
    fn the_fuzzy_gate_opens_only_for_the_exact_one() {
        assert!(fuzzy_armed(Some("1")));
        assert!(!fuzzy_armed(None));
        assert!(!fuzzy_armed(Some("0")));
        assert!(!fuzzy_armed(Some("yes")));
        assert!(!fuzzy_armed(Some("1 ")));
    }
    #[test]
    fn a_solution_answers_where_the_strict_law_refused() {
        let root = fs_temp("fuzzy-ambig");
        let tied = root.join("tied");
        std::fs::create_dir_all(&tied).unwrap();
        std::fs::write(&tied.join("A.csproj"), b"<Project/>").unwrap();
        std::fs::write(&tied.join("B.csproj"), b"<Project/>").unwrap();
        std::fs::write(&root.join("Repo.sln"), b"# solution").unwrap();
        let mut fuzzy = config_naming(&tied);
        resolve_project_path_with(&mut fuzzy, true);
        assert_eq!(
            fuzzy.project_path,
            root.join("Repo.sln").display().to_string(),
            "armed fuzzy takes the nearest solution where strict refused"
        );
        let mut strict = config_naming(&tied);
        resolve_project_path_with(&mut strict, false);
        assert_eq!(
            strict.project_path,
            tied.display().to_string(),
            "the same tree, unequipped, keeps the old refusal byte-for-byte"
        );
    }
    #[test]
    fn a_walk_that_found_no_project_takes_the_nearest_solution() {
        let root = fs_temp("fuzzy-walk");
        let nested = root.join("tests").join("Controllers");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(&root.join("Only.sln"), b"# solution").unwrap();
        let mut fuzzy = config_naming(&nested);
        resolve_project_path_with(&mut fuzzy, true);
        assert_eq!(
            fuzzy.project_path,
            root.join("Only.sln").display().to_string(),
            "fuzzy walks: the NEAREST ancestor naming one solution wins"
        );
        let mut strict = config_naming(&nested);
        resolve_project_path_with(&mut strict, false);
        assert_eq!(
            strict.project_path,
            nested.display().to_string(),
            "unarmed, the empty walk-up still leaves dotnet's MSB1003 in place"
        );
    }
    #[test]
    fn the_solution_fallback_never_guesses_between_two_solutions() {
        let dir = fs_temp("fuzzy-twosln");
        std::fs::write(&dir.join("Alpha.sln"), b"# a").unwrap();
        std::fs::write(&dir.join("Beta.sln"), b"# b").unwrap();
        let mut config = config_naming(&dir);
        resolve_project_path_with(&mut config, true);
        assert_eq!(
            config.project_path,
            dir.display().to_string(),
            "two solutions is an ambiguity like two projects: dotnet names it, we don't"
        );
    }
    #[test]
    fn a_strict_answer_beats_a_solution_under_fuzzy() {
        let dir = fs_temp("fuzzy-strictfirst");
        std::fs::write(&dir.join("Only.csproj"), b"<Project/>").unwrap();
        std::fs::write(&dir.join("Repo.sln"), b"# solution").unwrap();
        let mut config = config_naming(&dir);
        resolve_project_path_with(&mut config, true);
        assert_eq!(
            config.project_path,
            dir.join("Only.csproj").display().to_string(),
            "the strict provable answer keeps precedence armed or not"
        );
    }
    fn scoped(fqn: &str, scope: Option<&str>, framework: Option<&str>) -> LaunchConfig {
        LaunchConfig {
            project_path: "p.csproj".into(),
            fully_qualified_name: fqn.into(),
            cwd: String::new(),
            env: None,
            scope: scope.map(String::from),
            framework: framework.map(String::from),
            case_arguments: Vec::new(),
            method_parameters: Vec::new(),
            results_log: false,
            ..Default::default()
        }
    }
    #[test]
    fn the_method_scope_filter_is_untouched_by_scope_awareness() {
        assert_eq!(
            build_filter(&scoped("Basic.AddTests.AddsOneAndTwo", None, None)),
            "FullyQualifiedName=Basic.AddTests.AddsOneAndTwo"
        );
        assert_eq!(
            build_filter(&scoped("Basic.AddTests.AddsOneAndTwo", Some("method"), None)),
            "FullyQualifiedName=Basic.AddTests.AddsOneAndTwo"
        );
    }
    #[test]
    fn an_nunit_case_filter_pairs_the_method_with_the_case_body() {
        let mut config = scoped("Ns.CalcTests.AddsCases", Some("case"), Some("nunit"));
        config.case_arguments = vec!["1".into(), "2".into(), "3".into()];
        assert_eq!(
            build_filter(&config),
            "FullyQualifiedName~Ns.CalcTests.AddsCases&FullyQualifiedName~1,2,3"
        );
    }
    #[test]
    fn an_mstest_row_filter_selects_the_row_by_name() {
        let mut config = scoped("Ns.CalcTests.AddsCases", Some("case"), Some("mstest"));
        config.case_arguments = vec!["2".into(), "3".into(), "5".into()];
        assert_eq!(
            build_filter(&config),
            "FullyQualifiedName~Ns.CalcTests.AddsCases&Name~2,3,5"
        );
    }
    #[test]
    fn an_xunit_case_filter_zips_parameters_with_arguments() {
        let mut config = scoped("Ns.TheoryTests.AddsCases", Some("case"), Some("xunit"));
        config.case_arguments = vec!["2".into(), "3".into(), "5".into()];
        config.method_parameters = vec!["a".into(), "b".into(), "expected".into()];
        assert_eq!(
            build_filter(&config),
            "FullyQualifiedName~Ns.TheoryTests.AddsCases&DisplayName~a: 2, b: 3, expected: 5"
        );
    }
    #[test]
    fn a_case_filter_without_parameters_falls_back_to_positional_display() {
        let mut config = scoped("Ns.TheoryTests.AddsCases", Some("case"), Some("xunit"));
        config.case_arguments = vec!["1".into(), "2".into()];
        assert_eq!(
            build_filter(&config),
            "FullyQualifiedName~Ns.TheoryTests.AddsCases&DisplayName~1, 2"
        );
    }
    #[test]
    fn a_string_slot_case_filter_ands_content_and_param_parts() {
        let mut config = scoped("Ns.TheoryTests.Mixed3", Some("case"), Some("xunit"));
        config.case_arguments = vec!["a".into(), "1".into(), "True".into()];
        config.method_parameters = vec!["n".into(), "flag".into()];
        config.case_string_slots = vec![1];
        assert_eq!(
            build_filter(&config),
            "FullyQualifiedName~Ns.TheoryTests.Mixed3&DisplayName~a&DisplayName~n: 1&DisplayName~flag: True"
        );
    }
    #[test]
    fn string_slot_markers_keep_the_parameter_names_slot_aligned() {
        let mut config = scoped("Ns.TheoryTests.Middle", Some("case"), Some("xunit"));
        config.case_arguments = vec!["1".into(), "x".into(), "2".into()];
        config.method_parameters = vec!["a".into(), "b".into()];
        config.case_string_slots = vec![2];
        assert_eq!(
            build_filter(&config),
            "FullyQualifiedName~Ns.TheoryTests.Middle&DisplayName~a: 1&DisplayName~x&DisplayName~b: 2"
        );
    }
    #[test]
    fn no_string_slot_markers_keep_the_classic_zip_byte_for_byte() {
        let mut config = scoped("Ns.TheoryTests.AddsCases", Some("case"), Some("xunit"));
        config.case_arguments = vec!["2".into(), "3".into(), "5".into()];
        config.method_parameters = vec!["a".into(), "b".into(), "expected".into()];
        config.case_string_slots = Vec::new();
        assert_eq!(
            build_filter(&config),
            "FullyQualifiedName~Ns.TheoryTests.AddsCases&DisplayName~a: 2, b: 3, expected: 5"
        );
    }
    #[test]
    fn raw_values_derive_the_string_slots_and_match_the_marker_lane() {
        let mut config = scoped("Ns.TheoryTests.Mixed3", Some("case"), Some("xunit"));
        config.case_arguments = vec!["a".into(), "1".into(), "True".into()];
        config.method_parameters = vec!["s".into(), "n".into(), "flag".into()];
        config.case_raw_arguments = vec!["\"a\"".into(), "1".into(), "True".into()];
        assert_eq!(
            build_filter(&config),
            "FullyQualifiedName~Ns.TheoryTests.Mixed3&DisplayName~a&DisplayName~n: 1&DisplayName~flag: True"
        );
    }
    #[test]
    fn all_primitive_raw_values_keep_the_classic_zip() {
        let mut config = scoped("Ns.TheoryTests.TwoInts", Some("case"), Some("xunit"));
        config.case_arguments = vec!["1".into(), "2".into()];
        config.method_parameters = vec!["a".into(), "b".into()];
        config.case_raw_arguments = vec!["1".into(), "2".into()];
        assert_eq!(
            build_filter(&config),
            "FullyQualifiedName~Ns.TheoryTests.TwoInts&DisplayName~a: 1, b: 2"
        );
    }
    #[test]
    fn shipped_string_slot_markers_win_over_raw_derivation() {
        let mut config = scoped("Ns.TheoryTests.Middle", Some("case"), Some("xunit"));
        config.case_arguments = vec!["x".into(), "y".into()];
        config.method_parameters = vec!["b".into()];
        config.case_string_slots = vec![2];
        config.case_raw_arguments = vec!["\"x\"".into(), "y".into()];
        assert_eq!(
            build_filter(&config),
            "FullyQualifiedName~Ns.TheoryTests.Middle&DisplayName~b: x&DisplayName~y"
        );
    }
    #[test]
    fn a_class_filter_is_the_trailing_dot_substring() {
        assert_eq!(
            build_filter(&scoped("Ns.FirstTests", Some("class"), None)),
            "FullyQualifiedName~Ns.FirstTests."
        );
        assert_eq!(
            build_filter(&scoped("Ns.FirstTests.", Some("class"), None)),
            "FullyQualifiedName~Ns.FirstTests.",
            "an already-dotted class name must not gain a second dot"
        );
    }
    #[test]
    fn a_file_filter_ors_the_classes_of_the_file() {
        assert_eq!(
            build_filter(&scoped("Ns.FirstTests|Ns.SecondTests", Some("file"), None)),
            "FullyQualifiedName~Ns.FirstTests.|FullyQualifiedName~Ns.SecondTests."
        );
    }
    #[test]
    fn forced_host_debug_overrides_any_config_value() {
        let mut env = std::collections::BTreeMap::new();
        env.insert("ZATARA_SESSION".to_string(), "b".to_string());
        env.insert("VSTEST_HOST_DEBUG".to_string(), "0".to_string());
        let config = LaunchConfig {
            project_path: "p.csproj".into(),
            fully_qualified_name: "f".into(),
            cwd: String::new(),
            env: Some(env),
            ..Default::default()
        };
        let pairs = test_env(&config);
        assert!(pairs.contains(&("ZATARA_SESSION".to_string(), "b".to_string())), "config env must survive");
        let host_debug: Vec<&str> =
            pairs.iter().filter(|(k, _)| k == "VSTEST_HOST_DEBUG").map(|(_, v)| v.as_str()).collect();
        assert_eq!(host_debug.last().copied(), Some("1"), "the pause flag must be forced last, so it wins");
        assert!(host_debug.len() >= 2, "the config's value must be present too — proven overridden by order");
    }
    #[test]
    fn work_dir_falls_back_to_the_project_directory() {
        let mut config = LaunchConfig {
            project_path: "C:/w/fx/Basic/Basic.csproj".into(),
            fully_qualified_name: "f".into(),
            cwd: String::new(),
            env: None,
            ..Default::default()
        };
        assert_eq!(work_dir(&config), "C:/w/fx/Basic");
        config.cwd = "C:/elsewhere".into();
        assert_eq!(work_dir(&config), "C:/elsewhere", "an explicit cwd wins");
    }
    #[test]
    fn parses_the_pid_from_the_pause_banner() {
        let banner = b"Host debugging is enabled. Please attach debugger to \
                      testhost process to continue.\r\n\
                      Process Id: 24680, Name: testhost\r\n";
        assert_eq!(parse_pause_pid(banner), Some(24680));
    }
    #[test]
    fn finds_a_pid_whose_banner_straddles_chunks() {
        let mut watcher = PauseWatcher::default();
        assert_eq!(watcher.on_chunk(b"test host chatter\x1b[2K Process Id: 246"), None);
        assert_eq!(watcher.on_chunk(b"80, Name: testhost\r\n"), Some(24680));
    }
    #[test]
    fn ordinary_output_hides_no_pid() {
        assert_eq!(parse_pause_pid(b"Passed! - Failed: 0, Passed: 1\r\n"), None);
        assert_eq!(parse_pause_pid(b"Process Id: nope, Name: x"), None);
        assert_eq!(parse_pause_pid(b"Process Id: 99999999999999999999999, Name: x"), None);
    }
    #[test]
    fn only_the_first_banner_fires() {
        let mut watcher = PauseWatcher::default();
        assert_eq!(watcher.on_chunk(b"Process Id: 11, Name: testhost"), Some(11));
        assert_eq!(watcher.on_chunk(b"Process Id: 22, Name: testhost"), None);
    }
    #[test]
    fn the_banner_knob_defaults_then_parses() {
        assert_eq!(banner_timeout(None), std::time::Duration::from_secs(30));
        assert_eq!(banner_timeout(Some("3")), std::time::Duration::from_secs(3));
        assert_eq!(
            banner_timeout(Some("not-a-number")),
            std::time::Duration::from_secs(30),
            "an unparsable knob must not poison the default"
        );
    }
    #[test]
    fn this_process_is_live_to_the_probe() {
        assert!(process_exists(u64::from(std::process::id())));
    }
    #[test]
    fn a_reaped_child_is_not_live_to_the_probe() {
        let mut cmd = std::process::Command::new(if cfg!(windows) { "ping" } else { "true" });
        if cfg!(windows) {
            cmd.args(["-n", "1", "127.0.0.1"]);
        }
        let mut child = cmd
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("short-lived probe target");
        let pid = u64::from(child.id());
        child.wait().expect("probe target reaped");
        drop(child);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while process_exists(pid) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(!process_exists(pid), "a reaped pid must read as dead");
    }
    #[test]
    fn a_pid_beyond_the_platform_range_is_not_live() {
        assert!(!process_exists(u64::MAX), "no platform has a process number that large");
    }
    #[test]
    fn reserved_pids_are_never_live_to_the_probe() {
        assert!(!process_exists(0), "pid 0 is a signal-group target, not an attachable process");
        assert!(!process_exists(1), "the OS root is never an attach target");
    }
    #[test]
    fn a_spawned_child_descends_from_its_launcher() {
        let mut cmd = std::process::Command::new(if cfg!(windows) { "ping" } else { "sleep" });
        if cfg!(windows) {
            cmd.args(["-n", "10", "127.0.0.1"]);
        } else {
            cmd.arg("10");
        }
        let mut child = cmd
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("descendant probe target");
        let pid = u64::from(child.id());
        let own = u64::from(std::process::id());
        assert!(pid_descends_from(pid, own), "a process this test spawned is inside its tree");
        assert!(pid_descends_from(own, own), "an anchor is inside its own tree");
        assert!(!pid_descends_from(own, pid), "the launcher does not descend from what it launched");
        assert!(
            !pid_descends_from(0, own) && !pid_descends_from(1, own),
            "reserved pids belong to no tree"
        );
        assert!(!pid_descends_from(u64::MAX, own), "no platform numbers reach that high");
        let _ = child.kill();
        let _ = child.wait();
    }
    #[test]
    fn the_tree_walk_stops_at_a_broken_link_a_cycle_and_the_bound() {
        let chain = std::collections::HashMap::from([(4u32, 3u32), (3, 2), (2, 1)]);
        assert!(
            descends_via(4, 1, |current| chain.get(&current).copied()),
            "a three hop chain reaches its root"
        );
        assert!(
            !descends_via(4, 9, |current| chain.get(&current).copied()),
            "a chain that never names the anchor is not that tree"
        );
        let cycle = std::collections::HashMap::from([(2u32, 3u32), (3, 2)]);
        assert!(
            !descends_via(2, 7, |current| cycle.get(&current).copied()),
            "a parent cycle ends at the walk bound instead of spinning forever"
        );
        assert!(!descends_via(4, 1, |_| None), "a parent the OS cannot name is not in the tree");
        assert!(
            !descends_via(2, 1, |current| if current == 2 { Some(0) } else { None }),
            "parent 0 is the tree floor: nothing descends through it"
        );
    }
    #[cfg(unix)]
    #[test]
    fn a_grandchild_descends_through_a_parent_that_is_not_the_anchor() {
        let mut shell = std::process::Command::new("sh")
            .args(["-c", "sleep 30 & echo $!; wait"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("a two level tree");
        let shell_pid = shell.id();
        let printed = {
            use std::io::BufRead;
            let pipe = shell.stdout.take().expect("stdout is piped");
            let mut line = String::new();
            std::io::BufReader::new(pipe)
                .read_line(&mut line)
                .expect("the grandchild pid is printed");
            line
        };
        let grandchild: u32 = printed.trim().parse().expect("a pid on stdout");
        assert!(
            parent_of(grandchild).is_some_and(|parent| parent == shell_pid),
            "the lookup names the direct parent, not the anchor"
        );
        assert!(
            pid_descends_from(u64::from(grandchild), u64::from(std::process::id())),
            "a grandchild is inside the launched tree the same way a test host is"
        );
        unsafe { libc::kill(grandchild as libc::pid_t, libc::SIGKILL) };
        let _ = shell.kill();
        let _ = shell.wait();
    }
    fn spawn_group_leader_sleeper() -> std::process::Child {
        let mut cmd = std::process::Command::new(if cfg!(windows) { "ping" } else { "sleep" });
        if cfg!(windows) {
            cmd.args(["-n", "300", "127.0.0.1"]);
        } else {
            cmd.arg("300");
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                cmd.process_group(0);
            }
        }
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("group-leader sleeper spawns")
    }
    fn terminated_within(child: &mut std::process::Child, budget: std::time::Duration) -> bool {
        let deadline = std::time::Instant::now() + budget;
        loop {
            if matches!(child.try_wait(), Ok(Some(_))) {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
    #[test]
    fn kill_all_takes_every_adopted_tree_within_the_teardown_bound() {
        let mut sleeper = spawn_group_leader_sleeper();
        let registry = TreeRegistry::default();
        registry.adopt(Some(sleeper.id()));
        registry.kill_all();
        registry.kill_all();
        assert!(
            terminated_within(&mut sleeper, std::time::Duration::from_secs(5)),
            "the session's own teardown must end its tree inside the 5 s bound"
        );
        let _ = sleeper.wait();
    }
    #[test]
    fn adopting_and_killing_an_absent_pid_changes_nothing() {
        let mut sleeper = std::process::Command::new(if cfg!(windows) { "ping" } else { "true" })
            .args(if cfg!(windows) { ["-n", "1", "127.0.0.1"].as_slice() } else { [].as_slice() })
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("short-lived adoption target");
        let pid = sleeper.id();
        sleeper.wait().expect("reaped before adoption");
        let registry = TreeRegistry::default();
        registry.adopt(Some(pid));
        registry.kill_all();
        assert!(process_exists(u64::from(std::process::id())), "the survivor of a dead-pid kill is the killer itself — still here");
    }
    #[cfg(windows)]
    #[test]
    fn a_closed_kill_on_close_job_takes_the_processes_inside_it() {
        let Some(job) = create_kill_on_close_job() else {
            eprintln!(
                "job configuration refused by this environment (sandboxed kernel?) — skipping; proxy takes its explicit-kill fallback (see integration crash test)"
            );
            return;
        };
        let mut sleeper = std::process::Command::new("ping")
            .args(["-n", "300", "127.0.0.1"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("job member spawns");
        if !assign_to_job(job.0, sleeper.id()) {
            eprintln!(
                "process is already in a non-nestable job (sandboxed harness?) — skipping assignment leg; proxy behaves the same way and falls back to explicit kill"
            );
            let _ = sleeper.kill();
            let _ = sleeper.wait();
            return;
        }
        drop(job);
        assert!(
            terminated_within(&mut sleeper, std::time::Duration::from_secs(5)),
            "closing the job's last handle — a process death, by any means — must take its members"
        );
        let _ = sleeper.wait();
    }
}
