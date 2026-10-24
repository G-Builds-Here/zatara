use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchProfile {
    pub name: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExeTarget {
    pub csproj: PathBuf,
    pub dll: PathBuf,
    pub profile: LaunchProfile,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    Target(ExeTarget),
    Multiple(Vec<PathBuf>),
    NotFound { root: PathBuf },
    TestOnly { root: PathBuf, tests: Vec<PathBuf> },
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryError {
    MissingProject {
        solution: PathBuf,
        referenced: String,
    },
    MultipleSolutions(Vec<PathBuf>),
    DllMissing { csproj: PathBuf, dll: PathBuf },
    MissingTargetFramework { csproj: PathBuf },
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
pub fn resolve(root: &Path) -> Result<Resolution, DiscoveryError> {
    let projects = match solution_at(root)? {
        Some(solution) => referenced_projects(&solution)?,
        None => scan_csproj(root),
    };
    let mut exes: Vec<PathBuf> = Vec::new();
    let mut tests: Vec<PathBuf> = Vec::new();
    for csproj in projects {
        if !target_type_is_exe(&csproj) {
            continue;
        }
        if test_project_shape(&csproj) {
            tests.push(csproj);
        } else {
            exes.push(csproj);
        }
    }
    exes.sort();
    tests.sort();
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
        0 if !tests.is_empty() => Ok(Resolution::TestOnly {
            root: root.to_path_buf(),
            tests,
        }),
        0 => Ok(Resolution::NotFound {
            root: root.to_path_buf(),
        }),
        _ => Ok(Resolution::Multiple(exes)),
    }
}
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
fn sln_project_paths(raw: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for line in raw.lines() {
        let Some(after) = line.strip_prefix("Project(") else {
            continue;
        };
        let Some(eq) = after.find("= ") else { continue };
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
fn target_type_is_exe(csproj: &Path) -> bool {
    let Ok(raw) = std::fs::read_to_string(csproj) else {
        return false;
    };
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
fn sdk_is_web(raw: &str) -> bool {
    let mut rest = raw;
    while let Some(start) = rest.find("Sdk=\"") {
        let value_start = start + "Sdk=\"".len();
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
pub(crate) fn test_project_shape(csproj: &Path) -> bool {
    let Ok(raw) = std::fs::read_to_string(csproj) else {
        return false;
    };
    test_shape_in(&raw)
}
fn test_shape_in(raw: &str) -> bool {
    if truthy_property(raw, "IsTestProject")
        || truthy_property(raw, "TestingPlatformDotnetTestSupport")
    {
        return true;
    }
    const TEST_PACKAGE_PREFIXES: [&str; 6] = [
        "xunit",
        "nunit",
        "mstest",
        "tunit",
        "microsoft.testing.platform",
        "microsoft.net.test.sdk",
    ];
    package_references(raw).iter().any(|name| {
        let name = name.to_ascii_lowercase();
        TEST_PACKAGE_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
    })
}
pub(crate) fn mtp_test_project(csproj: &Path) -> bool {
    if !target_type_is_exe(csproj) {
        return false;
    }
    let Ok(raw) = std::fs::read_to_string(csproj) else {
        return false;
    };
    mtp_shape_in(&raw)
}
fn mtp_shape_in(raw: &str) -> bool {
    if truthy_property(raw, "TestingPlatformDotnetTestSupport") {
        return true;
    }
    package_references(raw).iter().any(|name| {
        let name = name.to_ascii_lowercase();
        name.starts_with("xunit.v3") || name.starts_with("microsoft.testing.platform")
    })
}
fn truthy_property(raw: &str, tag: &str) -> bool {
    tag_values(raw, tag)
        .into_iter()
        .last()
        .is_some_and(|value| value.eq_ignore_ascii_case("true"))
}
fn package_references(raw: &str) -> Vec<String> {
    const OPEN: &str = "<PackageReference";
    const ATTR: &str = concat!("Include=", "\u{22}");
    let mut names = Vec::new();
    let mut rest = raw;
    while let Some(start) = rest.find(OPEN) {
        rest = &rest[start + OPEN.len()..];
        let body_end = rest.find('>').unwrap_or(rest.len());
        let body = &rest[..body_end];
        let Some(attr) = body.find(ATTR) else { continue };
        let value_start = attr + ATTR.len();
        let Some(end) = body[value_start..].find('\u{22}') else {
            continue;
        };
        names.push(body[value_start..value_start + end].to_string());
    }
    names
}
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
pub(crate) fn locate_built_dll(csproj: &Path) -> Result<PathBuf, DiscoveryError> {
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
fn read_launch_profile(csproj: &Path) -> Result<LaunchProfile, DiscoveryError> {
    let dir = csproj.parent().expect("csproj file has a parent");
    let path = dir.join("Properties").join("launchSettings.json");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Ok(default_profile());
    };
    let raw = raw.strip_prefix('\u{feff}').unwrap_or(&raw);
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|_| DiscoveryError::InvalidLaunchSettings { path })?;
    let Some(profiles) = value.get("profiles").and_then(|p| p.as_object()) else {
        return Ok(default_profile());
    };
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
fn default_profile() -> LaunchProfile {
    LaunchProfile {
        name: "default".to_string(),
        args: Vec::new(),
        env: BTreeMap::new(),
    }
}
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
        let raw = r#"{ "profiles": { "zzz-first": { "commandName": "Project" },
                            "aaa-second": {} } }"#;
        assert_eq!(first_profile_name(raw).as_deref(), Some("zzz-first"));
    }
    #[test]
    fn a_file_without_a_profiles_object_has_no_first_profile() {
        assert_eq!(first_profile_name(r#"{ "iisSettings": {} }"#), None);
    }
    #[test]
    fn a_v3_project_declaring_xunit_v3_and_the_platform_switch_is_test_shaped() {
        let raw = "<Project Sdk=\"Microsoft.NET.Sdk\"><PropertyGroup>\
                   <OutputType>Exe</OutputType><TargetFramework>net8.0</TargetFramework>\
                   <TestingPlatformDotnetTestSupport>true</TestingPlatformDotnetTestSupport>\
                   </PropertyGroup><ItemGroup>\
                   <PackageReference Include=\"xunit.v3\" Version=\"3.2.2\" />\
                   </ItemGroup></Project>";
        assert!(test_shape_in(raw));
        assert!(mtp_shape_in(raw), "the same file is the MTP lane's shape");
    }
    #[test]
    fn a_v2_test_project_is_test_shaped_but_never_the_mtp_lane() {
        let raw = "<Project Sdk=\"Microsoft.NET.Sdk\"><PropertyGroup>\
                   <TargetFramework>net8.0</TargetFramework></PropertyGroup><ItemGroup>\
                   <PackageReference Include=\"Microsoft.NET.Test.Sdk\" Version=\"17.8.0\" />\
                   <PackageReference Include=\"xunit\" Version=\"2.6.6\" />\
                   <PackageReference Include=\"xunit.runner.visualstudio\" Version=\"2.5.6\" />\
                   </ItemGroup></Project>";
        assert!(test_shape_in(raw));
        assert!(
            !mtp_shape_in(raw),
            "v2 packages alone must not route a launch to the MTP lane"
        );
    }
    #[test]
    fn a_test_project_property_act_is_decisive_without_any_package() {
        assert!(test_shape_in(
            "<Project><PropertyGroup><IsTestProject>true</IsTestProject></PropertyGroup></Project>"
        ));
        assert!(
            test_shape_in(
                "<Project><PropertyGroup><IsTestProject>True</IsTestProject></PropertyGroup></Project>"
            ),
            "truthiness is case-insensitive"
        );
        assert!(mtp_shape_in(
            "<Project><PropertyGroup><TestingPlatformDotnetTestSupport>true</TestingPlatformDotnetTestSupport></PropertyGroup></Project>"
        ));
    }
    #[test]
    fn a_plain_app_is_test_shaped_by_nothing() {
        let raw = "<Project Sdk=\"Microsoft.NET.Sdk.Web\"><PropertyGroup>\
                   <TargetFramework>net9.0</TargetFramework></PropertyGroup><ItemGroup>\
                   <PackageReference Include=\"Newtonsoft.Json\" Version=\"13.0.3\" />\
                   <ProjectReference Include=\"..\\Lib\\Lib.csproj\" />\
                   </ItemGroup></Project>";
        assert!(!test_shape_in(raw));
        assert!(!mtp_shape_in(raw));
    }
    #[test]
    fn the_package_walker_takes_package_includes_and_nothers() {
        let raw = "<ItemGroup>\
                   <PackageReference Include=\"NUnit\" Version=\"4.0.0\" />\
                   <ProjectReference Include=\"..\\xunit\\xunit.csproj\" />\
                   <Reference Include=\"Microsoft.Testing.Platform\" />\
                   <PackageReference Include='tunit'><Version>1.0</Version></PackageReference>\
                   </ItemGroup>";
        assert_eq!(package_references(raw), vec!["NUnit"]);
    }
    #[test]
    fn the_last_property_declaration_wins() {
        let raw = "<Project><PropertyGroup><IsTestProject>true</IsTestProject>\
                   </PropertyGroup><PropertyGroup><IsTestProject>false</IsTestProject>\
                   </PropertyGroup></Project>";
        assert!(!test_shape_in(raw));
    }
}
