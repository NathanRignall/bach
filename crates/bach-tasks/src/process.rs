//! Processes on this machine: liveness, sessions, signals and their listening ports.
//!
//! A task is everything in its session: bach-tasks starts each one with `setsid`, so the session id is
//! the task's pid. Process groups are no good for this, because tools like process-compose or a
//! shell with job control put each child in a group of its own (but leave it in the session).
use crate::sys;
use std::{
    collections::HashMap,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

/// Expands a leading `~` to the home directory.
pub(crate) fn expand_home(path: &str) -> PathBuf {
    let home = || std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from);
    match path {
        "~" => home(),
        p => match p.strip_prefix("~/") {
            Some(rest) => home().join(rest),
            None => PathBuf::from(p),
        },
    }
}

pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// When `pid` started (see [`sys::Stat::start`]), if it exists.
pub(crate) fn start_time(pid: u32) -> Option<u64> {
    sys::stat(pid).map(|s| s.start)
}

pub(crate) fn parent_of(pid: u32) -> Option<u32> {
    sys::stat(pid).map(|s| s.ppid)
}

pub(crate) fn session_of(pid: u32) -> Option<u32> {
    sys::stat(pid).map(|s| s.sid)
}

/// Every live (non-zombie) process in session `sid`.
pub(crate) fn pids_in_session(sid: u32) -> Vec<u32> {
    sys::all_pids()
        .into_iter()
        .filter(|pid| sys::stat(*pid).is_some_and(|s| s.sid == sid && !s.zombie))
        .collect()
}

pub(crate) fn task_alive(pid: u32, start: Option<u64>) -> bool {
    sys::stat(pid).is_some_and(|s| !s.zombie && start.is_none_or(|want| want == s.start))
}

/// Signals every process in session `sid`, whatever process group it moved itself into.
pub(crate) fn signal_session(sid: u32, sig: i32) {
    // The session leader's group first, so a supervisor hears about it before its children do.
    // SAFETY: plain kill(2); a negative pid is the whole process group.
    unsafe {
        libc::kill(-(sid as i32), sig);
    }
    for pid in pids_in_session(sid) {
        // SAFETY: plain kill(2) on one process.
        unsafe {
            libc::kill(pid as i32, sig);
        }
    }
}

/// Listening TCP ports of every process in session `sid`.
pub(crate) fn ports_of_session(sid: u32) -> Vec<u16> {
    let mut session: HashMap<u32, Option<u32>> = HashMap::new();
    let mut ports: Vec<u16> = crate::probe::socket_owners()
        .into_iter()
        .filter_map(|(port, pid)| {
            let pid = pid?;
            (*session.entry(pid).or_insert_with(|| session_of(pid)) == Some(sid)).then_some(port)
        })
        .collect();
    ports.sort_unstable();
    ports.dedup();
    ports
}
