//! zatara-types — the single definition of every cross-process contract in
//! the zatara system (blueprint DR: extension -> types <- proxy).
//!
//! The crate is dependency-light by design: serde wire shapes only, and it
//! imports nothing from either consumer so the extension (wasm) and the proxy
//! (native) can compile against the same types without dragging each other's
//! world across the boundary.

mod adapter_contract;
mod dap;
mod launch_config;

pub use adapter_contract::{NETCOREDBG_ARG, RELEASE_WARNING_ENV, WORK_DIR_ENV};
pub use dap::{
    Breakpoint, OutputEventBody, SetBreakpointsArguments, SetBreakpointsResponseBody, Source,
    SourceBreakpoint,
};
pub use launch_config::LaunchConfig;
