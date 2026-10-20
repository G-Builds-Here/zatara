//! Locator eligibility + scenario builder (LOC-0114 AC1, LOC-0128).
//!
//! Two claims, two shipped env tags, no inference between them. A zatara
//! gutter action is recognisable by one env pair: the exact-FQN filter
//! tasks.json ships in `ZATARA_TEST_FILTER` (LOC-0105's tag/env contract).
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
    build_task
        .env
        .iter()
        .find(|(key, _)| key == TEST_FILTER_ENV)
        .and_then(|(_, value)| value.strip_prefix(FILTER_PREFIX))
        .filter(|fqn| !fqn.is_empty())
        .map(str::to_string)
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
    let fully_qualified_name = parse_test_filter(build_task)?;
    let cwd = build_task.cwd.clone()?;
    let project_path = project_path_from_dir(&cwd)?;
    Some(LaunchConfig {
        project_path,
        fully_qualified_name,
        cwd,
        env: Some(build_task.env.iter().cloned().collect()),
        // A gutter action is a method-scope click on an exact FQN: absent
        // scope means `method` and absent framework means `xunit` on the
        // wire, and the case-only argument vectors are empty — all four
        // serialize as omitted keys, so the emitted config stays the
        // four-field LOC-0106 shape AC1 quotes. A gutter session never
        // opts into the trx capture (D27): false serializes as an omitted
        // key, and only a hand-written scenario flips resultsLog.
        scope: None,
        framework: None,
        case_arguments: Vec::new(),
        method_parameters: Vec::new(),
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
