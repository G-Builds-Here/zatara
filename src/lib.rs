//! zatara — Zed extension crate for .NET gutter test run/debug (thin wasm glue).
//!
//! Per DR-1/DR-7 the extension side only does locator scenarios, config
//! translation, and binary resolution; all process work lives in the native
//! zatara-proxy adapter. The trait methods below are thin shells over pure
//! builders (`locator`, `adapter`) — the builders are what the acceptance
//! suites drive, and the only operations they are allowed are path math and
//! JSON shape.

// The launch-config contract lives in zatara-types (SCHEMA_EQ: one definition,
// extension -> types <- proxy). Importing it here — rather than mirroring the
// struct — is what keeps the two sides of the process boundary from drifting.
pub use zatara_types::LaunchConfig;
use zed_extension_api::{
    DebugAdapterBinary, DebugScenario, DebugTaskDefinition, Extension, StartDebuggingRequestArgumentsRequest,
    TaskTemplate, Worktree, register_extension,
};

pub mod adapter;
pub mod binary_manager;
pub mod locator;

/// The extension. No state: every answer is computed from the request.
#[derive(Default)]
pub struct Zatara;

impl Extension for Zatara {
    fn new() -> Self {
        Self
    }

    /// Phase one of the gutter debug flow: the task is turned into a debug
    /// scenario whose `config` is the complete LOC-0106 launch config, so no
    /// build task is needed (epic DR-2 — the proxy owns build and test). If
    /// the locator-without-build-task spike fails on a Zed version, the
    /// documented answer is `locator::echo_fallback_scenario` — same
    /// scenario, plus a static no-op echo build step.
    fn dap_locator_create_scenario(
        &mut self,
        _locator_name: String,
        build_task: TaskTemplate,
        resolved_label: String,
        debug_adapter_name: String,
    ) -> Option<DebugScenario> {
        locator::native_scenario(&build_task, resolved_label, debug_adapter_name)
    }

    /// Zed probes this before starting a session whose scenario carries no
    /// build task (every gutter Debug we emit — DR-2). An error here makes Zed
    /// abort with "cannot determine how to run this debug scenario", so this
    /// must answer, never decline. The answer is always Launch: this adapter's
    /// config names a project and a test to run, and the proxy launches that
    /// process itself (build, paused test, attach are the proxy's own steps).
    /// No config we produce or accept can mean Attach.
    fn dap_request_kind(
        &mut self,
        _adapter_name: String,
        _config: serde_json::Value,
    ) -> Result<StartDebuggingRequestArgumentsRequest, String> {
        Ok(StartDebuggingRequestArgumentsRequest::Launch)
    }

    /// What to spawn as the `csharp` debug adapter: the resolved zatara-proxy
    /// command, the resolved netcoredbg path in its documented argv slot.
    /// A resolution failure is the resolver's message, verbatim — never an
    /// invented command (AC2).
    fn get_dap_binary(
        &mut self,
        _adapter_name: String,
        config: DebugTaskDefinition,
        user_provided_debug_adapter_path: Option<String>,
        _worktree: &Worktree,
    ) -> Result<DebugAdapterBinary, String> {
        adapter::dap_binary_for(&config, user_provided_debug_adapter_path.as_deref())
    }
}

register_extension!(Zatara);
