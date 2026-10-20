//! Acceptance tests for the program-mode launch-config contract:
//! program-shaped deserialization with DR-1 mode inference, the attach
//! shape's reject-don't-ignore law, the LOC-0109 test-lane key-absence law
//! under the new field set (byte-identity stays guarded by the unedited
//! pin in `tests/launch_config.rs`), and the ZATARA_LAUNCH_MODE knob
//! const/fn.

use std::collections::BTreeMap;

use zatara_types::{
    launch_mode_from_env, launch_mode_from_knob, LaunchConfig, LaunchMode, LaunchRequest,
    ZATARA_LAUNCH_MODE,
};

/// The canonical test-mode config of the LOC-0109 byte pin — the same field
/// values `tests/launch_config.rs` pins, with every program-mode field left
/// at its default.
fn canonical_test_mode_config() -> LaunchConfig {
    LaunchConfig {
        project_path: "C:/w/proj/proj.csproj".to_string(),
        fully_qualified_name: "Unit.Tests.CalcTests.AddsOneAndTwo".to_string(),
        cwd: "C:/w/proj".to_string(),
        env: Some(BTreeMap::from([(
            "DOTNET_FILE".to_string(),
            "C:/w/proj/Tests.cs".to_string(),
        )])),
        ..Default::default()
    }
}

#[test]
fn program_shaped_config_deserializes_and_infers_project_mode() {
    // Arrange — the program shape exactly as a community debug.json declares it
    let json = r#"{"program": "bin/Debug/net9.0/App.dll", "args": ["--verbose"], "cwd": "C:/work/app", "request": "launch"}"#;

    // Act
    let config: LaunchConfig =
        serde_json::from_str(json).expect("the program shape is a valid config");

    // Assert — every field carries its exact value (AC's Then clause) …
    assert_eq!(config.program.as_deref(), Some("bin/Debug/net9.0/App.dll"));
    assert_eq!(
        config.args.as_deref(),
        Some(["--verbose".to_string()].as_slice())
    );
    assert_eq!(config.cwd, "C:/work/app");
    assert_eq!(config.request, Some(LaunchRequest::Launch));
    // … and DR-1: program presence resolves the project lane.
    assert_eq!(config.resolved_mode(), LaunchMode::Project);
}

#[test]
fn test_shaped_config_infers_test_mode_with_program_fields_absent() {
    // Arrange — the gutter-test shape, program vocabulary never mentioned
    let json = r#"{"projectPath": "C:/work/app", "fullyQualifiedName": "App.Tests.One.Two"}"#;

    // Act
    let config: LaunchConfig =
        serde_json::from_str(json).expect("the test shape is a valid config");

    // Assert — no program means the test lane, and nothing program-shaped got filled
    assert_eq!(config.resolved_mode(), LaunchMode::Test);
    assert_eq!(config.program, None);
    assert_eq!(config.args, None);
    assert_eq!(config.request, None);
    assert_eq!(config.process_id, None);
}

#[test]
fn both_shaped_config_resolves_project_from_program_presence() {
    // Arrange — a config shaped like both modes at once
    let json = r#"{"program": "bin/App.dll", "fullyQualifiedName": "App.Tests.One.Two"}"#;

    // Act — presence decides, so this parses; no error (AC's Scenario)
    let config: LaunchConfig = serde_json::from_str(json)
        .expect("a config shaped like both modes is valid, not an error");

    // Assert
    assert_eq!(config.resolved_mode(), LaunchMode::Project);
}

#[test]
fn attach_shape_carries_the_request_and_the_pid() {
    // Arrange — manual-pid attach (LOC-0130's config shape)
    let json = r#"{"request": "attach", "process_id": 4712}"#;

    // Act
    let config: LaunchConfig =
        serde_json::from_str(json).expect("the attach shape is a valid config");

    // Assert
    assert_eq!(config.request, Some(LaunchRequest::Attach));
    assert_eq!(config.process_id, Some(4712));
}

#[test]
fn attach_without_a_pid_is_rejected_naming_the_missing_field() {
    // reject-don't-ignore: there is no silent default pid to fall back to
    let json = r#"{"request": "attach"}"#;

    let err =
        serde_json::from_str::<LaunchConfig>(json).expect_err("attach without a pid must not parse");

    assert!(
        err.to_string().contains("process_id"),
        "the failure names the field the user forgot, got: {err}"
    );
}

#[test]
fn a_test_lane_config_without_a_target_name_is_rejected() {
    // The proxy's incomplete-config fail path (pinned end-to-end in
    // `proxy_stdio`) rests on this: neither program nor attach means the test
    // lane, and the test lane names its target with fullyQualifiedName.
    let json = r#"{"projectPath": "C:/w/proj/CartModelTests.csproj"}"#;

    let err = serde_json::from_str::<LaunchConfig>(json)
        .expect_err("a config that names no program and no test has no target");

    assert!(
        err.to_string().contains("fullyQualifiedName"),
        "the failure names the missing test-lane target, got: {err}"
    );
}

#[test]
fn unset_program_and_pid_keys_are_absent_from_the_wire() {
    // Arrange — a config whose program-mode keys are None
    let config = canonical_test_mode_config();

    // Act
    let value = serde_json::to_value(&config).expect("serializes");
    let wire = serde_json::to_string(&config).expect("serializes");

    // Assert — key-absent: neither `present=false`-style keys nor null values
    let map = value.as_object().expect("the config serializes to a map");
    assert!(
        !map.contains_key("program"),
        "None program must be key-absent, got: {value}"
    );
    assert!(
        !map.contains_key("process_id"),
        "None process_id must be key-absent, got: {value}"
    );
    assert!(!wire.contains("null"), "no field may serialize as null, got: {wire}");
}

#[test]
fn knob_fn_maps_only_the_exact_values() {
    // env-knob law: exact values act, everything else defers to inference
    assert_eq!(launch_mode_from_knob(Some("project")), Some(LaunchMode::Project));
    assert_eq!(launch_mode_from_knob(Some("test")), Some(LaunchMode::Test));
    assert_eq!(
        launch_mode_from_knob(Some("Project")),
        None,
        "wrong case never coerces"
    );
    assert_eq!(
        launch_mode_from_knob(Some("both")),
        None,
        "a value that is neither lane never coerces"
    );
    assert_eq!(launch_mode_from_knob(None), None, "unset defers to inference");
}

#[test]
fn knob_reads_the_environment_at_its_boundary_and_never_reaches_the_wire() {
    // The only test that touches ZATARA_LAUNCH_MODE; the mutations are
    // sequential inside this one test fn.
    std::env::set_var(ZATARA_LAUNCH_MODE, "project");
    assert_eq!(
        launch_mode_from_env(),
        Some(LaunchMode::Project),
        "the exact knob value forces the project lane (DR-1 escape)"
    );

    // DR-1: the knob is an env escape, never a wire field — a serialized
    // config carries no trace of it.
    let wire = serde_json::to_string(&canonical_test_mode_config()).expect("serializes");
    assert!(
        !wire.contains(ZATARA_LAUNCH_MODE) && !wire.contains("launchMode"),
        "the knob never appears on the wire, got: {wire}"
    );

    std::env::set_var(ZATARA_LAUNCH_MODE, "Project");
    assert_eq!(launch_mode_from_env(), None, "wrong case never coerces at the boundary");
    std::env::set_var(ZATARA_LAUNCH_MODE, "both");
    assert_eq!(launch_mode_from_env(), None, "a non-lane value never coerces at the boundary");

    std::env::remove_var(ZATARA_LAUNCH_MODE);
    assert_eq!(launch_mode_from_env(), None, "unset defers to AC1 inference");
}
