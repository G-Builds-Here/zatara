//! DAP relay message structs — the bodies the proxy forwards across the
//! process boundary without loss (LOC-0106 AC2).
//!
//! Every struct carries a flattened `extra` map: a field zatara-types has
//! never seen must survive deserialize -> re-serialize byte-equivalent, so an
//! adapter update can never break the relay (forward compatibility). The
//! message envelope (`seq`/`type`/`command` routing) is LOC-0107's relay
//! behavior; this crate defines the payload contracts only.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// A DAP `Source`, as forwarded in breakpoint requests and output events.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_reference: Option<i64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// One requested breakpoint (request side — a line the caller asks to bind).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceBreakpoint {
    pub line: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<i64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `arguments` of a `setBreakpoints` request.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetBreakpointsArguments {
    pub source: Source,
    pub breakpoints: Vec<SourceBreakpoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_modified: Option<bool>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `body` of an `output` event — the stream Zed's Console tab renders.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputEventBody {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    pub output: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variables_reference: Option<i64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// One bound breakpoint (response side — what the adapter verified).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Breakpoint {
    pub verified: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instruction_reference: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<i64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `body` of a `setBreakpoints` response.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetBreakpointsResponseBody {
    pub breakpoints: Vec<Breakpoint>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
