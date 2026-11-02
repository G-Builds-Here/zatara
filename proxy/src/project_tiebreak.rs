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
    fn solution_members(&self, sln: &Path) -> Option<Vec<PathBuf>>;
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
const MAX_SOLUTION_MEMBER_READS: usize = 24;
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SolutionTie {
    HoldsTestProject(PathBuf),
    NamedLikeTests(PathBuf),
}
impl SolutionTie {
    pub(crate) fn project(&self) -> &Path {
        match self {
            SolutionTie::HoldsTestProject(path) | SolutionTie::NamedLikeTests(path) => path,
        }
    }
    pub(crate) fn reason(&self) -> &'static str {
        match self {
            SolutionTie::HoldsTestProject(_) => "the only solution here that contains a test project",
            SolutionTie::NamedLikeTests(_) => "fuzzy match: the only name here that says test",
        }
    }
}
pub(crate) fn break_solution_tie(
    solutions: &[PathBuf],
    fuzzy: bool,
    facts: &dyn CandidateFacts,
) -> Option<SolutionTie> {
    if solutions.len() < 2 {
        return None;
    }
    let mut holds_test: Vec<&PathBuf> = Vec::new();
    let mut members_read = 0usize;
    let mut complete = true;
    for sln in solutions {
        let Some(members) = facts.solution_members(sln) else {
            complete = false;
            break;
        };
        if members_read + members.len() > MAX_SOLUTION_MEMBER_READS {
            complete = false;
            break;
        }
        members_read += members.len();
        if members.iter().any(|m| facts.declares_test_host(m)) {
            holds_test.push(sln);
        }
    }
    if complete {
        if holds_test.len() == 1 {
            return Some(SolutionTie::HoldsTestProject(
                holds_test.into_iter().next().expect("one").clone(),
            ));
        }
    }
    if fuzzy {
        let named: Vec<&PathBuf> = solutions
            .iter()
            .filter(|sln| {
                sln.file_stem()
                    .is_some_and(|stem| stem.to_string_lossy().to_ascii_lowercase().contains("test"))
            })
            .collect();
        if named.len() == 1 {
            return Some(SolutionTie::NamedLikeTests(
                (*named.into_iter().next().expect("one")).clone(),
            ));
        }
    }
    None
}
pub(crate) fn solution_member_projects(raw: &str, base: &Path) -> Vec<PathBuf> {
    raw.lines()
        .filter_map(|line| {
            let after = line.strip_prefix("Project(")?;
            let (_type_guid, rest) = after.split_once("\")")?;
            let path = rest.split('"').nth(3)?;
            if !path.to_ascii_lowercase().ends_with(".csproj") {
                return None;
            }
            Some(base.join(path.replace('\\', std::path::MAIN_SEPARATOR_STR)))
        })
        .collect()
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
    fn solution_members(&self, sln: &Path) -> Option<Vec<PathBuf>> {
        let raw = std::fs::read_to_string(sln).ok()?;
        let base = sln.parent().unwrap_or(Path::new("."));
        Some(solution_member_projects(&raw, base))
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
        members: BTreeMap<String, Option<Vec<String>>>,
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
                members: BTreeMap::new(),
            }
        }
        fn with_members(mut self, members: &[(&str, Option<&[&str]>)]) -> Self {
            self.members = members
                .iter()
                .map(|(sln, list)| {
                    (
                        (*sln).to_string(),
                        list.map(|names| names.iter().map(|s| s.to_string()).collect()),
                    )
                })
                .collect();
            self
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
        fn solution_members(&self, sln: &Path) -> Option<Vec<PathBuf>> {
            let key = sln.file_name()?.to_string_lossy().into_owned();
            self.members
                .get(&key)?
                .as_ref()
                .map(|names| names.iter().map(|n| PathBuf::from("C:/repo").join(n)).collect())
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
    fn sl(name: &str) -> PathBuf {
        PathBuf::from("C:/repo").join(name)
    }
    #[test]
    fn the_solution_parse_reads_the_lines_a_real_sln_writes() {
        const REAL: &str = "\
Microsoft Visual Studio Solution File, Format Version 12.00
Project(\"{FAE04EC0-301F-11D3-BF4B-00C04F79EFBC}\") = \"App\", \"src\\App\\App.csproj\", \"{8E87D182-5AD9-4C4E-9D3E-0C5B9C0B4E01}\"
EndProject
Project(\"{9A19103F-16F7-4668-BE54-9A1E7A4F7556}\") = \"Tests\", \"tests\\App.Tests\\App.Tests.csproj\", \"{8E87D182-5AD9-4C4E-9D3E-0C5B9C0B4E02}\"
EndProject
Project(\"{2150E333-8F63-4F36-94BC-87A4C4706939}\") = \"Solution Items\", \"Solution Items\", \"{8E87D182-5AD9-4C4E-9D3E-0C5B9C0B4E03}\"
EndProject
Project(\"{D9595E5D-178B-4280-9606-1935E6BF6F26}\") = \"Shared\", \"shared\\Shared.projitems\", \"{8E87D182-5AD9-4C4E-9D3E-0C5B9C0B4E04}\")
EndProject
";
        let members = solution_member_projects(REAL, Path::new("C:/repo"));
        assert_eq!(
            members,
            vec![
                PathBuf::from("C:/repo").join("src").join("App").join("App.csproj"),
                PathBuf::from("C:/repo").join("tests").join("App.Tests").join("App.Tests.csproj"),
            ]
        );
        assert!(solution_member_projects("random text\n", Path::new("C:/repo")).is_empty());
    }
    #[test]
    fn the_only_solution_with_a_test_member_answers_unarmed() {
        let facts = Scripted::new(&["App.Tests.csproj"], &[]).with_members(&[
            ("App.sln", Some(&["App.csproj"])),
            ("App.Tests.sln", Some(&["App.Tests.csproj"])),
        ]);
        let tie = break_solution_tie(&[sl("App.sln"), sl("App.Tests.sln")], false, &facts);
        assert_eq!(tie, Some(SolutionTie::HoldsTestProject(sl("App.Tests.sln"))));
        assert!(tie.is_some_and(|t| t.reason().contains("contains a test project")));
    }
    #[test]
    fn two_solutions_with_test_members_refuse_strictly_and_name_only_when_armed() {
        let both = Scripted::new(&["One.Tests.csproj", "Two.Tests.csproj"], &[]).with_members(&[
            ("Alpha.Tests.sln", Some(&["One.Tests.csproj"])),
            ("Beta.Tests.sln", Some(&["Two.Tests.csproj"])),
        ]);
        assert_eq!(
            break_solution_tie(&[sl("Alpha.Tests.sln"), sl("Beta.Tests.sln")], false, &both),
            None,
            "two solutions holding tests are two survivors: dotnet's error stays"
        );
        assert_eq!(
            break_solution_tie(&[sl("Alpha.Tests.sln"), sl("Beta.Tests.sln")], true, &both),
            None,
            "armed but BOTH names say test: still not a fact, still a refusal"
        );
        let misleading_name = Scripted::new(&["Real.Tests.csproj"], &[]).with_members(&[
            ("App.sln", Some(&["Real.Tests.csproj"])),
            ("Everything.Tests.sln", Some(&["Plain.csproj"])),
        ]);
        assert_eq!(
            break_solution_tie(&[sl("App.sln"), sl("Everything.Tests.sln")], false, &misleading_name),
            Some(SolutionTie::HoldsTestProject(sl("App.sln"))),
            "the solution whose MEMBERS declare a test host wins over the one merely named for it"
        );
    }
    #[test]
    fn an_unreadable_solution_is_never_read_as_having_no_test_project() {
        let facts = Scripted::new(&["App.Tests.csproj"], &[]).with_members(&[
            ("App.sln", None),
            ("App.Tests.sln", Some(&["App.Tests.csproj"])),
        ]);
        assert_eq!(
            break_solution_tie(&[sl("App.sln"), sl("App.Tests.sln")], false, &facts),
            None
        );
        let blank = Scripted::new(&["App.Tests.csproj"], &[]);
        assert_eq!(
            break_solution_tie(&[sl("App.sln"), sl("App.Tests.sln")], false, &blank),
            None
        );
    }
    #[test]
    fn a_single_solution_is_not_a_solution_tie() {
        let facts = Scripted::new(&[], &[]);
        assert_eq!(break_solution_tie(&[sl("Only.sln")], true, &facts), None);
        assert_eq!(break_solution_tie(&[], true, &facts), None);
    }
    #[test]
    fn membership_beyond_the_read_cap_sinks_the_claim_not_the_gate() {
        let many: Vec<&str> = (0..MAX_SOLUTION_MEMBER_READS + 1)
            .map(|i| if i == 0 { "Host.csproj" } else { "Plain.csproj" })
            .collect();
        let facts = Scripted::new(&["Host.csproj"], &[]).with_members(&[
            ("Big.Tests.sln", Some(many.as_slice())),
            ("Other.sln", Some(&["Plain.csproj"])),
        ]);
        assert_eq!(
            break_solution_tie(&[sl("Big.Tests.sln"), sl("Other.sln")], false, &facts),
            None,
            "an unread tail means no ONLY-claim, even when the name is the tie the cap hid"
        );
        assert_eq!(
            break_solution_tie(&[sl("Big.Tests.sln"), sl("Other.sln")], true, &facts),
            Some(SolutionTie::NamedLikeTests(sl("Big.Tests.sln"))),
            "the cap sinks the FACT rung only; the gated name rung still answers when armed"
        );
    }
}
