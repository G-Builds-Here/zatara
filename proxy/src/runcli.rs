use std::path::Path;
use std::process::ExitCode;
use zatara_types::LaunchConfig;
use crate::discovery;
use crate::mtp_case;
use crate::runner;
const CAPTURE_PREFIX: &str = "ZED_CUSTOM_csharp_";
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
        env: get("DOTNET_FILE")
            .filter(|value| !value.is_empty())
            .map(|value| std::collections::BTreeMap::from([("DOTNET_FILE".to_string(), value)])),
        method_parameters: numbered(get, &format!("{CAPTURE_PREFIX}method_param_"))?,
        case_raw_arguments: numbered(get, &format!("{CAPTURE_PREFIX}case_raw_"))?,
        ..Default::default()
    };
    runner::resolve_project_path(&mut config);
    Ok(config)
}
#[derive(PartialEq, Eq, Debug)]
enum Lane {
    Case,
    Method,
    Class,
}
fn decide_lane(get: &dyn Fn(&str) -> Option<String>) -> Lane {
    if get("ZATARA_TEST_CASE").as_deref() == Some("1") {
        return Lane::Case;
    }
    let has_method = get(&format!("{CAPTURE_PREFIX}method_name"))
        .is_some_and(|value| !value.is_empty());
    if has_method {
        Lane::Method
    } else {
        Lane::Class
    }
}
fn plain_plan(get: &dyn Fn(&str) -> Option<String>) -> Result<(LaunchConfig, Vec<String>), String> {
    let capture = |name: &str| get(&format!("{CAPTURE_PREFIX}{name}"));
    let claimed = |name: &str, value: Option<String>| -> Result<String, String> {
        let value = value
            .filter(|value| !value.is_empty())
            .ok_or(format!("missing capture `ZED_CUSTOM_csharp_{name}`"))?;
        if value.contains("${") {
            return Err(format!(
                "capture `ZED_CUSTOM_csharp_{name}` is unexpanded ({value}) — Zed did not substitute it"
            ));
        }
        Ok(value)
    };
    let namespace = claimed("namespace", capture("namespace"))?;
    let class = claimed("class_name", capture("class_name"))?;
    let method = match capture("method_name") {
        Some(value) if !value.is_empty() => Some(claimed("method_name", Some(value))?),
        _ => None,
    };
    let cwd = get("ZED_DIRNAME")
        .filter(|value| !value.is_empty())
        .ok_or("missing `ZED_DIRNAME` — no working directory to run in")?;
    let filter = get("ZATARA_TEST_FILTER")
        .filter(|value| !value.is_empty())
        .ok_or("missing `ZATARA_TEST_FILTER` — the plain Run task names no filter")?;
    if filter.contains("${") {
        return Err(format!(
            "`ZATARA_TEST_FILTER` is unexpanded ({filter}) — Zed did not substitute it"
        ));
    }
    let (scope, fully_qualified_name) = match &method {
        Some(method) => ("method", format!("{namespace}.{class}.{method}")),
        None => ("class", format!("{namespace}.{class}")),
    };
    let mut config = LaunchConfig {
        project_path: cwd.clone(),
        fully_qualified_name,
        cwd,
        scope: Some(scope.to_string()),
        env: get("DOTNET_FILE")
            .filter(|value| !value.is_empty())
            .map(|value| std::collections::BTreeMap::from([("DOTNET_FILE".to_string(), value)])),
        ..Default::default()
    };
    runner::resolve_project_path(&mut config);
    let argv = vec![
        "dotnet".to_string(),
        "test".to_string(),
        config.project_path.clone(),
        "--filter".to_string(),
        filter,
        "--".to_string(),
        if method.is_some() { "--filter-method".to_string() } else { "--filter-class".to_string() },
        config.fully_qualified_name.clone(),
    ];
    Ok((config, argv))
}
pub fn run_from_env() -> ExitCode {
    let get = |key: &str| std::env::var(key).ok();
    match decide_lane(&get) {
        Lane::Case => run_case_lane(&get),
        Lane::Method | Lane::Class => run_plain_lane(&get),
    }
}
fn run_case_lane(get: &dyn Fn(&str) -> Option<String>) -> ExitCode {
    let config = match case_config_from(get) {
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
fn run_plain_lane(get: &dyn Fn(&str) -> Option<String>) -> ExitCode {
    let (config, argv) = match plain_plan(get) {
        Ok(plan) => plan,
        Err(message) => {
            eprintln!("zatara-proxy run-case: {message}");
            return ExitCode::FAILURE;
        }
    };
    let program = argv[0].clone();
    let status = std::process::Command::new(&program)
        .args(&argv[1..])
        .current_dir(runner::work_dir(&config))
        .status();
    child_exit(status, &program)
}
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
            ("ZATARA_TEST_CASE", "1"),
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
        let config = case_config_from(&env(&row())).expect("the row env names a case");
        assert_eq!(config.fully_qualified_name, "Shapes2.ShapeTests2.Mixed3");
        assert_eq!(config.scope.as_deref(), Some("case"));
        assert_eq!(config.framework.as_deref(), Some("xunit"));
        assert_eq!(config.case_raw_arguments, vec!["\"a\"", "1", "True"]);
        assert_eq!(config.project_path, "C:/work/xvcase");
        assert_eq!(
            crate::runner::build_filter(&config),
            "FullyQualifiedName~Shapes2.ShapeTests2.Mixed3&DisplayName~a&DisplayName~n: 1&DisplayName~flag: True"
        );
    }
    #[test]
    fn an_unexpanded_capture_declines_the_run() {
        let mut pairs = row();
        pairs[5] = ("ZED_CUSTOM_csharp_case_arg_1", "${ZED_CUSTOM_csharp_case_arg_1}");
        let err = case_config_from(&env(&pairs)).expect_err("an unexpanded capture is a decline");
        assert!(err.contains("unexpanded"), "got: {err}");
    }
    #[test]
    fn a_row_without_arguments_declines() {
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
    fn method_click() -> Vec<(&'static str, &'static str)> {
        vec![
            ("ZED_CUSTOM_csharp_namespace", "Shapes"),
            ("ZED_CUSTOM_csharp_class_name", "CalcTests"),
            ("ZED_CUSTOM_csharp_method_name", "AddsOne"),
            ("ZED_DIRNAME", "C:/work/app"),
            ("DOTNET_FILE", "C:/work/app/CalcTests.cs"),
            ("ZATARA_TEST_FILTER", "FullyQualifiedName=Shapes.CalcTests.AddsOne"),
        ]
    }
    fn class_click() -> Vec<(&'static str, &'static str)> {
        vec![
            ("ZED_CUSTOM_csharp_namespace", "Shapes"),
            ("ZED_CUSTOM_csharp_class_name", "CalcTests"),
            ("ZED_DIRNAME", "C:/work/app"),
            ("DOTNET_FILE", "C:/work/app/CalcTests.cs"),
            ("ZATARA_TEST_FILTER", "FullyQualifiedName~Shapes.CalcTests."),
        ]
    }
    #[test]
    fn the_tag_and_captures_pick_the_lane() {
        assert_eq!(decide_lane(&env(&row())), Lane::Case);
        assert_eq!(decide_lane(&env(&method_click())), Lane::Method);
        assert_eq!(decide_lane(&env(&class_click())), Lane::Class);
        let untagged: Vec<(&str, &str)> =
            row().into_iter().filter(|(k, _)| *k != "ZATARA_TEST_CASE").collect();
        assert_eq!(decide_lane(&env(&untagged)), Lane::Method);
    }
    #[test]
    fn a_method_click_ships_the_verbatim_filter_and_the_forwarded_method() {
        let (config, argv) = plain_plan(&env(&method_click())).expect("a method click names a run");
        assert_eq!(config.scope.as_deref(), Some("method"));
        assert_eq!(config.fully_qualified_name, "Shapes.CalcTests.AddsOne");
        assert_eq!(
            argv,
            vec![
                "dotnet",
                "test",
                "C:/work/app",
                "--filter",
                "FullyQualifiedName=Shapes.CalcTests.AddsOne",
                "--",
                "--filter-method",
                "Shapes.CalcTests.AddsOne",
            ]
        );
    }
    #[test]
    fn a_class_click_forwards_the_class_without_the_filter_dot() {
        let (config, argv) = plain_plan(&env(&class_click())).expect("a class click names a run");
        assert_eq!(config.scope.as_deref(), Some("class"));
        assert_eq!(config.fully_qualified_name, "Shapes.CalcTests");
        assert_eq!(
            argv,
            vec![
                "dotnet",
                "test",
                "C:/work/app",
                "--filter",
                "FullyQualifiedName~Shapes.CalcTests.",
                "--",
                "--filter-class",
                "Shapes.CalcTests",
            ]
        );
    }
    #[test]
    fn a_plain_click_without_a_filter_declines() {
        let pairs: Vec<(&str, &str)> = method_click()
            .into_iter()
            .filter(|(k, _)| *k != "ZATARA_TEST_FILTER")
            .collect();
        let err = plain_plan(&env(&pairs)).expect_err("no filter names no run");
        assert!(err.contains("ZATARA_TEST_FILTER"), "got: {err}");
    }
    #[test]
    fn an_unexpanded_filter_declines_the_plain_run() {
        let mut pairs = method_click();
        pairs[5] = ("ZATARA_TEST_FILTER", "FullyQualifiedName=${ZED_CUSTOM_csharp_method_name}");
        let err = plain_plan(&env(&pairs)).expect_err("an unexpanded filter names nothing");
        assert!(err.contains("unexpanded"), "got: {err}");
    }
    #[test]
    fn an_unexpanded_capture_declines_the_plain_run() {
        let mut pairs = method_click();
        pairs[2] = ("ZED_CUSTOM_csharp_method_name", "${ZED_CUSTOM_csharp_method_name}");
        let err = plain_plan(&env(&pairs)).expect_err("an unexpanded capture names nothing");
        assert!(err.contains("unexpanded"), "got: {err}");
    }
}
