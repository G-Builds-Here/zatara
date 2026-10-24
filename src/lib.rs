pub use zatara_types::LaunchConfig;
use zed_extension_api::{
    DebugAdapterBinary, DebugScenario, DebugTaskDefinition, Extension, StartDebuggingRequestArgumentsRequest,
    TaskTemplate, Worktree, register_extension,
};
pub mod adapter;
pub mod binary_manager;
pub mod locator;
#[derive(Default)]
pub struct Zatara;
impl Extension for Zatara {
    fn new() -> Self {
        Self
    }
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
    fn dap_request_kind(
        &mut self,
        _adapter_name: String,
        config: serde_json::Value,
    ) -> Result<StartDebuggingRequestArgumentsRequest, String> {
        Ok(adapter::request_kind_of(&config))
    }
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
