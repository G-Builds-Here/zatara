//! Acceptance tests for the DAP relay contract (LOC-0106 AC2): bodies the
//! proxy forwards must survive deserialize -> re-serialize with every field
//! and value intact, including fields zatara-types has never seen.

use std::fs;

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

use zatara_types::{OutputEventBody, SetBreakpointsArguments, SetBreakpointsResponseBody};

fn fixture(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/dap")
        .join(name);
    fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("fixture {} must be committed: {e}", path.display()))
}

/// The roundtrip the relay performs: deserialize the body as T, re-serialize.
fn relay_as_text<T>(raw: &str, what: &str) -> String
where
    T: DeserializeOwned + Serialize,
{
    let parsed: T =
        serde_json::from_str(raw).unwrap_or_else(|e| panic!("{what} must deserialize: {e}"));
    serde_json::to_string(&parsed).unwrap_or_else(|e| panic!("{what} must re-serialize: {e}"))
}

fn relay_as_value<T>(raw: &str, what: &str) -> Value
where
    T: DeserializeOwned + Serialize,
{
    let text = relay_as_text::<T>(raw, what);
    serde_json::from_str(&text).expect("relayed text must be valid JSON")
}

#[test]
fn set_breakpoints_request_body_roundtrips_without_loss() {
    // Arrange
    let raw = fixture("set_breakpoints_request_body.json");
    let original: Value = serde_json::from_str(&raw).expect("fixture is valid JSON");

    // Act
    let relayed = relay_as_value::<SetBreakpointsArguments>(&raw, "setBreakpoints request body");

    // Assert
    assert_eq!(
        relayed, original,
        "every field and value must survive the relay"
    );
}

#[test]
fn output_event_body_roundtrips_without_loss() {
    // Arrange
    let raw = fixture("output_event_body.json");
    let original: Value = serde_json::from_str(&raw).expect("fixture is valid JSON");

    // Act
    let relayed = relay_as_value::<OutputEventBody>(&raw, "output event body");

    // Assert
    assert_eq!(
        relayed, original,
        "every field and value must survive the relay"
    );
}

#[test]
fn set_breakpoints_response_body_preserves_unknown_field() {
    // Arrange — fixture carries `futureField`, a key zatara-types has never seen
    let raw = fixture("set_breakpoints_response_body_unknown_field.json");
    let original: Value = serde_json::from_str(&raw).expect("fixture is valid JSON");

    // Act
    let relayed_text =
        relay_as_text::<SetBreakpointsResponseBody>(&raw, "response body with unknown field");
    let relayed: Value =
        serde_json::from_str(&relayed_text).expect("relayed text must be valid JSON");

    // Assert — the unknown field's name and value bytes pass through unchanged
    assert!(
        relayed_text.contains("\"futureField\":{\"addedIn\":\"netcoredbg-0.20\"}"),
        "unknown field must be preserved byte-equivalent, got: {relayed_text}"
    );
    assert_eq!(
        relayed, original,
        "every field and value must survive the relay"
    );
}
