//! When the agent process ends without finishing, the run must say how, so it can be diagnosed.
use bach_core::{
    adapters::AgentKind,
    runs::{RunRequest, Runs},
};
use serde_json::Value;
use std::{os::unix::fs::PermissionsExt, sync::mpsc, time::Duration};

const FAKE_CLAUDE: &str = r#"#!/bin/sh
read first
echo '{"type":"system","subtype":"init","session_id":"s1","model":"claude-test"}'
echo "something went wrong on stderr" >&2
case "$BACH_TEST_MODE" in
  kill) kill -9 $$ ;;
  code) exit 3 ;;
esac
"#;

async fn run(dir: &std::path::Path, mode: &str) -> Vec<Value> {
    std::env::set_var("BACH_TEST_MODE", mode);
    let (tx, rx) = mpsc::channel();
    Runs::default()
        .start(
            std::sync::Arc::new(move |ev| {
                let _ = tx.send(serde_json::to_value(&ev).unwrap());
            }),
            RunRequest {
                agent: AgentKind::Claude,
                prompt: "hi".into(),
                cwd: Some(dir.to_string_lossy().into()),
                session_id: None,
                model: None,
                allowed_tools: vec![],
                session_key: None,
            },
        )
        .await
        .unwrap();
    let mut events = vec![];
    loop {
        let ev = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let done = ev["type"] == "done";
        events.push(ev);
        if done {
            return events;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unexpected_exit_is_reported_with_how_and_why() {
    let dir = std::env::temp_dir().join(format!("bach-exit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("claude");
    std::fs::write(&script, FAKE_CLAUDE).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::env::set_var(
        "PATH",
        format!("{}:{}", dir.display(), std::env::var("PATH").unwrap()),
    );

    let ev = run(&dir, "kill").await;
    let err = ev
        .iter()
        .find(|e| e["type"] == "error")
        .expect("an error event");
    let msg = err["message"].as_str().unwrap();
    assert!(msg.contains("killed by signal 9"), "{msg}");
    assert!(
        msg.contains("something went wrong on stderr"),
        "the agent's own stderr is included: {msg}"
    );
    assert_eq!(ev.last().unwrap()["isError"], true);

    let ev = run(&dir, "code").await;
    let msg = ev.iter().find(|e| e["type"] == "error").unwrap()["message"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(msg.contains("exited with code 3"), "{msg}");

    // A run that ends cleanly reports nothing.
    let ev = run(&dir, "ok").await;
    assert!(!ev.iter().any(|e| e["type"] == "error"), "{ev:?}");
    let _ = std::fs::remove_dir_all(dir);
}
