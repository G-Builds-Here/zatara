#![windows_subsystem = "windows"]
use std::process::ExitCode;
#[cfg(windows)]
fn hide_child_windows() {
    const STD_INPUT: u32 = 0xFFFF_FFF6;
    const STD_OUTPUT: u32 = 0xFFFF_FFF5;
    const STD_ERROR: u32 = 0xFFFF_FFF4;
    const SW_HIDE: i32 = 0;
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
        if !GetConsoleWindow().is_null() {
            return;
        }
        let stdio = [GetStdHandle(STD_INPUT), GetStdHandle(STD_OUTPUT), GetStdHandle(STD_ERROR)];
        if AllocConsole() == 0 {
            return;
        }
        ShowWindow(GetConsoleWindow(), SW_HIDE);
        for (which, handle) in [(STD_INPUT, stdio[0]), (STD_OUTPUT, stdio[1]), (STD_ERROR, stdio[2])] {
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
    if std::env::args().nth(1).as_deref() == Some("run-case") {
        return zatara_proxy::run_case();
    }
    zatara_proxy::run()
}
