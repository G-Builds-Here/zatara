use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
const MAX_SDK_QUERIES: usize = 8;
const SDK_QUERY_BUDGET: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(20);
pub(crate) trait CandidateFacts {
    fn declares_test_host(&self, csproj: &Path) -> bool;
    fn compiles_file(&self, candidates: &[PathBuf], file: &Path) -> Vec<Option<bool>>;
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Tie {
    DeclaresTestHost(PathBuf),
    CompilesClickedFile(PathBuf),
    NamedLikeTests(PathBuf),
}
impl Tie {
    pub(crate) fn project(&self) -> &Path {
        match self {
            Tie::DeclaresTestHost(path) | Tie::CompilesClickedFile(path) | Tie::NamedLikeTests(path) => path,
        }
    }
    pub(crate) fn reason(&self) -> &'static str {
        match self {
            Tie::DeclaresTestHost(_) => "the only project here that declares a test host",
            Tie::CompilesClickedFile(_) => "the only project here that compiles the clicked file",
            Tie::NamedLikeTests(_) => "fuzzy match: the only name here that says test",
        }
    }
}
pub(crate) fn break_tie(
    candidates: &[PathBuf],
    clicked: Option<&Path>,
    fuzzy: bool,
    facts: &dyn CandidateFacts,
) -> Option<Tie> {
    if candidates.len() < 2 {
        return None;
    }
    let declared: Vec<PathBuf> = candidates
        .iter()
        .filter(|csproj| facts.declares_test_host(csproj))
        .cloned()
        .collect();
    if declared.len() == 1 {
        return Some(Tie::DeclaresTestHost(declared.into_iter().next().expect("one")));
    }
    if let Some(file) = clicked {
        let pool: &[PathBuf] = if declared.is_empty() { candidates } else { &declared };
        if pool.len() <= MAX_SDK_QUERIES {
            let owners: Vec<PathBuf> = pool
                .iter()
                .zip(facts.compiles_file(pool, file))
                .filter(|(_, owns)| *owns == Some(true))
                .map(|(csproj, _)| csproj.clone())
                .collect();
            if owners.len() == 1 {
                return Some(Tie::CompilesClickedFile(owners.into_iter().next().expect("one")));
            }
        }
    }
    if fuzzy {
        let named: Vec<PathBuf> = candidates
            .iter()
            .filter(|csproj| {
                csproj.file_stem()
                    .is_some_and(|stem| stem.to_string_lossy().to_ascii_lowercase().contains("test"))
            })
            .cloned()
            .collect();
        if named.len() == 1 {
            return Some(Tie::NamedLikeTests(named.into_iter().next().expect("one")));
        }
    }
    None
}
pub(crate) struct SdkFacts;
impl CandidateFacts for SdkFacts {
    fn declares_test_host(&self, csproj: &Path) -> bool {
        crate::discovery::test_host_declared(csproj)
    }
    fn compiles_file(&self, candidates: &[PathBuf], file: &Path) -> Vec<Option<bool>> {
        let answers = ask_sdk(candidates);
        answers
            .into_iter()
            .map(|answer| answer.map(|paths| owns(&paths, file)))
            .collect()
    }
}
fn ask_sdk(candidates: &[PathBuf]) -> Vec<Option<Vec<PathBuf>>> {
    let mut running: Vec<(Child, JoinHandle<String>)> = Vec::new();
    for csproj in candidates {
        let spawned = Command::new("dotnet")
            .arg("msbuild")
            .arg(csproj)
            .arg("-getItem:Compile")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .stdout(Stdio::piped())
            .spawn();
        let Ok(mut child) = spawned else {
            running.clear();
            break;
        };
        let Some(mut stdout) = child.stdout.take() else {
            running.push((child, std::thread::spawn(String::new)));
            continue;
        };
        let reader = std::thread::spawn(move || {
            let mut buf = String::new();
            let _ = stdout.read_to_string(&mut buf);
            buf
        });
        running.push((child, reader));
    }
    let deadline = Instant::now() + SDK_QUERY_BUDGET;
    loop {
        if running.iter().all(|(_, reader)| reader.is_finished()) {
            break;
        }
        if Instant::now() > deadline {
            for (child, _) in running.iter_mut() {
                let _ = child.kill();
                let _ = child.wait();
            }
            break;
        }
        std::thread::sleep(POLL);
    }
    running
        .into_iter()
        .map(|(_, reader)| {
            let raw = reader.join().ok()?;
            compile_paths(&raw)
        })
        .collect()
}
fn compile_paths(raw: &str) -> Option<Vec<PathBuf>> {
    let json = &raw[raw.find('{')?..];
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let items = value.get("Items")?.get("Compile")?.as_array()?;
    let paths: Vec<PathBuf> = items
        .iter()
        .filter_map(|item| item.get("FullPath").and_then(|v| v.as_str()))
        .map(PathBuf::from)
        .collect();
    if !items.is_empty() && paths.is_empty() {
        return None;
    }
    Some(paths)
}
fn owns(compiled: &[PathBuf], file: &Path) -> bool {
    let Some(file) = canonical(file) else {
        return false;
    };
    compiled.iter().filter_map(|p| canonical(p)).any(|p| p == file)
}
fn canonical(path: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(path).ok()
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    struct Scripted {
        hosts: Vec<String>,
        owners: BTreeMap<String, Vec<String>>,
        silent: Vec<String>,
    }
    impl Scripted {
        fn new(hosts: &[&str], owners: &[(&str, &[&str])]) -> Self {
            Self::with_silent(hosts, owners, &[])
        }
        fn with_silent(hosts: &[&str], owners: &[(&str, &[&str])], silent: &[&str]) -> Self {
            Self {
                hosts: hosts.iter().map(|s| s.to_string()).collect(),
                owners: owners
                    .iter()
                    .map(|(file, owners)| {
                        ((*file).to_string(), owners.iter().map(|s| s.to_string()).collect())
                    })
                    .collect(),
                silent: silent.iter().map(|s| s.to_string()).collect(),
            }
        }
    }
    impl CandidateFacts for Scripted {
        fn declares_test_host(&self, csproj: &Path) -> bool {
            let name = csproj.file_name().unwrap().to_string_lossy().into_owned();
            self.hosts.iter().any(|host| *host == name)
        }
        fn compiles_file(&self, candidates: &[PathBuf], file: &Path) -> Vec<Option<bool>> {
            let key = file.file_name().unwrap().to_string_lossy().into_owned();
            if self.silent.iter().any(|quiet| *quiet == key) {
                return vec![None; candidates.len()];
            }
            let owners = self.owners.get(&key).cloned().unwrap_or_default();
            candidates
                .iter()
                .map(|csproj| {
                    let name = csproj.file_name().unwrap().to_string_lossy().into_owned();
                    owners.contains(&name).then_some(true)
                })
                .collect()
        }
    }
    fn cs(name: &str) -> PathBuf {
        PathBuf::from("C:/repo").join(name)
    }
    fn clicked(name: &str) -> PathBuf {
        PathBuf::from("C:/repo").join(name)
    }
    #[test]
    fn a_single_candidate_is_not_a_tie() {
        let facts = Scripted::new(&["A.csproj"], &[]);
        assert_eq!(break_tie(&[cs("A.csproj")], None, true, &facts), None);
        assert_eq!(break_tie(&[], None, true, &facts), None);
    }
    #[test]
    fn the_declared_test_host_wins_without_consulting_the_sdk() {
        let facts = Scripted::new(&["Catalog.Tests.csproj"], &[]);
        let tie = break_tie(
            &[cs("Catalog.csproj"), cs("Catalog.Tests.csproj")],
            None,
            false,
            &facts,
        );
        assert_eq!(
            tie,
            Some(Tie::DeclaresTestHost(cs("Catalog.Tests.csproj"))),
            "one declaration is the fact; the other project hosts no tests"
        );
    }
    #[test]
    fn two_declared_hosts_fall_through_to_the_file_fact() {
        let facts = Scripted::new(
            &["Unit.csproj", "Integration.csproj"],
            &[("MathTests.cs", &["Unit.csproj"][..])],
        );
        let tie = break_tie(
            &[cs("Unit.csproj"), cs("Integration.csproj")],
            Some(&clicked("MathTests.cs")),
            false,
            &facts,
        );
        assert_eq!(tie, Some(Tie::CompilesClickedFile(cs("Unit.csproj"))));
    }
    #[test]
    fn a_file_owned_by_both_projects_or_by_neither_is_a_refusal() {
        let both = Scripted::new(
            &["Unit.csproj", "Integration.csproj"],
            &[("MathTests.cs", &["Unit.csproj", "Integration.csproj"][..])],
        );
        assert_eq!(
            break_tie(
                &[cs("Unit.csproj"), cs("Integration.csproj")],
                Some(&clicked("MathTests.cs")),
                false,
                &both
            ),
            None,
            "two SDKs both claiming the file is a genuine tie — glob-overlapping \
             projects are exactly what the -getItem query cannot separate"
        );
        let neither = Scripted::new(&["Unit.csproj", "Integration.csproj"], &[("Elsewhere.cs", &[][..])]);
        assert_eq!(
            break_tie(
                &[cs("Unit.csproj"), cs("Integration.csproj")],
                Some(&clicked("Elsewhere.cs")),
                false,
                &neither
            ),
            None
        );
    }
    #[test]
    fn an_unanswerable_sdk_query_decides_nothing() {
        let silent =
            Scripted::with_silent(&[], &[("MathTests.cs", &["Unit.csproj"][..])], &["MathTests.cs"]);
        assert_eq!(
            break_tie(
                &[cs("Unit.csproj"), cs("Integration.csproj")],
                Some(&clicked("MathTests.cs")),
                false,
                &silent
            ),
            None,
            "no answer is not an answer"
        );
        let awake = Scripted::new(&[], &[("MathTests.cs", &["Unit.csproj"][..])]);
        assert_eq!(
            break_tie(
                &[cs("Unit.csproj"), cs("Integration.csproj")],
                Some(&clicked("MathTests.cs")),
                false,
                &awake
            ),
            Some(Tie::CompilesClickedFile(cs("Unit.csproj")))
        );
    }
    #[test]
    fn only_a_declared_host_competes_for_the_clicked_file() {
        let facts = Scripted::new(&["A.csproj", "B.csproj"], &[("MathTests.cs", &["Lib.csproj"][..])]);
        assert_eq!(
            break_tie(
                &[cs("Lib.csproj"), cs("A.csproj"), cs("B.csproj")],
                Some(&clicked("MathTests.cs")),
                false,
                &facts
            ),
            None,
            "the only owner is a library that hosts no tests: no answer, no \
             whole-suite run under the nearest test project"
        );
    }
    #[test]
    fn the_name_heuristic_needs_the_gate_and_a_unique_name() {
        let facts = Scripted::new(&[], &[]);
        let tied = [cs("Catalog.csproj"), cs("Catalog.Tests.csproj")];
        assert_eq!(break_tie(&tied, None, false, &facts), None, "default-off");
        assert_eq!(
            break_tie(&tied, None, true, &facts),
            Some(Tie::NamedLikeTests(cs("Catalog.Tests.csproj"))),
            "armed: one name says test"
        );
        let two_names = [cs("Unit.Tests.csproj"), cs("Integration.Tests.csproj"), cs("Catalog.csproj")];
        assert_eq!(
            break_tie(&two_names, None, true, &facts),
            None,
            "two names saying test is the tie the heuristic must not rank"
        );
    }
    #[test]
    fn a_folder_beyond_the_query_cap_is_not_interrogated() {
        let many: Vec<PathBuf> = (0..MAX_SDK_QUERIES + 2).map(|i| cs(&format!("P{i}.csproj"))).collect();
        let facts = Scripted::new(&[], &[]);
        assert_eq!(break_tie(&many, Some(&clicked("X.cs")), false, &facts), None);
    }
    #[test]
    fn the_parse_reads_the_measured_item_json() {
        const MEASURED: &str = r#"{
  "Items": {
    "Compile": [
      {
        "Identity": "Rows3.cs",
        "FullPath": "C:\\Users\\Merlin\\AppData\\Local\\Temp\\xv3case\\Rows3.cs",
        "RootDir": "C:\\",
        "Filename": "Rows3",
        "Extension": ".cs",
        "RelativeDir": ""
      }
    ]
  }
}"#;
        let paths = compile_paths(MEASURED).expect("the measured shape parses");
        assert_eq!(paths.len(), 1);
        assert_eq!(
            paths[0],
            PathBuf::from("C:\\Users\\Merlin\\AppData\\Local\\Temp\\xv3case\\Rows3.cs")
        );
    }
    #[test]
    fn an_unreadable_sdk_answer_is_unanswerable_not_false() {
        assert_eq!(compile_paths(""), None);
        assert_eq!(compile_paths("{ \"Items\": {} }"), None);
        assert_eq!(compile_paths("{ \"Items\": { \"Compile\": [{\"Identity\": \"a.cs\"}] } }"), None);
        assert_eq!(compile_paths("not json at all"), None);
        const LED_WITH_A_WARNING: &str =
            "MSBUILD : warning MSB4011: something\n{\"Items\":{\"Compile\":[{\"FullPath\":\"a.cs\"}]}}";
        assert!(compile_paths(LED_WITH_A_WARNING)
            .is_some_and(|paths| paths == vec![PathBuf::from("a.cs")]));
    }
    #[test]
    fn a_tie_carries_the_reason_its_pane_line_needs() {
        assert_eq!(
            Tie::DeclaresTestHost(cs("A.csproj")).reason(),
            "the only project here that declares a test host"
        );
        assert_eq!(Tie::CompilesClickedFile(cs("A.csproj")).project(), Path::new("C:/repo/A.csproj"));
    }
}
