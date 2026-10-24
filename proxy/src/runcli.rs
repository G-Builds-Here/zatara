//! The plain-Run lane (XUNIT-V3-MTP): `zatara-proxy run-case`.
//!
//! The generic per-arity Run templates point their command at this binary,
//! so the Run lane's case filter is composed here — the same `build_filter`
//! the debug lane rides, one grammar table for both lanes. Zed injects every
//! match capture as `ZED_CUSTOM_csharp_*` env into the spawned task process
//! (live-proven 2026-10-23, plain Run with no debug session), so the row's
//! identity arrives through the environment: no task arguments, no shell
//! quoting, no metachar risk.
//!
//! Decline-don't-guess: a missing capture, an unexpanded `${…}` marker
//! (Zed did not substitute — a filter built from it would name nothing), or
//! a missing working directory all end the run with a message and a failure
//! code, never with a silently-wrong filter.

use std::path::Path;
use std::process::ExitCode;

use zatara_types::LaunchConfig;

use crate::discovery;
use crate::mtp_case;
use crate::runner;

/// The capture env prefix Zed injects per gutter match.
const CAPTURE_PREFIX: &str = "ZED_CUSTOM_csharp_";

/// Build the numbered capture list `prefix{N}` (N = 1, 2, …) — stopped at
/// the first gap, the same law as the locator's `env_list`. An unexpanded
/// `${` marker is a decline, not a value.
fn numbered(
    get: &dyn Fn(&str) -> Option<String>,
    prefix: &str,
) -> Result<Vec<String>, String> {
    let mut values = Vec::new();
    for n in 1u32.. {
        match get(&format!("{prefix}{n}")) {
            Some(value) if value.contains("${") => {
                return Err(format!(
                    "capture `{prefix}{n}` is unexpanded ({value}) — Zed did not substitute it"
                ));
            }
            Some(value) if !value.is_empty() => values.push(value),
            _ => break,
        }
    }
    Ok(values)
}

/// The case-scope launch config the Run lane's environment names, pure over
/// the env getter (the env-knob law: the knob logic never reads the real
/// environment, so it tests without racing).
fn case_config_from(
    get: &dyn Fn(&str) -> Option<String>,
) -> Result<LaunchConfig, String> {
    let capture = |name: &str| get(&format!("{CAPTURE_PREFIX}{name}"));
    let namespace = capture("namespace").filter(|v| !v.is_empty()).ok_or("missing capture `ZED_CUSTOM_csharp_namespace`")?;
    let class = capture("class_name").filter(|v| !v.is_empty()).ok_or("missing capture `ZED_CUSTOM_csharp_class_name`")?;
    let method = capture("method_name").filter(|v| !v.is_empty()).ok_or("missing capture `ZED_CUSTOM_csharp_method_name`")?;
    let cwd = get("ZED_DIRNAME").filter(|v| !v.is_empty()).ok_or("missing `ZED_DIRNAME` — no working directory to run in")?;
    let case_arguments = numbered(get, &format!("{CAPTURE_PREFIX}case_arg_"))?;
    if case_arguments.is_empty() {
        return Err("no `case_arg_N` captures — a Run-case task names no row".to_string());
    }
    for value in &case_arguments {
        if value.contains("${") {
            return Err(format!("capture value is unexpanded ({value})"));
        }
    }
    let mut config = LaunchConfig {
        project_path: cwd.clone(),
        fully_qualified_name: format!("{namespace}.{class}.{method}"),
        cwd,
        scope: Some("case".to_string()),
        framework: Some("xunit".to_string()),
        case_arguments,
        // An unexpanded marker in ANY capture family declines the whole run:
        // half a row's identity on the filter is exactly the silently-wrong
        // filter class this lane exists to kill.
        method_parameters: numbered(get, &format!("{CAPTURE_PREFIX}method_param_"))?,
        case_raw_arguments: numbered(get, &format!("{CAPTURE_PREFIX}case_raw_"))?,
        ..Default::default()
    };
    runner::resolve_project_path(&mut config);
    Ok(config)
}

/// The `run-case` entry: compose the filter, run `dotnet test` in the
/// gutter row's directory, stream stdio to the Run tab, pass the exit code
/// through (Run-tab semantics unchanged: the task's result is dotnet's).
///
/// An MTP (xunit v3) project takes the row-id lane instead — the vstest
/// `--filter` this argv speaks is silently dropped by the MTP runner (probe
/// row 2), so the row resolves through discovery to its unique `-id`.
pub fn run_from_env() -> ExitCode {
    let get = |key: &str| std::env::var(key).ok();
    let config = match case_config_from(&get) {
        Ok(config) => config,
        Err(message) => {
            eprintln!("zatara-proxy run-case: {message}");
            return ExitCode::FAILURE;
        }
    };
    if discovery::mtp_test_project(Path::new(&config.project_path)) {
        return run_mtp_case(&config);
    }
    let argv = runner::test_argv(&config);
    let program = argv[0].clone();
    let status = std::process::Command::new(&program)
        .args(&argv[1..])
        .current_dir(runner::work_dir(&config))
        .status();
    child_exit(status, &program)
}

/// The v3 row-id Run lane: build (a plain Run has no build step of its own —
/// unlike the debug lane, whose relay already built before dispatch), locate
/// the built dll, resolve the clicked row to its unique case id, then run
/// exactly that case. Decline-don't-guess still rules: a row that matches no
/// case (or two) ends the run with the module's message, never a wider scope.
fn run_mtp_case(config: &LaunchConfig) -> ExitCode {
    let csproj = Path::new(&config.project_path);
    let build = std::process::Command::new("dotnet")
        .arg("build")
        .arg(&config.project_path)
        .current_dir(runner::work_dir(config))
        .status();
    match build {
        Ok(status) if status.success() => {}
        Ok(status) => {
            eprintln!(
                "zatara-proxy run-case: `dotnet build` failed before the row could be resolved ({status})"
            );
            return ExitCode::FAILURE;
        }
        Err(e) => {
            eprintln!("zatara-proxy run-case: failed to spawn `dotnet build`: {e}");
            return ExitCode::FAILURE;
        }
    }
    let dll = match discovery::locate_built_dll(csproj) {
        Ok(dll) => dll,
        Err(e) => {
            eprintln!("zatara-proxy run-case: {e}");
            return ExitCode::FAILURE;
        }
    };
    let id = match mtp_case::resolve_case_id(&dll, config) {
        Ok(id) => id,
        Err(message) => {
            eprintln!("zatara-proxy run-case: {message}");
            return ExitCode::FAILURE;
        }
    };
    // The resolved row rides the tab (owner live cycle 2026-10-24: the run
    // was right — Total: 1, the clicked row — but nothing in the output said
    // WHICH row it had run; the summary line alone cannot tell two sibling
    // rows apart).
    eprintln!(
        "zatara: xunit v3 single case: {}",
        mtp_case::case_display_name(
            &config.fully_qualified_name,
            &config.method_parameters,
            &config.case_raw_arguments
        )
    );
    let status = std::process::Command::new("dotnet")
        .arg(&dll)
        .args(mtp_case::id_args(&id))
        .current_dir(runner::work_dir(config))
        .status();
    child_exit(status, "dotnet")
}

/// Run-tab exit law, shared by both lanes: success is success, a failing
/// child forwards its own code when it fits a `u8` (Zed shows it as the task
/// exit), and a spawn failure is a message plus the failure code.
fn child_exit(status: std::io::Result<std::process::ExitStatus>, program: &str) -> ExitCode {
    match status {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(status) => status
            .code()
            .and_then(|code| u8::try_from(code).ok())
            .map(ExitCode::from)
            .unwrap_or(ExitCode::FAILURE),
        Err(e) => {
            eprintln!("zatara-proxy run-case: failed to spawn {program}: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: BTreeMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |key: &str| map.get(key).map(|v| v.clone())
    }

    fn row() -> Vec<(&'static str, &'static str)> {
        vec![
            ("ZED_CUSTOM_csharp_namespace", "Shapes2"),
            ("ZED_CUSTOM_csharp_class_name", "ShapeTests2"),
            ("ZED_CUSTOM_csharp_method_name", "Mixed3"),
            ("ZED_DIRNAME", "C:/work/xvcase"),
            ("ZED_CUSTOM_csharp_case_arg_1", "a"),
            ("ZED_CUSTOM_csharp_case_arg_2", "1"),
            ("ZED_CUSTOM_csharp_case_arg_3", "True"),
            ("ZED_CUSTOM_csharp_case_raw_1", "\"a\""),
            ("ZED_CUSTOM_csharp_case_raw_2", "1"),
            ("ZED_CUSTOM_csharp_case_raw_3", "True"),
            ("ZED_CUSTOM_csharp_method_param_1", "s"),
            ("ZED_CUSTOM_csharp_method_param_2", "n"),
            ("ZED_CUSTOM_csharp_method_param_3", "flag"),
        ]
    }

    #[test]
    fn the_row_env_builds_the_case_wire_the_debug_lane_ships() {
        // The Run lane's wire must equal what the locator emits for the same
        // row: case scope, xunit, FQN from the captures, RAW list verbatim.
        let config = case_config_from(&env(&row())).expect("the row env names a case");
        assert_eq!(config.fully_qualified_name, "Shapes2.ShapeTests2.Mixed3");
        assert_eq!(config.scope.as_deref(), Some("case"));
        assert_eq!(config.framework.as_deref(), Some("xunit"));
        assert_eq!(config.case_raw_arguments, vec!["\"a\"", "1", "True"]);
        // The project path is the cwd until discovery answers (the directory
        // here does not exist, so the resolver keeps it — the same
        // keep-what-you-cannot-prove law the debug lane follows).
        assert_eq!(config.project_path, "C:/work/xvcase");
        // One filter, one table: the AND-substring law on the derived slots.
        assert_eq!(
            crate::runner::build_filter(&config),
            "FullyQualifiedName~Shapes2.ShapeTests2.Mixed3&DisplayName~a&DisplayName~n: 1&DisplayName~flag: True"
        );
    }

    #[test]
    fn an_unexpanded_capture_declines_the_run() {
        // The hide-law guard in wire form: a `${…}` marker reaching this far
        // means Zed never substituted it — a filter built on it names
        // nothing, so the run declines with a message instead.
        let mut pairs = row();
        pairs[4] = ("ZED_CUSTOM_csharp_case_arg_1", "${ZED_CUSTOM_csharp_case_arg_1}");
        let err = case_config_from(&env(&pairs)).expect_err("an unexpanded capture is a decline");
        assert!(err.contains("unexpanded"), "got: {err}");
    }

    #[test]
    fn a_row_without_arguments_declines() {
        // A method click rides the method template, never this lane; an
        // argument-less case task names every row of the method — the
        // silently-wrong filter class this epic kills.
        let pairs: Vec<(&str, &str)> = row()
            .into_iter()
            .filter(|(k, _)| !k.contains("case_") && !k.contains("method_param_"))
            .collect();
        let err = case_config_from(&env(&pairs)).expect_err("no args names no row");
        assert!(err.contains("case_arg"), "got: {err}");
    }

    #[test]
    fn a_missing_working_directory_declines() {
        let pairs: Vec<(&str, &str)> = row()
            .into_iter()
            .filter(|(k, _)| *k != "ZED_DIRNAME")
            .collect();
        let err = case_config_from(&env(&pairs)).expect_err("no cwd cannot run");
        assert!(err.contains("ZED_DIRNAME"), "got: {err}");
    }
}
