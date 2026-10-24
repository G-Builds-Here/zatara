use std::ffi::OsStr;
use std::path::{Path, PathBuf};
pub const MANUAL_INSTALL_HINT: &str = "https://github.com/Samsung/netcoredbg/releases";
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
pub const DEFAULT_BREW_PREFIXES: [&str; 2] = ["/opt/homebrew", "/usr/local"];
const NETCOREDBG: &str = "netcoredbg";
pub const PROBE_ARM_ENV: &str = "ZATARA_MACOS_NDBG_PROBE";
pub const BREW_PREFIXES_ENV: &str = "ZATARA_BREW_PREFIXES";
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NdbgSource {
    SettingsOverride,
    PathLookup,
    HomebrewPrefix,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedNdbg {
    pub path: PathBuf,
    pub source: NdbgSource,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeOutcome {
    Found(ResolvedNdbg),
    NotFound,
}
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
