use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::Mutex;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
pub(crate) const ENGINE_LOGGING_ENV: &str = "ZATARA_ENGINE_LOGGING";
pub(crate) const PROXY_LOG_ENV: &str = "ZATARA_PROXY_LOG";
pub(crate) const ENGINE_LOG_NAME: &str = "netcoredbg-engine.log";
pub(crate) const PROXY_LOG_NAME: &str = "zatara-proxy.log";
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
pub(crate) fn usable_work_dir(work_dir: Option<&OsStr>) -> Option<&OsStr> {
    work_dir.filter(|dir| !dir.is_empty())
}
pub(crate) struct ProxyLog {
    file: Mutex<File>,
}
impl ProxyLog {
    pub(crate) fn open(path: &Path) -> std::io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(ProxyLog { file: Mutex::new(file) })
    }
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
    #[test]
    fn an_armed_gate_without_a_work_dir_resolves_nothing() {
        assert_eq!(gated_log_target(Some("1"), None, "x.log"), None);
        assert_eq!(
            gated_log_target(Some("1"), Some(OsStr::new("")), "x.log"),
            None,
            "an empty work dir must not resolve a log into the process CWD"
        );
    }
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
