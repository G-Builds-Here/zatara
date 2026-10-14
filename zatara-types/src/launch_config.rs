use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The one launch-config contract that crosses the extension/proxy boundary
/// (survey anti-pattern 8: duplicate config contracts — this is the copy
/// neither consumer is allowed to make). Wire shape is camelCase; the
/// extension side writes it, the proxy side reads it (SCHEMA_EQ).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchConfig {
    pub project_path: String,
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
    /// Test-results capture (owner triage D27): `true` makes the proxy append
    /// dotnet's own `--logger trx` to the test step, so each run writes a
    /// `.trx` results file under `TestResults/`. Off by default — absent or
    /// `false` serializes as an omitted key, and a default config's wire
    /// bytes are byte-for-byte the pre-D27 bytes.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub results_log: bool,
}
