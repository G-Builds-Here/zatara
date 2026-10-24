use std::io::Write as _;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
const FLOOD_LINES: u32 = 10_000;
fn epoch_us() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after 1970")
        .as_micros()
}
fn main() {
    if std::env::var_os("ZATARA_FAKE_DOTNET_MARKER").is_some() {
        arm_parent_death_watchdog(None);
        thread::sleep(Duration::from_secs(3600));
        return;
    }
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let first = argv.first().map(String::as_str).unwrap_or("");
    let mode = std::env::var("FAKE_DOTNET_MODE").unwrap_or_else(|_| "banner".into());
    let started = Instant::now();
    let injected_us: u64 =
        std::env::var("ZATARA_FAKE_SLEEP_US").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    match first {
        "build" => {
            if injected_us > 0 {
                thread::sleep(Duration::from_micros(injected_us));
            }
            println!("Fake build succeeded. 0 Warning(s) 0 Error(s)");
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
            let marker: Option<Child> = match mode.as_str() {
                "banner" | "silent" | "completed" => spawn_debugger_tree(),
                _ => None,
            };
            match mode.as_str() {
                "completed" => {
                    print_pause_banner(marker.as_ref().map(Child::id));
                    thread::sleep(Duration::from_secs(2));
                    println!("Passed! - Failed: 0, Passed: 1");
                    reap_marker(marker);
                    return;
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
            if let Some(child) = marker {
                std::mem::forget(child);
            }
            thread::sleep(Duration::from_secs(3600));
        }
        other => {
            eprintln!("zatara-fake-dotnet: unexpected argv: {other}");
            std::process::exit(2);
        }
    }
}
fn write_split_summary(prefix: &str, suffix: &str, code: i32) -> ! {
    print!("{prefix}");
    std::io::stdout().flush().ok();
    thread::sleep(Duration::from_millis(400));
    println!("{suffix}");
    std::io::stdout().flush().ok();
    std::process::exit(code)
}
fn print_pause_banner(default_pid: Option<u32>) {
    let pid = std::env::var("FAKE_DOTNET_PID").unwrap_or_else(|_| {
        default_pid
            .map(|pid| pid.to_string())
            .unwrap_or_else(|| std::process::id().to_string())
    });
    println!("Host debugging is enabled. Please attach debugger to testhost process to continue.");
    println!("Process Id: {pid}, Name: testhost");
}
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
fn spawn_debugger_tree() -> Option<Child> {
    thread::sleep(Duration::from_millis(100));
    let marker = spawn_marker_child();
    arm_parent_death_watchdog(marker.as_ref().map(Child::id));
    marker
}
fn reap_marker(marker: Option<Child>) {
    if let Some(mut child) = marker {
        let _ = child.kill();
        let _ = child.wait();
    }
}
fn arm_parent_death_watchdog(marker_pid: Option<u32>) {
    #[cfg(unix)]
    {
        let _ = marker_pid;
        let expected_parent = unsafe { libc::getppid() };
        thread::spawn(move || loop {
            thread::sleep(Duration::from_millis(25));
            if unsafe { libc::getppid() } != expected_parent {
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
fn resume_like(started: Instant, fail: bool, report: bool) -> ! {
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
        println!(
            "Failed!  - Failed:     1, Passed:     0, Skipped:     0, Total:     1, Duration: < 1 ms - FakeFx.dll (net8.0)"
        );
    } else {
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
fn report_baseline(started: Instant, injected_us: u64) {
    let measured = started.elapsed().as_micros() as u64;
    println!("ZATARA-FAKE-BASELINE-US: {}", measured.saturating_sub(injected_us));
    std::io::stdout().flush().ok();
}
