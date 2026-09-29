//! A sub-agent left running in the background keeps the run going past the main agent's first
//! result: Claude takes another turn once it finishes, and only that one ends the run.
use bach_core::{
    adapters::AgentKind,
    runs::{RunRequest, Runs},
};
use serde_json::Value;
use std::{os::unix::fs::PermissionsExt, sync::mpsc, time::Duration};

// Recorded from the real CLI (trimmed): the result comes while the sub-agent still runs, and
// stdin must stay open until the follow-up turn's result.
const FAKE_CLAUDE: &str = r#"#!/bin/sh
read first
echo '{"type":"system","subtype":"init","session_id":"s1","model":"claude-test"}'
echo '{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_a","name":"Agent","input":{}}]}}'
echo '{"type":"system","subtype":"task_started","task_id":"a1","tool_use_id":"toolu_a","description":"Wait","subagent_type":"general-purpose","is_backgrounded":true,"task_type":"local_agent"}'
echo '{"type":"system","subtype":"task_started","task_id":"b1","tool_use_id":"toolu_b","description":"Sleep","is_backgrounded":true,"task_type":"local_bash"}'
echo '{"type":"result","subtype":"success","is_error":false,"total_cost_usd":0.01}'
sleep 0.3
echo '{"type":"system","subtype":"task_notification","task_id":"a1","tool_use_id":"toolu_a","status":"completed","summary":"hi"}'
echo '{"type":"system","subtype":"init","session_id":"s1","model":"claude-test"}'
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"It said hi"}]}}'
echo '{"type":"result","subtype":"success","is_error":false,"total_cost_usd":0.02}'
# Like the real CLI, wait for stdin to close before exiting.
cat >/dev/null
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_background_sub_agent_keeps_the_run_going() {
    let dir = std::env::temp_dir().join(format!("bach-background-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("claude");
    std::fs::write(&script, FAKE_CLAUDE).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::env::set_var(
        "PATH",
        format!("{}:{}", dir.display(), std::env::var("PATH").unwrap()),
    );

    let (tx, rx) = mpsc::channel();
    let runs = Runs::default();
    let run_id = runs
        .start(
            std::sync::Arc::new(move |ev| {
                let _ = tx.send(serde_json::to_value(&ev).unwrap());
            }),
            RunRequest {
                agent: AgentKind::Claude,
                prompt: "hi".into(),
                images: vec![],
                cwd: Some(dir.to_string_lossy().into()),
                session_id: None,
                model: None,
                permission_mode: None,
                allowed_tools: vec![],
                session_key: None,
                run_id: None,
            },
        )
        .await
        .unwrap();

    let mut events: Vec<Value> = vec![];
    loop {
        let ev = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let done = ev["type"] == "done";
        events.push(ev);
        if done {
            break;
        }
    }
    let types: Vec<&str> = events.iter().map(|e| e["type"].as_str().unwrap()).collect();
    assert!(
        types.contains(&"text"),
        "the run ended before the follow-up turn: {types:?}"
    );
    assert_eq!(events.last().unwrap()["costUsd"], 0.02);
    assert_eq!(types.iter().filter(|t| **t == "done").count(), 1);

    // Stdin was closed after the last result, so the process exits and the run ends.
    for _ in 0..100 {
        if !runs.is_live(&run_id).await {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!runs.is_live(&run_id).await);
    assert!(rx.recv_timeout(Duration::from_millis(200)).is_err(), "nothing after done");
    let _ = std::fs::remove_dir_all(dir);
}
