//! BinaryManager: turn a binary name into a trustworthy path (LOC-0110).
//!
//! Resolution order (Epic AC-E4 / story S4):
//! 1. the user-settings path override (the `dap.netcoredbg.binary` pattern
//!    the owner ruled to preserve — Q2), returned verbatim, zero network;
//! 2. the extension work-dir cache, but only after re-hashing it against the
//!    pinned release manifest — a cache entry is never trusted from memory;
//! 3. a pinned release download, SHA256-verified against the LOC-0108
//!    `manifest.json` before a single byte is cached or returned.
//!
//! A mismatch is a hard error naming both hashes and yielding nothing
//! usable; unverified bytes never reach the cache. This module is transport-
//! agnostic behind [`Fetcher`]: the wasm guest keeps pure resolution logic
//! (sha2/serde compile for wasm32-wasip2), and whoever drives the download
//! injects the transport.

use std::fmt;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

/// Subdirectory of the extension work dir that holds verified assets.
pub const CACHE_DIR: &str = "binary-cache";
/// Manifest asset name as published by the release workflow (LOC-0108).
pub const MANIFEST_ASSET: &str = "manifest.json";

/// One asset as recorded by `scripts/build_release_manifest.py`: name, pinned
/// release version, and the sha256 of the bytes as staged (the same bytes the
/// release will serve).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ManifestAsset {
    pub name: String,
    pub version: String,
    pub sha256: String,
}

/// The published `manifest.json` (`{"assets": [...]}`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Manifest {
    pub assets: Vec<ManifestAsset>,
}

/// The two binaries this story resolves. The asset-name prefixes match the
/// staging names the release workflow publishes
/// (`zatara-proxy-{ver}-{target}{.exe|}`, `netcoredbg-{pin}-{target}.{ext}`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryId {
    ZataraProxy,
    NetcoreDbg,
}

impl BinaryId {
    fn prefix(self) -> &'static str {
        match self {
            Self::ZataraProxy => "zatara-proxy-",
            Self::NetcoreDbg => "netcoredbg-",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::ZataraProxy => "zatara-proxy",
            Self::NetcoreDbg => "netcoredbg",
        }
    }
}

/// Target platforms of the release matrix (release.yml `matrix.target`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformTarget {
    WinX64,
    LinuxX64,
    LinuxArm64,
    MacosX64,
    MacosArm64,
}

impl PlatformTarget {
    /// The matrix `target` string baked into every published asset name.
    pub fn tag(self) -> &'static str {
        match self {
            Self::WinX64 => "win-x64",
            Self::LinuxX64 => "linux-x64",
            Self::LinuxArm64 => "linux-arm64",
            Self::MacosX64 => "macos-x64",
            Self::MacosArm64 => "macos-arm64",
        }
    }

    /// Best-effort target of the machine the code runs on.
    ///
    /// STD-ONLY truth: the cfgs read the *compile* target. Inside the wasm
    /// guest they read `wasi` and the fallback answers — a guest must ask
    /// the host instead (`zed_extension_api::current_platform`, routed
    /// through `adapter::target_for`). Live repro, owner 2026-10-13: this
    /// function silently chose LinuxX64 on a Windows editor.
    pub fn from_host() -> Self {
        #[cfg(target_os = "windows")]
        {
            Self::WinX64
        }
        #[cfg(target_os = "macos")]
        {
            if std::env::consts::ARCH == "aarch64" {
                Self::MacosArm64
            } else {
                Self::MacosX64
            }
        }
        #[cfg(target_os = "linux")]
        {
            if std::env::consts::ARCH == "aarch64" {
                Self::LinuxArm64
            } else {
                Self::LinuxX64
            }
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
        {
            // wasm guest and other targets do not themselves download; the
            // host that drives resolution passes an explicit target.
            Self::LinuxX64
        }
    }
}

/// Where the resolved bytes came from, as reported to the caller (and the CI
/// log): the only source that touches the network is `Downloaded`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedSource {
    SettingsOverride,
    VerifiedCache,
    Downloaded,
}

/// A binary path the manager vouches for — except for `SettingsOverride`,
/// where the user's own setting *is* the voucher (owner ruling Q2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedBinary {
    pub path: PathBuf,
    pub source: ResolvedSource,
    /// sha256 of the verified bytes; `None` for a settings override, whose
    /// bytes the manager never reads.
    pub sha256: Option<String>,
    /// Published asset name the verification was performed against.
    pub asset: Option<String>,
}

/// Transport seam: fetch the bytes behind a URL. Keeping this a trait is what
/// lets resolution be tested with a zero-request network spy (story AC1's
/// observable contract) and lets the wasm guest share the logic with the
/// native drivers that do the fetching.
pub trait Fetcher {
    fn fetch(&self, url: &str) -> Result<Vec<u8>, FetchError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// Could not reach the server / broken transport.
    Transport(String),
    /// Server answered with a non-2xx status.
    Status { url: String, code: u16 },
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(detail) => write!(f, "fetch failed: {detail}"),
            Self::Status { url, code } => write!(f, "fetch of {url} answered HTTP {code}"),
        }
    }
}
impl std::error::Error for FetchError {}

/// Where published assets live: the `{base}/{asset}` download endpoint of a
/// pinned release (GitHub's `releases/latest/download` route, or any mirror
/// that serves the same layout — which is what CI points at).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseSource {
    pub download_base: String,
}

impl ReleaseSource {
    pub fn github_latest(repo: &str) -> Self {
        Self { download_base: format!("https://github.com/{repo}/releases/latest/download") }
    }

    pub fn asset_url(&self, name: &str) -> String {
        format!("{}/{}", self.download_base.trim_end_matches('/'), name)
    }

    pub fn fetch_manifest(&self, fetcher: &dyn Fetcher) -> Result<Manifest, ResolveError> {
        let bytes = fetcher
            .fetch(&self.asset_url(MANIFEST_ASSET))
            .map_err(ResolveError::Fetch)?;
        serde_json::from_slice(&bytes).map_err(|e| ResolveError::ManifestParse(e.to_string()))
    }
}

/// Pick the manifest entry for `binary` on `target`. Exactly one entry must
/// match — the release publishes one asset per binary per matrix target, so
/// zero or multiple matches mean a manifest this code must not guess over.
pub fn select_asset<'m>(
    manifest: &'m Manifest,
    binary: BinaryId,
    target: PlatformTarget,
) -> Result<&'m ManifestAsset, ResolveError> {
    let needle = format!("-{}", target.tag());
    let matches: Vec<&ManifestAsset> = manifest
        .assets
        .iter()
        .filter(|a| a.name.starts_with(binary.prefix()) && a.name.contains(&needle))
        .collect();
    match matches.as_slice() {
        [one] => {
            // Containment before use: the manifest is *input*, and its
            // asset name flows into the cache path join and the download
            // URL. A malicious manifest pins its own hashes, so SHA256
            // cannot defend against it — only refusing a name that is not
            // a plain file name can keep resolution inside the cache dir
            // and the request line.
            if !is_plain_file_name(&one.name) {
                return Err(ResolveError::UnsafeAssetName(one.name.clone()));
            }
            Ok(*one)
        }
        _ => Err(ResolveError::NoAssetForTarget {
            binary: binary.label(),
            target: target.tag().to_string(),
            candidates: manifest.assets.iter().map(|a| a.name.clone()).collect(),
        }),
    }
}

/// A name usable as a single path segment in the cache dir and as a bare
/// URL/request-line token on both path syntaxes (POSIX `/` and Windows
/// `\`), with no control characters for request-line injection.
fn is_plain_file_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && !name.chars().any(char::is_control)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    Fetch(FetchError),
    ManifestParse(String),
    NoAssetForTarget { binary: &'static str, target: String, candidates: Vec<String> },
    /// Manifest asset name would escape the cache dir or inject into the
    /// request line — refused before any path was built (path-containment
    /// guard; a malicious manifest self-signs its hashes, so SHA256 can't
    /// catch this).
    UnsafeAssetName(String),
    ChecksumMismatch { asset: String, expected: String, actual: String },
    Cache(String),
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Fetch(e) => write!(f, "{e}"),
            Self::ManifestParse(e) => write!(f, "release manifest could not be parsed: {e}"),
            Self::NoAssetForTarget { binary, target, candidates } => write!(
                f,
                "no {binary} asset for target {target} in the release manifest (candidates: {})",
                candidates.join(", ")
            ),
            Self::ChecksumMismatch { asset, expected, actual } => write!(
                f,
                "checksum mismatch for '{asset}': manifest expects sha256 {expected}, \
                 actual sha256 {actual}; the asset is not usable and was not cached"
            ),
            Self::UnsafeAssetName(name) => write!(
                f,
                "release manifest asset name is not a plain file name and was refused: {name:?}"
            ),
            Self::Cache(e) => write!(f, "binary cache error: {e}"),
        }
    }
}
impl std::error::Error for ResolveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Fetch(e) => Some(e),
            _ => None,
        }
    }
}

/// Resolves the story's two binaries against a work dir and a release source.
/// All fields are injectable so every branch of the order is testable;
/// `work_dir` is the extension work dir (the cache lives under it).
/// No `Debug`: the fetcher is a trait object injected by the caller — whose
/// transport (and secrets) the manager must not assume anything about.
pub struct BinaryManager<'f> {
    pub work_dir: PathBuf,
    pub target: PlatformTarget,
    pub release_source: ReleaseSource,
    pub fetcher: &'f dyn Fetcher,
}

impl<'f> BinaryManager<'f> {
    /// Resolve `binary`. `settings_override` is the user-settings path for
    /// this binary (`Some` when the setting is set); when honored it is
    /// returned untouched and no request — not even the manifest — is made.
    pub fn resolve(
        &self,
        binary: BinaryId,
        settings_override: Option<&Path>,
    ) -> Result<ResolvedBinary, ResolveError> {
        if let Some(path) = settings_override {
            return Ok(ResolvedBinary {
                path: path.to_path_buf(),
                source: ResolvedSource::SettingsOverride,
                sha256: None,
                asset: None,
            });
        }

        let manifest = self.release_source.fetch_manifest(self.fetcher)?;
        let asset = select_asset(&manifest, binary, self.target)?;
        let cache_path = self.work_dir.join(CACHE_DIR).join(&asset.name);

        if cache_path.is_file() {
            let actual = sha256_of_file(&cache_path)
                .map_err(|e| ResolveError::Cache(format!("{}: {e}", cache_path.display())))?;
            if actual != asset.sha256 {
                // AC2: a stored entry is evidence, not a guarantee. It just
                // got re-hashed and lost — that is the hard error, not a
                // cue to silently re-download. Recovery (deleting the bad
                // entry) is an operator/upstream decision; handing out a
                // path over a mismatch is never one.
                return Err(ResolveError::ChecksumMismatch {
                    asset: asset.name.clone(),
                    expected: asset.sha256.clone(),
                    actual,
                });
            }
            eprintln!("[zatara] cache hit: {} (verified, no download)", asset.name);
            return Ok(ResolvedBinary {
                path: cache_path,
                source: ResolvedSource::VerifiedCache,
                sha256: Some(actual),
                asset: Some(asset.name.clone()),
            });
        }

        let url = self.release_source.asset_url(&asset.name);
        eprintln!("[zatara] downloading {} from {url}", asset.name);
        let bytes = self.fetcher.fetch(&url).map_err(ResolveError::Fetch)?;
        let actual = sha256_hex(&bytes);
        if actual != asset.sha256 {
            // Hard stop: nothing is cached, nothing is returned. The error
            // carries both hashes so the operator can tell a stale pin from
            // a tampered asset.
            return Err(ResolveError::ChecksumMismatch {
                asset: asset.name.clone(),
                expected: asset.sha256.clone(),
                actual,
            });
        }

        self.place_in_cache(&cache_path, &bytes)?;
        Ok(ResolvedBinary {
            path: cache_path,
            source: ResolvedSource::Downloaded,
            sha256: Some(actual),
            asset: Some(asset.name.clone()),
        })
    }

    /// Cache the verified bytes without a window where a half-written file
    /// could be read back as a cache hit.
    fn place_in_cache(&self, cache_path: &Path, bytes: &[u8]) -> Result<(), ResolveError> {
        let dir = cache_path
            .parent()
            .ok_or_else(|| ResolveError::Cache("cache path has no parent".into()))?;
        std::fs::create_dir_all(dir)
            .map_err(|e| ResolveError::Cache(format!("{}: {e}", dir.display())))?;
        let tmp = cache_path.with_extension("part");
        let result = (|| -> std::io::Result<()> {
            let mut file = std::fs::File::create(&tmp)?;
            file.write_all(bytes)?;
            file.flush()?;
            drop(file);
            // Windows rename refuses to clobber an existing target — and a
            // file is exactly where a failed-verification entry may sit.
            let _ = std::fs::remove_file(cache_path);
            std::fs::rename(&tmp, cache_path)
        })();
        result.map_err(|e| ResolveError::Cache(format!("{}: {e}", cache_path.display())))
    }

    /// Degraded-mode lookup (owner ruling 2026-10-13: an unreachable release
    /// is a warning, never a blocker): with the manifest unreachable there is
    /// no pin to select an entry name from, so a cached asset is found by the
    /// naming convention release.yml stages under — the binary's prefix plus
    /// the matrix-target tag. The bytes cannot be re-hashed (the pin *is* the
    /// missing manifest), so a hit is legitimate only alongside a surfaced
    /// warning (`RELEASE_WARNING_ENV`).
    ///
    /// When several cached versions match, the NEWEST wins by version order
    /// ([`pick_newest_cached`], D3) — never by file-name lexicography, which
    /// loses at exactly the rc.9 → rc.10 boundary a rolling dev cache crosses.
    pub fn cached_asset(&self, binary: BinaryId) -> Option<PathBuf> {
        let needle = format!("-{}", self.target.tag());
        let entries = std::fs::read_dir(self.work_dir.join(CACHE_DIR)).ok()?;
        let hits: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file()
                    && path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| {
                            name.starts_with(binary.prefix()) && name.contains(&needle)
                        })
            })
            .collect();
        pick_newest_cached(&hits, binary.prefix(), &needle)
    }
}

/// Version order over cache-entry names, newest first (D3, 2026-10-14 triage:
/// the shipped pick was `sort()` + `first()` — pure lexicography, so a cache
/// holding `rc.2` and `rc.9` answered with the OLDER entry, and the rc.9 →
/// rc.10 era was the live break). Semver law: numeric core segments compare
/// numerically, a release outranks its own pre-release (`0.1.0` >
/// `0.1.0-rc.9`), pre-release identifiers compare pairwise (numeric
/// numerically, numeric below alphanumeric). An unparseable version never
/// outranks a parseable one; among unparseables the old lexicographic-first
/// behavior stands. Fully deterministic: equal versions break ties on the
/// name, so the same directory always answers the same path.
fn pick_newest_cached(paths: &[PathBuf], prefix: &str, needle: &str) -> Option<PathBuf> {
    use std::cmp::Ordering;
    let mut keyed: Vec<(Option<VersionKey>, PathBuf)> = paths
        .iter()
        .filter_map(|path| {
            let name = path.file_name()?.to_str()?;
            // A crash mid-write can leave `place_in_cache`'s staging file in
            // the directory; it carries the winning name and losing bytes,
            // so it is never a cache answer.
            if name.ends_with(".part") {
                return None;
            }
            let after = name.strip_prefix(prefix)?;
            let at = after.rfind(needle)?;
            Some((version_key(&after[..at]), path.clone()))
        })
        .collect();
    keyed.sort_by(|a, b| {
        b.0.is_some()
            .cmp(&a.0.is_some())
            .then_with(|| match (&a.0, &b.0) {
                (Some(x), Some(y)) => y.cmp(x),
                _ => Ordering::Equal,
            })
            .then_with(|| a.1.cmp(&b.1))
    });
    keyed.into_iter().next().map(|(_, path)| path)
}

/// A parsed release/pre-release version segment of a cache-entry name,
/// e.g. `0.1.0-rc.9` → core [0,1,0], pre [rc, 9]. Anything unparseable
/// answers `None` and sorts below every `Some`.
#[derive(PartialEq, Eq, Debug)]
struct VersionKey {
    core: Vec<u64>,
    pre: Option<Vec<PreId>>,
}

#[derive(PartialEq, Eq, Debug, Clone, PartialOrd, Ord)]
enum PreId {
    /// Semver: numeric identifiers sort below alphanumeric ones.
    Num(u64),
    Alpha(String),
}

fn version_key(raw: &str) -> Option<VersionKey> {
    let (core_raw, pre_raw) = match raw.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (raw, None),
    };
    let core: Vec<u64> = core_raw
        .split('.')
        .map(str::parse::<u64>)
        .collect::<Result<_, _>>()
        .ok()?;
    if core.is_empty() {
        return None;
    }
    let pre = pre_raw.map(|pre| {
        pre.split('.')
            .map(|id| match id.parse::<u64>() {
                Ok(num) => PreId::Num(num),
                Err(_) => PreId::Alpha(id.to_ascii_lowercase()),
            })
            .collect()
    });
    Some(VersionKey { core, pre })
}

impl PartialOrd for VersionKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for VersionKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        for (a, b) in self.core.iter().zip(other.core.iter()) {
            match a.cmp(b) {
                Ordering::Equal => {}
                ord => return ord,
            }
        }
        match self.core.len().cmp(&other.core.len()) {
            Ordering::Equal => {}
            // `0.1` vs `0.1.0`: shorter core sorts below (never published;
            // determinism only).
            ord => return ord,
        }
        match (&self.pre, &other.pre) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (Some(a), Some(b)) => {
                for (x, y) in a.iter().zip(b.iter()) {
                    let ord = match (x, y) {
                        (PreId::Num(x), PreId::Num(y)) => x.cmp(y),
                        (PreId::Num(_), PreId::Alpha(_)) => Ordering::Less,
                        (PreId::Alpha(_), PreId::Num(_)) => Ordering::Greater,
                        (PreId::Alpha(x), PreId::Alpha(y)) => x.cmp(y),
                    };
                    if ord != Ordering::Equal {
                        return ord;
                    }
                }
                a.len().cmp(&b.len())
            }
        }
    }
}

/// sha256 of bytes in memory, lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    to_hex(&hasher.finalize())
}

/// Streaming sha256 so a large asset never has to sit fully in memory —
/// mirroring the release tooling's hashing (`build_release_manifest.py`).
///
/// The buffer is deliberately small. This function runs inside the
/// editor's wasm guest, whose main thread stack is small (wasi's, not the
/// native test harness's megabytes): a 1 MiB stack array — the previous
/// size — trapped the guest the first time a cache hit ever reached this
/// code (fresh machine, 2026-10-14: `memory fault ... out of bounds
/// memory access`, `sha256_of_file` at wasm frame 0). Old machines never
/// ran it: their releases were unreachable and the degraded path
/// (`cached_asset`) skips re-hashing, so the first real cache-hit
/// verification in the product's history was a live user's. 64 KiB is
/// irrelevant to throughput and far under any plausible guest stack.
pub fn sha256_of_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 1 << 16];
    loop {
        let read = file.read(&mut buf)?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(to_hex(&hasher.finalize()))
}

fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Plain-HTTP fetcher over std TCP — the transport the CI download-e2e job
/// uses against its loopback release server. Deliberately http-only: TLS and
/// GitHub API auth belong to the production wiring story (LOC-0114+), which
/// injects its own transport here; resolution never cares.
pub struct HttpGetFetcher;

impl Fetcher for HttpGetFetcher {
    fn fetch(&self, url: &str) -> Result<Vec<u8>, FetchError> {
        let transport = |detail: String| FetchError::Transport(format!("{url}: {detail}"));
        let rest = url
            .strip_prefix("http://")
            .ok_or_else(|| transport("only http:// is served here".into()))?;
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        let mut stream = std::net::TcpStream::connect(authority).map_err(|e| transport(e.to_string()))?;
        let request =
            format!("GET {path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).map_err(|e| transport(e.to_string()))?;
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).map_err(|e| transport(e.to_string()))?;

        let head_end = find_head_end(&raw).ok_or_else(|| transport("no header terminator".into()))?;
        let head = String::from_utf8_lossy(&raw[..head_end]);
        let status = head
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|code| code.parse::<u16>().ok())
            .ok_or_else(|| transport("unparseable status line".into()))?;
        if status != 200 {
            return Err(FetchError::Status { url: url.to_string(), code: status });
        }
        Ok(raw[head_end + 4..].to_vec())
    }
}

fn find_head_end(raw: &[u8]) -> Option<usize> {
    raw.windows(4).position(|w| w == b"\r\n\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cached(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(|n| PathBuf::from(n)).collect()
    }

    fn pick(names: &[&str]) -> Option<String> {
        pick_newest_cached(
            &cached(names),
            "zatara-proxy-",
            "-win-x64",
        )
        .map(|p| p.file_name().unwrap().to_str().unwrap().to_string())
    }

    /// The D3 law in the pair lexicography got WRONG for years of rc
    /// numbering: `rc.2` sorts before `rc.9` as text, so the old
    /// sort()+first() answered with the older binary.
    #[test]
    fn a_higher_rc_outranks_a_lower_one_whatever_the_text_says() {
        assert_eq!(
            pick(&[
                "zatara-proxy-0.1.0-rc.2-win-x64.exe",
                "zatara-proxy-0.1.0-rc.9-win-x64.exe",
            ]),
            Some("zatara-proxy-0.1.0-rc.9-win-x64.exe".into()),
            "lexicographic-first would answer rc.2"
        );
        assert_eq!(
            pick(&[
                "zatara-proxy-0.1.0-rc.10-win-x64.exe",
                "zatara-proxy-0.1.0-rc.11-win-x64.exe",
            ]),
            Some("zatara-proxy-0.1.0-rc.11-win-x64.exe".into()),
            "the live break: at the double-digit boundary text order inverts"
        );
    }

    /// Semver law on top of the ledger's case: the release outranks its own
    /// pre-releases, and pre-release cores still decide first.
    #[test]
    fn a_release_outranks_its_own_pre_releases() {
        assert_eq!(
            pick(&[
                "zatara-proxy-0.1.0-rc.9-win-x64.exe",
                "zatara-proxy-0.1.0-win-x64.exe",
            ]),
            Some("zatara-proxy-0.1.0-win-x64.exe".into())
        );
        assert_eq!(
            pick(&[
                "zatara-proxy-0.1.0-win-x64.exe",
                "zatara-proxy-0.2.0-rc.1-win-x64.exe",
            ]),
            Some("zatara-proxy-0.2.0-rc.1-win-x64.exe".into()),
            "a newer core beats an older release"
        );
    }

    /// An unparseable name never beats a versioned entry — but an all-garbage
    /// cache still answers (the old lexicographic behavior), because the
    /// degraded path must not stop answering where it answered before.
    #[test]
    fn unparseable_versions_sink_below_parseable_ones_but_still_answer_alone() {
        assert_eq!(
            pick(&[
                "zatara-proxy-aaa-win-x64.exe",
                "zatara-proxy-0.1.0-rc.3-win-x64.exe",
            ]),
            Some("zatara-proxy-0.1.0-rc.3-win-x64.exe".into())
        );
        assert_eq!(
            pick(&[
                "zatara-proxy-zzz-win-x64.exe",
                "zatara-proxy-aaa-win-x64.exe",
            ]),
            Some("zatara-proxy-aaa-win-x64.exe".into()),
            "all unparseable: the old lexicographic-first law stands"
        );
    }

    /// The pick stays a function of the directory contents alone.
    #[test]
    fn equal_versions_tie_break_on_the_name_not_on_read_order() {
        let a = "zatara-proxy-0.1.0-win-x64.exe";
        let b = "zatara-proxy-0.1.0-win-x64.exe.bak";
        assert_eq!(pick(&[b, a]), pick(&[a, b]));
        assert_eq!(pick(&[b, a]), Some(a.into()));
    }

    /// `place_in_cache` stages through `<name>.part`; a crash can leave it
    /// behind. It wears the winning entry name and holds half the bytes —
    /// the version law must never hand it to a session.
    #[test]
    fn a_staged_part_file_never_wins() {
        assert_eq!(
            pick(&[
                "zatara-proxy-0.1.0-rc.9-win-x64.part",
                "zatara-proxy-0.1.0-rc.8-win-x64.exe",
            ]),
            Some("zatara-proxy-0.1.0-rc.8-win-x64.exe".into())
        );
        assert_eq!(
            pick(&["zatara-proxy-0.1.0-rc.9-win-x64.part"]),
            None,
            "a directory of only staged files answers no cache hit"
        );
    }

    #[test]
    fn the_netcoredbg_pin_shape_parses() {
        // `netcoredbg-3.1.39-win-x64.zip` — plain core, no pre-release.
        let k = version_key("3.1.39").expect("plain pin parses");
        assert_eq!(k.core, vec![3, 1, 39]);
        assert_eq!(k.pre, None);
        // Upstream's build-suffixed shape (`3.2.0-1092`) parses as a
        // pre-release: below a plain 3.2.0 by semver law, above 3.1.x by core.
        let suffixed = version_key("3.2.0-1092").expect("build-suffixed parses");
        assert!(suffixed < version_key("3.2.0").unwrap());
        assert!(suffixed > version_key("3.1.39").unwrap());
    }
}
