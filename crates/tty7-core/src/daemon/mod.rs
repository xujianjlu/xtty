pub mod control;
pub mod duplex;
/// Upgrading the daemon in place, keeping the ptys and the shells on them.
///
/// Unix only: `execve` is what makes it possible.
#[cfg(unix)]
pub mod handoff;
pub mod history;
pub mod install;
pub mod pane;
pub mod pidfile;
pub mod procinfo;
pub mod protocol;
pub(crate) mod remote;
pub mod remote_link;
pub mod router;
pub mod scrollback;
pub mod server;
pub mod singleton;
pub mod spawn;
pub mod transport;

pub(crate) const DETECTED_SHELL_ENV: &str = "TTY7_DETECTED_SHELL";

pub(crate) mod shell_integration;
