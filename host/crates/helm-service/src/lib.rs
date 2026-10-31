//! HelmService: a Windows service that keeps the Helm agent running in the
//! active console session.
//!
//! Running as LocalSystem, the service asks for the token of the user signed
//! in to the console session (`WTSGetActiveConsoleSessionId`,
//! `WTSQueryUserToken`), starts the agent as that user on their desktop
//! (`CreateProcessAsUserW`), restarts it with exponential backoff when it
//! exits, and moves it when the console session changes (logon, logoff,
//! fast user switching).
//!
//! The decision logic ([`supervisor`], [`backoff`], [`cmdline`], [`cli`]) is
//! pure and platform-independent so it can be unit-tested; the OS calls live
//! in the Windows-only `win` module.

pub mod backoff;
pub mod cli;
pub mod cmdline;
pub mod supervisor;

#[cfg(windows)]
pub mod win;

/// SCM service name.
pub const SERVICE_NAME: &str = "HelmService";
/// Name shown in services.msc.
pub const DISPLAY_NAME: &str = "Helm Agent Service";
/// Agent executable expected next to `helm-service.exe` by default.
pub const AGENT_EXE: &str = "helm-agent.exe";
