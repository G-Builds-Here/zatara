use std::collections::BTreeMap;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", try_from = "LaunchConfigWire")]
pub struct LaunchConfig {
    pub project_path: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub fully_qualified_name: String,
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub framework: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub case_arguments: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub method_parameters: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub case_string_slots: Vec<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub case_raw_arguments: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub results_log: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<LaunchRequest>,
    #[serde(default, rename = "process_id", skip_serializing_if = "Option::is_none")]
    pub process_id: Option<u64>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LaunchRequest {
    Launch,
    Attach,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchMode {
    Project,
    Test,
}
impl LaunchConfig {
    pub fn resolved_mode(&self) -> LaunchMode {
        if self.program.is_some() || self.hint_root().is_some() {
            LaunchMode::Project
        } else {
            LaunchMode::Test
        }
    }
    pub fn hint_root(&self) -> Option<&str> {
        (self.program.is_none()
            && self.fully_qualified_name.is_empty()
            && !self.project_path.is_empty())
        .then_some(self.project_path.as_str())
    }
}
pub const ZATARA_LAUNCH_MODE: &str = "ZATARA_LAUNCH_MODE";
pub fn launch_mode_from_knob(value: Option<&str>) -> Option<LaunchMode> {
    match value {
        Some("project") => Some(LaunchMode::Project),
        Some("test") => Some(LaunchMode::Test),
        _ => None,
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LaunchConfigWire {
    project_path: Option<String>,
    fully_qualified_name: Option<String>,
    cwd: Option<String>,
    env: Option<BTreeMap<String, String>>,
    scope: Option<String>,
    framework: Option<String>,
    #[serde(default)]
    case_arguments: Vec<String>,
    #[serde(default)]
    method_parameters: Vec<String>,
    #[serde(default)]
    case_string_slots: Vec<u32>,
    #[serde(default)]
    case_raw_arguments: Vec<String>,
    #[serde(default)]
    results_log: bool,
    program: Option<String>,
    args: Option<Vec<String>>,
    request: Option<LaunchRequest>,
    #[serde(rename = "process_id")]
    process_id: Option<u64>,
}
impl TryFrom<LaunchConfigWire> for LaunchConfig {
    type Error = String;
    fn try_from(wire: LaunchConfigWire) -> Result<Self, Self::Error> {
        if wire.request == Some(LaunchRequest::Attach) && wire.process_id.is_none() {
            return Err("missing field `process_id`".to_string());
        }
        let hint_pair = wire
            .project_path
            .as_deref()
            .is_some_and(|path| !path.is_empty())
            && wire.cwd.as_deref().is_some_and(|cwd| !cwd.is_empty());
        if wire.program.is_none()
            && wire.request != Some(LaunchRequest::Attach)
            && wire.fully_qualified_name.is_none()
            && !hint_pair
        {
            return Err("missing field `fullyQualifiedName`".to_string());
        }
        Ok(LaunchConfig {
            project_path: wire.project_path.unwrap_or_default(),
            fully_qualified_name: wire.fully_qualified_name.unwrap_or_default(),
            cwd: wire.cwd.unwrap_or_default(),
            env: wire.env,
            scope: wire.scope,
            framework: wire.framework,
            case_arguments: wire.case_arguments,
            method_parameters: wire.method_parameters,
            case_string_slots: wire.case_string_slots,
            case_raw_arguments: wire.case_raw_arguments,
            results_log: wire.results_log,
            program: wire.program,
            args: wire.args,
            request: wire.request,
            process_id: wire.process_id,
        })
    }
}
pub fn launch_mode_from_env() -> Option<LaunchMode> {
    launch_mode_from_knob(std::env::var(ZATARA_LAUNCH_MODE).ok().as_deref())
}
#[cfg(test)]
mod hint_pair_tests {
    use super::*;
    #[test]
    fn the_hint_pair_parses_and_resolves_the_project_lane() {
        let json = r#"{"projectPath": "C:/work/app", "cwd": "C:/work/app"}"#;
        let config: LaunchConfig =
            serde_json::from_str(json).expect("the hint pair names a launch target for discovery");
        assert_eq!(config.project_path, "C:/work/app");
        assert_eq!(config.cwd, "C:/work/app");
        assert!(
            config.fully_qualified_name.is_empty(),
            "a hint carries no test target"
        );
        assert_eq!(config.program, None);
        assert_eq!(config.resolved_mode(), LaunchMode::Project);
    }
    #[test]
    fn a_config_shy_of_the_pair_is_still_rejected_naming_the_test_target() {
        for json in [
            r#"{"projectPath": "C:/w/proj/CartModelTests.csproj"}"#,
            r#"{"cwd": "C:/work/app"}"#,
            r#"{"projectPath": "", "cwd": "C:/work/app"}"#,
            r#"{"projectPath": "C:/work/app", "cwd": ""}"#,
        ] {
            let err = serde_json::from_str::<LaunchConfig>(json)
                .expect_err("half a hint names nothing to run");
            assert!(
                err.to_string().contains("fullyQualifiedName"),
                "the failure names the missing test-lane target, got: {err}"
            );
        }
    }
    #[test]
    fn inference_keeps_every_lane_where_the_other_lanes_put_it() {
        assert_eq!(LaunchConfig::default().resolved_mode(), LaunchMode::Test);
        assert_eq!(
            LaunchConfig {
                project_path: "C:/w/proj/proj.csproj".into(),
                fully_qualified_name: "Unit.Tests.One.Two".into(),
                cwd: "C:/w/proj".into(),
                ..Default::default()
            }
            .resolved_mode(),
            LaunchMode::Test
        );
        assert_eq!(
            LaunchConfig {
                program: Some("bin/App.dll".into()),
                ..Default::default()
            }
            .resolved_mode(),
            LaunchMode::Project
        );
    }
}
