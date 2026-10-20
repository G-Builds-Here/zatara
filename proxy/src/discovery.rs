//! Proxy-side exe-project discovery (LOC-0125, blueprint STORY-004).
//!
//! Lives in the proxy because the filesystem lives here (DR-4); it is
//! called at launch time for a project-mode config that carries the
//! locator's projectPath hint without a program (the wiring is LOC-0129).
//! The fitness law — the locator never imports this module (DR-3) — is
//! structural: the extension crate does not depend on this crate at all,
//! so `crates/zatara/src/locator.rs` cannot name anything below.
//!
//! The solution file is the project-set contract (DR-6, owner directive):
//! .sln/.slnx are first-class, the recursive csproj scan is the fallback.
//! Discovery never guesses (owner law): every outcome names what it found
//! — a dangling solution reference errors with the project as the
//! solution wrote it, ambiguity carries the candidate csproj paths, and
//! NotFound carries the scanned root. Outcomes surface verbatim upstream
//! (LOC-0129); this module never rewords, never auto-picks.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The launch profile riding a resolved target: the profile name plus
/// the launch inputs from `Properties/launchSettings.json` — args from
/// `commandLineArgs` (whitespace-split; one arg per token), env from
/// `environmentVariables`. A project without the file carries `default`
/// with both empty — an absent launchSettings is never an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchProfile {
    pub name: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

/// One runnable project: its csproj, the built dll located next to it,
/// and the profile to launch it with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExeTarget {
    pub csproj: PathBuf,
    pub dll: PathBuf,
    pub profile: LaunchProfile,
}

/// What one resolve run says about the repo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// Exactly one Exe project — the launch target.
    Target(ExeTarget),
    /// Two or more Exe projects with nothing deciding between them —
    /// csproj candidates in sorted order, no dll chosen (LOC-0129 turns
    /// this into an error naming the candidates; the fix is a solution
    /// file or a pinned `program`, never a guess here).
    Multiple(Vec<PathBuf>),
    /// Scanned `root` and found nothing runnable — names the root that
    /// was scanned.
    NotFound { root: PathBuf },
}

/// The project-set contract is broken: discovery ran but the repo is not
/// shaped the way its own solution file claims. Surfaces verbatim
/// upstream; distinct from NotFound, which is a clean answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryError {
    /// The solution references a project absent from disk — `referenced`
    /// is the path exactly as the solution wrote it, so the user can
    /// grep it out of the file.
    MissingProject {
        solution: PathBuf,
        referenced: String,
    },
    /// More than one solution file at the root — picking either side's
    /// project set would be a guess, so both are named.
    MultipleSolutions(Vec<PathBuf>),
    /// The one Exe project is found but its built dll is not there —
    /// names both the project and the dll path expected. Building is
    /// preLaunchTask's job (DR-2), so this stays an error, not a build.
    DllMissing { csproj: PathBuf, dll: PathBuf },
    /// The one Exe project declares no TargetFramework, so the built
    /// dll's path cannot be named — a fabricated path in an error would
    /// be a lie, so the project itself is named instead.
    MissingTargetFramework { csproj: PathBuf },
    /// `launchSettings.json` exists but is not valid JSON — shrugging
    /// past it would launch with args and env silently dropped.
    InvalidLaunchSettings { path: PathBuf },
}

impl std::fmt::Display for DiscoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiscoveryError::MissingProject {
                solution,
                referenced,
            } => write!(
                f,
                "solution {} references project {referenced}, which does not exist",
                solution.display()
            ),
            DiscoveryError::MultipleSolutions(solutions) => write!(
                f,
                "root holds more than one solution file ({}); discovery cannot pick one",
                solutions
                    .iter()
                    .map(|s| s.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            DiscoveryError::DllMissing { csproj, dll } => write!(
                f,
                "project {} has no built dll at {}; run a build first",
                csproj.display(),
                dll.display()
            ),
            DiscoveryError::MissingTargetFramework { csproj } => write!(
                f,
                "project {} declares no TargetFramework; discovery cannot name its built dll",
                csproj.display()
            ),
            DiscoveryError::InvalidLaunchSettings { path } => {
                write!(f, "{} is not valid JSON", path.display())
            }
        }
    }
}

impl std::error::Error for DiscoveryError {}

/// Resolve `root` to the one runnable exe project, or say why there is
/// not exactly one: solution-first enumeration, executable filter
/// ([`target_type_is_exe`] — declared type, or the Web SDK's implicit
/// Exe), built-dll locate, launchSettings profile carried.
pub fn resolve(root: &Path) -> Result<Resolution, DiscoveryError> {
    let projects = match solution_at(root)? {
        Some(solution) => referenced_projects(&solution)?,
        None => scan_csproj(root),
    };
    let mut exes: Vec<PathBuf> = projects
        .into_iter()
        .filter(|csproj| target_type_is_exe(csproj))
        .collect();
    exes.sort();
    match exes.len() {
        1 => {
            let csproj = exes.into_iter().next().expect("one exe");
            let dll = locate_built_dll(&csproj)?;
            let profile = read_launch_profile(&csproj)?;
            Ok(Resolution::Target(ExeTarget {
                csproj,
                dll,
                profile,
            }))
        }
        0 => Ok(Resolution::NotFound {
            root: root.to_path_buf(),
        }),
        // Two or more: the answer is the candidate list, verbatim —
        // naming them is the whole output, choosing among them is not
        // discovery's call to make (owner law).
        _ => Ok(Resolution::Multiple(exes)),
    }
}

/// The single solution file at the root, if there is one. Two or more is
/// an ambiguity error — which project set wins is exactly the kind of
/// choice discovery never makes.
fn solution_at(root: &Path) -> Result<Option<PathBuf>, DiscoveryError> {
    let mut solutions = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            let matches = path.is_file()
                && matches!(
                    path.extension().and_then(|e| e.to_str()),
                    Some("sln") | Some("slnx")
                );
            if matches {
                solutions.push(path);
            }
        }
    }
    solutions.sort();
    match solutions.len() {
        0 => Ok(None),
        1 => Ok(solutions.into_iter().next()),
        _ => Err(DiscoveryError::MultipleSolutions(solutions)),
    }
}

/// The csproj paths a solution file references, joined to its directory,
/// in document order. Every reference is existence-checked: an entry
/// naming a project that is not on disk is a broken contract (AC1), not
/// an entry to quietly drop.
fn referenced_projects(solution: &Path) -> Result<Vec<PathBuf>, DiscoveryError> {
    let raw = std::fs::read_to_string(solution).map_err(|_| DiscoveryError::MissingProject {
        solution: solution.to_path_buf(),
        referenced: solution.display().to_string(),
    })?;
    let dir = solution.parent().expect("solution file has a parent");
    let referenced = match solution.extension().and_then(|e| e.to_str()) {
        Some("slnx") => slnx_project_paths(&raw),
        _ => sln_project_paths(&raw),
    };
    let mut projects = Vec::new();
    for text in referenced {
        // Forward and back slashes both appear in solution files; Path
        // normalizes separators, so the joined path is native either way.
        let joined = dir.join(text.replace('\\', "/"));
        if !joined.is_file() {
            return Err(DiscoveryError::MissingProject {
                solution: solution.to_path_buf(),
                referenced: text,
            });
        }
        projects.push(joined);
    }
    Ok(projects)
}

/// .slnx: `<Project Path="src/Api/Api.csproj" />` entries carry the
/// project set; the scan keeps only csproj targets (folders and other
/// nodes carry no runnable project).
fn slnx_project_paths(raw: &str) -> Vec<String> {
    let mut paths = Vec::new();
    let mut rest = raw;
    while let Some(tag) = rest.find("<Project ") {
        rest = &rest[tag + "<Project ".len()..];
        let Some(attr) = rest.find("Path=\"") else { break };
        let value_start = attr + "Path=\"".len();
        let Some(value_end) = rest[value_start..].find('"') else {
            break;
        };
        let value = &rest[value_start..value_start + value_end];
        if value.rsplit('.').next().is_some_and(|ext| ext.eq_ignore_ascii_case("csproj")) {
            paths.push(value.to_string());
        }
        rest = &rest[value_start + value_end..];
    }
    paths
}

/// .sln: `Project("{type-guid}") = "Name", "src\Api\Api.csproj", "{id}"`
/// lines carry the project set — the second quoted field is the path.
/// Solution-folder pseudo-entries carry an extensionless path and drop
/// out on the same csproj test that filters .slnx nodes.
fn sln_project_paths(raw: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for line in raw.lines() {
        let Some(after) = line.strip_prefix("Project(") else {
            continue;
        };
        // Past the header, the fields after `= ` are `"Name", "path",
        // "{id}"` — the quoted strings, in order; field 1 is the path.
        let Some(eq) = after.find("= ") else { continue };
        // `'\u{22}'` is the quote character — spelled as an escape so this
        // file's raw-quote count stays even: docs_setting_drift harvests
        // quoted env names by alternating on `"` across the whole source
        // tree, and one odd-parity file flips that harvest for everything
        // after it (the harvest that proves ZATARA_MACOS_NDBG_PROBE ships).
        let mut fields = Vec::new();
        let mut rest = &after[eq + 2..];
        while let Some(start) = rest.find('\u{22}') {
            rest = &rest[start + 1..];
            let Some(end) = rest.find('\u{22}') else { break };
            fields.push(&rest[..end]);
            rest = &rest[end + 1..];
        }
        let Some(&path) = fields.get(1) else { continue };
        if path.rsplit('.').next().is_some_and(|ext| ext.eq_ignore_ascii_case("csproj")) {
            paths.push(path.to_string());
        }
    }
    paths
}

/// Recursive csproj scan: every `*.csproj` under `root`, skipping build
/// output (`bin/`, `obj/`) and dot-directories, in sorted order so the
/// scan's answers are deterministic run to run.
fn scan_csproj(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    scan_csproj_into(root, &mut found);
    found.sort();
    found
}

fn scan_csproj_into(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // Symlinks are never followed: a linked directory tree could loop
        // the walk forever, and a linked csproj is not this repo's file
        // to answer for.
        if entry.file_type().is_ok_and(|ft| ft.is_symlink()) {
            continue;
        }
        if path.is_dir() {
            if name == "bin" || name == "obj" || name.starts_with('.') {
                continue;
            }
            scan_csproj_into(&path, found);
        } else if path.is_file()
            && path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("csproj"))
        {
            found.push(path);
        }
    }
}

/// The TargetType=Exe filter: an MSBuild `<TargetType>` element whose
/// value is `Exe`. Libraries, test projects, and content carry no such
/// element (or a different value) and never enter the candidate set.
/// The scan is hand-written — this repo carries no XML parser and needs
/// one property, not a DOM.
/// Whether a csproj names a runnable app, by MSBuild's own law
/// (live defect 2026-10-16, eCommerce_Project): an explicit type wins —
/// modern SDK-style projects spell it `<OutputType>` (the legacy
/// `<TargetType>` still parses), and `Exe`/`WinExe` run while
/// `Library`/`Module` do not. With no explicit type the SDK decides:
/// `Microsoft.NET.Sdk.Web` implies an Exe (every ASP.NET Core app is one,
/// declared or not) and every other SDK implies a library. The old check
/// — `TargetType == Exe`, declared-only — saw no runnable project in a
/// Web-SDK solution whose app project declared nothing, and the door
/// refused the exact project its owner expected to run.
fn target_type_is_exe(csproj: &Path) -> bool {
    let Ok(raw) = std::fs::read_to_string(csproj) else {
        return false;
    };
    // The last declaration wins, as in MSBuild's own property evaluation
    // (a later PropertyGroup redefines an earlier one). OutputType is the
    // modern spelling and checked first; TargetType is its legacy name.
    let declared = tag_values(&raw, "OutputType")
        .into_iter()
        .last()
        .or_else(|| tag_values(&raw, "TargetType").into_iter().last());
    match declared {
        Some(value) => {
            let value = value.to_ascii_lowercase();
            value == "exe" || value == "winexe"
        }
        None => sdk_is_web(&raw),
    }
}

/// The `Sdk="..."` attribute of the `Project` element: a Web SDK implies
/// the Exe the csproj never writes.
fn sdk_is_web(raw: &str) -> bool {
    let mut rest = raw;
    while let Some(start) = rest.find("Sdk=\"") {
        let value_start = start + "Sdk=\"".len();
        // Spelled with an escape so this file keeps even raw-quote parity
        // (docs_setting_drift harvests on raw quote bytes; see the note
        // beside the sln field walker above).
        let Some(end) = rest[value_start..].find('\u{22}') else {
            break;
        };
        if rest[value_start..value_start + end].ends_with(".Web") {
            return true;
        }
        rest = &rest[value_start + end..];
    }
    false
}

/// Every element body for `tag` in an MSBuild file, in document order —
/// a property may appear in more than one PropertyGroup and every
/// occurrence counts.
fn tag_values(raw: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut values = Vec::new();
    let mut rest = raw;
    while let Some(start) = rest.find(&open) {
        let value_start = start + open.len();
        let Some(end) = rest[value_start..].find(&close) else {
            break;
        };
        values.push(rest[value_start..value_start + end].trim().to_string());
        rest = &rest[value_start + end..];
    }
    values
}

/// The dll a built project drops: `bin/Debug/<TargetFramework>/` under
/// the project directory, named by `<AssemblyName>` or the csproj stem.
/// A missing dll is an error naming the expected path — discovery does
/// not build (DR-2) and does not pretend otherwise.
fn locate_built_dll(csproj: &Path) -> Result<PathBuf, DiscoveryError> {
    let raw = std::fs::read_to_string(csproj).unwrap_or_default();
    let dir = csproj.parent().expect("csproj file has a parent");
    let stem = csproj.file_stem().expect("csproj file has a stem");
    let assembly = tag_values(&raw, "AssemblyName")
        .into_iter()
        .next()
        .unwrap_or_else(|| stem.to_string_lossy().into_owned());
    let dll = match tag_values(&raw, "TargetFramework").into_iter().next() {
        Some(tfm) => dir.join("bin").join("Debug").join(tfm).join(format!("{assembly}.dll")),
        None => {
            return Err(DiscoveryError::MissingTargetFramework {
                csproj: csproj.to_path_buf(),
            })
        }
    };
    if dll.is_file() {
        Ok(dll)
    } else {
        Err(DiscoveryError::DllMissing {
            csproj: csproj.to_path_buf(),
            dll,
        })
    }
}

/// The launch profile of one project. No `Properties/launchSettings.json`
/// is not an error: the `default` profile carries empty args and empty
/// env, and the project launches on its own settings.
fn read_launch_profile(csproj: &Path) -> Result<LaunchProfile, DiscoveryError> {
    let dir = csproj.parent().expect("csproj file has a parent");
    let path = dir.join("Properties").join("launchSettings.json");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Ok(default_profile());
    };
    // Visual Studio saves launchSettings.json with a leading UTF-8 BOM
    // (EF BB BF). The file is valid JSON to every .NET tool, but a strict
    // parser reads the BOM as leading garbage - strip it; the parse that
    // follows stays strict.
    let raw = raw.strip_prefix('\u{feff}').unwrap_or(&raw);
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|_| DiscoveryError::InvalidLaunchSettings { path })?;
    let Some(profiles) = value.get("profiles").and_then(|p| p.as_object()) else {
        return Ok(default_profile());
    };
    // Profile selection follows `dotnet run`: with no profile named, the
    // first profile in the file wins — file order, not name order.
    let selected = profiles
        .keys()
        .find(|name| Some(name.as_str()) == first_profile_name(raw).as_deref())
        .or_else(|| profiles.keys().next())
        .map(String::clone);
    let Some(name) = selected else {
        return Ok(default_profile());
    };
    let profile = &profiles[&name];
    let args = profile
        .get("commandLineArgs")
        .and_then(|v| v.as_str())
        .map(|line| line.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default();
    let env = profile
        .get("environmentVariables")
        .and_then(|v| v.as_object())
        .map(|map| {
            map.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default();
    Ok(LaunchProfile { name, args, env })
}

/// The empty fallback profile: named `default`, nothing to add to argv,
/// nothing to add to env.
fn default_profile() -> LaunchProfile {
    LaunchProfile {
        name: "default".to_string(),
        args: Vec::new(),
        env: BTreeMap::new(),
    }
}

/// The first profile name in the file's own key order. `serde_json`'s
/// map is name-sorted, so the file order is read off the raw text: the
/// first quoted string inside the `profiles` object is its first key —
/// JSON objects always put the key before the value. A file the scan
/// cannot grade falls back to the sorted first key.
fn first_profile_name(raw: &str) -> Option<String> {
    let after = raw.find("\"profiles\"")?;
    let after = raw[after..].find('{')? + after + 1;
    let start = raw[after..].find('"')? + after;
    let end = raw[start + 1..].find('"')? + start + 1;
    Some(raw[start + 1..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_slnx_scan_takes_project_entries_and_skips_the_rest() {
        let raw = "<Solution>\n  <Project Path=\"src/Api/Api.csproj\" />\n  \
                   <Folder Name=\"/tools/\" />\n  <Project Path=\"src/Lib/Lib.csproj\" />\n</Solution>";
        assert_eq!(
            slnx_project_paths(raw),
            vec!["src/Api/Api.csproj", "src/Lib/Lib.csproj"]
        );
    }

    #[test]
    fn the_sln_scan_reads_the_second_quoted_field_as_the_project_path() {
        let raw = "Project(\"{9A19103F-16F7-4668-BE54-9A1E7A4F7556}\") = \
                   \"Api\", \"src\\Api\\Api.csproj\", \"{6C9A1D2E-0000-0000-0000-000000000000}\"\n\
                   EndProject\n\
                   Global\nEndGlobal\n";
        assert_eq!(sln_project_paths(raw), vec!["src\\Api\\Api.csproj"]);
    }

    #[test]
    fn the_sln_scan_survives_a_comma_in_the_display_name() {
        let raw = "Project(\"{9A19103F-16F7-4668-BE54-9A1E7A4F7556}\") = \
                   \"My, App\", \"src\\App\\App.csproj\", \"{id}\"\nEndProject\n";
        assert_eq!(sln_project_paths(raw), vec!["src\\App\\App.csproj"]);
    }

    #[test]
    fn the_sln_scan_keeps_only_csproj_entries() {
        // Solution-folder pseudo-entries carry an extensionless path.
        let raw = "Project(\"{2150E333-8FDC-42A3-9474-1A3956D46DE8}\") = \
                   \"Solution Items\", \"Solution Items\", \"{id}\"\nEndProject\n";
        assert!(sln_project_paths(raw).is_empty());
    }

    #[test]
    fn a_property_repeats_across_property_groups_and_every_value_counts() {
        let raw = "<Project><PropertyGroup><TargetFramework>net9.0</TargetFramework>\
                   </PropertyGroup><PropertyGroup><TargetType>Library</TargetType>\
                   </PropertyGroup><PropertyGroup><TargetType>Exe</TargetType>\
                   </PropertyGroup></Project>";
        assert_eq!(tag_values(raw, "TargetType"), ["Library", "Exe"]);
        assert_eq!(tag_values(raw, "TargetFramework"), ["net9.0"]);
    }

    #[test]
    fn the_first_profile_name_follows_file_order_not_name_order() {
        // "https" sorts before names that come earlier in the file —
        // `dotnet run` picks by file order, and so does the scan.
        let raw = r#"{ "profiles": { "zzz-first": { "commandName": "Project" },
                            "aaa-second": {} } }"#;
        assert_eq!(first_profile_name(raw).as_deref(), Some("zzz-first"));
    }

    #[test]
    fn a_file_without_a_profiles_object_has_no_first_profile() {
        assert_eq!(first_profile_name(r#"{ "iisSettings": {} }"#), None);
    }
}
