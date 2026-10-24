use std::borrow::Cow;
use std::path::{Path, PathBuf};
use serde_json::Value;
use zatara_types::{LaunchConfig, LaunchMode};
use crate::discovery::Resolution;
use crate::relay::{adapter_lost, Tunnel};
use std::sync::Arc;
pub(crate) fn dispatch_mode(config: &LaunchConfig, knob: Option<LaunchMode>) -> LaunchMode {
    knob.unwrap_or_else(|| config.resolved_mode())
}
fn program_path(program: &str, cwd: &str) -> PathBuf {
    let path = std::path::Path::new(program);
    if path.is_absolute() || cwd.is_empty() {
        path.to_path_buf()
    } else {
        std::path::Path::new(cwd).join(program)
    }
}
pub(crate) fn start(tunnel: Arc<Tunnel>, config: &LaunchConfig, request: &Value, launch_seq: i64) {
    let (program, resolved_from_hint) = match config.program.as_deref() {
        Some(program) => (program.to_string(), false),
        None => {
            let Some(root) = config.hint_root() else {
                fail(
                    &tunnel,
                    launch_seq,
                    "project-mode launch without a `program` field — nothing to run",
                );
                return;
            };
            match crate::discovery::resolve(Path::new(root)) {
                Ok(Resolution::Target(target)) => (target.dll.display().to_string(), true),
                Ok(Resolution::Multiple(candidates)) => {
                    fail(&tunnel, launch_seq, &multiple_failure(&candidates));
                    return;
                }
                Ok(Resolution::NotFound { root }) => {
                    fail(&tunnel, launch_seq, &not_found_failure(&root));
                    return;
                }
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
fn not_found_failure(root: &Path) -> String {
    format!(
        "zatara: program debug needs a runnable app (exe) project, and none was found under {}. If this folder only holds tests, use the gutter run/debug buttons instead; to run a specific dll anyway, add a .zed/debug.json scenario whose \"program\" key is its path, e.g. \"program\": \"C:/path/to/Your.dll\".",
        root.display()
    )
}
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
fn launch_with_program(request: &Value, program: &str) -> Value {
    let mut outbound = request.clone();
    if let Some(arguments) = outbound.get_mut("arguments").and_then(Value::as_object_mut) {
        arguments.insert("program".to_string(), Value::String(program.to_string()));
    }
    outbound
}
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
        assert_eq!(hint_only().hint_root(), Some("C:/work/app"));
        assert_eq!(
            LaunchConfig {
                program: Some("App.dll".into()),
                project_path: "C:/work/app".into(),
                ..Default::default()
            }
            .hint_root(),
            None
        );
        assert_eq!(
            LaunchConfig {
                project_path: "C:/w/proj/CartModelTests.csproj".into(),
                fully_qualified_name: "Tests.One".into(),
                ..Default::default()
            }
            .hint_root(),
            None
        );
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
