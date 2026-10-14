//! macOS netcoredbg discovery (LOC-0116, re-scoped by D28 2026-10-14):
//! the walk for the Macs the download lane cannot serve.
//!
//! Apple Silicon IS served now — Samsung/netcoredbg publishes an official
//! osx-arm64 build and the pinned zatara release stages it, so the
//! extension resolves and names it like every other platform. This module
//! remains for the rest: Intel Macs (upstream publishes no macOS Intel
//! build at all — verified 2026-10-14 — and there is no Homebrew formula
//! either, the 404 that killed the old guidance), plus Apple-Silicon
//! sessions degraded to no named binary (unreachable release, empty
//! cache). The *proxy* finds the debugger instead of fetching it: the
//! settings path verbatim first, a PATH walk next, the Homebrew prefix's
//! bin directory last (a fine place to FIND a manual install — no formula
//! to install). Nothing found is not a crash: the guidance names the
//! manual remedies that exist.
//!
//! [`discover`] takes its inputs (PATH, prefixes) and its candidate
//! predicate as arguments — that seam is what lets the acceptance suite
//! grade the probe *order* (including "never consulted" claims) on any
//! host, not only on Macs. The logic itself is path arithmetic plus a
//! metadata query: nothing here is Mac-specific at compile time.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Where the manual remedy lives: the upstream release page whose
/// osx-arm64 asset zatara itself serves (and whose absence this walk's
/// none-found answer explains). The exact URL the guidance tests quote;
/// [`missing_binary_message`] embeds it. (The retired `brew install
/// netcoredbg` hint named a formula that does not exist — D28, 2026-10-14.)
pub const MANUAL_INSTALL_HINT: &str = "https://github.com/Samsung/netcoredbg/releases";

/// The none-found verdict: a message, not a stack — naming what was
/// missing and the manual remedy that actually exists for this Mac.
pub fn missing_binary_message() -> String {
    format!(
        "zatara: netcoredbg was not found on PATH or under a Homebrew prefix \
         on this Mac. zatara downloads the official Apple-silicon build \
         automatically — getting this answer means nothing was named and \
         nothing was cached yet (and upstream publishes no macOS Intel \
         build at all). Install netcoredbg manually from {MANUAL_INSTALL_HINT} \
         (osx-arm64; Intel Macs build from source) and keep it on PATH."
    )
}

/// The canonical Homebrew install prefixes: Apple Silicon and Intel. Every
/// real brew install lives under one of these, so the probe never needs to
/// ask brew itself (no `brew --prefix` spawn — probing a path beats
/// running a program to learn a path).
pub const DEFAULT_BREW_PREFIXES: [&str; 2] = ["/opt/homebrew", "/usr/local"];

/// The binary name, as PATH and manual-install conventions both spell it.
const NETCOREDBG: &str = "netcoredbg";

/// Arming the Mac discovery branch on non-Mac hosts (tests/CI only —
/// production Macs are armed by their OS). Like `ZATARA_NETCOREDBG`, an
/// env seam: the probe is host-agnostic logic run against a host
/// filesystem, so every CI OS can exercise the branch end-to-end.
pub const PROBE_ARM_ENV: &str = "ZATARA_MACOS_NDBG_PROBE";

/// Lets the suites pin the Homebrew prefixes the probe walks, the same way
/// `ZATARA_NETCOREDBG` pins the binary path. Production leaves it unset and
/// gets [`DEFAULT_BREW_PREFIXES`].
pub const BREW_PREFIXES_ENV: &str = "ZATARA_BREW_PREFIXES";

/// Which source supplied the debugger — reported so logs and tests can
/// tell "honored your setting" from "found it ourselves".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NdbgSource {
    /// The user's configured path, returned untouched without an
    /// existence grade — the voucher semantics of the LOC-0110 settings
    /// override, preserved here.
    SettingsOverride,
    /// Found by the PATH walk.
    PathLookup,
    /// Found under a Homebrew prefix's `bin` directory.
    HomebrewPrefix,
}

/// A netcoredbg path plus the source it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedNdbg {
    pub path: PathBuf,
    pub source: NdbgSource,
}

/// The verdict of one discovery run: found, or none of the three sources
/// had it (AC2 turns the latter into guidance).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeOutcome {
    Found(ResolvedNdbg),
    NotFound,
}

/// The three-source order: settings (verbatim, zero probes) → PATH →
/// Homebrew prefixes. `is_candidate` grades each named file; the walk
/// stops at the first yes.
pub fn discover(
    settings_override: Option<&Path>,
    path_var: Option<&OsStr>,
    brew_prefixes: &[PathBuf],
    is_candidate: &dyn Fn(&Path) -> bool,
) -> ProbeOutcome {
    if let Some(path) = settings_override {
        return ProbeOutcome::Found(ResolvedNdbg {
            path: path.to_path_buf(),
            source: NdbgSource::SettingsOverride,
        });
    }
    if let Some(entries) = path_var {
        for dir in std::env::split_paths(entries) {
            let candidate = dir.join(NETCOREDBG);
            if is_candidate(&candidate) {
                return ProbeOutcome::Found(ResolvedNdbg {
                    path: candidate,
                    source: NdbgSource::PathLookup,
                });
            }
        }
    }
    for prefix in brew_prefixes {
        let candidate = prefix.join("bin").join(NETCOREDBG);
        if is_candidate(&candidate) {
            return ProbeOutcome::Found(ResolvedNdbg {
                path: candidate,
                source: NdbgSource::HomebrewPrefix,
            });
        }
    }
    ProbeOutcome::NotFound
}

/// The production candidate grade: an existing file that is executable
/// where executability is graded at all (unix mode bits); on Windows,
/// existence is the grade.
pub fn host_is_candidate(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}
