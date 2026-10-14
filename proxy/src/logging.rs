//! The two shipped diagnostics gates (LOC-0117) and the proxy's single log
//! sink. Both are env-armed and default-off: an absent setting is an off
//! setting, and an on value the code doesn't know stays off too — only
//! `"1"` flips a gate. Diagnostics keep this law even after the 2026-10-14
//! D18 flip made `ZATARA_RUNNER` (runner.rs) default-on: an accidentally
//! armed gate is a machine writing files nobody asked for, an accidentally
//! off runner is a session that silently does nothing — opposite failure
//! directions, opposite defaults.
//!
//! The gates share one hard location rule: the extension work dir (handed in
//! as `WORK_DIR_ENV` by `get_dap_binary`) is the *only* place either may ever
//! write. A gate armed with no work dir known stays off rather than invent a
//! location — the shipped path writes nothing to disk that the user did not
//! ask for, and nothing to shared temp ever (DR-3; the superseded prototype's
//! fixed-name `%TEMP%` logs are the anti-pattern).
//!
//! AC1's flag is netcoredbg's own `--engineLogging=<file>` (wire-traffic
//! JSONL the child writes itself — the proxy only puts the path in argv);
//! AC2's sink is this module's `ProxyLog`, written by the proxy alone.

use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::Mutex;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// AC1's gate: when set to `1`, the adapter child is spawned with netcoredbg's
/// engine-logging flag writing wire traffic under the work dir.
pub(crate) const ENGINE_LOGGING_ENV: &str = "ZATARA_ENGINE_LOGGING";

/// AC2's gate: when set to `1`, the proxy logs its own session lifecycle to
/// the single file named below, under the work dir.
pub(crate) const PROXY_LOG_ENV: &str = "ZATARA_PROXY_LOG";

/// The one wire-traffic file AC1's gate may produce (written by the adapter
/// child itself, at the path the proxy hands it).
pub(crate) const ENGINE_LOG_NAME: &str = "netcoredbg-engine.log";

/// The one proxy-owned log file AC2's gate may produce.
pub(crate) const PROXY_LOG_NAME: &str = "zatara-proxy.log";

/// Resolve a gate's log file inside the work dir: armed (`"1"`) *and* a known
/// work dir, or nothing. Pure — the unit tests below are the whole gate
/// contract; the callers own what it does with the target.
pub(crate) fn gated_log_target(
    gate: Option<&str>,
    work_dir: Option<&OsStr>,
    file_name: &str,
) -> Option<PathBuf> {
    if gate != Some("1") {
        return None;
    }
    usable_work_dir(work_dir).map(|dir| PathBuf::from(dir).join(file_name))
}

/// The work dir only *is* a location when it names one: an empty value
/// means the process CWD by path convention, and the CWD is not a
/// sanctioned log location (DR-3) — an empty value is treated as no work
/// dir, so the gate stays off instead of inventing the one place it must
/// never write. A relative value is taken as given: it comes from the
/// operator's own environment (a settings gate, not untrusted input), and
/// the production carrier always hands over an absolute path.
pub(crate) fn usable_work_dir(work_dir: Option<&OsStr>) -> Option<&OsStr> {
    work_dir.filter(|dir| !dir.is_empty())
}

/// The proxy's single log sink: one append-mode file, opened only when
/// AC2's gate resolved a target. Every line carries an epoch-ms stamp; write
/// failures are reported once on stderr and never fail the session — losing
/// the log must never lose the debug run.
pub(crate) struct ProxyLog {
    file: Mutex<File>,
}

impl ProxyLog {
    /// Open (creating, appending) the resolved sink.
    pub(crate) fn open(path: &Path) -> std::io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(ProxyLog { file: Mutex::new(file) })
    }

    /// Append one lifecycle line. Best effort by design (see struct doc).
    pub(crate) fn note(&self, line: &str) {
        let mut file = self.file.lock().expect("proxy log lock poisoned");
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        if writeln!(file, "{stamp} {line}").and_then(|()| file.flush()).is_err() {
            eprintln!("zatara-proxy: proxy log write failed (continuing without it)");
        }
    }
}

#[cfg(test)]
mod gate_tests {
    use super::*;

    /// The knob, pure — the `ZATARA_RUNNER` law: only the exact value arms.
    #[test]
    fn only_the_exact_one_arms_a_gate() {
        assert!(gated_log_target(Some("1"), Some(OsStr::new("/wd")), "x.log").is_some());
        for disarmed in ["0", "true", "yes", "", " 1", "1 "] {
            assert!(
                gated_log_target(Some(disarmed), Some(OsStr::new("/wd")), "x.log").is_none(),
                "`{disarmed}` must not arm a default-off gate"
            );
        }
        assert!(gated_log_target(None, Some(OsStr::new("/wd")), "x.log").is_none());
    }

    /// An armed gate with no work dir stays off: the work dir is the only
    /// sanctioned log location, so there is nowhere to write and the gate
    /// does not invent one (DR-3) — not even implicitly: an empty value
    /// would otherwise name the process CWD, which is no sanctioned
    /// location either.
    #[test]
    fn an_armed_gate_without_a_work_dir_resolves_nothing() {
        assert_eq!(gated_log_target(Some("1"), None, "x.log"), None);
        assert_eq!(
            gated_log_target(Some("1"), Some(OsStr::new("")), "x.log"),
            None,
            "an empty work dir must not resolve a log into the process CWD"
        );
    }

    /// Armed and located: the sink is the named file inside the work dir —
    /// one file, one directory, never a path the user configured per-session.
    #[test]
    fn an_armed_gate_resolves_its_named_file_inside_the_work_dir() {
        assert_eq!(
            gated_log_target(Some("1"), Some(OsStr::new("/wd")), ENGINE_LOG_NAME),
            Some(PathBuf::from("/wd").join(ENGINE_LOG_NAME))
        );
        assert_eq!(
            gated_log_target(Some("1"), Some(OsStr::new("/wd")), PROXY_LOG_NAME),
            Some(PathBuf::from("/wd").join(PROXY_LOG_NAME))
        );
    }
}
