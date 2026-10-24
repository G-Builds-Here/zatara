//! The programrunner arm (LOC-0124, epic LOC-0121 DR-5): project-mode
//! launches ride the relay, not the runner.
//!
//! The spawn shape is pinned by the LOC-0133 probe verdict (`dll-shape`):
//! the relay's existing adapter spawn — `netcoredbg --interpreter=vscode`,
//! already verified shape — plus the program travelling as the launch's
//! `program` argument, `args`/`env`/`cwd` alongside it. The relay forwards
//! the validated launch body untouched (the raw-forward rule it has always
//! obeyed), so the fields arrive exactly as given, and the debuggee's
//! stdout comes back as raw DAP `output` events through the same pump that
//! has always relayed events byte-for-byte. No build, no test step, no
//! vstest entry point on this path (DR-2) — the whole point of a separate
//! arm is that the test lane's orchestration never sees a program launch.
//!
//! The two pieces of work this arm owns before the handoff (LOC-0129
//! extends one with the other): a config that carries the locator's
//! projectPath hint with no program asks [`crate::discovery`] to resolve
//! the root, and the resolved dll rides the launch as its `program` —
//! discovery's other outcomes (ambiguity, an empty root, a broken
//! project-set contract) become the launch's failure, discovery's words
//! verbatim. And the program about to run must exist on disk: netcoredbg
//! reports a missing file late and generically; the proxy says it early
//! and specifically — the exact path — and ends the session without
//! leaving its adapter child behind. A config with a pinned `program`
//! consults neither: the escape hatch (DR-7) stays the direct path.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use serde_json::Value;
use zatara_types::{LaunchConfig, LaunchMode};

use crate::discovery::Resolution;
use crate::relay::{adapter_lost, Tunnel};
use std::sync::Arc;

/// The dispatch decision for a validated launch: the knob's answer when it
/// has one (`ZATARA_LAUNCH_MODE`, resolved at the relay's boundary via
/// `launch_mode_from_env` — only the exact `project`/`test` ever produce
/// `Some`, LOC-0122's no-coercion law), LOC-0122's inference otherwise.
/// The decision itself is pure and testable: knob-present always beats,
/// knob-absent always defers.
pub(crate) fn dispatch_mode(config: &LaunchConfig, knob: Option<LaunchMode>) -> LaunchMode {
    knob.unwrap_or_else(|| config.resolved_mode())
}

/// Where a program launch's `program` is looked for on disk: as given when
/// absolute (or when the config carried no cwd), resolved against the
/// config's cwd when relative. The same shape netcoredbg will launch from.
fn program_path(program: &str, cwd: &str) -> PathBuf {
    let path = std::path::Path::new(program);
    if path.is_absolute() || cwd.is_empty() {
        path.to_path_buf()
    } else {
        std::path::Path::new(cwd).join(program)
    }
}

/// Take the validated project-mode launch. The program to run is either
/// the config's own (a pinned path, forwarded raw — the DR-7 escape hatch
/// and LOC-0124's discipline, discovery never consulted) or discovery's
/// answer for the locator's hint-only config (LOC-0129: resolve once
/// against the hinted root, and the resolved dll rides the launch body as
/// its `program`). Everything the resolved program is not — an ambiguous
/// set, an empty root, a broken project-set contract — is answered where
/// it stands, discovery's words verbatim, the adapter child killed so the
/// pump's session-end sequence closes the session behind the answer (no
/// netcoredbg child survives a launch the arm refused). A program the disk
/// does not have — pinned or freshly resolved racing its own build away —
/// fails naming the exact path. Otherwise the launch goes to the adapter:
/// `--interpreter=vscode` spawn plus program/args/env/cwd on the launch
/// body is the probe-pinned shape, and the relay's event pump streams the
/// debuggee's stdout back verbatim.
pub(crate) fn start(tunnel: Arc<Tunnel>, config: &LaunchConfig, request: &Value, launch_seq: i64) {
    let (program, resolved_from_hint) = match config.program.as_deref() {
        Some(program) => (program.to_string(), false),
        None => {
            let Some(root) = config.hint_root() else {
                // Reachable only through the knob (the hint pair and any
                // program-bearing shape resolve otherwise): a forced
                // project launch with nothing to run and no hint to
                // resolve has no safe default — name what is missing.
                fail(
                    &tunnel,
                    launch_seq,
                    "project-mode launch without a `program` field — nothing to run",
                );
                return;
            };
            match crate::discovery::resolve(Path::new(root)) {
                Ok(Resolution::Target(target)) => (target.dll.display().to_string(), true),
                // Outcomes, verbatim (LOC-0125's surfaced-words law): the
                // candidates, the root, the broken contract — discovery's
                // answers become the client's answer, nothing between them.
                Ok(Resolution::Multiple(candidates)) => {
                    fail(&tunnel, launch_seq, &multiple_failure(&candidates));
                    return;
                }
                Ok(Resolution::NotFound { root }) => {
                    fail(&tunnel, launch_seq, &not_found_failure(&root));
                    return;
                }
                // The xunit v3 / MTP repo: every Exe under the root runs
                // tests, none is an app. The door says what it excluded
                // and points at the lane that launches those very
                // projects filtered (the runner's MTP debug lane), with
                // the pin escape kept — a named `program` still debugs
                // any of them directly.
                Ok(Resolution::TestOnly { root, tests }) => {
                    fail(&tunnel, launch_seq, &test_only_failure(&root, &tests));
                    return;
                }
                Err(error) => {
                    fail(&tunnel, launch_seq, &error.to_string());
                    return;
                }
            }
        }
    };
    if !program_path(&program, &config.cwd).is_file() {
        fail(
            &tunnel,
            launch_seq,
            &format!("program not found: {program}"),
        );
        return;
    }
    // The handoff takes the ending with it (LOC-0126): the session enters
    // the program law and the adapter is adopted as this session's tree
    // BEFORE the launch reaches it — so the debuggee netcoredbg spawns is
    // born inside the containment, and every session-end path the relay
    // owns takes the whole tree down with it.
    tunnel.begin_program_session();
    let outbound: Cow<Value> = if resolved_from_hint {
        Cow::Owned(launch_with_program(request, &program))
    } else {
        Cow::Borrowed(request)
    };
    if tunnel.forward_to_child(&outbound).is_err() {
        adapter_lost();
    }
}

/// The ambiguous-repo launch error: every candidate csproj named exactly
/// as discovery returned it (verbatim-surfacing law — the candidates are
/// the answer; choosing among them is nobody's launch-time call).
fn multiple_failure(candidates: &[PathBuf]) -> String {
    format!(
        "more than one runnable project ({}); pin one with a solution file at the root or a `program` in the launch config",
        candidates
            .iter()
            .map(|candidate| candidate.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// The nothing-found launch error, in user terms (owner ruling
/// 2026-10-16): the menu door clicked on a tests-only folder reads as a
/// crash — so the message names what program debug needs, offers the
/// gutter lanes as the alternative when that's what the folder is, and
/// keeps the escape hatch discovery's law leaves — pinning `program`
/// skips discovery entirely (DR-7). The scanned root stays named verbatim.
fn not_found_failure(root: &Path) -> String {
    format!(
        "zatara: program debug needs a runnable app (exe) project, and none was found under {}. If this folder only holds tests, use the gutter run/debug buttons instead; to run a specific dll anyway, add a .zed/debug.json scenario whose \"program\" key is its path, e.g. \"program\": \"C:/path/to/Your.dll\".",
        root.display()
    )
}

/// The tests-only-repo launch error (xunit v3 / MTP, 2026-10-20): the
/// door excluded the test-shaped Exes and nothing app-shaped remained —
/// so the message owns the exclusion (names every test project verbatim,
/// never pretends the scan found nothing), points at the gutter lanes
/// that launch these projects filtered, and keeps the pin escape the
/// exclusion law leaves (`program` skips discovery entirely, DR-7). Same
/// three obligations as [`not_found_failure`], different facts.
fn test_only_failure(root: &Path, tests: &[PathBuf]) -> String {
    format!(
        "zatara: program debug needs a runnable app (exe) project; under {} every runnable (exe) project is a test project ({}), so the door excluded all of them. Use the gutter run/debug buttons on a test in one of these projects instead; to debug one of them anyway, add a .zed/debug.json scenario whose \"program\" key is its built dll, e.g. \"program\": \"C:/path/to/YourTests.dll\".",
        root.display(),
        tests
            .iter()
            .map(|candidate| candidate.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// The launch body the adapter receives when discovery injected a program:
/// the client's request with `program` set to the resolved dll, every other
/// field and the envelope left exactly as the client sent them.
fn launch_with_program(request: &Value, program: &str) -> Value {
    let mut outbound = request.clone();
    if let Some(arguments) = outbound.get_mut("arguments").and_then(Value::as_object_mut) {
        arguments.insert("program".to_string(), Value::String(program.to_string()));
    }
    outbound
}

/// Fail the launch where it stands and end the session behind the answer:
/// the client gets the DAP error, the adapter child is killed so the pump's
/// session-end sequence closes the session — no netcoredbg child survives a
/// launch the arm refused.
fn fail(tunnel: &Tunnel, launch_seq: i64, message: &str) {
    tunnel.error_response(launch_seq, "launch", message);
    tunnel.kill_adapter();
}

#[cfg(test)]
mod hint_wiring_tests {
    use super::*;
    use serde_json::json;

    fn hint_only() -> LaunchConfig {
        LaunchConfig {
            project_path: "C:/work/app".into(),
            cwd: "C:/work/app".into(),
            ..Default::default()
        }
    }

    #[test]
    fn the_hint_only_shape_hands_discovery_its_root_and_everything_else_keeps_its_path() {
        // The rule itself has one authority — LaunchConfig::hint_root, the
        // predicate start() routes discovery on. Pinned from the consumer's
        // side: what the programrunner arm actually asks.
        assert_eq!(hint_only().hint_root(), Some("C:/work/app"));
        // Pinned program: the config speaks for itself (DR-7).
        assert_eq!(
            LaunchConfig {
                program: Some("App.dll".into()),
                project_path: "C:/work/app".into(),
                ..Default::default()
            }
            .hint_root(),
            None
        );
        // A test-target config forced Project by the knob is still the test
        // lane's shape — discovery never resolves a named test.
        assert_eq!(
            LaunchConfig {
                project_path: "C:/w/proj/CartModelTests.csproj".into(),
                fully_qualified_name: "Tests.One".into(),
                ..Default::default()
            }
            .hint_root(),
            None
        );
        // No hint, no root: the knob-forced program-less fail stands.
        assert_eq!(LaunchConfig::default().hint_root(), None);
    }

    #[test]
    fn the_ambiguity_error_names_every_candidate_exactly_as_discovery_returned_them() {
        let candidates = vec![
            PathBuf::from("C:/work/app\\src\\Api\\Api.csproj"),
            PathBuf::from("C:/work/app\\src\\Worker\\Worker.csproj"),
        ];
        let text = multiple_failure(&candidates);
        for candidate in &candidates {
            assert!(
                text.contains(&candidate.display().to_string()),
                "the error must name {candidate:?} verbatim; it said: {text}"
            );
        }
    }

    #[test]
    fn the_not_found_error_names_the_root_the_gutter_alternative_and_pinning_program() {
        let text = not_found_failure(Path::new("C:/work/app"));
        assert!(
            text.contains("C:/work/app"),
            "the error must name the scanned root; it said: {text}"
        );
        assert!(
            text.contains("gutter"),
            "a tests-only folder must be told its real lane, not left reading like a crash; it said: {text}"
        );
        assert!(
            text.contains("\"program\""),
            "the escape hatch must be shown as the quoted config key, not the bare word; it said: {text}"
        );
        assert!(
            text.contains(".zed/debug.json"),
            "the escape hatch must say WHERE the key lives; it said: {text}"
        );
    }

    /// The tests-only door answer (xunit v3 / MTP): every candidate it
    /// excluded stays named verbatim (the verbatim-surfacing law — an
    /// exclusion the user cannot see is a silent policy), the scanned
    /// root stands named, the gutter lanes are offered, and the pin
    /// escape keeps its quoted key and its file.
    #[test]
    fn the_tests_only_error_names_every_excluded_project_and_the_gutter_lane() {
        let tests = vec![
            PathBuf::from("C:/work/app\\tests\\V3\\V3.csproj"),
            PathBuf::from("C:/work/app\\tests\\V3b\\V3b.csproj"),
        ];
        let text = test_only_failure(Path::new("C:/work/app"), &tests);
        assert!(text.contains("C:/work/app"), "the scanned root stands named: {text}");
        for candidate in &tests {
            assert!(
                text.contains(&candidate.display().to_string()),
                "an exclusion must name what it excluded: {candidate:?} missing from {text}"
            );
        }
        assert!(text.contains("test project"), "the exclusion must say what these are: {text}");
        assert!(text.contains("gutter"), "the real lane must be offered: {text}");
        assert!(
            text.contains("\"program\""),
            "the escape hatch must be shown as the quoted config key: {text}"
        );
        assert!(text.contains(".zed/debug.json"), "the escape must say WHERE: {text}");
    }

    #[test]
    fn the_resolved_program_rides_the_launch_body_and_nothing_else_moves() {
        let request = json!({
            "seq": 7, "type": "request", "command": "launch",
            "arguments": { "projectPath": "C:/work/app", "cwd": "C:/work/app" },
        });
        let outbound = launch_with_program(
            &request,
            "C:/work/app\\src\\Api\\bin\\Debug\\net9.0\\Api.dll",
        );
        assert_eq!(
            outbound["seq"],
            json!(7),
            "envelope untouched — the relay rewrites numbers itself"
        );
        assert_eq!(outbound["arguments"]["cwd"], json!("C:/work/app"));
        assert_eq!(outbound["arguments"]["projectPath"], json!("C:/work/app"));
        assert_eq!(
            outbound["arguments"]["program"],
            json!("C:/work/app\\src\\Api\\bin\\Debug\\net9.0\\Api.dll")
        );
    }

    #[test]
    fn a_request_without_an_arguments_object_gains_none_out_of_thin_air() {
        // Unreachable past the relay's validator (a null-arguments launch
        // never parses), but the rewrite must not invent structure either.
        let request = json!({ "seq": 1, "type": "request", "command": "launch" });
        let outbound = launch_with_program(&request, "App.dll");
        assert_eq!(outbound, request);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zatara_types::launch_mode_from_knob;

    fn config_with(program: Option<&str>) -> LaunchConfig {
        LaunchConfig {
            program: program.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn the_knob_beats_inference_in_both_directions() {
        let both = LaunchConfig {
            program: Some("App.dll".into()),
            fully_qualified_name: "Ns.Test".into(),
            ..Default::default()
        };
        assert_eq!(
            dispatch_mode(&both, launch_mode_from_knob(Some("project"))),
            LaunchMode::Project
        );
        assert_eq!(
            dispatch_mode(&both, launch_mode_from_knob(Some("test"))),
            LaunchMode::Test
        );
        // The no-coercion law carries through the dispatch too: a knob
        // value that is not exactly `project`/`test` never reroutes.
        assert_eq!(
            dispatch_mode(&both, launch_mode_from_knob(Some("Project"))),
            dispatch_mode(&both, None)
        );
    }

    #[test]
    fn an_unset_knob_defers_to_program_presence() {
        assert_eq!(
            dispatch_mode(&config_with(Some("App.dll")), None),
            LaunchMode::Project
        );
        assert_eq!(dispatch_mode(&config_with(None), None), LaunchMode::Test);
    }

    #[test]
    fn an_absolute_program_path_is_checked_as_given() {
        // The absolute-path law is host-relative: a `C:/...` literal is NOT
        // absolute on a posix runner (the join prepends the cwd instead), so
        // the sample path must be native to the OS the suite runs on.
        let (given, cwd) = if cfg!(windows) {
            ("C:/work/app/bin/App.dll", "C:/other")
        } else {
            ("/work/app/bin/App.dll", "/other")
        };
        assert!(
            std::path::Path::new(given).is_absolute(),
            "the sample path must be absolute on this host"
        );
        assert_eq!(program_path(given, cwd), PathBuf::from(given));
    }

    #[test]
    fn a_relative_program_path_is_resolved_against_the_config_cwd() {
        assert_eq!(
            program_path("bin/Debug/net9.0/App.dll", "C:/work/app"),
            PathBuf::from("C:/work/app").join("bin/Debug/net9.0/App.dll")
        );
    }

    #[test]
    fn a_relative_program_without_a_cwd_is_left_as_is() {
        assert_eq!(
            program_path("bin/App.dll", ""),
            PathBuf::from("bin/App.dll")
        );
    }
}
