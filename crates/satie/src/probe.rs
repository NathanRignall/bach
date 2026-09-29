//! Looking at the machine on behalf of Satie: who listens on which port, what a task's
//! processes are, whether a local URL answers, and reading log files in chunks.
//!
//! Linux (`/proc`); everything degrades to "nothing found" elsewhere.
use crate::process::{pids_in_session, proc_stat, session_of};
use satie_protocol::LogChunk;
use serde::Serialize;
use std::{
    collections::HashMap,
    io::{Read, Seek, SeekFrom},
    path::Path,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

// ---------------------------------------------------------------------------------------------
// Listening sockets and who owns them
// ---------------------------------------------------------------------------------------------

/// One listening TCP socket and (if we may see it) its owner.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Listener {
    pub port: u16,
    pub pid: Option<u32>,
    /// The owner's session; a Satie task leads its own session.
    pub sid: Option<u32>,
    pub command: String,
    pub cwd: Option<String>,
    /// Seconds since the owner started.
    pub up_secs: Option<u64>,
}

/// `(port, owning pid)` for every listening TCP socket. The pid is None for sockets whose owner
/// we are not allowed to inspect (another user's process).
pub fn socket_owners() -> Vec<(u16, Option<u32>)> {
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
    if let Ok(procs) = std::fs::read_dir("/proc") {
        for p in procs.filter_map(Result::ok) {
            let Some(pid) = p.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else {
                continue;
            };
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
    }
    let mut out: Vec<(u16, Option<u32>)> = listening
        .iter()
        .map(|(inode, port)| (*port, owner.get(inode).copied()))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

fn clock_ticks() -> f64 {
    // SAFETY: sysconf has no preconditions.
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if hz > 0 {
        hz as f64
    } else {
        100.0
    }
}

fn describe_process(pid: u32) -> (String, Option<String>, Option<u64>) {
    let raw = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
    let mut command = String::from_utf8_lossy(&raw)
        .replace('\0', " ")
        .trim()
        .to_string();
    if command.is_empty() {
        command = std::fs::read_to_string(format!("/proc/{pid}/comm"))
            .unwrap_or_default()
            .trim()
            .to_string();
    }
    if command.chars().count() > 200 {
        command = command.chars().take(200).collect::<String>() + "…";
    }
    let cwd = std::fs::read_link(format!("/proc/{pid}/cwd"))
        .ok()
        .map(|p| p.to_string_lossy().into_owned());
    let up_secs = proc_stat(pid).and_then(|(_, ticks)| {
        let uptime: f64 = std::fs::read_to_string("/proc/uptime")
            .ok()?
            .split_whitespace()
            .next()?
            .parse()
            .ok()?;
        Some((uptime - ticks as f64 / clock_ticks()).max(0.0) as u64)
    });
    (command, cwd, up_secs)
}

/// Every listening TCP socket on the machine, with what we can tell about its owner.
pub fn listeners() -> Vec<Listener> {
    type Described = (String, Option<String>, Option<u64>, Option<u32>);
    let mut cache: HashMap<u32, Described> = HashMap::new();
    socket_owners()
        .into_iter()
        .map(|(port, pid)| match pid {
            Some(pid) => {
                let (command, cwd, up_secs, sid) = cache
                    .entry(pid)
                    .or_insert_with(|| {
                        let (c, cwd, up) = describe_process(pid);
                        (c, cwd, up, session_of(pid))
                    })
                    .clone();
                Listener {
                    port,
                    pid: Some(pid),
                    sid,
                    command,
                    cwd,
                    up_secs,
                }
            }
            None => Listener {
                port,
                pid: None,
                sid: None,
                command: "(a process we can't inspect)".into(),
                cwd: None,
                up_secs: None,
            },
        })
        .collect()
}

/// Human-readable age: "45s", "12m", "3h 5m", "1d 5h".
pub fn age(secs: u64) -> String {
    match secs {
        s if s < 90 => format!("{s}s"),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86400 => format!("{}h {}m", s / 3600, (s % 3600) / 60),
        s => format!("{}d {}h", s / 86400, (s % 86400) / 3600),
    }
}

/// Names of the processes in a session, with counts ("workerd ×8"), most numerous first. The
/// wrapper shells Satie itself adds are left out.
pub fn process_summary(sid: u32) -> Vec<String> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for pid in pids_in_session(sid) {
        // The command as started ("node", "workerd"), not the kernel's 15-character thread name
        // (Node reports its threads as "MainThread"). Fall back to that when there's no command line.
        let raw = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
        let first = String::from_utf8_lossy(&raw)
            .split('\0')
            .next()
            .and_then(|a| a.split_whitespace().next())
            .and_then(|a| a.rsplit('/').next())
            .unwrap_or_default()
            .to_string();
        let comm = if first.is_empty() {
            std::fs::read_to_string(format!("/proc/{pid}/comm"))
                .unwrap_or_default()
                .trim()
                .to_string()
        } else {
            first
        };
        if !comm.is_empty() && comm != "sh" {
            *counts.entry(comm).or_default() += 1;
        }
    }
    let mut v: Vec<(String, usize)> = counts.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v.into_iter()
        .map(|(n, c)| if c > 1 { format!("{n} ×{c}") } else { n })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Port conflicts named in a log
// ---------------------------------------------------------------------------------------------

/// Numbers that could be ports (2–5 digits) on lines that say a port/address was already in use.
pub fn ports_named_in_conflicts(log: &str) -> Vec<u16> {
    const PHRASES: [&str; 4] = [
        "already in use",
        "eaddrinuse",
        "address in use",
        "address already in use",
    ];
    let mut ports = vec![];
    for line in log.lines() {
        let lower = line.to_lowercase();
        if !PHRASES.iter().any(|p| lower.contains(p)) {
            continue;
        }
        let bytes = line.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i].is_ascii_digit() {
                let start = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                // A standalone number, not part of a word or a longer identifier.
                let before = start.checked_sub(1).map(|j| bytes[j]);
                let after = bytes.get(i).copied();
                // Part of a word, or of an IP address / version number ("127.0.0.1", "5.2.1").
                let glued = |b: Option<u8>| {
                    b.is_some_and(|b| b.is_ascii_alphabetic() || b == b'_' || b == b'.')
                };
                if (2..=5).contains(&(i - start)) && !glued(before) && !glued(after) {
                    if let Ok(p) = line[start..i].parse::<u16>() {
                        ports.push(p);
                    }
                }
            } else {
                i += 1;
            }
        }
    }
    ports.sort_unstable();
    ports.dedup();
    ports
}

/// Text of a log to scan for start-up errors: the whole file if small, else both ends (errors
/// usually appear first, but a long-running process can also fail late).
pub fn log_for_scanning(path: &Path) -> String {
    const EDGE: u64 = 256 * 1024;
    let Ok(mut f) = std::fs::File::open(path) else {
        return String::new();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let read = |f: &mut std::fs::File, from: u64, n: u64| {
        let mut buf = Vec::new();
        f.seek(SeekFrom::Start(from)).ok();
        f.take(n).read_to_end(&mut buf).ok();
        String::from_utf8_lossy(&buf).into_owned()
    };
    if len <= 2 * EDGE {
        read(&mut f, 0, len)
    } else {
        let head = read(&mut f, 0, EDGE);
        head + "\n" + &read(&mut f, len - EDGE, EDGE)
    }
}

// ---------------------------------------------------------------------------------------------
// HTTP check (loopback only)
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub struct HttpProbe {
    pub status: u16,
    pub ms: u128,
}

/// Splits a `http://localhost:PORT/path` URL, refusing anything that is not plain http on this machine.
fn parse_local_url(url: &str) -> Result<(String, u16, String, String), String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or("Only plain http:// URLs on this machine can be checked.")?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
        let (h, tail) = v6.split_once(']').ok_or("Bad URL.")?;
        (
            h,
            tail.strip_prefix(':')
                .and_then(|p| p.parse::<u16>().ok())
                .unwrap_or(80),
        )
    } else {
        match authority.rsplit_once(':') {
            Some((h, p)) => (h, p.parse::<u16>().map_err(|_| "Bad port in URL.")?),
            None => (authority, 80),
        }
    };
    if !matches!(host, "localhost" | "127.0.0.1" | "::1") {
        return Err(format!("Only localhost can be checked, not `{host}`."));
    }
    Ok((
        host.to_string(),
        port,
        path.to_string(),
        authority.to_string(),
    ))
}

/// Checks that a URL is one `http_get` is willing to request, without requesting it.
pub fn validate_local_url(url: &str) -> Result<(), String> {
    parse_local_url(url).map(|_| ())
}

/// `GET` a local URL and report the status code. Plain `http://` on loopback only: this is a
/// health check for what the user is running, not a way for an agent to reach other hosts.
pub async fn http_get(url: &str, timeout: Duration) -> Result<HttpProbe, String> {
    let (host, port, path, authority) = parse_local_url(url)?;
    let host = host.as_str();
    let started = Instant::now();
    let attempt = async {
        let mut s = TcpStream::connect((host, port))
            .await
            .map_err(|e| e.to_string())?;
        let req = format!("GET {path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\nAccept: */*\r\nUser-Agent: satie\r\n\r\n");
        s.write_all(req.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; 512];
        let mut got = 0;
        // The status line is all we need; it arrives in the first bytes.
        while got < buf.len() && !buf[..got].contains(&b'\n') {
            let n = s.read(&mut buf[got..]).await.map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            got += n;
        }
        let head = String::from_utf8_lossy(&buf[..got]);
        head.lines()
            .next()
            .and_then(|l| l.strip_prefix("HTTP/"))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|c| c.parse::<u16>().ok())
            .ok_or_else(|| "The server did not answer with HTTP.".to_string())
    };
    match tokio::time::timeout(timeout, attempt).await {
        Ok(Ok(status)) => Ok(HttpProbe {
            status,
            ms: started.elapsed().as_millis(),
        }),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(format!("no answer within {}s", timeout.as_secs_f32())),
    }
}

// ---------------------------------------------------------------------------------------------
// Log files in chunks
// ---------------------------------------------------------------------------------------------

/// Reads a log from `from` (or, with None, its last `max_bytes`). Continuations return whole lines
/// only while `live`, so a line being written isn't shown in two halves.
pub fn read_chunk(
    path: &Path,
    from: Option<u64>,
    max_bytes: u64,
    live: bool,
) -> std::io::Result<LogChunk> {
    let mut f = std::fs::File::open(path)?;
    let size = f.metadata()?.len();
    let mut restarted = false;
    let mut start = match from {
        Some(f) if f <= size && size - f <= max_bytes => f,
        Some(f) => {
            restarted = f > size || from.is_some();
            size.saturating_sub(max_bytes)
        }
        None => size.saturating_sub(max_bytes),
    };
    if from.is_none() {
        restarted = false;
    }
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    f.take(size - start).read_to_end(&mut buf)?;

    // Starting mid-file: drop the partial first line.
    let mid_line = start > 0 && from.is_none_or(|f| f != start);
    let mut skip = 0;
    if mid_line {
        if let Some(nl) = buf.iter().position(|b| *b == b'\n') {
            skip = nl + 1;
        }
    }
    let mut end = buf.len();
    if live {
        // Hold back an unfinished last line until its newline arrives.
        end = buf
            .iter()
            .rposition(|b| *b == b'\n')
            .map_or(skip, |i| i + 1)
            .max(skip);
    }
    let text = String::from_utf8_lossy(&buf[skip.min(end)..end]).into_owned();
    start += skip as u64;
    Ok(LogChunk {
        text,
        offset: start,
        next: start + (end - skip.min(end)) as u64,
        size,
        restarted,
    })
}

// ---------------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write, process::Command};

    fn tmp(label: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("bach-probe-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn finds_who_listens_on_a_port() {
        // A listener that is not ours to describe as a Satie task: we hold it ourselves.
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let found = listeners()
            .into_iter()
            .find(|x| x.port == port)
            .expect("our own listener");
        assert_eq!(found.pid, Some(std::process::id()));
        assert!(
            found.command.contains("probe") || !found.command.is_empty(),
            "{found:?}"
        );
        assert!(found.cwd.is_some() && found.up_secs.is_some(), "{found:?}");
        assert_eq!(found.sid, session_of(std::process::id()));
        assert!(!listeners()
            .iter()
            .any(|x| x.port == port && x.pid != Some(std::process::id())));
        drop(l);
        // Another test may take the freed port straight away; we just mustn't hold it any more.
        assert!(
            !listeners()
                .iter()
                .any(|x| x.port == port && x.pid == Some(std::process::id())),
            "gone once closed"
        );
    }

    #[test]
    fn formats_ages() {
        assert_eq!(age(5), "5s");
        assert_eq!(age(600), "10m");
        assert_eq!(age(3 * 3600 + 5 * 60), "3h 5m");
        assert_eq!(age(29 * 3600 + 2000), "1d 5h");
    }

    #[test]
    fn summarises_a_session() {
        let dir = tmp("summary");
        // Leader of its own session, running two `sleep`s.
        let mut child = {
            use std::os::unix::process::CommandExt;
            let mut c = Command::new("sh");
            c.arg("-c").arg("sleep 4901 & sleep 4901 & wait");
            unsafe {
                c.pre_exec(|| {
                    libc::setsid();
                    Ok(())
                });
            }
            c.spawn().unwrap()
        };
        std::thread::sleep(Duration::from_millis(400));
        let summary = process_summary(child.id());
        assert!(summary.contains(&"sleep ×2".to_string()), "{summary:?}");
        assert!(
            !summary.iter().any(|s| s == "sh"),
            "wrapper shells are hidden: {summary:?}"
        );
        unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
        child.wait().ok();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn reads_port_numbers_out_of_conflict_lines_only() {
        let log = "starting 1234 workers\n\
                   Error: Port 5173 is already in use\n\
                   listen EADDRINUSE: address already in use 127.0.0.1:8787\n\
                   worker4001 ready\n\
                   Address in use: bind(0.0.0.0:9000)\n\
                   serving on 3000\n";
        // Lines without the phrase (1234, 3000) and numbers glued to words (worker4001) don't count.
        assert_eq!(ports_named_in_conflicts(log), vec![5173, 8787, 9000]);
        assert!(ports_named_in_conflicts("nothing wrong here on 8080").is_empty());
    }

    #[tokio::test]
    async fn http_check_reports_status_and_only_talks_to_localhost() {
        use tokio::io::AsyncWriteExt;
        let server = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = server.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = server.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let n = s.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let status = if req.starts_with("GET /missing") {
                        "404 Not Found"
                    } else {
                        "200 OK"
                    };
                    let _ = s
                        .write_all(
                            format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\n\r\n").as_bytes(),
                        )
                        .await;
                });
            }
        });
        let ok = http_get(&format!("http://127.0.0.1:{port}/"), Duration::from_secs(2))
            .await
            .unwrap();
        assert_eq!(ok.status, 200);
        let via_name = http_get(
            &format!("http://localhost:{port}/missing"),
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        assert_eq!(via_name.status, 404, "an error status is still an answer");

        // Nothing there / not HTTP / not local / not http:// are all refused with a reason.
        let closed = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        assert!(http_get(
            &format!("http://127.0.0.1:{closed}/"),
            Duration::from_secs(1)
        )
        .await
        .is_err());
        assert!(http_get("http://example.com/", Duration::from_secs(1))
            .await
            .unwrap_err()
            .contains("Only localhost"));
        assert!(http_get("https://localhost/", Duration::from_secs(1))
            .await
            .unwrap_err()
            .contains("plain http"));
    }

    #[test]
    fn reads_logs_in_chunks_without_splitting_lines() {
        let dir = tmp("chunks");
        let path = dir.join("x.log");
        std::fs::write(&path, "one\ntwo\nthr").unwrap();

        // First read: the tail. While live, the unfinished last line is held back.
        let c = read_chunk(&path, None, 1 << 20, true).unwrap();
        assert_eq!((c.text.as_str(), c.offset, c.next), ("one\ntwo\n", 0, 8));
        // Finished (not live): everything, including the last unterminated line.
        assert_eq!(
            read_chunk(&path, None, 1 << 20, false).unwrap().text,
            "one\ntwo\nthr"
        );

        // The line completes and more arrives: continue exactly where we stopped.
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"ee\nfour\n")
            .unwrap();
        let c2 = read_chunk(&path, Some(c.next), 1 << 20, true).unwrap();
        assert_eq!((c2.text.as_str(), c2.offset), ("three\nfour\n", 8));
        assert!(!c2.restarted);
        assert_eq!(c2.next, c2.size);
        // Nothing new -> empty, same position.
        let c3 = read_chunk(&path, Some(c2.next), 1 << 20, true).unwrap();
        assert_eq!((c3.text.as_str(), c3.next), ("", c2.next));

        // A huge file: only the tail, starting on a line boundary.
        let big: String = (0..1000).map(|i| format!("line {i}\n")).collect();
        std::fs::write(&path, &big).unwrap();
        let t = read_chunk(&path, None, 200, true).unwrap();
        assert!(
            t.text.len() <= 200 && t.text.starts_with("line "),
            "{:?}",
            t.text
        );
        assert!(t.text.ends_with("line 999\n"));
        assert!(t.offset > 0);

        // Asking to continue from far behind skips ahead (and says so); a shrunk file restarts.
        let skipped = read_chunk(&path, Some(0), 200, true).unwrap();
        assert!(skipped.restarted && skipped.text.ends_with("line 999\n"));
        std::fs::write(&path, "fresh\n").unwrap();
        let shrunk = read_chunk(&path, Some(9999), 1 << 20, true).unwrap();
        assert!(shrunk.restarted && shrunk.text == "fresh\n", "{shrunk:?}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
