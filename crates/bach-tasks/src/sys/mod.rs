//! What the OS tells us about processes and sockets, one implementation per platform.
//!
//! - `stat(pid)`: the status of a process, if it exists (zombies included)
//! - `all_pids()`: every process id on the machine
//! - `argv(pid)`, `name(pid)`, `cwd(pid)`, `age_secs(pid)`: what it runs, where, since when
//! - `socket_owners()`: `(port, owning pid)` for every listening TCP socket; the pid is None for
//!   sockets whose owner we may not inspect
//!
//! Every function answers "don't know" (None, empty) for a process that is gone or that we may
//! not inspect.

/// The parts of a process's status bach-tasks needs.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Stat {
    pub zombie: bool,
    pub ppid: u32,
    pub sid: u32,
    /// When it started, in a platform unit (clock ticks since boot on Linux, microseconds since
    /// the epoch on macOS). Only ever compared for equality, to tell a reused pid apart.
    pub start: u64,
}

#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod imp;
#[cfg(target_os = "macos")]
#[path = "macos.rs"]
mod imp;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
#[path = "other.rs"]
mod imp;

pub(crate) use imp::{age_secs, all_pids, argv, cwd, name, socket_owners, stat};
