//! Processes on this machine: liveness, sessions, signals and their listening ports.
//!
//! A task is everything in its session: Satie starts each one with `setsid`, so the session id is
//! the task's pid. Process groups are no good for this, because tools like process-compose or a
//! shell with job control put each child in a group of its own (but leave it in the session).
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

/// `(state, start time in ticks)` from `/proc/<pid>/stat`, if the process exists.
pub(crate) fn proc_stat(pid: u32) -> Option<(char, u64)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name is in parentheses and may contain spaces; fields follow the last `)`.
    let rest = &stat[stat.rfind(')')? + 2..];
    let fields: Vec<&str> = rest.split(' ').collect();
    Some((
        fields.first()?.chars().next()?,
        fields.get(19)?.parse().ok()?,
    ))
}

pub(crate) fn parent_of(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = &stat[stat.rfind(')')? + 2..];
    rest.split(' ').nth(1)?.parse().ok() // field 4, ppid
}

pub(crate) fn session_of(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = &stat[stat.rfind(')')? + 2..];
    rest.split(' ').nth(3)?.parse().ok() // field 6, session
}

/// Every live (non-zombie) process in session `sid`.
pub(crate) fn pids_in_session(sid: u32) -> Vec<u32> {
    let Ok(procs) = std::fs::read_dir("/proc") else {
        return vec![];
    };
    procs
        .filter_map(Result::ok)
        .filter_map(|p| p.file_name().to_str()?.parse::<u32>().ok())
        .filter(|pid| session_of(*pid) == Some(sid))
        .filter(|pid| proc_stat(*pid).is_some_and(|(state, _)| state != 'Z'))
        .collect()
}

pub(crate) fn task_alive(pid: u32, ticks: Option<u64>) -> bool {
    match proc_stat(pid) {
        Some((state, t)) => state != 'Z' && ticks.is_none_or(|want| want == t),
        None => false,
    }
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

/// Listening TCP ports of every process in session `sid` (Linux; empty elsewhere).
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
