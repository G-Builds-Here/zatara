//! Thin binary wrapper: the GUI-subsystem attribute (no console window at
//! spawn — adapters are spawned headless, like every other Zed adapter),
//! the hidden-console shim, plus the lib entry. All behavior lives in
//! `zatara_proxy`.

#![windows_subsystem = "windows"]

use std::process::ExitCode;

/// Give this GUI-subsystem process a console of its own — hidden — so the
/// children it spawns never allocate one visibly.
///
/// Why: a console-subsystem child of a parent WITHOUT a console gets a
/// freshly allocated console (visible conhost window) at `CreateProcess`
/// time. The proxy is `windows_subsystem = "windows"`, so it has none —
/// every pty step it spawned (portable-pty 0.9 passes no
/// `CREATE_NO_WINDOW`; its `CreateProcessW` flags are
/// `EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT`) flashed a
/// console window: per `dotnet build`/`dotnet test` in the editor, and ~2
/// per session in test runs watched on a desktop (owner report
/// 2026-10-13, "30 terminal windows that pop up and then close").
/// With a console already present in the parent, `CreateProcess` attaches
/// the child to THAT one instead of allocating — no window, before the
/// pty re-attach replaces the child's std handles anyway.
///
/// The console is hidden once and kept for the process lifetime. The
/// original std handles are saved before `AllocConsole` (which re-points
/// this process's own std to the new console) and restored after — the
/// DAP stdio with Zed rides the original pipes untouched, which the
/// stdio/e2e suites prove on every run.
#[cfg(windows)]
fn hide_child_windows() {
    const STD_INPUT: u32 = 0xFFFF_FFF6; // (DWORD)-10
    const STD_OUTPUT: u32 = 0xFFFF_FFF5; // (DWORD)-11
    const STD_ERROR: u32 = 0xFFFF_FFF4; // (DWORD)-12
    const SW_HIDE: i32 = 0;
    // Hand-written bindings, the house convention (see runner.rs — no
    // windows-sys dependency).
    #[link(name = "kernel32")]
    extern "system" {
        fn GetStdHandle(which: u32) -> *mut core::ffi::c_void;
        fn SetStdHandle(which: u32, handle: *mut core::ffi::c_void) -> i32;
        fn GetConsoleWindow() -> *mut core::ffi::c_void;
        fn AllocConsole() -> i32;
    }
    #[link(name = "user32")]
    extern "system" {
        fn ShowWindow(hwnd: *mut core::ffi::c_void, cmd: i32) -> i32;
    }
    unsafe {
        // Launched from a terminal/CI: a console already exists and every
        // child inherits it — nothing flashes, and stealing it would only
        // hide the operator's own view of nothing.
        if !GetConsoleWindow().is_null() {
            return;
        }
        let stdio = [GetStdHandle(STD_INPUT), GetStdHandle(STD_OUTPUT), GetStdHandle(STD_ERROR)];
        if AllocConsole() == 0 {
            return; // headless session with no window station: children keep
                    // today's behavior, the relay keeps its pipes either way.
        }
        ShowWindow(GetConsoleWindow(), SW_HIDE);
        for (which, handle) in [(STD_INPUT, stdio[0]), (STD_OUTPUT, stdio[1]), (STD_ERROR, stdio[2])] {
            // Keep the console's own handle when there was no real pipe
            // (manual double-click launch): honest console beats a null.
            if !handle.is_null() && handle != (-1isize) as *mut _ {
                let _ = SetStdHandle(which, handle);
            }
        }
    }
}

#[cfg(not(windows))]
fn hide_child_windows() {}

fn main() -> ExitCode {
    hide_child_windows();
    zatara_proxy::run()
}
