//! The console-subsystem Run-lane helper (XUNIT-V3-MTP).
//!
//! One crate-root, one job: run the plain-Run case and let its output ride
//! the inherited console. This is the whole reason it is a separate binary —
//! the subsystem attribute is per crate-root, so `main.rs` stays
//! `#![windows_subsystem = "windows"]` (no console window flashes during a
//! debug session) while this root keeps the default console subsystem.
//!
//! Why the split matters on the Run tab: under the pseudo-terminal Zed
//! composes tasks through, a GUI-subsystem process's live stdout never
//! renders (the G2 live cycle 2026-10-23 — the exit code rides home, the
//! text does not). A console-subsystem process inherits that pseudo-terminal
//! and its live bytes render, the way every console program (bash, plain
//! pwsh) rendered through it. So the launcher runs this helper when it is
//! present and keeps the buffered-proxy hop only as the fallback, and the
//! Run tab streams the row's output as dotnet prints it — with dotnet's
//! green/red verdict styling, since a console child is what dotnet reads a
//! terminal from.
//!
//! No behavior lives here. The row's identity arrives as the
//! `ZED_CUSTOM_csharp_*` capture env exactly as it does for the proxy, and
//! `run_case()` composes the filter, spawns dotnet, and passes its exit code
//! through — one grammar table and one decline-don't-guess law for both
//! binaries. No hidden-console shim: that shim exists because the proxy has
//! no console to give its children; this process has one from birth, and the
//! dotnet it spawns inherits it.

use std::process::ExitCode;

fn main() -> ExitCode {
    zatara_proxy::run_case()
}
