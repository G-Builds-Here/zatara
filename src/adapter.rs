use crate::binary_manager::{
    BinaryId, BinaryManager, ExecMarker, FetchError, Fetcher, PlatformTarget, ReleaseSource,
    ResolveError,
};
use std::path::{Path, PathBuf};
use zatara_types::{NETCOREDBG_ARG, WORK_DIR_ENV};
use zed_extension_api::http_client::{HttpMethod, HttpRequest, RedirectPolicy};
use zed_extension_api::{
    current_platform, Architecture, DebugAdapterBinary, DebugTaskDefinition, Os,
    StartDebuggingRequestArguments, StartDebuggingRequestArgumentsRequest,
};
const RELEASE_REPO: &str = "G-Builds-Here/zatara";
#[allow(unreachable_patterns)]
pub fn target_for(os: Os, arch: Architecture) -> PlatformTarget {
    match (os, arch) {
        (Os::Windows, _) => PlatformTarget::WinX64,
        (Os::Mac, Architecture::Aarch64) => PlatformTarget::MacosArm64,
        (Os::Mac, _) => PlatformTarget::MacosX64,
        (Os::Linux, Architecture::Aarch64) => PlatformTarget::LinuxArm64,
        (Os::Linux, _) => PlatformTarget::LinuxX64,
        _ => PlatformTarget::from_host(),
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterPaths {
    pub proxy: PathBuf,
    pub netcoredbg: PathBuf,
}
pub fn proxy_arguments(paths: &AdapterPaths) -> Vec<String> {
    vec![
        NETCOREDBG_ARG.to_string(),
        paths.netcoredbg.display().to_string(),
    ]
}
pub fn adapter_binary(
    paths: &AdapterPaths,
    request_configuration: String,
    work_dir: &Path,
) -> DebugAdapterBinary {
    adapter_spec(
        &paths.proxy,
        proxy_arguments(paths),
        request_configuration,
        work_dir,
    )
}
pub fn proxy_only_binary(
    proxy: &Path,
    request_configuration: String,
    work_dir: &Path,
) -> DebugAdapterBinary {
    adapter_spec(proxy, Vec::new(), request_configuration, work_dir)
}
fn adapter_spec(
    command: &Path,
    arguments: Vec<String>,
    request_configuration: String,
    work_dir: &Path,
) -> DebugAdapterBinary {
    let (configuration, request) = launch_request_shape(&request_configuration);
    DebugAdapterBinary {
        command: Some(command.display().to_string()),
        arguments,
        envs: vec![(WORK_DIR_ENV.to_string(), work_dir.display().to_string())],
        cwd: None,
        connection: None,
        request_args: StartDebuggingRequestArguments {
            configuration,
            request,
        },
    }
}
pub fn launch_request_shape(
    request_configuration: &str,
) -> (String, StartDebuggingRequestArgumentsRequest) {
    let parsed = serde_json::from_str::<serde_json::Value>(request_configuration).ok();
    let kind = parsed
        .as_ref()
        .map(request_kind_of)
        .unwrap_or(StartDebuggingRequestArgumentsRequest::Launch);
    let configuration = match &parsed {
        Some(value) => {
            let peeled = scenario_config_peeled(value);
            if peeled != value {
                peeled.to_string()
            } else {
                request_configuration.to_string()
            }
        }
        None => request_configuration.to_string(),
    };
    (configuration, kind)
}
pub fn scenario_config_peeled(config: &serde_json::Value) -> &serde_json::Value {
    let mut current = config;
    loop {
        let next = match current {
            serde_json::Value::Object(map) if map.len() == 1 => match map.get("config") {
                Some(inner @ serde_json::Value::Object(_)) => Some(inner),
                _ => None,
            },
            _ => None,
        };
        match next {
            Some(inner) => current = inner,
            None => return current,
        }
    }
}
pub fn request_kind_of(config: &serde_json::Value) -> StartDebuggingRequestArgumentsRequest {
    match scenario_config_peeled(config)
        .get("request")
        .and_then(serde_json::Value::as_str)
    {
        Some("attach") => StartDebuggingRequestArgumentsRequest::Attach,
        _ => StartDebuggingRequestArgumentsRequest::Launch,
    }
}
pub fn resolve_adapter_paths(
    manager: &BinaryManager,
    proxy_override: Option<&Path>,
    netcoredbg_override: Option<&Path>,
) -> Result<AdapterPaths, String> {
    let proxy = manager
        .resolve(BinaryId::ZataraProxy, proxy_override)
        .map_err(|e| e.to_string())?;
    let netcoredbg = manager
        .resolve(BinaryId::NetcoreDbg, netcoredbg_override)
        .map_err(|e| e.to_string())?;
    Ok(AdapterPaths {
        proxy: proxy.path,
        netcoredbg: netcoredbg.path,
    })
}
pub fn dap_binary_for(
    config: &DebugTaskDefinition,
    user_provided_debug_adapter_path: Option<&str>,
) -> Result<DebugAdapterBinary, String> {
    let work_dir = std::env::current_dir()
        .map_err(|e| format!("zatara: cannot locate the extension work dir: {e}"))?;
    let fetcher = ExtensionHttpFetcher;
    let (host_os, host_arch) = current_platform();
    let host_target = target_for(host_os, host_arch);
    let exec_marker = if matches!(host_target, PlatformTarget::WinX64) {
        HostExecMarker::Noop
    } else {
        HostExecMarker::Chmod
    };
    let manager = BinaryManager {
        work_dir,
        target: host_target,
        release_source: ReleaseSource::github_latest(RELEASE_REPO),
        fetcher: &fetcher,
        exec_marker: &exec_marker,
    };
    let proxy_override = user_provided_debug_adapter_path.map(Path::new);
    let name_netcoredbg = host_target != PlatformTarget::MacosX64;
    dap_binary_with(
        &manager,
        proxy_override,
        config.config.clone(),
        name_netcoredbg,
    )
}
pub fn dap_binary_with(
    manager: &BinaryManager,
    proxy_override: Option<&Path>,
    request_configuration: String,
    name_netcoredbg: bool,
) -> Result<DebugAdapterBinary, String> {
    let proxy = match manager.resolve(BinaryId::ZataraProxy, proxy_override) {
        Ok(resolved) => resolved.path,
        Err(e) if release_unreachable(&e) => {
            return match manager.cached_asset(BinaryId::ZataraProxy) {
                Some(cached) => Ok(degraded_binary(manager, cached, request_configuration, &e)),
                None => Err(unreachable_without_proxy_message(&manager.work_dir, &e)),
            };
        }
        Err(e) => return Err(e.to_string()),
    };
    if proxy_override.is_none() && matches!(manager.target, PlatformTarget::WinX64) {
        if let Err(e) = manager.resolve(BinaryId::ZataraRunCase, None) {
            eprintln!(
                "[zatara] run-case helper unavailable this session ({e}); row Run \
                 keeps working through the launcher's buffered fallback"
            );
        }
    }
    if !name_netcoredbg {
        return Ok(proxy_only_binary(
            &proxy,
            request_configuration,
            &manager.work_dir,
        ));
    }
    match manager.resolve(BinaryId::NetcoreDbg, None) {
        Ok(netcoredbg) => Ok(adapter_binary(
            &AdapterPaths {
                proxy,
                netcoredbg: netcoredbg.path,
            },
            request_configuration,
            &manager.work_dir,
        )),
        Err(e) if release_unreachable(&e) => {
            Ok(degraded_binary(manager, proxy, request_configuration, &e))
        }
        Err(e) => Err(e.to_string()),
    }
}
fn release_unreachable(e: &ResolveError) -> bool {
    matches!(e, ResolveError::Fetch(_))
}
fn degraded_binary(
    manager: &BinaryManager,
    proxy: PathBuf,
    request_configuration: String,
    reason: &ResolveError,
) -> DebugAdapterBinary {
    let spec = match manager.cached_asset(BinaryId::NetcoreDbg) {
        Some(netcoredbg) => adapter_binary(
            &AdapterPaths { proxy, netcoredbg },
            request_configuration,
            &manager.work_dir,
        ),
        None => proxy_only_binary(&proxy, request_configuration, &manager.work_dir),
    };
    eprintln!(
        "[zatara] degraded session: the release manifest is unreachable, this session's \
         binaries were not verified against the pinned hashes: {reason}"
    );
    spec
}
fn unreachable_without_proxy_message(work_dir: &Path, reason: &ResolveError) -> String {
    format!(
        "{reason}\nzatara: no zatara-proxy binary on this machine and the release is \
         unreachable (a private repo 404s its release assets for the extension's \
         anonymous client). Cache scanned: {}. Remedies: set \"dap\": \
         {{\"zatara-netcoredbg\": {{\"binary\": \"<path to zatara-proxy>\"}}}} in Zed settings, \
         run one session while a reachable release is available (the cache then serves \
         degraded sessions offline), or publish the releases where this client can reach them.",
        work_dir.join(crate::binary_manager::CACHE_DIR).display()
    )
}
pub struct ExtensionHttpFetcher;
pub fn get_request(url: &str) -> Result<HttpRequest, String> {
    HttpRequest::builder()
        .method(HttpMethod::Get)
        .redirect_policy(RedirectPolicy::FollowAll)
        .url(url)
        .build()
}
impl Fetcher for ExtensionHttpFetcher {
    fn fetch(&self, url: &str) -> Result<Vec<u8>, FetchError> {
        let request = get_request(url).map_err(FetchError::Transport)?;
        request
            .fetch()
            .map(|response| response.body)
            .map_err(FetchError::Transport)
    }
}
pub enum HostExecMarker {
    Chmod,
    Noop,
}
impl ExecMarker for HostExecMarker {
    fn mark_executable(&self, path: &Path) -> Result<(), String> {
        match self {
            Self::Noop => Ok(()),
            Self::Chmod => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let metadata = std::fs::metadata(path).map_err(|e| e.to_string())?;
                    let mut perms = metadata.permissions();
                    let mode = perms.mode() | 0o111;
                    perms.set_mode(mode);
                    std::fs::set_permissions(path, perms).map_err(|e| e.to_string())?;
                    Ok(())
                }
                #[cfg(not(unix))]
                {
                    Ok(())
                }
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn download_requests_follow_redirects() {
        let request = get_request("https://example.invalid/x").expect("request builds");
        assert!(
            matches!(request.redirect_policy, RedirectPolicy::FollowAll),
            "download requests must carry FollowAll, found {:?}",
            request.redirect_policy
        );
    }
    #[cfg(unix)]
    #[test]
    fn host_exec_marker_chmod_makes_file_executable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join("zatara_test_exec");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test.bin");
        std::fs::write(&path, b"data").expect("write");
        let meta = std::fs::metadata(&path).expect("metadata");
        assert_eq!(meta.permissions().mode() & 0o111, 0);
        let marker = HostExecMarker::Chmod;
        marker.mark_executable(&path).expect("mark");
        let meta = std::fs::metadata(&path).expect("metadata");
        assert_ne!(meta.permissions().mode() & 0o111, 0);
        let _ = std::fs::remove_file(&path);
    }
    #[test]
    fn host_exec_marker_noop_succeeds() {
        let marker = HostExecMarker::Noop;
        let path = std::path::Path::new("/nonexistent");
        assert!(marker.mark_executable(path).is_ok());
    }
    #[cfg(unix)]
    #[test]
    fn host_exec_marker_chmod_errors_on_missing_file() {
        let marker = HostExecMarker::Chmod;
        let path = std::path::Path::new("/tmp/zatara_nonexistent_12345.bin");
        let _ = std::fs::remove_file(path);
        let err = marker.mark_executable(path).expect_err("should error");
        assert!(!err.is_empty());
    }
}
