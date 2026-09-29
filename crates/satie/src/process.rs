//! Processes on this machine: liveness, process groups, signals and their listening ports.
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

pub(crate) fn process_group_of(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = &stat[stat.rfind(')')? + 2..];
    rest.split(' ').nth(2)?.parse().ok() // field 5, pgrp
}

pub(crate) fn task_alive(pid: u32, ticks: Option<u64>) -> bool {
    match proc_stat(pid) {
        Some((state, t)) => state != 'Z' && ticks.is_none_or(|want| want == t),
        None => false,
    }
}

pub(crate) fn signal_group(pgid: u32, sig: i32) {
    // SAFETY: plain kill(2) on a negative pid, i.e. the whole process group.
    unsafe {
        libc::kill(-(pgid as i32), sig);
    }
}

/// Listening TCP ports of every process in group `pgid` (Linux; empty elsewhere).
pub(crate) fn ports_of_group(pgid: u32) -> Vec<u16> {
    let mut group_of: HashMap<u32, Option<u32>> = HashMap::new();
    let mut ports: Vec<u16> = crate::probe::socket_owners()
        .into_iter()
        .filter_map(|(port, pid)| {
            let pid = pid?;
            (*group_of.entry(pid).or_insert_with(|| process_group_of(pid)) == Some(pgid))
                .then_some(port)
        })
        .collect();
    ports.sort_unstable();
    ports.dedup();
    ports
}
