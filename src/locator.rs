//! Locator eligibility + scenario builder (LOC-0114 AC1, LOC-0128).
//!
//! Two claims, two shipped env tags, no inference between them. A zatara
//! gutter action is recognisable by one env pair: the exact-FQN filter
//! tasks.json ships in `ZATARA_TEST_FILTER` (LOC-0105's tag/env contract).
//! A case-row task is the same pair plus the exact `ZATARA_TEST_CASE=1`
//! scope tag and its numbered arg/param envs — the wire then carries
//! `scope: "case"` and the proxy's grammar table owns the filter (2026-10-21
//! case-row Debug).
//! The program-build door is recognisable by the other: the exact
//! `ZATARA_PROGRAM_BUILD=1` pair LOC-0127's template ships (DR-3: exact tag
//! only — near-miss values and other env claim nothing). Everything else
//! here is path math and JSON shape — the only two things DR-7 lets this
//! crate do.
//!
//! The scenario carries `build: None` (epic DR-2): the proxy builds and
//! tests inside its own pty, so the scenario's `config` is the LOC-0106
//! launch config, written exactly as `zatara_types::LaunchConfig` serializes
//! it — never a hand-assembled copy.
//!
//! Spike (Zed 1.23, unresolvable in CI): if a locator without a build task
//! never fires, `echo_fallback_scenario` is the documented answer — the same
//! scenario with a static echo build task attached, so nothing else about
//! the session changes.

use zed_extension_api::{
    BuildTaskDefinition, BuildTaskDefinitionTemplatePayload, DebugScenario, TaskTemplate,
};
use zatara_types::LaunchConfig;

/// The locator this extension registers (echoed back in fallback build
/// tasks so phase two returns to us).
pub const LOCATOR_NAME: &str = "zatara";
/// The env key LOC-0105's tasks.json carries the exact-FQN filter in.
pub const TEST_FILTER_ENV: &str = "ZATARA_TEST_FILTER";
/// The only filter form this locator answers — an exact match, never a
/// substring (the survey V3 defect class).
pub const FILTER_PREFIX: &str = "FullyQualifiedName=";
/// The fixed no-op line the documented fallback build step echoes. It is
/// the only observable difference between the two scenario shapes.
pub const ECHO_BUILD_LINE: &str = "zatara: no build step - the proxy owns build and test";

/// Extracts the exact-FQN filter value, or `None` when the task is not a
/// zatara gutter action (no filter env) or not an exact filter.
pub fn parse_test_filter(build_task: &TaskTemplate) -> Option<String> {
    env_value(build_task, TEST_FILTER_ENV)
        .and_then(|value| value.strip_prefix(FILTER_PREFIX))
        .filter(|fqn| !fqn.is_empty())
        .map(str::to_string)
}

/// The env key the shipped case-row tasks carry their scope tag in
/// (DR-3 exact tag again: the string `1`, nothing else claims case scope).
pub const TEST_CASE_ENV: &str = "ZATARA_TEST_CASE";
/// The exact scope-tag value.
pub const TEST_CASE_TAG: &str = "1";
/// The env key carrying the framework name for a case-scoped launch
/// (`nunit` / `mstest` / `xunit`). The locator passes it through verbatim —
/// the filter grammar table lives proxy-side (`runner::build_filter`).
pub const TEST_FRAMEWORK_ENV: &str = "ZATARA_TEST_FRAMEWORK";
/// Env keys carrying one displayed case argument each (`…_1` … `…_3`).
pub const CASE_ARG_ENV_PREFIX: &str = "ZATARA_CASE_ARG_";
/// Env keys carrying one method parameter name each, zipped over the case
/// arguments to rebuild xunit's named-argument display.
pub const CASE_PARAM_ENV_PREFIX: &str = "ZATARA_CASE_PARAM_";
/// Env keys marking which case-argument slots hold a string literal
/// (`…_1` … `…_3`, shipped only by the InlineData-with-string tasks). The
/// value is the slot's content; only its presence is read — it decides,
/// proxy-side, whether the slot's filter part is the AND-substring law's
/// bare `DisplayName~<content>` (quotes never survive the filter transport)
/// or the classic `DisplayName~<param>: <value>`.
pub const CASE_STR_ENV_PREFIX: &str = "ZATARA_CASE_STR_";
/// Env keys carrying one InlineData value each as written in source
/// (`…_1` … `…_3`, shipped by the per-arity generic tasks, where the key
/// exists for every slot of the arity). The locator forwards the list
/// verbatim — at locator time each value is the literal `${ZED_CUSTOM_…}`
/// marker text (the law documented on `env_indices`), and Zed expands the
/// returned config on launch. Proxy-side, a value beginning with a double
/// quote marks that slot's string lane, which is why the generic templates
/// never need the per-shape STR markers.
pub const CASE_RAW_ENV_PREFIX: &str = "ZATARA_CASE_RAW_";
/// The substring-FQN prefix a shipped case filter begins with — the case
/// filter's FQN segment ends at the first `&` (illegal in any FQN).
pub const CASE_FILTER_PREFIX: &str = "FullyQualifiedName~";

fn env_value<'a>(build_task: &'a TaskTemplate, key: &str) -> Option<&'a str> {
    build_task
        .env
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

/// The numbered env values for a prefix (`…_1`, `…_2`, …), ordered by the
/// numeric suffix, empty values dropped — a template for a smaller arity
/// simply carries fewer keys, never empty ones.
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

/// The numeric suffixes of a numbered env prefix the task ships — key
/// presence is the fact, an empty value is the only decline. Live proof
/// (owner round 2, 2026-10-21): Zed hands the locator the UNRESOLVED task
/// template and expands the RETURNED config afterwards — the same pass
/// `project_path_from_dir` documents for `$ZED_` paths — so a gutter
/// marker's value always travels as its literal `${ZED_CUSTOM_…}` text.
/// Rejecting braces silently declined every live string-slot marker (the
/// Mixed3 `No test matches` defect). A shipped template carries
/// `ZATARA_CASE_STR_N` only when the matched pattern asserted a string
/// slot: no capture, no task — the braces are the capture speaking, not a
/// hole. Ordered by suffix.
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

/// The case-scope FQN: the substring-filter segment before the first `&`.
/// A case-tagged task whose filter is not in the shipped shape is declined,
/// never guessed.
pub fn parse_case_filter(build_task: &TaskTemplate) -> Option<String> {
    env_value(build_task, TEST_FILTER_ENV)
        .and_then(|value| value.strip_prefix(CASE_FILTER_PREFIX))
        .map(|rest| rest.split('&').next().unwrap_or(rest).to_string())
        .filter(|fqn| !fqn.is_empty())
}

/// Whether this task claims case scope: the exact tag only.
pub fn claims_case(build_task: &TaskTemplate) -> bool {
    env_value(build_task, TEST_CASE_ENV) == Some(TEST_CASE_TAG)
}

/// The case-scoped wire parts, or `None` to decline. A case launch with no
/// captured arguments is declined on purpose: the proxy's rebuilt body would
/// be the empty substring, which matches every row of the method — a
/// silently-wrong filter, the defect class this epic kills.
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

/// Derives the project file path from the gutter task's working directory:
/// a .NET project directory is named after its project file. The directory's
/// own separator style is preserved (a task written with forward slashes
/// never comes back with backslashes).
///
/// A raw task template gets NO path math (live defect, owner 2026-10-13):
/// Zed hands the locator the unresolved template (`cwd: "$ZED_DIRNAME"`) and
/// expands variables in the returned config itself. `"$X/$X.csproj"` would
/// expand BOTH occurrences at that later pass — the doubled-path
/// `…\proj.Tests/C:\…\proj.Tests.csproj` that `dotnet build` correctly
/// rejected (MSB1009). The single-occurrence passthrough is the only safe
/// shape: it expands once, into the task directory. The proxy owns the
/// directory→project-file step (`resolve_project_path`) — it is the one
/// side that ever sees resolved strings, and the one side with a filesystem.
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

/// The LOC-0106 launch config for one gutter task, or `None` when the task
/// is not locatable. The task's env travels with it verbatim.
pub fn launch_config(build_task: &TaskTemplate) -> Option<LaunchConfig> {
    // Two filter shapes, two claims (DR-3 exact tags decide which, no
    // inference): the exact `FullyQualifiedName=` method/class click, and
    // the shipped case-row task (`ZATARA_TEST_CASE=1` plus a
    // `FullyQualifiedName~…&…` substring filter). A case-tagged task whose
    // filter is not in the shipped shape is declined, never guessed.
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
        // A method-scope click keeps the pre-scope wire exactly: absent
        // scope means `method` and absent framework means `xunit` on the
        // wire, and the case-only argument vectors are empty — all four
        // serialize as omitted keys, so the emitted config stays the
        // four-field LOC-0106 shape AC1 quotes. A case-scope click carries
        // the four fields, and the proxy's `build_filter` case arm rebuilds
        // the row predicate from them (the grammar table lives there, not
        // here). A gutter session never opts into the trx capture (D27):
        // false serializes as an omitted key, and only a hand-written
        // scenario flips resultsLog.
        scope,
        framework,
        case_arguments,
        method_parameters,
        case_string_slots,
        // The generic per-arity tasks ship one RAW marker per slot; a classic
        // per-shape task ships none, and omit-if-empty keeps its wire bytes
        // unchanged (the same law `case_string_slots` follows above).
        case_raw_arguments,
        results_log: false,
        // The program-mode field set is absent too: a gutter action is
        // always a test-lane config, and every program-mode field is
        // omit-if-none — the emitted bytes stay the LOC-0109 shape.
        ..Default::default()
    })
}

/// The env key LOC-0127's shipped build door carries its eligibility tag in.
pub const PROGRAM_BUILD_ENV: &str = "ZATARA_PROGRAM_BUILD";
/// The exact tag value the door ships — the string `1`, never a truthy
/// spelling, never padded (DR-3: exact tag only, no shadowing).
pub const PROGRAM_BUILD_TAG: &str = "1";

/// The debug-side label every program scenario carries, and the label of
/// the claimable task entry itself. The shipped pair (Zed 1.24.x law,
/// measured live 2026-10-17): Zed deduplicates a spawned task out of the
/// fresh task list for the life of the project window, and the debug
/// scenario is derived only from a FRESH task — so the entry a user spawns
/// as a plain build (`zatara: build (workspace)`, untagged) must never be
/// the entry the locator claims. Sharing this label with the claimable task
/// is what makes one name across the task list and the debug picker; DR-3
/// still claims by the env tag alone, which now ships on the claimable
/// entry, not the plain-build entry (supersedes the same-day label-decouple
/// ruling — spawned-door dedup made the old shape self-defeating).
pub const PROGRAM_SCENARIO_LABEL: &str = "zatara: build & debug (workspace)";

/// Whether a task is the shipped program-build door: its env carries the
/// exact tag pair. Absent, near-miss, or differently-named env claims
/// nothing — eligibility is the tag, never an inference.
pub fn claims_program_build(build_task: &TaskTemplate) -> bool {
    build_task
        .env
        .iter()
        .any(|(key, value)| key == PROGRAM_BUILD_ENV && value == PROGRAM_BUILD_TAG)
}

/// Builds the program-mode scenario for a claimed door task: the launch-time
/// hint pair only — `projectPath` and `cwd` both carry the task's cwd (the
/// worktree root), with no path math and no template expansion (the
/// single-occurrence law from the gutter lane; `$ZED_WORKTREE_ROOT` passes
/// through and Zed expands it once, later). No `program` field: discovery
/// resolves the program at launch (LOC-0129). The task's env is NOT copied —
/// scenario creation resolves nothing (the echo-fallback / deferred-task
/// precedent), so no `${env:...}` literal can bake into the config.
/// A door task without a cwd has no root to hint and is declined, not
/// invented.
pub fn program_scenario(
    build_task: &TaskTemplate,
    _resolved_label: String,
    debug_adapter_name: String,
) -> Option<DebugScenario> {
    if !claims_program_build(build_task) {
        return None;
    }
    let root = build_task.cwd.clone()?;
    // The one contract definition serializes the hint (SCHEMA_EQ): an empty
    // FQN is key-absent, so the emitted config is exactly the pair.
    let config = LaunchConfig {
        project_path: root.clone(),
        cwd: root,
        ..Default::default()
    };
    Some(DebugScenario {
        // The debug-side name - the same name the claimable task entry
        // carries, so the picker and the task list agree.
        label: PROGRAM_SCENARIO_LABEL.into(),
        adapter: debug_adapter_name,
        build: None,
        config: serde_json::to_string(&config).ok()?,
        tcp_connection: None,
    })
}

/// Builds the native scenario: the config is already complete, so no build
/// task is attached (`build: None`).
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

/// The documented spike fallback: the same scenario with a trivial static
/// build task (echo of a fixed no-op line, zero env resolution) — used only
/// if core verification proves a locator does not fire without a build task.
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
