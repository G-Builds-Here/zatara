use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The one launch-config contract that crosses the extension/proxy boundary
/// (survey anti-pattern 8: duplicate config contracts — this is the copy
/// neither consumer is allowed to make). Wire shape is camelCase; the
/// extension side writes it, the proxy side reads it (SCHEMA_EQ).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", try_from = "LaunchConfigWire")]
pub struct LaunchConfig {
    pub project_path: String,
    /// An empty FQN serializes as an omitted key: the program-build door's
    /// scenario (LOC-0128) is the launch-time hint pair only, and the hint
    /// is not a test lane — no `fullyQualifiedName` key may ride. The
    /// test-lane wire bytes are untouched (a test lane's FQN is never
    /// empty; the locator declines an empty filter), so the LOC-0109 byte
    /// pin stands. Parse-side strictness is deliberately NOT mirrored:
    /// `TryFrom` still rejects a nameless test lane (reject-don't-ignore);
    /// the hint-only shape is a discovery input resolved at launch
    /// (LOC-0129), not a config the proxy consumes as-is.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub fully_qualified_name: String,
    pub cwd: String,
    /// `None` serializes as an omitted key, never as `null` — an unset env
    /// must not appear on the wire at all. BTreeMap so multi-key env output
    /// is deterministic, byte-for-byte.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<BTreeMap<String, String>>,
    /// What the gutter click selected: `method` (the default — the whole
    /// named test), `case` (one parameterized case), `class` (every test of
    /// the named class), `file` (every test of the file — then
    /// `fullyQualifiedName` carries the file's classes `|`-joined). Absent
    /// means `method`: pre-scope wire clients are unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// The test framework behind the target — `nunit`, `mstest`, `xunit` —
    /// consulted only for `case` scope, whose filter grammar differs per
    /// adapter (see `runner::build_filter`). Absent means `xunit`, the
    /// framework the first case filters shipped against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub framework: Option<String>,
    /// The displayed arguments of the invoked case (nunit `[TestCase]`,
    /// mstest `[DataRow]`, xunit `[InlineData]`), as displayed tokens.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub case_arguments: Vec<String>,
    /// The invoked method's parameter names, zipped with `case_arguments` to
    /// rebuild xunit's named-argument display (`a: 1, b: 2, …`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub method_parameters: Vec<String>,
    /// 1-based indices of the `case_arguments` slots whose InlineData
    /// argument is a string literal (from `ZATARA_CASE_STR_*` task env).
    /// The proxy's AND-substring filter law reads them: a string slot is
    /// matched by its bare content (`DisplayName~<content>`), because quotes
    /// never survive the filter transport; primitive slots keep the
    /// `DisplayName~<param>: <value>` part. Empty means the classic
    /// all-primitives zip — omit-if-empty, so every pre-string wire byte
    /// is unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub case_string_slots: Vec<u32>,
    /// The per-slot InlineData values as written in source (`"a"` WITH the
    /// quotes, `1` bare), from `ZATARA_CASE_RAW_*` task env — shipped by the
    /// per-arity generic templates, where one key exists for every slot of
    /// the arity so the template never references a capture the match lacks
    /// (Zed's hide law). A value beginning with a double quote marks that
    /// slot's string lane; the proxy derives `case_string_slots` from the
    /// same list when
    /// the STR markers are absent. Omit-if-empty: a classic per-shape task's
    /// wire bytes are unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub case_raw_arguments: Vec<String>,
    /// Test-results capture (owner triage D27): `true` makes the proxy append
    /// dotnet's own `--logger trx` to the test step, so each run writes a
    /// `.trx` results file under `TestResults/`. Off by default — absent or
    /// `false` serializes as an omitted key, and a default config's wire
    /// bytes are byte-for-byte the pre-D27 bytes.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub results_log: bool,
    /// The program-mode field set (LOC-0121 §6): a plain program launch
    /// carries `program` (the dll/apphost to debug), its `args`, and a
    /// `request` (`launch` or `attach`); manual-pid attach also carries
    /// `process_id`. `program` presence is the mode inference (DR-1).
    /// LOC-0109 law: every one of these is omit-if-none, so a test-lane
    /// config's wire bytes are byte-for-byte the pre-program-mode bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<LaunchRequest>,
    /// Manual-pid attach (LOC-0130's lane). Deliberate exception to the
    /// camelCase rename: the wire key is `process_id`, snake_case, pinned
    /// verbatim by the contract's exact JSON shapes.
    #[serde(default, rename = "process_id", skip_serializing_if = "Option::is_none")]
    pub process_id: Option<u64>,
}

/// What the config asks the proxy to do: start a program, or attach to a
/// running pid (LOC-0130's lane). Wire values are lowercase `launch` /
/// `attach`; absent means a pre-program-mode client sent no request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LaunchRequest {
    Launch,
    Attach,
}

/// The dispatch mode a launch resolves to — the test lane or the program
/// lane (DR-1). A resolution, never a wire field: the config carries fields,
/// the mode is what they mean.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchMode {
    Project,
    Test,
}

impl LaunchConfig {
    /// DR-1 inference, extended by the locator's hint pair: `program`
    /// present resolves Project; [`LaunchConfig::hint_root`] answering
    /// resolves Project too — the hint is discovery's launch-time input,
    /// and LOC-0129 turns it into a spawn. Everything else (the FQN-
    /// carrying test lane, and the bare config only a knob-forced fail
    /// ever sees) resolves Test. The `ZATARA_LAUNCH_MODE` knob can force
    /// the dispatch mode at the caller's from_env boundary; the knob is
    /// never a wire field.
    pub fn resolved_mode(&self) -> LaunchMode {
        if self.program.is_some() || self.hint_root().is_some() {
            LaunchMode::Project
        } else {
            LaunchMode::Test
        }
    }

    /// The one definition of the locator's hint-only shape (LOC-0129): a
    /// `projectPath` with no program and no test target — `Some(root)` is
    /// the directory discovery resolves the program from at launch. A
    /// config carrying a test target is the test lane's shape even when
    /// the knob forces Project: the knob forces dispatch, never a
    /// resolution. Wire-level acceptance of this shape is the hint pair in
    /// the `TryFrom<LaunchConfigWire>` impl (which additionally requires a
    /// `cwd`); this is the resolved-config rule every consumer routes on.
    pub fn hint_root(&self) -> Option<&str> {
        (self.program.is_none()
            && self.fully_qualified_name.is_empty()
            && !self.project_path.is_empty())
        .then_some(self.project_path.as_str())
    }
}

/// `ZATARA_LAUNCH_MODE` — the DR-1 knob escape, an env var and never a wire
/// field.
pub const ZATARA_LAUNCH_MODE: &str = "ZATARA_LAUNCH_MODE";

/// The knob, pure (env-knob law, the `runner_armed` template): only the
/// exact values `project` and `test` act — anything else, including wrong
/// case and anything unrecognized, defers to inference. No coercion: a
/// typo'd knob must not silently reroute a launch.
pub fn launch_mode_from_knob(value: Option<&str>) -> Option<LaunchMode> {
    match value {
        Some("project") => Some(LaunchMode::Project),
        Some("test") => Some(LaunchMode::Test),
        _ => None,
    }
}

/// The deserialization twin of [`LaunchConfig`] — the same single wire shape
/// (this file stays the one definition), with every field absent-tolerant so
/// the partial shapes (program without a project hint, attach with only a
/// request and a pid) parse. The strictness the wire owes lives in the
/// [`TryFrom`] below, not in required fields.
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
        // reject-don't-ignore: an attach with no pid has no safe default —
        // a silent pid would attach the debugger to the wrong process.
        if wire.request == Some(LaunchRequest::Attach) && wire.process_id.is_none() {
            return Err("missing field `process_id`".to_string());
        }
        // reject-don't-ignore, pinned by the proxy's incomplete-config fail
        // path: a config that is neither a program launch (no `program`) nor
        // an attach is the test lane, and the test lane's target is
        // `fullyQualifiedName` — a config without one names nothing to run.
        // The locator's launch-time hint pair (LOC-0128's door shape:
        // `projectPath` and `cwd`, both non-empty) is the one nameless shape
        // that does name a target — discovery's root (LOC-0129 resolves it
        // at launch). Half a pair names no directory, so it is no pair.
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

/// The knob read at its from_env boundary.
pub fn launch_mode_from_env() -> Option<LaunchMode> {
    launch_mode_from_knob(std::env::var(ZATARA_LAUNCH_MODE).ok().as_deref())
}

#[cfg(test)]
mod hint_pair_tests {
    use super::*;

    /// The door's launch-time hint pair (LOC-0128's shape): a root named by
    /// `projectPath` beside a `cwd` — no test target, no program, no
    /// request. Discovery resolves the program at launch (LOC-0129), so the
    /// pair is a valid config, not the nameless test lane the fail path
    /// rejects.
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

    /// The incomplete-config fail path stands for every shape that is
    /// neither a program launch, an attach, nor the full hint pair: one
    /// half of the pair names no discovery root and no test target.
    #[test]
    fn a_config_shy_of_the_pair_is_still_rejected_naming_the_test_target() {
        for json in [
            // The shape pinned by `launch_config_program_mode`: a
            // projectPath alone, no cwd beside it.
            r#"{"projectPath": "C:/w/proj/CartModelTests.csproj"}"#,
            r#"{"cwd": "C:/work/app"}"#,
            // An empty pair member names no directory — no pair, no pass.
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

    /// The inference arms the other lanes rely on, pinned here beside the
    /// new one so the three can never drift apart: bare resolves Test (the
    /// knob-forced program-less fail rests on this), an FQN resolves Test,
    /// a program resolves Project.
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
