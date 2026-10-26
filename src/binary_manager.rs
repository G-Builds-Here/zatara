use std::fmt;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use serde::Deserialize;
use sha2::{Digest, Sha256};
pub const CACHE_DIR: &str = "binary-cache";
pub const MANIFEST_ASSET: &str = "manifest.json";
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ManifestAsset {
    pub name: String,
    pub version: String,
    pub sha256: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Manifest {
    pub assets: Vec<ManifestAsset>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryId {
    ZataraProxy,
    NetcoreDbg,
    ZataraRunCase,
}
impl BinaryId {
    fn prefix(self) -> &'static str {
        match self {
            Self::ZataraProxy => "zatara-proxy-",
            Self::NetcoreDbg => "netcoredbg-",
            Self::ZataraRunCase => "zatara-run-case-",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::ZataraProxy => "zatara-proxy",
            Self::NetcoreDbg => "netcoredbg",
            Self::ZataraRunCase => "zatara-run-case",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformTarget {
    WinX64,
    LinuxX64,
    LinuxArm64,
    MacosX64,
    MacosArm64,
}
impl PlatformTarget {
    pub fn tag(self) -> &'static str {
        match self {
            Self::WinX64 => "win-x64",
            Self::LinuxX64 => "linux-x64",
            Self::LinuxArm64 => "linux-arm64",
            Self::MacosX64 => "macos-x64",
            Self::MacosArm64 => "macos-arm64",
        }
    }
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
            Self::LinuxX64
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedSource {
    SettingsOverride,
    VerifiedCache,
    Downloaded,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedBinary {
    pub path: PathBuf,
    pub source: ResolvedSource,
    pub sha256: Option<String>,
    pub asset: Option<String>,
}
pub trait Fetcher {
    fn fetch(&self, url: &str) -> Result<Vec<u8>, FetchError>;
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    Transport(String),
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
pub trait ExecMarker {
    fn mark_executable(&self, path: &Path) -> Result<(), String>;
}
pub struct BinaryManager<'f> {
    pub work_dir: PathBuf,
    pub target: PlatformTarget,
    pub release_source: ReleaseSource,
    pub fetcher: &'f dyn Fetcher,
    pub exec_marker: &'f dyn ExecMarker,
}
impl<'f> BinaryManager<'f> {
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
                return Err(ResolveError::ChecksumMismatch {
                    asset: asset.name.clone(),
                    expected: asset.sha256.clone(),
                    actual,
                });
            }
            self.mark_executable(&cache_path);
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
            return Err(ResolveError::ChecksumMismatch {
                asset: asset.name.clone(),
                expected: asset.sha256.clone(),
                actual,
            });
        }
        self.place_in_cache(&cache_path, &bytes)?;
        self.mark_executable(&cache_path);
        Ok(ResolvedBinary {
            path: cache_path,
            source: ResolvedSource::Downloaded,
            sha256: Some(actual),
            asset: Some(asset.name.clone()),
        })
    }
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
            let _ = std::fs::remove_file(cache_path);
            std::fs::rename(&tmp, cache_path)
        })();
        result.map_err(|e| ResolveError::Cache(format!("{}: {e}", cache_path.display())))
    }
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
        let winner = pick_newest_cached(&hits, binary.prefix(), &needle)?;
        self.mark_executable(&winner);
        Some(winner)
    }
    fn mark_executable(&self, path: &Path) {
        if let Err(e) = self.exec_marker.mark_executable(path) {
            eprintln!(
                "[zatara] could not mark {} executable ({e}) — if the spawn \
                 reports Permission denied, run: chmod +x \"{}\"",
                path.display(),
                path.display()
            );
        }
    }
}
fn pick_newest_cached(paths: &[PathBuf], prefix: &str, needle: &str) -> Option<PathBuf> {
    use std::cmp::Ordering;
    let mut keyed: Vec<(Option<VersionKey>, PathBuf)> = paths
        .iter()
        .filter_map(|path| {
            let name = path.file_name()?.to_str()?;
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
#[derive(PartialEq, Eq, Debug)]
struct VersionKey {
    core: Vec<u64>,
    pre: Option<Vec<PreId>>,
}
#[derive(PartialEq, Eq, Debug, Clone, PartialOrd, Ord)]
enum PreId {
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
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    to_hex(&hasher.finalize())
}
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
    #[test]
    fn equal_versions_tie_break_on_the_name_not_on_read_order() {
        let a = "zatara-proxy-0.1.0-win-x64.exe";
        let b = "zatara-proxy-0.1.0-win-x64.exe.bak";
        assert_eq!(pick(&[b, a]), pick(&[a, b]));
        assert_eq!(pick(&[b, a]), Some(a.into()));
    }
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
        let k = version_key("3.1.39").expect("plain pin parses");
        assert_eq!(k.core, vec![3, 1, 39]);
        assert_eq!(k.pre, None);
        let suffixed = version_key("3.2.0-1092").expect("build-suffixed parses");
        assert!(suffixed < version_key("3.2.0").unwrap());
        assert!(suffixed > version_key("3.1.39").unwrap());
    }
}
