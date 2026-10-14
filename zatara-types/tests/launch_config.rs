//! Acceptance tests for the launch-config wire contract (LOC-0106 AC1):
//! camelCase keys, exact serialized shape, env omitted when unset.

use std::collections::BTreeMap;

use zatara_types::LaunchConfig;

fn sample_config(env: Option<BTreeMap<String, String>>) -> LaunchConfig {
    LaunchConfig {
        project_path: "C:/w/proj/proj.csproj".to_string(),
        fully_qualified_name: "Unit.Tests.CalcTests.AddsOneAndTwo".to_string(),
        cwd: "C:/w/proj".to_string(),
        env,
        // Scope fields unset: the byte-identity assertions below are the
        // proof that a pre-scope config still serializes exactly as before.
        ..Default::default()
    }
}

#[test]
fn serializes_populated_launch_config_to_exact_camel_case_json() {
    // Arrange
    let env = BTreeMap::from([("DOTNET_FILE".to_string(), "C:/w/proj/Tests.cs".to_string())]);
    let config = sample_config(Some(env));

    // Act
    let json = serde_json::to_string(&config).expect("launch config must serialize");

    // Assert — keys and order are literal per the AC's Then clause
    let expected = "{\"projectPath\":\"C:/w/proj/proj.csproj\",\
                    \"fullyQualifiedName\":\"Unit.Tests.CalcTests.AddsOneAndTwo\",\
                    \"cwd\":\"C:/w/proj\",\
                    \"env\":{\"DOTNET_FILE\":\"C:/w/proj/Tests.cs\"}}";
    assert_eq!(json, expected);
}

#[test]
fn results_log_is_off_by_default_and_never_appears_unasked() {
    // D27: the trx capture is a config option, OFF by default. A default
    // config's wire bytes must be exactly the pre-D27 bytes — the key is
    // omitted, never serialized as `false` or `null`.
    let json = serde_json::to_string(&sample_config(None)).expect("serializes");
    assert!(!json.contains("resultsLog"), "off means absent from the wire: {json}");
}

#[test]
fn results_log_on_carries_the_documented_key_both_ways() {
    // Extension side out…
    let config = LaunchConfig {
        results_log: true,
        ..sample_config(None)
    };
    let json = serde_json::to_string(&config).expect("serializes");
    assert!(
        json.contains("\"resultsLog\":true"),
        "the on-switch crosses the wire under the documented camelCase key: {json}"
    );
    // …and the hand-written debug.json side in (the proxy's parse).
    let hand: LaunchConfig = serde_json::from_str(
        "{\"projectPath\":\"p\",\"fullyQualifiedName\":\"f\",\"cwd\":\"c\",\
         \"resultsLog\":true}",
    )
    .expect("a hand-written scenario with resultsLog parses");
    assert!(hand.results_log);
}

#[test]
fn omits_env_key_when_env_is_unset() {
    // Arrange
    let config = sample_config(None);

    // Act
    let json = serde_json::to_string(&config).expect("launch config must serialize");

    // Assert — unset env is omitted, never serialized as null
    assert!(
        !json.contains("\"env\""),
        "unset env must not appear in the JSON, got: {json}"
    );
    assert!(
        !json.contains("null"),
        "no field may serialize as null, got: {json}"
    );
}
