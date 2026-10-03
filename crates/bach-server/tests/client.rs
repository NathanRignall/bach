//! The desktop app's side of a remote connection (`bach-client`), against a real
//! `bach-server attach`. Over SSH the command is just `ssh <host> bach-server attach`.
use bach_client::{Remote, Status};
use bach_protocol::ErrorCode;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

fn tmp(label: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("bach-client-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// `bach-server attach` for the database in `dir`, as a shell command so its environment is set.
fn attach_command(dir: &Path) -> Vec<String> {
    vec![
        "sh".into(),
        "-c".into(),
        format!(
            "BACH_DB='{}' exec '{}' attach",
            dir.join("data/bach.db").display(),
            env!("CARGO_BIN_EXE_bach-server")
        ),
    ]
}

struct Watch {
    statuses: Arc<Mutex<Vec<Status>>>,
    events: Arc<Mutex<Vec<Value>>>,
}

fn connect(label: &str, command: Vec<String>) -> (Remote, Watch) {
    let (statuses, events) = (Arc::new(Mutex::new(vec![])), Arc::new(Mutex::new(vec![])));
    let remote = Remote::connect(
        label,
        command,
        {
            let events = events.clone();
            move |e| events.lock().unwrap().push(e)
        },
        {
            let statuses = statuses.clone();
            move |s| statuses.lock().unwrap().push(s)
        },
    );
    (remote, Watch { statuses, events })
}

async fn until(what: &str, cond: impl Fn() -> bool) {
    for _ in 0..200 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for: {what}");
}

fn server_pids(dir: &Path) -> Vec<i32> {
    let want = format!("BACH_DB={}", dir.join("data/bach.db").display());
    std::fs::read_dir("/proc")
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().to_str()?.parse::<i32>().ok())
        .filter(|pid| {
            let cmd = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
            let env = std::fs::read(format!("/proc/{pid}/environ")).unwrap_or_default();
            cmd.split(|b| *b == 0).nth(1) == Some(b"serve".as_slice())
                && env.split(|b| *b == 0).any(|v| v == want.as_bytes())
        })
        .collect()
}

extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn talks_to_a_remote_backend_and_reconnects() {
    let dir = tmp("remote");
    let (remote, watch) = connect("testhost", attach_command(&dir));

    // Calls made while connecting wait for the connection.
    let agents = remote.call("list_agents", json!({})).await.unwrap();
    assert!(agents.as_array().unwrap().iter().any(|a| a["kind"] == "claude"));
    assert!(matches!(remote.status(), Status::Connected { .. }));
    let e = remote.call("nope", json!({})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid);

    // Events come through: a background task started over the connection.
    remote
        .call("start_task", json!({ "command": "sleep 4710", "cwd": dir, "name": "remote" }))
        .await
        .unwrap();
    until("a task event", || {
        watch.events.lock().unwrap().iter().any(|e| e["topic"] == "task")
    })
    .await;

    // The server going away: disconnected, then back (attach starts it again).
    for pid in server_pids(&dir) {
        unsafe { kill(pid, 15) };
    }
    until("a disconnect", || {
        watch
            .statuses
            .lock()
            .unwrap()
            .iter()
            .any(|s| matches!(s, Status::Disconnected { retrying: true, .. }))
    })
    .await;
    until("reconnected", || matches!(remote.status(), Status::Connected { .. })).await;
    let tasks = remote.call("list_tasks", json!({})).await.unwrap();
    let task = tasks.as_array().unwrap().iter().find(|t| t["name"] == "remote").unwrap().clone();
    assert_eq!(task["status"], "running", "the task outlived the server");
    remote
        .call("stop_task", json!({ "taskId": task["id"] }))
        .await
        .unwrap();

    drop(remote);
    for pid in server_pids(&dir) {
        unsafe { kill(pid, 15) };
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn explains_why_it_cannot_connect() {
    // What ssh does with a host it can't find.
    let (remote, _) = connect(
        "nowhere",
        vec![
            "sh".into(),
            "-c".into(),
            "echo 'ssh: Could not resolve hostname nowhere: Name or service not known' >&2; exit 255"
                .into(),
        ],
    );
    let e = remote.call("list_agents", json!({})).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::Unavailable);
    assert!(e.message.contains("Not connected to nowhere"), "{e}");
    assert!(e.message.contains("Could not resolve hostname"), "{e}");
    assert!(matches!(remote.status(), Status::Disconnected { retrying: true, .. }));

    // A server from a different build: said plainly, and not retried.
    let (remote, _) = connect(
        "orion",
        vec![
            "sh".into(),
            "-c".into(),
            r#"echo '{"kind":"hello","protocol":"0000","version":"0.0.1"}'; cat"#.into(),
        ],
    );
    let e = remote.call("list_agents", json!({})).await.unwrap_err();
    assert!(e.message.contains("different version"), "{e}");
    assert!(matches!(remote.status(), Status::Disconnected { retrying: false, .. }));
}
