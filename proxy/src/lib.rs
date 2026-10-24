pub mod asset;
pub mod discovery;
pub mod framing;
pub(crate) mod logging;
pub mod macos_netcoredbg;
pub(crate) mod mtp_case;
pub(crate) mod program;
pub mod runner;
pub mod relay;
pub(crate) mod runcli;
pub mod session;
use std::process::ExitCode;
pub fn run() -> ExitCode {
    match relay::serve(relay::AdapterSpec::from_env(), Box::new(session::NullSink)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("zatara-proxy: {e}");
            ExitCode::FAILURE
        }
    }
}
pub fn run_case() -> ExitCode {
    runcli::run_from_env()
}
