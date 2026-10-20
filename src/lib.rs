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

    /// Phase one of the debug flow, two lanes: a gutter task (exact-FQN
    /// filter env) is turned into a scenario whose `config` is the complete
    /// LOC-0106 launch config, so no build task is needed (epic DR-2 — the
    /// proxy owns build and test); the shipped program-build door
    /// (`ZATARA_PROGRAM_BUILD=1`, LOC-0127) is turned into the program-mode
    /// hint pair, resolved at launch (LOC-0129). Anything else is not
    /// claimed. If the locator-without-build-task spike fails on a Zed
    /// version, the documented answer is `locator::echo_fallback_scenario`
    /// — same scenario, plus a static no-op echo build step.
    fn dap_locator_create_scenario(
        &mut self,
        _locator_name: String,
        build_task: TaskTemplate,
        resolved_label: String,
        debug_adapter_name: String,
    ) -> Option<DebugScenario> {
        locator::native_scenario(&build_task, resolved_label.clone(), debug_adapter_name.clone())
            .or_else(|| locator::program_scenario(&build_task, resolved_label, debug_adapter_name))
    }

    /// Zed probes this before starting a session whose scenario carries no
    /// build task (every Debug we emit — DR-2). An error here makes Zed
    /// abort with "cannot determine how to run this debug scenario", so
    /// this must answer, never decline (LOC-0128 AC3: Err aborts the whole
    /// scenario — forbidden). The answer is read from the raw config, never
    /// from a `LaunchConfig` parse (shapes the proxy will reject later — an
    /// attach without a pid — still get their kind here, because declining
    /// at this gate would abort what the proxy's fail-loud path is supposed
    /// to name).
    ///
    /// This probe is a gate, not the wire: the DAP command Zed actually
    /// sends is the kind carried in `get_dap_binary`'s request_args
    /// (measured live at the T3 clean-room session, Zed
    /// 1.24.2+stable.373 — the probe alone never routes an attach). The
    /// two answers must therefore never disagree, and they cannot: both
    /// read through `adapter::request_kind_of` — `request: "attach"`
    /// answers Attach (manual-pid attach, LOC-0130's lane), everything
    /// else answers Launch.
    fn dap_request_kind(
        &mut self,
        _adapter_name: String,
        config: serde_json::Value,
    ) -> Result<StartDebuggingRequestArgumentsRequest, String> {
        Ok(adapter::request_kind_of(&config))
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
