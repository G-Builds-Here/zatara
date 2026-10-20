//! Scripted stand-in for the `dotnet` CLI (test infrastructure — never
//! built for shipping targets, see `required-features` in Cargo.toml).
//!
//! The runner integration tests copy this binary to a PATH dir under the
//! name `dotnet(.exe)` so a spawn resolves here instead of the real SDK.
//! It reproduces the two CLI surfaces the runner consumes — a green build,
//! and a `test` step whose vstest pause-banner behavior the tests choose via
//! env:
//!
//! - `FAKE_DOTNET_MODE=banner`    — print the vstest pause banner (pid from
//!   `FAKE_DOTNET_PID`, default: the marker child's own pid — a real
//!   descendant of this fake, the scripted shape of a vstest pause whose
//!   target sits inside the launched tree, which is all a runner that
//!   trusts only its own process tree can attach to) then block like a
//!   paused host.
//! - `FAKE_DOTNET_MODE=silent`    — print ordinary test chatter, never a
//!   banner, then block: the never-pauses sabotage the AC2 timeout needs.
//! - `FAKE_DOTNET_MODE=resume`    — banner as above, then block until the
//!   touch-file named by `FAKE_DOTNET_RESUME_FILE` appears (the scripted
//!   stand-in for the debugger resuming the paused host): then print the
//!   real-format Passed! summary, kill the marker child — a finished run
//!   leaves nothing of its tree alive — and exit 0.
//! - `FAKE_DOTNET_MODE=completed` — print the banner (so the debugger
//!   attaches to a live in-tree pid), hold briefly as if the debugger were
//!   running the test to completion, print `Passed!`, reap the marker, and
//!   exit 0: the scripted shape of a run that finished on its own.
//! - `FAKE_DOTNET_MODE=summary` — print ordinary chatter, then the green
//!   run summary split across two writes with a gap (a real console can
//!   emit one logical line as several pty writes), then exit 0.
//! - `FAKE_DOTNET_MODE=summary-fail` — print a failed-test line naming the
//!   test, then a `Failed!` summary (also split across two writes), exit 1.
//! - `FAKE_DOTNET_MODE=resume-fail` — the failing twin of `resume`: banner,
//!   wait for the touch-file, then print a failed-test line naming
//!   `FAKE_DOTNET_FAILING_TEST` (default `Basic.AddTests.AddsOneAndTwo`)
//!   and a real-format `Failed!` summary, reap the marker, exit 1 — the
//!   scripted shape of a session whose test asserts falsely.
//! - `FAKE_DOTNET_MODE=timed`   — `resume` plus self-reporting: each step
//!   (build and test) prints `ZATARA-FAKE-BASELINE-US: n` with the
//!   microseconds it actually spent, minus any `ZATARA_FAKE_SLEEP_US`
//!   injected into the build step. The latency-budget job subtracts these
//!   reports from the session wall time to isolate the proxy's own overhead.
//! - `ZATARA_FAKE_SLEEP_US=<n>` — microseconds to sleep at the top of the
//!   build step, before the build summary: the instrumented delay inside
//!   the proxy's pre-launch step (the timed budget's breach fixture).
//! - `FAKE_DOTNET_MODE=flood`   — print 10000 stdout lines, each stamped
//!   with the wall-clock microsecond it was written, then a summary, exit 0.
//!   The stamps let output-streaming tests measure delivery against the
//!   moment each line actually hit the pty.
//!
//! The debugger-facing modes (banner/silent/resume/resume-fail/timed/
//! completed) first spawn a
//! short-lived-but-detachable marker child (`ZATARA-MARKER-CHILD-PID: N` on
//! its own output line) so tests can prove the runner kills the whole
//! process *tree*, not just the direct child. The marker is another instance
//! of this same binary with `ZATARA_FAKE_DOTNET_MARKER=1` in its
//! environment: it does nothing but run the parent-death watchdog and wait
//! to be killed. The exiting modes (summary/summary-fail/flood) don't:
//! nothing exits to a tree-kill assertion, and leaking a marker that waits
//! to be killed past an already-finished test would dirty the machine for
//! no proof.
//!
//! The watchdog is the fixture-tree half of the crash-path contract, on
//! every platform: when the process owning this tree dies by any means —
//! including one no tree-killer ever reaches, the owner being
//! force-terminated mid-teardown — a tree still standing must fall with
//! it. POSIX detects it by the parent pid changing (the init reaper) and
//! group-kills; Windows detects it by the owner's exit code changing from
//! STILL_ACTIVE and terminates its own marker before falling. The 5 s
//! bound is the story's; the poll loop's slice is 25 ms.

use std::io::Write as _;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The flood scenario's line count — the AC1 scenario's Gherkin number.
const FLOOD_LINES: u32 = 10_000;

/// Wall-clock microseconds since the Unix epoch — the stamp the flood lines
/// carry, on the same clock the reading test process uses.
fn epoch_us() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after 1970")
        .as_micros()
}

fn main() {
    // The marker is this same binary wearing a different hat: no argv, no
    // output, one job — die the moment the fake that spawned it dies.
    if std::env::var_os("ZATARA_FAKE_DOTNET_MARKER").is_some() {
        arm_parent_death_watchdog(None);
        thread::sleep(Duration::from_secs(3600));
        return;
    }

    let argv: Vec<String> = std::env::args().skip(1).collect();
    let first = argv.first().map(String::as_str).unwrap_or("");
    let mode = std::env::var("FAKE_DOTNET_MODE").unwrap_or_else(|_| "banner".into());
    // Every step of a scripted session owns its own `started`: the timed
    // mode's baseline report is this process's honest wall-clock work, and
    // `ZATARA_FAKE_SLEEP_US` is the instrumented delay the latency-budget
    // breach fixture injects into the proxy's pre-launch step (the build).
    let started = Instant::now();
    let injected_us: u64 =
        std::env::var("ZATARA_FAKE_SLEEP_US").ok().and_then(|v| v.parse().ok()).unwrap_or(0);

    match first {
        "build" => {
            if injected_us > 0 {
                thread::sleep(Duration::from_micros(injected_us));
            }
            println!("Fake build succeeded. 0 Warning(s) 0 Error(s)");
            // Settle before dying: the runner spawns the next step the moment
            // this child exits, and an instant exit lets that announce race
            // past the pump's last read of this step's pty. A real build
            // spends seconds flushing; this fake only needs its bytes on the
            // wire before it dies.
            std::io::stdout().flush().ok();
            thread::sleep(Duration::from_millis(300));
            if mode == "timed" {
                report_baseline(started, injected_us);
            }
        }
        "test" => {
            match mode.as_str() {
                "resume" => resume_like(started, false, false),
                "resume-fail" => resume_like(started, true, false),
                "timed" => resume_like(started, false, true),
                _ => {}
            }
            // The debugger-facing modes own a marker child; the exiting
            // output-streaming modes leave no tree for a kill assertion.
            let marker: Option<Child> = match mode.as_str() {
                "banner" | "silent" | "completed" => spawn_debugger_tree(),
                _ => None,
            };
            match mode.as_str() {
                "completed" => {
                    // Byte-identical in shape to real vstest output: the
                    // notice line, then the pid banner — the debugger must
                    // find a live in-tree pid to attach to before the run
                    // "completes".
                    print_pause_banner(marker.as_ref().map(Child::id));
                    // Long enough for the runner to parse, verify and attach
                    // (all in-memory, milliseconds), short enough to not make
                    // the lifecycle tests crawl.
                    thread::sleep(Duration::from_secs(2));
                    println!("Passed! - Failed: 0, Passed: 1");
                    reap_marker(marker);
                    return; // exit 0: a completed run leaves nothing of its own
                }
                "banner" => {
                    print_pause_banner(marker.as_ref().map(Child::id));
                }
                "silent" => {
                    println!("Test run for a fake assembly (.NETTest)");
                    println!("Passed! - Failed: 0, Passed: 1");
                }
                "summary" => {
                    println!("Test run for a fake assembly (.NETTest)");
                    write_split_summary("Passed!  - Fail", "ed: 0, Passed: 3, Skipped: 0", 0);
                }
                "summary-fail" => {
                    println!("Test run for a fake assembly (.NETTest)");
                    println!("Failed AddTests.AddsOneAndTwo [1 ms]");
                    write_split_summary("Failed!  - Fail", "ed: 1, Passed: 0, Skipped: 0", 1);
                }
                "flood" => {
                    println!("Test run for a fake assembly (.NETTest)");
                    // 10000 lines, each stamped at the microsecond it was
                    // written — delivery tests grade arrival against the
                    // stamp, not against each other.
                    for line in 1..=FLOOD_LINES {
                        println!("zatara-flood line={line} written_us={}", epoch_us());
                    }
                    println!("Passed!  - Failed: 0, Passed: 3, Skipped: 0");
                    std::io::stdout().flush().ok();
                    std::process::exit(0);
                }
                other => {
                    eprintln!("zatara-fake-dotnet: unexpected FAKE_DOTNET_MODE: {other}");
                    std::process::exit(2);
                }
            }
            // banner/silent fall through; the marker must outlive this
            // process's Drop, dying only by the runner's tree kill or the
            // watchdog armed above: proof of tree-kill, not of a parent
            // that tidies up after itself.
            if let Some(child) = marker {
                std::mem::forget(child);
            }
            // A paused/running host keeps its console open indefinitely.
            thread::sleep(Duration::from_secs(3600));
        }
        other => {
            eprintln!("zatara-fake-dotnet: unexpected argv: {other}");
            std::process::exit(2);
        }
    }
}

/// Emit one logical console line as two writes with a gap — the shape of a
/// real console that splits a line across flush boundaries. Each write is
/// its own pty read; the line the client must receive is the whole of them,
/// byte-identical. Then exit with `code`.
fn write_split_summary(prefix: &str, suffix: &str, code: i32) -> ! {
    print!("{prefix}");
    std::io::stdout().flush().ok();
    thread::sleep(Duration::from_millis(400));
    println!("{suffix}");
    std::io::stdout().flush().ok();
    std::process::exit(code)
}

/// The pause banner, byte-identical in shape to real vstest output (probe
/// 2026-10-12): the notice line, then the pid banner. Shared by the
/// banner/completed/resume modes so they can never drift apart — the
/// runner's parser is pinned to this exact shape. The pid is
/// `FAKE_DOTNET_PID` when set, else the marker child (a real descendant of
/// this fake — the scripted shape of a vstest pause whose target sits
/// inside the launched tree), else this process's own pid.
fn print_pause_banner(default_pid: Option<u32>) {
    let pid = std::env::var("FAKE_DOTNET_PID").unwrap_or_else(|_| {
        default_pid
            .map(|pid| pid.to_string())
            .unwrap_or_else(|| std::process::id().to_string())
    });
    println!("Host debugging is enabled. Please attach debugger to testhost process to continue.");
    println!("Process Id: {pid}, Name: testhost");
}

/// Spawn the marker child and announce its pid; the caller decides whether
/// it leaks (banner/silent: it must survive this process's Drop to prove
/// tree-kill) or is reaped (completed: a finished run tidies up). The
/// marker's own death-watches are armed by its instance of `main`.
fn spawn_marker_child() -> Option<Child> {
    let exe = std::env::current_exe().ok()?;
    let child: std::io::Result<Child> = Command::new(exe)
        .env("ZATARA_FAKE_DOTNET_MARKER", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let child = child.ok();
    if let Some(child) = child.as_ref() {
        println!("ZATARA-MARKER-CHILD-PID: {}", child.id());
    }
    child
}

/// The debugger-facing tree setup, shared by `main`'s mode arms and
/// `resume_test_step` so the two can never drift apart: settle, spawn the
/// marker, arm the watchdog.
fn spawn_debugger_tree() -> Option<Child> {
    // Settle before any child of ours exists: the proxy pins this process
    // into its kill-on-close job microseconds after spawn, and job
    // membership is inherited only by children born *after* the assignment
    // (Windows crash-teardown race, see runner.rs).
    thread::sleep(Duration::from_millis(100));
    let marker = spawn_marker_child();
    // Arm after the marker exists: on Windows the watchdog must know the
    // marker's pid to take it down on the way out; on POSIX the group kill
    // covers it either way.
    arm_parent_death_watchdog(marker.as_ref().map(Child::id));
    marker
}

/// A finished run's clean teardown: kill and reap the marker so a session
/// that ended on its own leaves nothing of its tree alive — the shape an
/// orphan scan reads as a clean end, not a tree kill.
fn reap_marker(marker: Option<Child>) {
    if let Some(mut child) = marker {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Crash guarantee for the fixture tree, on every platform: if the process
/// that owns this tree dies by any means — `kill -9` / `TerminateProcess`
/// included, which no handler can catch — the tree falls within one poll
/// interval, carrying its own marker with it. POSIX sees the parent pid
/// change the instant the init reaper takes the child and SIGKILLs the
/// process group (live proxy teardown would have killed the same group:
/// pty children share it via portable-pty's setsid). Windows has no group
/// kill: it holds a handle to the owner, watches its exit code leave
/// STILL_ACTIVE, terminates the marker it knows about, and exits.
fn arm_parent_death_watchdog(marker_pid: Option<u32>) {
    #[cfg(unix)]
    {
        let _ = marker_pid; // the group kill below takes the marker anyway
        let expected_parent = unsafe { libc::getppid() };
        thread::spawn(move || loop {
            thread::sleep(Duration::from_millis(25));
            if unsafe { libc::getppid() } != expected_parent {
                // Negative pid = the whole group, this process included.
                // The take MUST be negative: a positive pgid names only the
                // process whose pid equals the group number - the group
                // leader, never the tree (live CI defect 2026-10-17: the
                // fixture tree outlived every parent-death on posix).
                unsafe { libc::kill(-libc::getpgid(0), libc::SIGKILL) };
                std::process::exit(1);
            }
        });
    }
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetCurrentProcessId() -> u32;
            fn CreateToolhelp32Snapshot(flags: u32, process_id: u32) -> *mut u8;
            fn Process32FirstW(snapshot: *mut u8, entry: *mut ProcessEntry32W) -> i32;
            fn Process32NextW(snapshot: *mut u8, entry: *mut ProcessEntry32W) -> i32;
            fn CloseHandle(handle: *mut u8) -> i32;
            fn OpenProcess(desired_access: u32, inherit_handle: i32, process_id: u32) -> *mut u8;
            fn GetExitCodeProcess(process: *mut u8, exit_code: *mut u32) -> i32;
            fn TerminateProcess(process: *mut u8, exit_code: u32) -> i32;
        }
        // The parent's identity is discovered once, while it is surely
        // alive; the handle taken then is what the poll reads — a dead
        // owner's object keeps answering `GetExitCodeProcess` with its
        // final code, which is exactly the STILL_ACTIVE transition sought.
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
        thread::spawn(move || {
            const SNAP_PROCESS: u32 = 0x2;
            const STILL_ACTIVE: u32 = 259;
            const QUERY_LIMITED_INFORMATION: u32 = 0x1000;
            const PROCESS_TERMINATE: u32 = 0x0001;

            let me = unsafe { GetCurrentProcessId() };
            let snapshot = unsafe { CreateToolhelp32Snapshot(SNAP_PROCESS, 0) };
            let mut parent = 0u32;
            if snapshot != std::ptr::null_mut() && snapshot != -1isize as *mut u8 {
                unsafe {
                    let mut entry: ProcessEntry32W = std::mem::zeroed();
                    entry.dw_size = std::mem::size_of::<ProcessEntry32W>() as u32;
                    if Process32FirstW(snapshot, &mut entry) != 0 {
                        loop {
                            if entry.th32_process_id == me {
                                parent = entry.th32_parent_process_id;
                                break;
                            }
                            if Process32NextW(snapshot, &mut entry) == 0 {
                                break;
                            }
                        }
                    }
                    CloseHandle(snapshot);
                }
            }
            let owner = unsafe { OpenProcess(QUERY_LIMITED_INFORMATION, 0, parent) };
            // No parent found or no handle to it: nothing to watch — a
            // watchdog that cannot see cannot promise, so it does not run.
            if owner.is_null() {
                return;
            }
            loop {
                thread::sleep(Duration::from_millis(25));
                let mut code = 0u32;
                let ok = unsafe { GetExitCodeProcess(owner, &mut code) };
                if ok != 0 && code == STILL_ACTIVE {
                    continue;
                }
                // The owner is gone (or unreadable — treated as gone: the
                // crash contract must not hinge on the probe surviving).
                if let Some(marker) = marker_pid {
                    let handle = unsafe { OpenProcess(PROCESS_TERMINATE, 0, marker) };
                    if !handle.is_null() {
                        unsafe { TerminateProcess(handle, 1) };
                        unsafe { CloseHandle(handle) };
                    }
                }
                unsafe { CloseHandle(owner) };
                std::process::exit(1);
            }
        });
    }
}

/// The scripted resume of a paused run, green or red: the pause banner, the
/// wait for the debugger's resume (the touch-file a test touches after
/// sending `continue`), the real-format run summary (`fail` selects the
/// vstest 17.8 `Failed!` shape naming `FAKE_DOTNET_FAILING_TEST`), and a
/// clean teardown — the marker child killed, exit 0 (or 1 for the failing
/// twin) — so an orphan scan can tell a finished session apart from a tree
/// kill. `report` adds the timed mode's self-baseline line.
fn resume_like(started: Instant, fail: bool, report: bool) -> ! {
    // Same tree setup as the other debugger-facing modes — armed even
    // though the clean path below reaps its own marker: a resume that
    // never arrives, or an owner killed mid-session, must still take the
    // tree down.
    let marker = spawn_debugger_tree();
    print_pause_banner(marker.as_ref().map(Child::id));

    let resume_file = std::env::var("FAKE_DOTNET_RESUME_FILE")
        .expect("a resuming FAKE_DOTNET_MODE requires FAKE_DOTNET_RESUME_FILE");
    let deadline = Instant::now() + Duration::from_secs(300);
    while !Path::new(&resume_file).exists() {
        if Instant::now() > deadline {
            eprintln!("zatara-fake-dotnet: resume never arrived — exiting like a host left paused");
            std::process::exit(1);
        }
        thread::sleep(Duration::from_millis(100));
    }
    if fail {
        let name = std::env::var("FAKE_DOTNET_FAILING_TEST")
            .unwrap_or_else(|_| "Basic.AddTests.AddsOneAndTwo".into());
        println!("Failed {name} [1 ms]");
        // Byte shape copied verbatim from a real vstest 17.8 failure summary
        // (same probe as the green line).
        println!(
            "Failed!  - Failed:     1, Passed:     0, Skipped:     0, Total:     1, Duration: < 1 ms - FakeFx.dll (net8.0)"
        );
    } else {
        // Byte shape copied verbatim from a real vstest 17.8 summary (probe 2026-10-12).
        println!(
            "Passed!  - Failed:     0, Passed:     1, Skipped:     0, Total:     1, Duration: < 1 ms - FakeFx.dll (net8.0)"
        );
    }
    if report {
        report_baseline(started, 0);
    }
    std::io::stdout().flush().ok();
    reap_marker(marker);
    std::process::exit(if fail { 1 } else { 0 });
}

/// The timed mode's self-report: microseconds this step really spent, minus
/// the synthetic `ZATARA_FAKE_SLEEP_US` injection — the session's
/// build+pause baseline, what the latency budget subtracts from total wall
/// time to isolate the proxy's own overhead.
fn report_baseline(started: Instant, injected_us: u64) {
    let measured = started.elapsed().as_micros() as u64;
    println!("ZATARA-FAKE-BASELINE-US: {}", measured.saturating_sub(injected_us));
    std::io::stdout().flush().ok();
}
