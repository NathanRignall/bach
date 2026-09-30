//! Linux: everything comes from `/proc`.
use super::Stat;
use std::{collections::HashMap, path::PathBuf};

pub(crate) fn stat(pid: u32) -> Option<Stat> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name is in parentheses and may contain spaces; fields follow the last `)`.
    let rest = &stat[stat.rfind(')')? + 2..];
    let f: Vec<&str> = rest.split(' ').collect();
    Some(Stat {
        zombie: f.first()?.starts_with('Z'),
        ppid: f.get(1)?.parse().ok()?,   // field 4
        sid: f.get(3)?.parse().ok()?,    // field 6
        start: f.get(19)?.parse().ok()?, // field 22, clock ticks since boot
    })
}

pub(crate) fn all_pids() -> Vec<u32> {
    let Ok(procs) = std::fs::read_dir("/proc") else {
        return vec![];
    };
    procs
        .filter_map(Result::ok)
        .filter_map(|p| p.file_name().to_str()?.parse::<u32>().ok())
        .collect()
}

pub(crate) fn argv(pid: u32) -> Vec<String> {
    let raw = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
    raw.split(|b| *b == 0)
        .filter(|a| !a.is_empty())
        .map(|a| String::from_utf8_lossy(a).into_owned())
        .collect()
}

pub(crate) fn name(pid: u32) -> Option<String> {
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    Some(comm.trim().to_string()).filter(|c| !c.is_empty())
}

pub(crate) fn cwd(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
}

pub(crate) fn age_secs(pid: u32) -> Option<u64> {
    let ticks = stat(pid)?.start;
    let uptime: f64 = std::fs::read_to_string("/proc/uptime")
        .ok()?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    // SAFETY: sysconf has no preconditions.
    let hz = match unsafe { libc::sysconf(libc::_SC_CLK_TCK) } {
        hz if hz > 0 => hz as f64,
        _ => 100.0,
    };
    Some((uptime - ticks as f64 / hz).max(0.0) as u64)
}

pub(crate) fn socket_owners() -> Vec<(u16, Option<u32>)> {
    // Listening sockets: inode -> port.
    let mut listening: HashMap<u64, u16> = HashMap::new();
    for table in ["/proc/net/tcp", "/proc/net/tcp6"] {
        let Ok(text) = std::fs::read_to_string(table) else {
            continue;
        };
        for line in text.lines().skip(1) {
            let f: Vec<&str> = line.split_whitespace().collect();
            // f[1] local address "HEXIP:HEXPORT", f[3] state (0A = LISTEN), f[9] socket inode.
            if f.len() > 9 && f[3] == "0A" {
                let port = f[1]
                    .rsplit(':')
                    .next()
                    .and_then(|p| u16::from_str_radix(p, 16).ok());
                if let (Some(port), Ok(inode)) = (port, f[9].parse::<u64>()) {
                    listening.insert(inode, port);
                }
            }
        }
    }
    // Which process holds each of those inodes.
    let mut owner: HashMap<u64, u32> = HashMap::new();
    for pid in all_pids() {
        let Ok(fds) = std::fs::read_dir(format!("/proc/{pid}/fd")) else {
            continue;
        };
        for fd in fds.filter_map(Result::ok) {
            let Ok(target) = std::fs::read_link(fd.path()) else {
                continue;
            };
            let inode = target
                .to_str()
                .and_then(|t| t.strip_prefix("socket:["))
                .and_then(|t| t.trim_end_matches(']').parse::<u64>().ok());
            if let Some(i) = inode.filter(|i| listening.contains_key(i)) {
                owner.entry(i).or_insert(pid);
            }
        }
    }
    listening
        .iter()
        .map(|(inode, port)| (*port, owner.get(inode).copied()))
        .collect()
}
