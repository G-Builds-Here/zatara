//! The argv contract crossing the extension/proxy boundary (LOC-0114): the
//! extension's `get_dap_binary` answer carries the resolved netcoredbg path
//! in the proxy's argv as this flag pair, and the proxy consumes it from
//! exactly here. One definition, same law as the launch-config shape
//! (SCHEMA_EQ) — the two sides of the process boundary cannot drift on it.

/// Flag introducing the resolved netcoredbg path in the proxy's argv:
/// `[zatara-proxy, --netcoredbg, <path>]`.
pub const NETCOREDBG_ARG: &str = "--netcoredbg";

/// Env var through which `get_dap_binary` hands the proxy the extension work
/// dir (the directory the host `chdir`ed the guest into — LOC-0110's binary
/// cache lives there). The extension fills `DebugAdapterBinary.envs` with it
/// unconditionally; the proxy reads it to resolve where the *only* sanctioned
/// log locations are (LOC-0117: diagnostics files land here and nowhere
/// else). Handing the path over is plumbing, not a gate: writing anything is
/// gated separately, default-off.
pub const WORK_DIR_ENV: &str = "ZATARA_WORK_DIR";

/// Env var through which a degraded session's reason *could* travel to the
/// proxy. **Reserved, set by nothing today:** the owner ruling (2026-10-13)
/// makes the unreachable-release note a log fact only — the extension logs
/// it to guest stderr (Zed's log) and keeps the session Console clean, so it
/// no longer fills this seam. A future proxy release that consumes it must
/// log it, not event it.
pub const RELEASE_WARNING_ENV: &str = "ZATARA_RELEASE_WARNING";
