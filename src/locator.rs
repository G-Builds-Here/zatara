use zed_extension_api::{
    BuildTaskDefinition, BuildTaskDefinitionTemplatePayload, DebugScenario, TaskTemplate,
};
use zatara_types::LaunchConfig;
pub const LOCATOR_NAME: &str = "zatara";
pub const TEST_FILTER_ENV: &str = "ZATARA_TEST_FILTER";
pub const FILTER_PREFIX: &str = "FullyQualifiedName=";
pub const ECHO_BUILD_LINE: &str = "zatara: no build step - the proxy owns build and test";
pub fn parse_test_filter(build_task: &TaskTemplate) -> Option<String> {
    env_value(build_task, TEST_FILTER_ENV)
        .and_then(|value| value.strip_prefix(FILTER_PREFIX))
        .filter(|fqn| !fqn.is_empty())
        .map(str::to_string)
}
pub const TEST_CASE_ENV: &str = "ZATARA_TEST_CASE";
pub const TEST_CASE_TAG: &str = "1";
pub const TEST_FRAMEWORK_ENV: &str = "ZATARA_TEST_FRAMEWORK";
pub const CASE_ARG_ENV_PREFIX: &str = "ZATARA_CASE_ARG_";
pub const CASE_PARAM_ENV_PREFIX: &str = "ZATARA_CASE_PARAM_";
pub const CASE_STR_ENV_PREFIX: &str = "ZATARA_CASE_STR_";
pub const CASE_RAW_ENV_PREFIX: &str = "ZATARA_CASE_RAW_";
pub const CASE_FILTER_PREFIX: &str = "FullyQualifiedName~";
fn env_value<'a>(build_task: &'a TaskTemplate, key: &str) -> Option<&'a str> {
    build_task
        .env
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}
fn env_list(build_task: &TaskTemplate, prefix: &str) -> Vec<String> {
    let mut numbered: Vec<(u32, String)> = build_task
        .env
        .iter()
        .filter(|(k, v)| k.starts_with(prefix) && !v.is_empty())
        .filter_map(|(k, v)| k[prefix.len()..].parse::<u32>().ok().map(|n| (n, v.clone())))
        .collect();
    numbered.sort_by_key(|(n, _)| *n);
    numbered.into_iter().map(|(_, v)| v).collect()
}
fn env_indices(build_task: &TaskTemplate, prefix: &str) -> Vec<u32> {
    let mut numbered: Vec<u32> = build_task
        .env
        .iter()
        .filter(|(k, v)| k.starts_with(prefix) && !v.is_empty())
        .filter_map(|(k, _)| k[prefix.len()..].parse::<u32>().ok())
        .collect();
    numbered.sort_unstable();
    numbered
}
pub fn parse_case_filter(build_task: &TaskTemplate) -> Option<String> {
    env_value(build_task, TEST_FILTER_ENV)
        .and_then(|value| value.strip_prefix(CASE_FILTER_PREFIX))
        .map(|rest| rest.split('&').next().unwrap_or(rest).to_string())
        .filter(|fqn| !fqn.is_empty())
}
pub fn claims_case(build_task: &TaskTemplate) -> bool {
    env_value(build_task, TEST_CASE_ENV) == Some(TEST_CASE_TAG)
}
fn case_parts(
    build_task: &TaskTemplate,
) -> Option<(String, Option<String>, Vec<String>, Vec<String>, Vec<u32>, Vec<String>)> {
    let fully_qualified_name = parse_case_filter(build_task)?;
    let case_arguments = env_list(build_task, CASE_ARG_ENV_PREFIX);
    if case_arguments.is_empty() {
        return None;
    }
    Some((
        fully_qualified_name,
        env_value(build_task, TEST_FRAMEWORK_ENV).map(str::to_string),
        case_arguments,
        env_list(build_task, CASE_PARAM_ENV_PREFIX),
        env_indices(build_task, CASE_STR_ENV_PREFIX),
        env_list(build_task, CASE_RAW_ENV_PREFIX),
    ))
}
pub fn project_path_from_dir(dir: &str) -> Option<String> {
    if dir.contains("$ZED_") {
        return Some(dir.to_string());
    }
    let trimmed = dir.trim_end_matches(['/', '\\']);
    let name = trimmed.rsplit(['/', '\\']).next().filter(|n| !n.is_empty())?;
    let separator = if trimmed.contains('\\') && !trimmed.contains('/') {
        '\\'
    } else {
        '/'
    };
    Some(format!("{trimmed}{separator}{name}.csproj"))
}
pub fn launch_config(build_task: &TaskTemplate) -> Option<LaunchConfig> {
    let (fully_qualified_name, scope, framework, case_arguments, method_parameters, case_string_slots, case_raw_arguments) =
        if claims_case(build_task) {
            let (fqn, framework, args, params, string_slots, raw_arguments) = case_parts(build_task)?;
            (fqn, Some("case".to_string()), framework, args, params, string_slots, raw_arguments)
        } else {
            (parse_test_filter(build_task)?, None, None, Vec::new(), Vec::new(), Vec::new(), Vec::new())
        };
    let cwd = build_task.cwd.clone()?;
    let project_path = project_path_from_dir(&cwd)?;
    Some(LaunchConfig {
        project_path,
        fully_qualified_name,
        cwd,
        env: Some(build_task.env.iter().cloned().collect()),
        scope,
        framework,
        case_arguments,
        method_parameters,
        case_string_slots,
        case_raw_arguments,
        results_log: false,
        ..Default::default()
    })
}
pub const PROGRAM_BUILD_ENV: &str = "ZATARA_PROGRAM_BUILD";
pub const PROGRAM_BUILD_TAG: &str = "1";
pub const PROGRAM_SCENARIO_LABEL: &str = "zatara: build & debug (workspace)";
pub fn claims_program_build(build_task: &TaskTemplate) -> bool {
    build_task
        .env
        .iter()
        .any(|(key, value)| key == PROGRAM_BUILD_ENV && value == PROGRAM_BUILD_TAG)
}
pub fn program_scenario(
    build_task: &TaskTemplate,
    _resolved_label: String,
    debug_adapter_name: String,
) -> Option<DebugScenario> {
    if !claims_program_build(build_task) {
        return None;
    }
    let root = build_task.cwd.clone()?;
    let config = LaunchConfig {
        project_path: root.clone(),
        cwd: root,
        ..Default::default()
    };
    Some(DebugScenario {
        label: PROGRAM_SCENARIO_LABEL.into(),
        adapter: debug_adapter_name,
        build: None,
        config: serde_json::to_string(&config).ok()?,
        tcp_connection: None,
    })
}
pub fn native_scenario(
    build_task: &TaskTemplate,
    resolved_label: String,
    debug_adapter_name: String,
) -> Option<DebugScenario> {
    Some(DebugScenario {
        label: resolved_label,
        adapter: debug_adapter_name,
        build: None,
        config: serde_json::to_string(&launch_config(build_task)?).ok()?,
        tcp_connection: None,
    })
}
pub fn echo_fallback_scenario(
    build_task: &TaskTemplate,
    resolved_label: String,
    debug_adapter_name: String,
) -> Option<DebugScenario> {
    let mut scenario = native_scenario(build_task, resolved_label, debug_adapter_name)?;
    scenario.build = Some(BuildTaskDefinition::Template(
        BuildTaskDefinitionTemplatePayload {
            locator_name: Some(LOCATOR_NAME.to_string()),
            template: TaskTemplate {
                label: "zatara: no-op build step".to_string(),
                command: "echo".to_string(),
                args: vec![ECHO_BUILD_LINE.to_string()],
                env: Vec::new(),
                cwd: None,
            },
        },
    ));
    Some(scenario)
}
