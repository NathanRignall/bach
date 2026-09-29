//! `bach-server attach`, as the desktop app runs it over SSH: starts the server if needed, then
//! carries one JSON frame per line.
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    time::Duration,
};

struct Attached {
    child: Child,
    stdin: ChildStdin,
    lines: std::io::Lines<BufReader<ChildStdout>>,
}

fn attach(db: &Path, port: u16) -> Attached {
    let mut child = Command::new(env!("CARGO_BIN_EXE_bach-server"))
        .arg("attach")
        .env("BACH_DB", db)
        .env("BACH_PORT", port.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    Attached {
        stdin: child.stdin.take().unwrap(),
        lines: BufReader::new(child.stdout.take().unwrap()).lines(),
        child,
    }
}

impl Attached {
    fn next(&mut self) -> Value {
        serde_json::from_str(&self.lines.next().expect("a frame").unwrap()).unwrap()
    }

    /// Sends a command and returns its reply, skipping events.
    fn call(&mut self, id: u64, cmd: &str, args: Value) -> Value {
        writeln!(self.stdin, "{}", json!({ "id": id, "cmd": cmd, "args": args })).unwrap();
        loop {
            let f = self.next();
            if f["kind"] != "event" {
                assert_eq!(f["id"], id, "{f}");
                return f;
            }
        }
    }
}

/// `bach-server serve` processes using this database.
fn servers_for(db: &Path) -> Vec<u32> {
    let want = format!("BACH_DB={}", db.display());
    std::fs::read_dir("/proc")
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().to_str()?.parse::<u32>().ok())
        .filter(|pid| {
            let cmd = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
            let env = std::fs::read(format!("/proc/{pid}/environ")).unwrap_or_default();
            cmd.split(|b| *b == 0).nth(1) == Some(b"serve".as_slice())
                && env.split(|b| *b == 0).any(|v| v == want.as_bytes())
        })
        .collect()
}

#[test]
fn attach_starts_one_server_and_carries_frames() {
    let dir = std::env::temp_dir().join(format!("bach-attach-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let db = dir.join("data/bach.db");
    // The WebSocket port is taken: the server must still come up, serving the socket only.
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = taken.local_addr().unwrap().port();

    // Two clients at once, with no server running: one server starts, both get through.
    let (mut a, mut b) = (attach(&db, port), attach(&db, port));
    for c in [&mut a, &mut b] {
        let hello = c.next();
        assert_eq!(hello["kind"], "hello", "{hello}");
        assert_eq!(hello["protocol"], bach_protocol::fingerprint());
    }
    let reply = a.call(1, "list_agents", json!({}));
    assert_eq!(reply["kind"], "reply");
    assert!(reply["result"].as_array().unwrap().iter().any(|x| x["kind"] == "claude"));
    let reply = b.call(7, "nope", json!({}));
    assert_eq!(reply["error"]["code"], "invalid");
    let servers = servers_for(&db);
    assert_eq!(servers.len(), 1, "{servers:?}");

    // Hanging up ends `attach`, not the server.
    for mut c in [a, b] {
        drop(c.stdin);
        let status = c.child.wait().unwrap();
        assert!(status.success());
    }
    assert_eq!(servers_for(&db), servers);
    let mut again = attach(&db, port);
    assert_eq!(again.next()["kind"], "hello");
    drop(again.stdin);
    again.child.wait().unwrap();

    // A command followed straight away by the end of input still gets its answer.
    let mut once = attach(&db, port);
    writeln!(once.stdin, "{}", json!({ "id": 3, "cmd": "list_agents", "args": {} })).unwrap();
    drop(once.stdin);
    let frames: Vec<Value> = once.lines.map(|l| serde_json::from_str(&l.unwrap()).unwrap()).collect();
    assert!(frames.iter().any(|f| f["kind"] == "reply" && f["id"] == 3), "{frames:?}");
    assert!(once.child.wait().unwrap().success());

    // Another `serve` for the same database refuses to start.
    let out = Command::new(env!("CARGO_BIN_EXE_bach-server"))
        .arg("serve")
        .env("BACH_DB", &db)
        .env("BACH_PORT", port.to_string())
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stderr).contains("already running"));

    for pid in servers {
        unsafe { libc_kill(pid as i32, 15) };
    }
    std::thread::sleep(Duration::from_millis(200));
    assert!(servers_for(&db).is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, sig: i32) -> i32;
}
