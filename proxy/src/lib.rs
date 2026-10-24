//! zatara-proxy — the stdio DAP proxy Zed spawns as its debug adapter.
//!
//! The proxy answers the client on its own piped stdio and relays every DAP
//! message to a netcoredbg child run in interpreter mode (blueprint DR-1).
//! The message envelope (`seq` routing, Content-Length framing) is defined
//! here; the payload contracts are imported from zatara-types and never
//! redefined — the dependency direction stays extension -> types <- proxy.
//!
//! Seams frozen for the build-orchestration and output-pumping stories
//! (LOC-0109 / LOC-0111): `session::RelaySink` (child-message and
//! child-exit stream hooks) and `session::Session` (handshake phase).

pub mod asset;
// Solution-first exe-project discovery (LOC-0125, DR-4): the filesystem
// lives on this side of the wall, so resolution is called at launch time
// (wired by LOC-0129), never by the locator (DR-3 — enforced structurally:
// the extension crate does not depend on this one).
pub mod discovery;
pub mod framing;
// The shipped diagnostics gates (LOC-0117): both env-armed, default-off,
// and confined to the extension work dir. Internal — the gates are read at
// startup here, never driven per-message.
pub(crate) mod logging;
// The macOS walk (LOC-0116, re-scoped by D28): probe PATH and Homebrew
// prefixes for the Macs the release matrix cannot ship an asset for —
// Intel (no upstream build exists) and degraded Apple-Silicon sessions.
pub mod macos_netcoredbg;
// xunit v3 / MTP single-case selection (XUNIT-V3-MTP): enumerate the built
// test dll, match a gutter row to its unique id, hand back the `-id` argv.
// Shared by the Run CLI and the MTP debug lane; internal, nothing external.
pub(crate) mod mtp_case;
// The programrunner arm (LOC-0124, DR-5): project-mode launches ride the
// relay raw — no build, no test step. Internal: the dispatch decision is
// made in the relay's launch arm, nothing drives this module directly.
pub(crate) mod program;
// pub (not pub(crate)): the real-SDK breadth-count tests drive `build_filter`
// directly — the filter the runner puts on argv is the filter under test.
pub mod runner;
pub mod relay;
// The plain-Run lane CLI (XUNIT-V3-MTP): driven only by the `run-case`
// argv branch in main, never by the DAP relay. Internal.
pub(crate) mod runcli;
pub mod session;

use std::process::ExitCode;

/// Entry for `main`: spawn the adapter, relay until the client hangs up.
pub fn run() -> ExitCode {
    match relay::serve(relay::AdapterSpec::from_env(), Box::new(session::NullSink)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("zatara-proxy: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Entry for `main`'s `run-case` branch: the plain-Run lane, one gutter
/// case row per process, stdio to the Run tab, dotnet's exit code passed
/// through.
pub fn run_case() -> ExitCode {
    runcli::run_from_env()
}
