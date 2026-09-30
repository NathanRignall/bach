//! Edits the user makes by hand reach the agent with their next message, and never write outside
//! the session's folder. A stand-in `claude` records what it is asked.
use bach_core::Api;
use bach_protocol::{ErrorCode, Session};
use serde_json::{json, Value};
use std::{os::unix::fs::PermissionsExt, time::Duration};

const FAKE_CLAUDE: &str = r#"#!/bin/sh
read first
echo "$first" >> "$BACH_TEST_DIR/prompts"
echo '{"type":"system","subtype":"init","session_id":"s1","model":"claude-test"}'
echo '{"type":"result","subtype":"success","is_error":false,"total_cost_usd":0}'
cat > /dev/null
"#;

async fn idle(api: &Api, id: &str) {
    for _ in 0..100 {
        let log = api.call("get_session", json!({ "sessionId": id })).await.unwrap();
        let s: Session = serde_json::from_value(log["session"].clone()).unwrap();
        if s.run_id.is_none() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the run didn't finish");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hand_edits_are_reported_to_the_agent_once() {
    let dir = std::env::temp_dir().join(format!("bach-manual-edits-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (bin, proj) = (dir.join("bin"), dir.join("proj"));
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(proj.join("src")).unwrap();
    std::fs::write(proj.join("src/a.rs"), "fn a() {}\n").unwrap();
    std::fs::write(dir.join("outside.txt"), "safe").unwrap();
    std::fs::write(bin.join("claude"), FAKE_CLAUDE).unwrap();
    std::fs::set_permissions(bin.join("claude"), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::env::set_var("PATH", format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()));
    std::env::set_var("BACH_TEST_DIR", &dir);
    let prompts = || std::fs::read_to_string(dir.join("prompts")).unwrap_or_default();

    let api = Api::open(&dir.join("data/bach.db")).await.unwrap();
    let s: Session = serde_json::from_value(
        api.call("start_session", json!({ "agent": "claude", "cwd": proj, "prompt": "first" }))
            .await
            .unwrap(),
    )
    .unwrap();
    let id = s.id;
    idle(&api, &id).await;
    assert!(!prompts().contains("edited"), "nothing was edited yet");

    let file: Value = api.call("read_file", json!({ "sessionId": id, "path": "src/a.rs" })).await.unwrap();
    let version = file["version"].as_str().unwrap().to_string();
    api.call(
        "write_file",
        json!({ "sessionId": id, "path": "src/a.rs", "text": "fn b() {}\n", "expectedVersion": version }),
    )
    .await
    .unwrap();
    assert_eq!(std::fs::read_to_string(proj.join("src/a.rs")).unwrap(), "fn b() {}\n");

    // A stale edit is refused and isn't reported; nor is one that escapes the folder.
    let e = api
        .call(
            "write_file",
            json!({ "sessionId": id, "path": "src/a.rs", "text": "x", "expectedVersion": version }),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Conflict);
    api.call(
        "write_file",
        json!({ "sessionId": id, "path": "../outside.txt", "text": "x", "expectedVersion": "", "overwrite": true }),
    )
    .await
    .unwrap_err();
    assert_eq!(std::fs::read_to_string(dir.join("outside.txt")).unwrap(), "safe");

    api.call("send_message", json!({ "sessionId": id, "prompt": "second" })).await.unwrap();
    idle(&api, &id).await;
    let sent = prompts();
    let second = sent.lines().nth(1).expect("a second prompt");
    assert!(second.contains("edited these files by hand") && second.contains("`src/a.rs`"), "{second}");
    assert!(second.contains("second") && !second.contains("outside"), "{second}");

    // Told once: the next message goes as written.
    api.call("send_message", json!({ "sessionId": id, "prompt": "third" })).await.unwrap();
    idle(&api, &id).await;
    let sent = prompts();
    assert!(!sent.lines().nth(2).unwrap().contains("edited"), "{sent}");
    let _ = std::fs::remove_dir_all(dir);
}
