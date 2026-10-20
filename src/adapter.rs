//! `get_dap_binary` wiring (LOC-0114 AC2): resolved paths in, adapter spec
//! out. The command is the proxy; the resolved netcoredbg path travels in
//! the documented `--netcoredbg <path>` argv pair — an argv array, never a
//! shell string. The flag itself is the shared contract in zatara-types
//! (`NETCOREDBG_ARG`, consumed identically by `zatara_proxy::relay`). A
//! resolution failure is reported to Zed as the resolver's own message —
//! never papered over with an invented command. One exception, by owner
//! ruling (2026-10-13): a release that cannot be *reached* degrades to a
//! warning over whatever binary is already here; see `dap_binary_with`.

use std::path::{Path, PathBuf};

use zed_extension_api::http_client::{HttpMethod, HttpRequest, RedirectPolicy};
use zed_extension_api::{
    Architecture, DebugAdapterBinary, DebugTaskDefinition, Os, StartDebuggingRequestArguments,
    StartDebuggingRequestArgumentsRequest, current_platform,
};
use zatara_types::{NETCOREDBG_ARG, WORK_DIR_ENV};

use crate::binary_manager::{
    BinaryId, BinaryManager, FetchError, Fetcher, PlatformTarget, ReleaseSource, ResolveError,
};

/// The repository whose pinned releases carry the assets (LOC-0108).
/// Public-surface split (2026-10-14): the private dev workspace no longer
/// publishes; releases are cut from the public extension repo, which ships
/// the proxy source they build from.
const RELEASE_REPO: &str = "G-Builds-Here/zatara";

/// Which matrix target the *host machine* is, per what Zed reports through
/// the host call. The compile-time `PlatformTarget::from_host` cannot answer
/// this from inside the guest: the wasm is built for `wasi`, so its cfgs
/// match no OS and it silently answered LinuxX64 on every editor — the live
/// session (owner repro, 2026-10-13) then looked for `-linux-x64` assets on
/// a Windows cache and reported "no proxy on this machine". Zed's
/// `current_platform` is the only truthful source in a wasm guest.
// The `_` arm is unreachable today — Os×Architecture is exhausted by the
// five arms above — but stays as the forward-compat answer: a future Os or
// Architecture variant lands on the honest std-cfg default instead of a
// compile error in the guest.
#[allow(unreachable_patterns)]
pub fn target_for(os: Os, arch: Architecture) -> PlatformTarget {
    match (os, arch) {
        (Os::Windows, _) => PlatformTarget::WinX64,
        (Os::Mac, Architecture::Aarch64) => PlatformTarget::MacosArm64,
        (Os::Mac, _) => PlatformTarget::MacosX64,
        (Os::Linux, Architecture::Aarch64) => PlatformTarget::LinuxArm64,
        (Os::Linux, _) => PlatformTarget::LinuxX64,
        // Wasm32-on-wasm32 or a future variant: no honest answer; the
        // std cfgs are the last-resort default.
        _ => PlatformTarget::from_host(),
    }
}

/// The two binaries an adapter spec needs, already resolved and trusted
/// (trust is BinaryManager's contract, LOC-0110 — this module only routes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterPaths {
    pub proxy: PathBuf,
    pub netcoredbg: PathBuf,
}

/// The proxy's argv as Zed must hand it: flag then path, two array slots.
pub fn proxy_arguments(paths: &AdapterPaths) -> Vec<String> {
    vec![
        NETCOREDBG_ARG.to_string(),
        paths.netcoredbg.display().to_string(),
    ]
}

/// The adapter spec for one resolved pair plus the session config the user's
/// scenario carried — passed through untouched (it reaches the proxy as the
/// DAP `launch` arguments, where zatara-types validates it). `work_dir` is
/// the extension work dir handed to the proxy through the envs seam (LOC-0117's
/// log gates resolve their one sanctioned log location from it); the handoff
/// is plumbing, never a gate.
pub fn adapter_binary(
    paths: &AdapterPaths,
    request_configuration: String,
    work_dir: &Path,
) -> DebugAdapterBinary {
    adapter_spec(&paths.proxy, proxy_arguments(paths), request_configuration, work_dir)
}

/// The adapter spec with netcoredbg left unresolved: the command is the
/// proxy and the argv pair is absent — the absence is what arms the
/// proxy's discovery walk (LOC-0116), so this shape is only legitimate
/// where that walk runs (the macOS branch of `dap_binary_for`). The work
/// dir rides the envs seam here too (LOC-0117): the walk changes *which*
/// binary the proxy names, not where its diagnostics may go.
pub fn proxy_only_binary(
    proxy: &Path,
    request_configuration: String,
    work_dir: &Path,
) -> DebugAdapterBinary {
    adapter_spec(proxy, Vec::new(), request_configuration, work_dir)
}

/// The one place a `DebugAdapterBinary` is assembled: command, arguments,
/// the untouched session config in the launch slot, and the work dir in the
/// envs seam. Plumbing, never a gate: the work dir is handed over so the
/// proxy's default-off diagnostics gates (LOC-0117) know their one
/// sanctioned log location. No gate env is ever set from the extension.
fn adapter_spec(
    command: &Path,
    arguments: Vec<String>,
    request_configuration: String,
    work_dir: &Path,
) -> DebugAdapterBinary {
    // T3 law (clean-room live finding, owner, Zed 1.24.2+stable.373): this
    // request kind IS the DAP command Zed sends — Zed builds the launch or
    // attach request from request_args (session initialize_sequence), never
    // from the dap_request_kind probe. Hardcoding Launch here made a
    // hand-written attach scenario arrive at the proxy as a launch, where
    // the test-lane rule rejected it ("missing field
    // `fullyQualifiedName`"). The kind therefore rides the config itself,
    // through the one shared rule.
    let (configuration, request) = launch_request_shape(&request_configuration);
    DebugAdapterBinary {
        command: Some(command.display().to_string()),
        arguments,
        envs: vec![(WORK_DIR_ENV.to_string(), work_dir.display().to_string())],
        cwd: None,
        connection: None,
        request_args: StartDebuggingRequestArguments { configuration, request },
    }
}

/// The two wire facts a session config carries, read in one place: the
/// configuration string the proxy receives as the DAP arguments, and the
/// request kind Zed turns into the DAP command.
///
/// Two shapes arrive from Zed. Locator-built scenarios carry the flat
/// launch config — its bytes are law (the LOC-0109 pin), so an unpeeled
/// config is returned verbatim. Hand-written `.zed/debug.json` entries can
/// arrive one layer deeper, wrapped under a single `config` key (measured
/// live at the T3 clean-room session: `{"config":{"request":"attach",
/// "process_id":…}}`). The proxy's contract is the flat zatara-types shape
/// either way, so any such wrapper is peeled before it reaches the wire.
/// Unparseable configurations ride through untouched as a launch —
/// reject-don't-ignore stays the proxy's job, not the guest's.
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

/// The peel rule both DAP answers share: an object of exactly one `config`
/// key whose value is an object is a wrapper, not a shape — step through
/// it (wrappers may nest). Anything else — a multi-key object,
/// `{"config": "a string"}`, a non-object — is the shape itself.
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

/// The request kind of a (possibly wrapper-shape) scenario config, shared
/// by the get_dap_binary wire answer and the `dap_request_kind` probe so
/// the two answers can never drift: a config naming `request: "attach"`
/// answers Attach, everything else — launch, test lane, unclassifiable —
/// answers Launch, the answer every shipped config already relied on.
pub fn request_kind_of(
    config: &serde_json::Value,
) -> StartDebuggingRequestArgumentsRequest {
    match scenario_config_peeled(config).get("request").and_then(serde_json::Value::as_str) {
        Some("attach") => StartDebuggingRequestArgumentsRequest::Attach,
        _ => StartDebuggingRequestArgumentsRequest::Launch,
    }
}

/// Resolve both binaries through BinaryManager (settings override → verified
/// cache → pinned download), surfacing any resolver error as the string Zed
/// shows. `proxy_override` is Zed's user-configured adapter path; the
/// netcoredbg override key (`dap.netcoredbg.binary`) has no read surface in
/// zed_extension_api 0.7.0 — the parameter keeps the seam open.
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
    Ok(AdapterPaths { proxy: proxy.path, netcoredbg: netcoredbg.path })
}

/// The `get_dap_binary` answer: resolve, then spec. The work dir is the
/// extension work dir the host `chdir`ed the guest into; the download
/// transport is the host-provided HTTP client (binary_manager's injected
/// transport note, discharged here).
pub fn dap_binary_for(
    config: &DebugTaskDefinition,
    user_provided_debug_adapter_path: Option<&str>,
) -> Result<DebugAdapterBinary, String> {
    let work_dir = std::env::current_dir()
        .map_err(|e| format!("zatara: cannot locate the extension work dir: {e}"))?;
    let fetcher = ExtensionHttpFetcher;
    let (host_os, host_arch) = current_platform();
    let host_target = target_for(host_os, host_arch);
    let manager = BinaryManager {
        work_dir,
        target: host_target,
        release_source: ReleaseSource::github_latest(RELEASE_REPO),
        fetcher: &fetcher,
    };
    // Zed's user-configured adapter binary names what Zed would spawn — for
    // zatara that is the proxy, so the override lands there.
    let proxy_override = user_provided_debug_adapter_path.map(Path::new);
    // The Intel-mac branch (LOC-0116, re-scoped by D28 2026-10-14): upstream
    // publishes no macOS Intel build, so on MacosX64 the extension names no
    // netcoredbg at all — leaving the argv pair absent arms the proxy's
    // settings→PATH→Homebrew-prefix probe, whose none-found answer names the
    // manual remedies. Apple Silicon is NOT that branch anymore: the pinned
    // release now stages Samsung's official netcoredbg-osx-arm64.zip, so
    // MacosArm64 resolves and names it exactly like win/linux. Passing the
    // bare default through would count as an explicit source and silence
    // the walk entirely. The work dir still rides the envs seam (LOC-0117):
    // the walk decides which binary to name, not where the default-off
    // gates may write.
    let name_netcoredbg = host_target != PlatformTarget::MacosX64;
    dap_binary_with(&manager, proxy_override, config.config.clone(), name_netcoredbg)
}

/// The injectable resolution core behind `dap_binary_for`.
///
/// Failure taxonomy (owner ruling 2026-10-13: an unreachable release is a
/// warning, never a session-blocker):
/// - The release cannot be *reached* (`ResolveError::Fetch` — offline, or a
///   private repo 404-ing its assets for the anonymous extension client):
///   degrade — run with the settings override or a cached binary, leave the
///   netcoredbg argv pair absent for the proxy's own walk, and hand the
///   proxy the reason through `RELEASE_WARNING_ENV` so the Console says so.
///   The one thing physics still refuses is spawning a proxy that exists
///   nowhere: with no override and no cache the error stands, carrying its
///   remedies.
/// - Everything else (checksum mismatch, unsafe asset name, unparseable
///   manifest): hard error, verbatim. Not being able to check is a warning;
///   checking and failing is not.
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
    if !name_netcoredbg {
        return Ok(proxy_only_binary(&proxy, request_configuration, &manager.work_dir));
    }
    match manager.resolve(BinaryId::NetcoreDbg, None) {
        Ok(netcoredbg) => Ok(adapter_binary(
            &AdapterPaths { proxy, netcoredbg: netcoredbg.path },
            request_configuration,
            &manager.work_dir,
        )),
        Err(e) if release_unreachable(&e) => {
            Ok(degraded_binary(manager, proxy, request_configuration, &e))
        }
        Err(e) => Err(e.to_string()),
    }
}

/// The degrade boundary: only an unreachable release (fetch-level failure)
/// warns instead of stopping. Integrity findings stay hard errors.
fn release_unreachable(e: &ResolveError) -> bool {
    matches!(e, ResolveError::Fetch(_))
}

/// The spec for a session that runs without verified binaries: the proxy
/// that exists on this machine, netcoredbg from cache when one matches this
/// target (else unnamed — the absent argv pair is what arms the proxy's own
/// env/PATH walk), and the reason carried to the Console through the proxy.
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
    // Owner ruling (2026-10-13): the degraded note is a LOG fact, not a UI
    // event. Guest stderr lands in Zed's log. The note is deliberately NOT
    // handed to the proxy through `RELEASE_WARNING_ENV`: shipped proxies
    // answer that seam with a Console event, and the ruling keeps the
    // Console clean. The constant stays defined for a future release that
    // consumes it as a log line only.
    eprintln!(
        "[zatara] degraded session: the release manifest is unreachable, this session's \
         binaries were not verified against the pinned hashes: {reason}"
    );
    spec
}

/// The error that stays when there is nothing on disk to run degraded: no
/// proxy exists to spawn. Carries the remedies instead of a bare failure.
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

/// The download transport the wasm guest injects into BinaryManager: the
/// extension host's own HTTP client, shape-mapped onto `Fetcher` — transport
/// glue only, no resolution logic (DR-7).
pub struct ExtensionHttpFetcher;

/// The GET request Zed's host executes for a download. Two host-client
/// traps, both found by live editors rather than the suites (which drive
/// the loopback transport, where neither applies):
/// - The method must be named explicitly — `build()` refuses a request
///   without one ("Method not set").
/// - The redirect policy defaults to `NoFollow` (zed_extension_api 0.7,
///   `http_client::RedirectPolicy`), and GitHub's `releases/*/download`
///   route answers EVERY asset request with a 302 to its signed asset
///   host. NoFollow therefore returns an empty 200-page body — a fresh
///   machine saw it as "release manifest could not be parsed: EOF while
///   parsing a value at line 1 column 0" (2026-10-14). The old machines
///   masked this forever: their caches were seeded, and the unreachable-
///   release degradation rule kept fetch failures off the happy path.
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
        // The host client folds non-2xx answers into its error string — the
        // Status variant stays for transports that see raw codes.
        request.fetch().map(|response| response.body).map_err(FetchError::Transport)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rule 3 on the fresh-machine defect (2026-10-14): the release route
    /// answers every asset request with a 302, and the host client's
    /// builder defaults to `NoFollow` — so a request built without an
    /// explicit policy downloads an empty body (owner saw it as the
    /// manifest parsing as EOF at position 0). Red on the pre-fix
    /// builder, green on the fix; the loopback suites cannot see either
    /// the default or the 302, which is exactly why it shipped.
    #[test]
    fn download_requests_follow_redirects() {
        let request = get_request("https://example.invalid/x").expect("request builds");
        assert!(
            matches!(request.redirect_policy, RedirectPolicy::FollowAll),
            "download requests must carry FollowAll, found {:?}",
            request.redirect_policy
        );
    }
}
