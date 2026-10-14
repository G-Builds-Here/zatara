//! In-proxy build orchestration (LOC-0109): a `ZATARA_RUNNER=1` launch runs
//! `dotnet build`, then a paused `dotnet test` in a pty, and drives
//! `attach{processId}` into the relay's own adapter child — no pid files, no
//! %TEMP% rendezvous, no shell quoting layers (blueprint DR-2/DR-3).
//!
//! The relay hands the launch over in its launch-validation Ok arm — the
//! same verdict that grants `SessionPhase::Launched` (only
//! [`Session::accept_launch`](crate::session::Session::accept_launch) does
//! that, in that arm, unchanged) — and from there the pipeline owns the
//! request: the raw launch is *not* forwarded to the adapter when the runner
//! is armed (the debugger receives `attach`, never `launch`), and the
//! client's launch request is answered by the pipeline's outcome. The
//! runner speaks to the client and the adapter only through the relay's
//! tunnel.
//!
//! Armed by env (like `ZATARA_NETCOREDBG`), off by default: the raw-launch
//! relay stays the shipping default until the extension story opts in.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use portable_pty::{native_pty_system, CommandBuilder, ExitStatus, PtySize};
use serde_json::{json, Value};
use zatara_types::LaunchConfig;

use crate::relay::Tunnel;

/// Marker prefix of the synthetic announcement the runner sends for every
/// pty spawn — the client sees each child it never spawned itself, argv and
/// all (output *formatting* is LOC-0111; this is bare existence-proof).
const SPAWN_MARKER: &str = "zatara: spawn ";

/// Marker prefix of the pid line sent for every adopted pty child: the
/// session's recorded process list, written to the only channel the session
/// has — its own output stream. Teardown tests look these numbers up after
/// the session ends; nothing is ever written to disk (DR-3).
const TREE_MARKER: &str = "zatara: tree pid ";

/// The documented default pause budget — AC2's Gherkin bound.
const DEFAULT_BANNER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// The pty geometry children inherit: wide enough that MSBuild's progress
/// renderer never line-wraps its own status text into the log.
const PTY_ROWS: u16 = 50;
const PTY_COLS: u16 = 220;

/// One pty `read` worth of output. Sized above typical console flushes so
/// chunk boundaries fall between lines rather than inside them — the
/// straddle machinery must still handle any split (`read` may return less).
const PTY_READ_CHUNK_BYTES: usize = 8192;

/// How long a failure path waits for a pty relay to reach EOF before
/// answering anyway. A dead child's stream ends in milliseconds; the
/// budget only ever bounds a survivor holding the pty open.
const PUMP_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// The settle waiter's cap on the same pump-finish signal, deliberately
/// two orders of magnitude shorter: by the time it runs the child is
/// already gone, so an unfinished pump is one idling on a survivor-held
/// pty with its output long since delivered — waiting it out would tax
/// every clean session by the full drain budget (the CI latency job's
/// 5.07 s constant, 2026-10-13). The grace bounds delivery latency of
/// the already-buffered tail instead.
const TEST_SETTLE_GRACE: std::time::Duration = std::time::Duration::from_millis(500);

/// On by default (D18 flip, owner ruling 2026-10-14): the runner is the
/// only mode with unit-test support, and the old default-off silently
/// dropped every unset environment — every new user — into a plain relay
/// that ends the session without running the test. `ZATARA_RUNNER=0` is
/// the single documented opt-out back to the plain-relay dev seam. The
/// pause-banner budget is overridable via
/// `ZATARA_RUNNER_BANNER_TIMEOUT_SECS` (whole seconds; default 30) — the
/// test suite uses the override to exercise the never-pauses path fast.
#[derive(Clone, Copy)]
pub(crate) struct RunnerConfig {
    /// How long the runner waits for the vstest pause banner after the test
    /// step spawns before declaring the host will never pause.
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

/// The knob, pure — the flipped `ZATARA_RUNNER` law: armed UNLESS the
/// value is exactly `0`. The old law armed only on the exact `1` and left
/// everything else (including unset and garbage) off; "unset means off" is
/// what stranded new users, and a typo'd value silently off is its shape
/// reborn. An operator who types `ZATARA_RUNNER=yes` meant to arm it.
fn runner_armed(raw: Option<&str>) -> bool {
    raw != Some("0")
}

/// The knob, pure: missing or unparsable means the documented default.
fn banner_timeout(raw: Option<&str>) -> std::time::Duration {
    raw.and_then(|s| s.parse::<u64>().ok())
        .map_or(DEFAULT_BANNER_TIMEOUT, std::time::Duration::from_secs)
}

/// The banner's pid marker, verbatim from vstest (`Process Id: 34060, Name:
/// testhost` — live probe 2026-10-12). A pure prefix: digits follow it and
/// a non-digit terminates them.
const PAUSE_MARKER: &[u8] = b"Process Id: ";

/// Take the validated launch: run build, then test, on a worker thread so
/// the relay loop keeps serving while dotnet works.
pub(crate) fn start(
    tunnel: Arc<Tunnel>,
    runner: RunnerConfig,
    config: LaunchConfig,
    launch_seq: i64,
) {
    std::thread::spawn(move || run(tunnel, runner, config, launch_seq));
}

/// The exact argv of the build step — built as argv, never as shell text
/// (DR-2: the quoting-layer class of failure is deleted at the root).
fn build_argv(config: &LaunchConfig) -> Vec<String> {
    vec!["dotnet".into(), "build".into(), config.project_path.clone()]
}

/// The vstest `--filter` expression for a launch — the grammar table from
/// live probes (2026-10-12, vstest 17.8 with the pinned adapters), one
/// shape per gutter scope:
///
/// - `method` (default, and any unrecognised scope): exact
///   `FullyQualifiedName=<fqn>` — byte-identical to the pre-scope filter.
/// - `case`: the method predicate ANDed (`&`) with the case-arguments
///   predicate, which differs per adapter — nunit matches the no-space
///   argument body on the FQN (`AddsCases(1,2,3)` includes it); mstest
///   excludes arguments from its FQN and cannot filter `DisplayName`, so
///   `Name~` is the row predicate; xunit names its theory arguments in the
///   `DisplayName` (`a: 1, b: 2`), rebuilt here by zipping
///   `methodParameters` over `caseArguments` (positional when unnamed).
/// - `class`: substring with the trailing dot — `=Ns.Class` matches zero
///   tests (live probe), `~Ns.Class.` selects exactly the class.
/// - `file`: `fullyQualifiedName` carries the file's classes `|`-joined;
///   each becomes a class predicate, `|` is vstest's OR.
///
/// Parens are illegal inside filter values (both the vstest lexer and the
/// xunit argument parser reject them) — which is why no shape here ever
/// emits `(<args>)`.
pub fn build_filter(config: &LaunchConfig) -> String {
    let fqn = &config.fully_qualified_name;
    match config.scope.as_deref().unwrap_or("method") {
        "case" => {
            let args: Vec<&str> = config.case_arguments.iter().map(String::as_str).map(str::trim).collect();
            match config.framework.as_deref().unwrap_or("xunit") {
                "nunit" => format!("FullyQualifiedName~{fqn}&FullyQualifiedName~{}", args.join(",")),
                "mstest" => format!("FullyQualifiedName~{fqn}&Name~{}", args.join(",")),
                _ => {
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

/// The class-scope substring needs its trailing dot to stop at the class
/// boundary; an already-dotted name must not gain a second one.
fn dot_terminate(class: &str) -> String {
    if class.ends_with('.') {
        class.to_string()
    } else {
        format!("{class}.")
    }
}

/// The test step argv: the scope's filter is its own argv entry, the
/// `--filter` flag its own — no shell ever sees a quoted compound. The
/// optional `resultsLog` (D27) appends dotnet's own trx writer, as two
/// more whole entries; off by default, so the ordinary argv is unchanged.
fn test_argv(config: &LaunchConfig) -> Vec<String> {
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

/// Where the orchestrated steps run: the launch config's cwd, falling back
/// to the project's own directory when the config left it blank.
fn work_dir(config: &LaunchConfig) -> String {
    if !config.cwd.is_empty() {
        return config.cwd.clone();
    }
    config
        .project_path
        .rsplit_once(['/', '\\'])
        .map(|(dir, _)| dir.to_string())
        .unwrap_or_default()
}

/// A child running in its own pty, announced to the client at birth.
struct Step {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    /// Completes when the relay thread has finished streaming this child's
    /// pty output and released the pty master. Failure paths wait on it so
    /// the child's final lines reach the client before the session-end
    /// events `fail_launch` triggers — bounded, because a survivor process
    /// holding the pty open (MSBuild's compiler server is the known one)
    /// must never wedge the pipeline past its deadline.
    pump_done: Option<std::sync::mpsc::Receiver<()>>,
}

impl Step {
    /// Wait (bounded) for the output pump to finish. Requires the stream to
    /// be at its end already — the child exited, or its tree was just
    /// killed — so the pty reader is at EOF and a normal pump lands well
    /// inside the budget.
    fn drain_pump(&mut self) {
        if let Some(done) = self.pump_done.take() {
            let _ = done.recv_timeout(PUMP_DRAIN_TIMEOUT);
        }
    }
}

/// The `output` event categories carrying a step's bytes: the client's
/// console can tell build from test. The runner's own narration (spawn
/// announcements, the rendezvous pid) stays in the plain "stdout" stream
/// the relay already uses — the contract is that the text reaches the
/// Console, not one category string.
const BUILD_CATEGORY: &str = "build";
const TEST_CATEGORY: &str = "test";

/// Spawn `argv` in a fresh pty with `extra_env` layered over the inherited
/// environment, announcing the spawn and streaming every byte of pty output
/// to the client as `output` events (under `category`) for the child's
/// lifetime. When `pids` is given, the stream is also scanned for the vstest
/// pause banner and the first pid found is sent through it (test step only).
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
    // Re-pin PATH from the proxy's own view. CommandBuilder seeds its base
    // env on Windows from the registry (HKLM/HKCU Environment), silently
    // replacing the PATH the proxy was launched with — an explicit env()
    // entry wins over that base, so launcher-set PATH (test shims included)
    // actually reaches the child.
    if let Some(path) = std::env::var_os("PATH") {
        cmd.env("PATH", path);
    }
    // The dumbest terminal in existence, declared *and* answered: fewer VT
    // queries from well-behaved children, honest answers from the rest.
    cmd.env("TERM", "dumb");
    for (key, value) in extra_env {
        cmd.env(key, value);
    }
    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("{}: spawn failed: {e}", argv[0]))?;
    // Adoption first, announcement second: every microsecond between spawn
    // and job assignment is a window where a descendant could be born
    // outside the kill-on-close job (the fixture settles before it spawns
    // any child of its own to keep that window empty). Once adopted, the
    // pid is this session's responsibility until teardown, and the client
    // sees it as a member of the session's recorded process list.
    let pid = child.process_id();
    tunnel.tree.adopt(pid);
    if let Some(pid) = pid {
        send_output(tunnel, format!("{TREE_MARKER}{pid}\n"), "stdout");
    }
    // The pump thread owns the master: the console/pts must outlive this
    // function while the child tree feeds it, and is dropped only when the
    // stream itself ends. The writer answers terminal queries the children
    // send back through the pty (see `answer_terminal_queries`).
    let reader = pair.master.try_clone_reader().ok();
    let answerer = pair.master.take_writer().ok();
    let stream_tunnel = Arc::clone(tunnel);
    let (pump_done_tx, pump_done_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        stream_pty_output(&stream_tunnel, reader, answerer, pids, category);
        drop(pair.master);
        let _ = pump_done_tx.send(());
    });
    drop(pair.slave); // nothing writes the slave side; let the reader see EOF
    Ok(Step { child, pump_done: Some(pump_done_rx) })
}

/// A pty with no terminal agent on the master end is a trap: console
/// programs (MSBuild's progress renderer) emit cursor-position queries that
/// pass through the pty expecting an answer — unanswered, the child blocks
/// forever waiting for it. The proxy answers on its children's behalf, as a
/// minimal terminal would.
#[derive(Default)]
struct TerminalAnswerer {
    /// The last few bytes of the previous combined window (carry + chunk):
    /// a query can straddle reads of any fragmentation, and the scan must
    /// still see it whole on the next arrival.
    tail: Vec<u8>,
}

impl TerminalAnswerer {
    fn on_chunk(&mut self, chunk: &[u8], writer: &mut dyn Write) {
        let mut window = std::mem::take(&mut self.tail);
        window.extend_from_slice(chunk);
        let mut responses: Vec<u8> = Vec::new();
        let mut from = 0;
        while let Some(at) = find_sub(&window, b"\x1b[6n", from) {
            // Cursor Position Report: row 2, column 1 — anywhere valid.
            responses.extend_from_slice(b"\x1b[2;1R");
            from = at + 4;
        }
        // Keep only a partial-query remnant: 3 bytes is the longest proper
        // prefix of the 4-byte query, and a scanned-complete query can
        // never re-match from that remnant (its ESC would be missing). The
        // carry is taken from the *combined* window — carrying from the
        // fresh chunk alone drops what earlier sub-3-byte reads delivered,
        // and a query split across such reads loses its ESC, is never
        // whole, is never answered, and the child hangs waiting (Gordon
        // commit-gate finding: the exact MSBuild stall this answers for).
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

/// Incremental scanner for the vstest pause banner's pid, fed one pty chunk
/// at a time: the banner can straddle chunk boundaries, so matching is
/// byte-by-byte. Fires once; every later banner is noise. The marker has no
/// repeated prefix (`P` occurs only at its start), so a plain restart-on-
/// mismatch walk is exact for it — no KMP needed.
#[derive(Default)]
struct PauseWatcher {
    /// Bytes of `PAUSE_MARKER` matched so far.
    matched: usize,
    /// Once the marker completed, the digits collected after it; `None`
    /// terminator finalizes the number.
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
                    // Non-digit terminator: the number is complete. A
                    // non-numeric or u64-overflowing run is not a pid —
                    // ignore it and keep scanning with this byte.
                    if !digits.is_empty() {
                        if let Some(pid) =
                            std::str::from_utf8(&digits).ok().and_then(|s| s.parse::<u64>().ok())
                        {
                            self.fired = true;
                            return Some(pid);
                        }
                    }
                    // fall through: the terminator byte may start the marker
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

/// Whole-text convenience wrapper over [`PauseWatcher`] for the unit tests.
#[cfg(test)]
fn parse_pause_pid(text: &[u8]) -> Option<u64> {
    PauseWatcher::default().on_chunk(text)
}

/// Holds a step's pty bytes until whole lines exist, so every `output`
/// event the client receives is a whole number of console lines: a real
/// console writes one logical line across several flushes (the vstest run
/// summary is the shape that matters), and a half-line event would make the
/// text the user reads arrive torn. Splitting only at `\n` can never cut a
/// UTF-8 codepoint (no multi-byte sequence contains 0x0A), so the lossy
/// conversion only ever sees complete bytes; the pty's `\r\n` endings ride
/// through untouched — byte-identity is the contract.
/// Cap on what an unterminated stream may accumulate in the hold: a child
/// that never closes a line (`\r`-only progress rendering) must not grow
/// the pump's memory without bound. At the budget the held bytes are
/// delivered as an event whole — the bound moves bytes to the client, it
/// never drops one (the framing codec bounds its header buffer likewise).
const LINE_PENDING_BUDGET: usize = 64 * 1024;

#[derive(Default)]
struct LineBatcher {
    pending: Vec<u8>,
}

impl LineBatcher {
    /// Absorb one pty read. When the buffered bytes close a line, return
    /// everything through the last newline; the unterminated remainder (if
    /// any) waits for the next read.
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

    /// The stream is at its end: surface any unterminated remainder. The
    /// last thing a child printed must not die in the buffer.
    fn finish(&mut self) -> Option<String> {
        if self.pending.is_empty() {
            return None;
        }
        Some(String::from_utf8_lossy(&std::mem::take(&mut self.pending)).into_owned())
    }
}

/// Pump pty output to the client until the stream ends (EOF/err once the
/// child tree is gone), delivering whole lines: bytes are batched to the
/// last newline of each read (see `LineBatcher`), never one event per raw
/// chunk. A reader that never materialized (platform oddity) ends the pump
/// silently rather than blocking anything. When `pids` is given, each chunk
/// is also scanned for the pause-banner pid; the pump's exit (dropping the
/// sender) is itself the "stream ended unparsed" signal. `category` labels
/// every event this step produces.
fn stream_pty_output(
    tunnel: &Tunnel,
    mut reader: Option<Box<dyn Read + Send>>,
    mut writer: Option<Box<dyn Write + Send>>,
    pids: Option<std::sync::mpsc::Sender<u64>>,
    category: &'static str,
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

/// Run the build step to completion, reporting its exit status.
fn wait_for_exit(step: &mut Step, program: &str) -> Result<ExitStatus, String> {
    step.child.wait().map_err(|e| format!("{program}: waiting for the child failed: {e}"))
}

fn announce(tunnel: &Tunnel, argv: &[String]) {
    let line = format!("{SPAWN_MARKER}{}\n", Value::Array(argv.iter().map(|a| json!(a)).collect()));
    send_output(tunnel, line, "stdout");
}

/// One DAP `output` event carrying `text` under `category` — the client's
/// only window into the orchestrated children.
fn send_output(tunnel: &Tunnel, text: String, category: &str) {
    tunnel.send_to_client(json!({
        "type": "event",
        "event": "output",
        "body": { "output": text, "category": category },
    }));
}

/// Fail the launch with an explanatory answer, then end the session by
/// shutting the adapter down (the pump then runs the session-end sequence).
fn fail_launch(tunnel: &Tunnel, launch_seq: i64, reason: &str) {
    tunnel.error_response(launch_seq, "launch", reason);
    tunnel.kill_adapter();
}

/// Fail the launch after a test step exists: its whole tree dies first, so
/// a session that failed mid-test never leaves a paused host — or any other
/// descendant of the CLI it spawned — alive past the session's end.
fn fail_and_kill_test(tunnel: &Tunnel, test: &mut Step, launch_seq: i64, reason: &str) {
    kill_test_tree(test);
    let _ = test.child.wait();
    // The killed tree's final bytes reach the client before the failure
    // answer: `fail_launch` ends the session, and output streaming past the
    // session-end events it triggers is how a log loses (or misplaces) its
    // last lines.
    test.drain_pump();
    fail_launch(tunnel, launch_seq, reason);
}

/// Child environment for the test step: the launch config's env map first,
/// then `VSTEST_HOST_DEBUG=1` forced last so no config can drop the pause.
fn test_env(config: &LaunchConfig) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> =
        config.env.iter().flat_map(|map| map.clone().into_iter()).collect();
    env.push(("VSTEST_HOST_DEBUG".to_string(), "1".to_string()));
    env
}

/// Kill the whole process tree rooted at `pid`. The direct child is the
/// dotnet CLI; the paused testhost (and vstest's own helpers) hang off it,
/// so killing only the CLI would orphan exactly the process a never-paused
/// session must not leave behind. Windows: snapshot the live parent table
/// and `TerminateProcess` the tree bottom-up — kernel calls only, nothing
/// that has to be *scheduled* before it kills. Unix: the pty child leads
/// its own process group (portable-pty setsid), so a negative-pid SIGKILL
/// reaches every descendant.
///
/// Assumes `pid` leads its own process group — true for every pty child
/// (portable-pty setsid's them); `TreeRegistry` registers nothing else.
fn kill_tree_pid(pid: u32) {
    #[cfg(windows)]
    {
        // Kernel calls first, and no waiting on anything: a launch-based
        // killer (`taskkill`) must itself be spawned and run before it
        // kills anything, and a loaded machine starves it — observed: the
        // 5 s contract lost while a `taskkill` waited to even start, and
        // blocking on its exit status stalled every kill behind it. The
        // parent table is the tree, one snapshot answers it; killing
        // leaves-first means no descendant is orphaned onto a new parent
        // while we still hold a handle to walk.
        // Walked = the snapshot saw the root; its kill then already rides
        // in the doomed list. Only an unwalked root needs the extra shot.
        let mut walked = false;
        if let Some(parents) = process_parent_map() {
            let mut children: std::collections::HashMap<u32, Vec<u32>> =
                std::collections::HashMap::new();
            for (&child, &parent) in &parents {
                children.entry(parent).or_default().push(child);
            }
            // Pre-order walk: every node is visited before its children
            // are pushed, so reversing the visit order kills each process
            // only after everything below it has been targeted.
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
        // A root born between snapshot and kill walked out of the tree
        // untouched; a dead-but-unreaped root still holds its pid against
        // reuse, so re-targeting an already-walked root buys nothing and
        // only widens the (already taskkill-blessed) pid-reuse window.
        if !walked {
            terminate_process(pid);
        }
        // Backstop for anything born after the snapshot — detached, so a
        // slow-to-launch taskkill can never hold the teardown hostage.
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
    #[cfg(unix)]
    {
        // Gate matches the `cfg(unix)` libc dependency (no wasm target for
        // this crate). A group kill whose errno says "no such group" means
        // the portable-pty pgid assumption broke — fall back to killing the
        // direct child by pid instead of leaking it in silence.
        let rc = unsafe { libc::kill(-(pid as libc::pid_t), libc::SIGKILL) };
        if rc != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
        }
    }
}

/// Terminate the single process `pid` — no tree walk, no waiting. A dead
/// or foreign pid is a no-op: `OpenProcess` simply refuses, and teardown
/// carries on with the rest of the doomed list.
#[cfg(windows)]
fn terminate_process(pid: u32) {
    // Hand-written kernel32 binding, the harness-test precedent again: no
    // windows-sys dependency, so GNU-host toolchains never need dlltool.
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

/// Kill the test step's whole process tree (see [`kill_tree_pid`]).
fn kill_test_tree(step: &mut Step) {
    if let Some(pid) = step.child.process_id() {
        kill_tree_pid(pid);
    }
}

/// Teardown registry: every process id whose lifetime belongs to this
/// session, plus (Windows) the one kill-on-close job object they are all
/// assigned to. The relay calls [`TreeRegistry::kill_all`] on every
/// session-end path it owns — a client `disconnect`, a hung-up stdin, the
/// adapter exiting first — so the runner's pty tree can never outlive the
/// session it was built for.
///
/// The job object is the kernel-side crash half: when the proxy itself is
/// force-killed there is no thread left to run a kill, but process death
/// closes its handles, and a kill-on-close job answers that close by
/// terminating every member (and their descendants) — the 0-orphan bound
/// holds past a `TerminateProcess`. It is not universal: a process already
/// inside someone else's non-nestable job cannot be assigned, so where
/// assignment fails (POSIX always; a sandboxed harness on Windows) the
/// crash path is the fixture tree's own parent-death watchdog —
/// `fake_dotnet` takes its tree down the moment its owner vanishes, on
/// every platform — and the kernel-call tree kill above covers every end
/// the proxy survives long enough to run.
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
    /// Take ownership of a freshly spawned pty child: record it for the
    /// explicit teardown pass and pin it into the kill-on-close job.
    /// `None` pids (spawn platforms without an id) are silently unowned —
    /// nothing else can name them either.
    pub(crate) fn adopt(&self, pid: Option<u32>) {
        let Some(pid) = pid else { return };
        // Poison tolerance: the state is a pid list and one handle — a
        // panic elsewhere holding this brief lock must not cost a later
        // session its teardown. Same discipline in `kill_all`.
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

    /// Kill every adopted tree, right now, on the calling thread. The relay
    /// runs this before it ends its own process so no teardown races a
    /// `process::exit`. Idempotent and safe on dead pids — a stop after a
    /// natural end is a no-op, not an error.
    pub(crate) fn kill_all(&self) {
        // Teardown never refuses to run because another thread panicked
        // holding this lock (see `adopt`).
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        // Windows first strike: closing the job's last handle asks the
        // kernel itself to terminate every member — no process launch to
        // starve under load, no tree walk to race. Dropping it here is
        // final: a session that has asked to die never adopts again, and
        // the crash-path guarantee needed no handle left open anyway.
        #[cfg(windows)]
        {
            state.job = None;
        }
        // The load-proof strike (and the only one on POSIX, or where the
        // job could not be created — a proxy already living inside someone
        // else's non-nestable job cannot adopt into its own): the explicit
        // tree kill, kernel calls end to end, idempotent on the pids the
        // kernel already took.
        for &pid in &state.pids {
            kill_tree_pid(pid);
        }
    }
}

/// Windows: a job object whose members all die the moment the last handle
/// to it closes — i.e. when the proxy dies, by any means. Hand-written
/// kernel32 bindings like [`process_exists`]'s: no windows-sys dependency,
/// so GNU-host toolchains never need dlltool import libraries.
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

/// `JOBOBJECT_BASIC_LIMIT_INFORMATION` (class 2): only `LimitFlags` is set;
/// `Default` zeroes the rest. `repr(C)` reproduces the Win64 padding exactly.
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

/// Create + configure the kill-on-close job. `None` on any failure: the
/// explicit `kill_all` path still covers every session end the proxy
/// survives; only the force-kill crash mode loses its guarantee, and the
/// crash test surfaces that loudly.
///
/// A refusal is a degraded fact, and the owner ruling on degraded facts
/// (2026-10-13) is a quiet log, never the console: these lines rode the
/// editor's stderr pane on every Zed launch (a kernel filter under Zed
/// answers `Set` with os error 87, so they printed on every adopt) and
/// read to the user as errors of the session. They now print only under
/// an armed diagnostics gate — the same arms that mean "this run is being
/// debugged". Hiding stays impossible: a genuinely broken job (wrong
/// info-class, broken config) orphans the tree where jobs DO configure,
/// and the integration crash-path test over the real binary is that
/// machine's loud gate.
#[cfg(windows)]
fn create_kill_on_close_job() -> Option<JobHandle> {
    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000;
    /// `JOBOBJECTINFOCLASS::JobObjectBasicLimitInformation` per the SDK
    /// enum — 1 is the query-only accounting class, which Set answers
    /// ERROR_INVALID_PARAMETER (an earlier revision of this file had the
    /// two inverted; the armed-gate note below and the crash-path
    /// integration gate keep that class of mistake from hiding). A kernel
    /// filter that refuses job *configuration* outright answers every
    /// class, so a refusal here is environment, not class — the unit
    /// test's skip leg names that environment.
    const JOB_OBJECT_BASIC_LIMIT_INFORMATION: i32 = 2;
    // Armed means "someone is debugging this proxy": one of the two
    // documented diagnostics gates is on. Default runs see nothing.
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

/// Assign the process `pid` to `job`. Fails if the process is already in a
/// non-nestable job (a sandboxed CI harness could do this) — then the
/// explicit kill path remains the only teardown, as on POSIX.
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

/// Does `pid` name a live process? The banner's pid is text off a stdout
/// stream — text can be stale or lies. The OS must vouch for the number
/// before the debugger is aimed at it.
fn process_exists(pid: u64) -> bool {
    // pid 0 and 1 are OS-reserved: 0 means "the signal-group target" (and
    // the System/idle process), 1 is init on unix. Never a testhost — and
    // both probes below would answer "alive" for them by accident.
    if pid < 2 {
        return false;
    }
    #[cfg(windows)]
    {
        // Hand-written kernel32 binding (the harness tests set the
        // precedent): a windows-sys dependency here would make every
        // GNU-host toolchain build need dlltool for its import library.
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
        // Gate matches the `cfg(unix)` libc dependency; a non-windows,
        // non-unix target was never supported and fails to link here.
        let Ok(pid32) = i32::try_from(pid) else { return false };
        // Signal 0 probes without touching the target: EPERM means
        // "exists, not mine"; only ESRCH means gone.
        let rc = unsafe { libc::kill(pid32, 0) };
        rc == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    }
}

/// Is `pid` `ancestor` itself, or a descendant of it? The banner's pid is
/// stdout text — a compromised build could name any process on the machine,
/// and `attach` grants the debugger full read/write on whatever pid it
/// carries. Liveness proves a process exists there; this proves the process
/// belongs to the tree this launch itself spawned, by walking the OS parent
/// chain. Unresolvable chains (a pid gone mid-walk, a cycle, a foreign
/// root) all answer `false` — trust the tree, not the text.
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
    let mut current = pid;
    // Depth bound: honest trees are shallower than this; the bound is for
    // malformed parent data.
    for _ in 0..64 {
        #[cfg(windows)]
        let parent = parents.as_ref().and_then(|map| map.get(&current).copied());
        #[cfg(unix)]
        let parent = parent_of(current);
        match parent {
            Some(p) if p == ancestor => return true,
            // 0 is the unowned root on both platforms (System's parent on
            // Windows, the kernel on unix); a self-parent is a cycle.
            Some(0) => return false,
            Some(p) if p == current => return false,
            Some(p) => current = p,
            // Gone from the table mid-walk, or never in it.
            None => return false,
        }
    }
    false
}

/// pid → ppid for every live process, from one Toolhelp32 snapshot. Hand-
/// written kernel32 binding like [`process_exists`]'s: no windows-sys
/// dependency, so GNU-host toolchains never need dlltool import libraries.
#[cfg(windows)]
fn process_parent_map() -> Option<std::collections::HashMap<u32, u32>> {
    #[repr(C)]
    struct ProcessEntry32W {
        dw_size: u32,
        cnt_usage: u32,
        th32_process_id: u32,
        /// ULONG_PTR in the C definition — pointer-sized, not two u32s.
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

/// Direct parent of `pid`, from procfs: field 4 of `/proc/<pid>/stat`,
/// counted after the `comm` field (which may itself hold spaces and
/// parens, so counting starts after the last `)`).
#[cfg(unix)]
fn parent_of(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_comm = stat.rsplit_once(')')?.1;
    after_comm.split_whitespace().nth(1).and_then(|field| field.parse().ok())
}

/// The project path as the steps should see it: a directory-shaped
/// `projectPath` becomes the project file inside it before any spawn
/// (live defect, owner 2026-10-13 — a gutter config arrives directory-shaped
/// because Zed expands `$ZED_DIRNAME` in the scenario config only AFTER the
/// locator returned, and the locator must pass the raw variable through
/// rather than do path math on it, see `zatara::locator::project_path_from_dir`).
/// Rewrite only where the filesystem proves the answer; everything else
/// travels untouched, so a hand-written path that names a project elsewhere
/// keeps its intent and a genuinely missing one stays dotnet's honest error.
fn resolve_project_path(config: &mut LaunchConfig) {
    let fuzzy = fuzzy_armed(std::env::var("ZATARA_FUZZY_MATCH").ok().as_deref());
    resolve_project_path_with(config, fuzzy);
}

/// The fuzzy gate (owner request, 2026-10-14): OFF by default, and only
/// the exact `1` arms it — an opt-in toward guessing, so a typo'd value
/// must not quietly widen project resolution. This is the deliberate
/// opposite of the runner's default-on law: the runner being off means a
/// session does nothing (loudly wrong), while fuzzy being ON can build
/// and run a project nobody pointed at (quietly wrong). Quietly-wrong
/// capabilities stay off until asked for.
fn fuzzy_armed(raw: Option<&str>) -> bool {
    raw == Some("1")
}

/// The strict law plus the opt-in fuzzy fallback.
///
/// Strict (unchanged): the directory names no project at all — live
/// defect, owner's second project (2026-10-14): the gutter hands over the
/// TEST FILE's directory, and when the test sits one folder deeper than
/// the project (`...\eCommerceStarterCode.Tests\Controllers\CartControllerTests.cs`)
/// that directory holds zero `.csproj` and dotnet's MSB1003 answers. The
/// provable answer is the nearest ANCESTOR that names exactly one project;
/// an ambiguous directory is deliberately NOT walked out of — dotnet's
/// ambiguity error beats leaving one project's directory for another's,
/// and only the empty case is the proven-not-meant case.
///
/// Fuzzy (`ZATARA_FUZZY_MATCH=1`, owner "run the tests using the closest
/// proj or sln file"): wherever strict refused — an ambiguous directory,
/// or nothing found all the way up — the nearest ancestor naming EXACTLY
/// ONE `.sln` answers. A solution is never guessed between: two `.sln`
/// files refuse, because a guessed solution builds and tests the whole
/// repo under someone else's name. The fallback can only ADD an answer
/// where strict had none; it never replaces a strict one.
fn resolve_project_path_with(config: &mut LaunchConfig, fuzzy: bool) {
    let path = Path::new(config.project_path.as_str());
    if path.is_file() {
        return; // an existing file is the caller's claim, untouched
    }
    if !path.is_dir() {
        return; // missing (or a bare relative fixture name): no guess allowed
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

/// Whether `dir` holds any `.csproj` at all — the walk-up's gate: only a
/// directory that names NO project qualifies as proven-not-meant.
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

/// The project file `dir` names, proven or refused: exactly one `*.csproj`
/// wins outright; several win only if the .NET naming convention (a project
/// named after its directory) picks one; zero or an unresolved tie is `None`
/// — dotnet's ambiguity error beats our guess. Sorted, so the tie-break sees
/// the same set in every run.
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

/// The solution `dir` names, or nothing: exactly one `*.sln` wins; zero or
/// more than one never does. No dir-name==stem tie-break here — solutions
/// are rarely named after their folder the way projects are, and a guessed
/// solution silently builds and tests the whole repository. The fuzzy
/// fallback (ZATARA_FUZZY_MATCH) walks up on this until one answers.
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
    // Gutter configs can carry the task DIRECTORY where a project file was
    // meant (Zed expands scenario-config variables after the locator's
    // answer — the locator passes the directory through, this resolves it).
    resolve_project_path(&mut config);
    let cwd = work_dir(&config);
    // Step 1: build. A failed build ends the session — the test host must
    // never start over broken sources.
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
    // A green build needs no pump drain: only the failure paths answer
    // and end the session, and those drain before answering (see
    // `Step::drain_pump`).
    // Step 2: the test, spawned paused (`VSTEST_HOST_DEBUG=1`). vstest
    // blocks the host until a debugger attaches and announces it with a
    // `Process Id:` banner — the pid is the whole rendezvous, no file ever
    // touches disk (DR-3). Waiting happens here, on the worker thread; the
    // pump keeps streaming the host's output to the client meanwhile.
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
            // Narrated so the client can watch the rendezvous happen.
            send_output(&tunnel, format!("zatara: parsed testhost pid: {pid}\n"), "stdout");
            // The banner is text off a stream; before the debugger is aimed
            // at this number, the OS must confirm a process lives there.
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
            // Liveness proves *a* process lives at that number; the number
            // itself is untrusted build text, and `attach` grants the
            // debugger full access to whatever it names. The pid must sit
            // inside the tree this launch spawned before the adapter sees
            // the request at all.
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
            // The debugger receives `attach`, never the raw `launch`; the
            // adapter's attach answer becomes the answer to the client's
            // launch request (in-flight correlation in the relay).
            if let Err(e) = tunnel.attach_process(pid, launch_seq) {
                fail_and_kill_test(
                    &tunnel,
                    &mut test,
                    launch_seq,
                    &format!("attach{{processId: {pid}}}: {e}"),
                );
                return;
            }
            // The step is the session's ending voice from here: the vstest
            // console prints its summary AFTER the debuggee dies, so the
            // relay must not announce session-end before this step's
            // bytes have drained (owner's lost `Passed!`, live defect
            // 2026-10-13). Hand ownership to a drainer that fires the
            // settle gate the moment the step is gone — by completion or
            // by any teardown kill — and its pty tail has landed.
            let (settle_tx, settle_rx) = std::sync::mpsc::channel();
            tunnel.arm_test_settle(settle_rx);
            std::thread::spawn(move || {
                let mut test = test;
                let _ = test.child.wait();
                // The child is gone, so whatever still comes is the pty's
                // already-buffered tail, and the pump delivers every read
                // the moment it arrives — the wait lands in milliseconds.
                // The grace is a CAP, deliberately far shorter than the
                // failure paths' full `drain_pump` budget: a survivor
                // (the scripted marker's watchdog, MSBuild's server) can
                // hold the pty open for that whole budget with the stream
                // long since flushed, and the CI latency job measured that
                // cap — 5.07 s of pure wait per session — landing inside
                // the session-end window this gate guards.
                if let Some(done) = test.pump_done.take() {
                    let _ = done.recv_timeout(TEST_SETTLE_GRACE);
                }
                let _ = settle_tx.send(());
            });
        }
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            // The host never paused. Its whole tree dies before the session
            // ends — a still-paused testhost outliving a dead session is
            // the leak this branch exists to prevent.
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
            // The pty stream closed before any banner: normally the test
            // process is already gone. Not guaranteed — with no reader
            // materialized the pump quits instantly while the child lives,
            // and an unconditional wait() would hang exactly there. Poll
            // instead, and tree-kill anything still running (a no-op once
            // the tree has exited).
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

    /// Rule 3 on the D18 flip (owner ruling 2026-10-14): an unset
    /// `ZATARA_RUNNER` used to mean plain relay — every new user's Debug
    /// session ended silently unless they'd found an undocumented env.
    /// Red on the old law (None armed nothing), green on the flip. The
    /// opt-out stays the exact value `0`; garbage no longer disarms
    /// (the old silent-off-on-typo was the same trap wearing a hat).
    #[test]
    fn the_runner_arms_by_default_and_only_the_exact_zero_opts_out() {
        assert!(runner_armed(None), "unset env must arm: default-off was the new-user trap");
        assert!(runner_armed(Some("1")), "the historical arm value still arms");
        assert!(runner_armed(Some("yes")), "a typo'd arm value arms rather than silently off-ing");
        assert!(!runner_armed(Some("0")), "the one documented opt-out still opts out");
        assert!(runner_armed(Some("0 ")), "only the exact `0` opts out — a trailing space is a typo, not an opt-out");
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
        // é is 0xC3 0xA9: the first read ends inside the codepoint.
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
        // The 4-byte query split across two pty reads must not be missed —
        // a missed CPR is what hangs MSBuild.
        let mut answerer = TerminalAnswerer::default();
        let mut sink: Vec<u8> = Vec::new();
        answerer.on_chunk(b"restore \x1b[6", &mut sink);
        answerer.on_chunk(b"n done", &mut sink);
        assert_eq!(sink, b"\x1b[2;1R", "the straddled query must be answered exactly once");
    }

    #[test]
    fn answers_a_query_fragmented_below_the_carry_width() {
        // Pty reads arrive at whatever size the console writes them — a CPR
        // can land as four sub-3-byte reads. The straddle carry must be the
        // tail of the *combined* window: carrying from the fresh chunk
        // alone drops the ESC on the second read, the query is never whole,
        // and MSBuild's renderer waits for an answer forever.
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
        // The boundary remnant kept for straddle detection must not let an
        // already-answered query re-match when the next chunk arrives.
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
        // D27: the capture is dotnet's own writer, appended as two whole
        // argv entries. The test above is the off half of this proof: the
        // default config's argv carries no logger at all.
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

    /// A fresh temp dir for this test's tag — the projectPath resolution
    /// tests all assert on real filesystem shapes.
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
        // A decoy beside it: a path that already names a file must not be
        // talked out of it by whatever a directory scan could find.
        std::fs::write(&dir.join("Other.csproj"), b"<Project/>").unwrap();
        let mut config = config_naming(&real);
        resolve_project_path(&mut config);
        assert_eq!(config.project_path, real.display().to_string());
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
        // Owner's live shape (2026-10-14): the gutter's directory is the
        // TEST FILE's folder (`...\eCommerceStarterCode.Tests\Controllers`),
        // the project one level up. Zero csproj in the named dir ⇒ walk.
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

        // The refusal keeps its shape: an ambiguous directory is NOT walked
        // out of — a directory that names two projects must stay dotnet's
        // ambiguity error, not silently become the parent's project.
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
        // Even with exactly one csproj in its parent: a missing file may be
        // an intended typo, and silently rewriting it would build the wrong
        // project without anyone asking. dotnet's MSB1009 is the honest voice.
        let dir = fs_temp("missing");
        std::fs::write(&dir.join("Main.csproj"), b"<Project/>").unwrap();
        let missing = dir.join("Main.typo.csproj");
        let mut config = config_naming(&missing);
        resolve_project_path(&mut config);
        assert_eq!(config.project_path, missing.display().to_string());
    }

    #[test]
    fn the_fuzzy_gate_opens_only_for_the_exact_one() {
        // Owner ruling 2026-10-14: default OFF (the opposite law of the
        // runner flip — fuzzy ON can run a project nobody pointed at, the
        // quiet direction), and a typo must not arm a guessing fallback.
        assert!(fuzzy_armed(Some("1")));
        assert!(!fuzzy_armed(None));
        assert!(!fuzzy_armed(Some("0")));
        assert!(!fuzzy_armed(Some("yes")));
        assert!(!fuzzy_armed(Some("1 ")));
    }

    #[test]
    fn a_solution_answers_where_the_strict_law_refused() {
        // Owner's fuzzy ask (2026-10-14): "run the tests using the closest
        // proj or sln file". The strict refusal being answered is an
        // ambiguous directory — two projects, no naming convention — with
        // the repo's solution one folder up.
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
        // The other strict failure: zero .csproj in any ancestor. A
        // solution at the root now answers the Debug lane too; the Run
        // lane gets the same answer from dotnet pointed at the root.
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
        // Fuzzy ADDs an answer, never replaces one: a directory with its
        // own single project resolves to the project even when a solution
        // sits beside it.
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
        }
    }

    #[test]
    fn the_method_scope_filter_is_untouched_by_scope_awareness() {
        // Byte-identical to the shipped filter (LOC-0109 pins): an absent
        // scope means exactly what it meant before case/class/file existed.
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
        // Grammar proven live 2026-10-12: vstest's `~` predicate on the
        // method plus `~` on the no-space argument body selects one [TestCase].
        let mut config = scoped("Ns.CalcTests.AddsCases", Some("case"), Some("nunit"));
        config.case_arguments = vec!["1".into(), "2".into(), "3".into()];
        assert_eq!(
            build_filter(&config),
            "FullyQualifiedName~Ns.CalcTests.AddsCases&FullyQualifiedName~1,2,3"
        );
    }

    #[test]
    fn an_mstest_row_filter_selects_the_row_by_name() {
        // MSTest 3.1.1: DisplayName is not a filterable property and the FQN
        // excludes row arguments — `Name~` is the row predicate that matches.
        let mut config = scoped("Ns.CalcTests.AddsCases", Some("case"), Some("mstest"));
        config.case_arguments = vec!["2".into(), "3".into(), "5".into()];
        assert_eq!(
            build_filter(&config),
            "FullyQualifiedName~Ns.CalcTests.AddsCases&Name~2,3,5"
        );
    }

    #[test]
    fn an_xunit_case_filter_zips_parameters_with_arguments() {
        // xunit's FullyQualifiedName excludes theory arguments; they live in
        // the DisplayName, named: `a: 2, b: 3, expected: 5`.
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
        // Older/unnamed xunit displays carry bare argument lists.
        let mut config = scoped("Ns.TheoryTests.AddsCases", Some("case"), Some("xunit"));
        config.case_arguments = vec!["1".into(), "2".into()];
        assert_eq!(
            build_filter(&config),
            "FullyQualifiedName~Ns.TheoryTests.AddsCases&DisplayName~1, 2"
        );
    }

    #[test]
    fn a_class_filter_is_the_trailing_dot_substring() {
        // `FullyQualifiedName=Ns.Class` matches zero tests (live probe);
        // the substring with the trailing dot runs exactly the class.
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
        // The gutter hands the file's classes as a `|`-joined list; each
        // becomes a class predicate, and `|` is vstest's OR.
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
        // CommandBuilder applies env in order (last insert wins): forcing the
        // pause flag *after* the config's map means the child sees 1 no
        // matter what the config tried to set.
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
        // Byte-shape of the real vstest banner (live probe 2026-10-12).
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
        // A non-numeric "pid" is noise, and so is one that cannot fit a u64.
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
        // Windows keeps the process object queryable while any handle to it
        // stays open — std's Child holds one until Drop — and deletion after
        // the last handle closes is asynchronous, hence the bounded poll.
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

    /// A long-lived sleeper that leads its own process group — the shape a
    /// pty child has (portable-pty setsid's them), and the assumption
    /// `kill_tree_pid`'s group kill is written against.
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
        // A second teardown pass over a dead tree must be silence, not an
        // error: a stop after a natural end is the common case, not the bug.
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
        registry.kill_all(); // must not panic, must not signal the living
        assert!(process_exists(u64::from(std::process::id())), "the survivor of a dead-pid kill is the killer itself — still here");
    }

    /// The crash-path guarantee, proven at the OS seam: a proxy force-kill
    /// leaves a closed job handle and nothing else — this test is that
    /// exact moment (close, don't kill), and the tree must fall.
    ///
    /// Job *configuration* is environment-dependent: a sandboxed harness
    /// can hook the kernel such that creating the object succeeds but no
    /// info-class may be set on it (measured here under the bash sandbox:
    /// class 2 answers 87, class 9 answers 24, while class 2 *queries*
    /// work — a filter, not a layout bug; unsandboxed runs configure
    /// normally). Where configuration is refused the proxy falls back to
    /// the explicit-kill path and this unit is not applicable — the
    /// integration crash-path test over the real proxy binary is the
    /// authoritative gate for the guarantee. A bad info-class must never
    /// hide behind this skip again: this unit's skip leg prints its own
    /// line whenever job configuration is refused here, and under an
    /// armed diagnostics gate `create_kill_on_close_job` names the OS
    /// error beside it. (Unarmed, the refusal is a quiet degraded fact by
    /// owner ruling — see the function's doc for why a real class mistake
    /// still cannot hide: it orphans the tree wherever jobs DO configure,
    /// and that is exactly where the crash-path test runs.)
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
        drop(job); // the last handle closing is what proxy death does
        assert!(
            terminated_within(&mut sleeper, std::time::Duration::from_secs(5)),
            "closing the job's last handle — a process death, by any means — must take its members"
        );
        let _ = sleeper.wait();
    }
}
