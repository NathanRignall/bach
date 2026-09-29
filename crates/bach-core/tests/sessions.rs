//! Sessions through the API, as a client drives them: start, approvals, follow-up messages,
//! renames, deletion. A stand-in `claude` asks for one approval per turn.
use bach_core::Api;
use bach_protocol::{Entry, ErrorCode, ServerEvent, Session, SessionEvent};
use serde_json::{json, Value};
use std::{os::unix::fs::PermissionsExt, path::Path, time::Duration};
use tokio::sync::broadcast;

const FAKE_CLAUDE: &str = r#"#!/bin/sh
echo "$@" >> "$BACH_TEST_DIR/args"
read first
echo '{"type":"system","subtype":"init","session_id":"s1","model":"claude-test"}'
echo '{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","unifiedWindows":{"five_hour":{"utilization":0.31,"resetsAt":1790706600},"seven_day":{"utilization":0.22,"resetsAt":1791068400}}}}'
echo '{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"tmux ls"},"permission_suggestions":[{"type":"addRules","rules":[{"toolName":"Bash","ruleContent":"tmux ls *"}],"behavior":"allow","destination":"localSettings"}],"tool_use_id":"tu1"}}'
read answer
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"done"}],"usage":{"input_tokens":10,"cache_read_input_tokens":400,"cache_creation_input_tokens":50,"output_tokens":40}},"parent_tool_use_id":null}'
echo '{"type":"result","subtype":"success","is_error":false,"total_cost_usd":0,"modelUsage":{"claude-haiku":{"contextWindow":200000},"claude-test":{"contextWindow":1000000}}}'
cat > /dev/null
"#;

async fn session(api: &Api, id: &str) -> (Session, Vec<Value>) {
    let log = api
        .call("get_session", json!({ "sessionId": id }))
        .await
        .unwrap();
    let session = serde_json::from_value(log["session"].clone()).unwrap();
    let entries = log["entries"].as_array().unwrap().clone();
    (session, entries)
}

/// Waits until the session satisfies `cond`.
async fn until(api: &Api, id: &str, what: &str, cond: impl Fn(&Session) -> bool) -> Session {
    for _ in 0..100 {
        let (s, _) = session(api, id).await;
        if cond(&s) {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for: {what}");
}

fn drain(rx: &mut broadcast::Receiver<ServerEvent>) -> Vec<SessionEvent> {
    let mut out = vec![];
    while let Ok(ev) = rx.try_recv() {
        if let ServerEvent::Session(e) = ev {
            out.push(e);
        }
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_from_first_message_to_deletion() {
    let dir = std::env::temp_dir().join(format!("bach-sessions-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (bin, proj) = (dir.join("bin"), dir.join("proj"));
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(bin.join("claude"), FAKE_CLAUDE).unwrap();
    std::fs::set_permissions(bin.join("claude"), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::env::set_var("PATH", format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()));
    std::env::set_var("BACH_TEST_DIR", &dir);
    let args = || std::fs::read_to_string(dir.join("args")).unwrap_or_default();

    let api = Api::open(&dir.join("data/bach.db")).await.unwrap();
    let mut events = api.subscribe();

    // The first message creates the session and starts the agent.
    let s: Session = serde_json::from_value(
        api.call(
            "start_session",
            json!({ "agent": "claude", "cwd": proj, "prompt": "  hello world  " }),
        )
        .await
        .unwrap(),
    )
    .unwrap();
    let id = s.id.clone();
    assert_eq!(s.title, "hello world");
    assert_eq!(s.workdir.as_deref(), proj.to_str());
    assert!(s.run_id.is_some());

    // It asks for approval; the session says it is waiting.
    let s = until(&api, &id, "an open approval", |s| s.open_approvals == ["r1"]).await;
    assert_eq!(s.agent_session_id.as_deref(), Some("s1"));
    assert_eq!(s.model.as_deref(), Some("claude-test"));

    // Sending again while it works is refused.
    let e = api
        .call("send_message", json!({ "sessionId": id, "prompt": "more" }))
        .await
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid);

    api.call(
        "answer_approval",
        json!({ "sessionId": id, "requestId": "r1", "decision": "allow_session" }),
    )
    .await
    .unwrap();
    let s = until(&api, &id, "the run to end", |s| s.run_id.is_none()).await;
    assert_eq!(s.allow_rules, ["Bash(tmux ls *)"], "remembered for the session");
    // How full its context is, with the window of the session's own model.
    let ctx = s.context.clone().expect("context usage");
    assert_eq!((ctx.used, ctx.window), (500, Some(1_000_000)));
    // The account's limits, kept by the backend rather than in the transcript.
    let usage = api.call("get_usage", Value::Null).await.unwrap();
    assert_eq!(usage["windows"]["five_hour"]["utilization"], 0.31);
    assert_eq!(usage["windows"]["seven_day"]["resetsAt"], 1_791_068_400_000i64);
    assert!(s.open_approvals.is_empty());

    // The transcript: what was said, what the agent did, what was decided.
    let (_, entries) = session(&api, &id).await;
    let kinds: Vec<String> = entries
        .iter()
        .map(|e| match e["entry"]["type"].as_str().unwrap() {
            "agent" => format!("agent:{}", e["entry"]["event"]["type"].as_str().unwrap()),
            other => other.to_string(),
        })
        .collect();
    assert_eq!(
        kinds,
        ["user", "agent:session", "agent:approval", "decision", "agent:text", "agent:done"]
    );
    assert_eq!(entries[0]["entry"]["text"], "hello world");
    assert_eq!(entries[0]["seq"], 1);
    let later = api
        .call("get_session", json!({ "sessionId": id, "afterSeq": 4 }))
        .await
        .unwrap();
    assert_eq!(later["entries"].as_array().unwrap().len(), 2);

    // Clients were told as it happened.
    let all: Vec<ServerEvent> = std::iter::from_fn(|| events.try_recv().ok()).collect();
    assert!(all.iter().any(|e| matches!(e, ServerEvent::Usage(u) if u.windows.contains_key("seven_day"))));
    let seen: Vec<SessionEvent> = all
        .into_iter()
        .filter_map(|e| match e {
            ServerEvent::Session(e) => Some(e),
            _ => None,
        })
        .collect();
    assert!(seen.iter().any(|e| matches!(e, SessionEvent::Entry { session_id, entry }
        if *session_id == id && matches!(entry.entry, Entry::Decision { .. }))));
    assert!(seen
        .iter()
        .any(|e| matches!(e, SessionEvent::Changed { session } if session.id == id && session.run_id.is_none())));

    // A follow-up resumes the agent's session with the rules approved so far.
    api.call("send_message", json!({ "sessionId": id, "prompt": "again" }))
        .await
        .unwrap();
    until(&api, &id, "the second approval", |s| s.open_approvals == ["r1"]).await;
    let second = args().lines().nth(1).unwrap().to_string();
    assert!(second.contains("--resume s1"), "{second}");
    assert!(second.contains("Bash(tmux ls *)"), "{second}");
    api.call(
        "answer_approval",
        json!({ "sessionId": id, "requestId": "r1", "decision": "deny" }),
    )
    .await
    .unwrap();
    until(&api, &id, "the second run to end", |s| s.run_id.is_none()).await;
    let e = api
        .call(
            "answer_approval",
            json!({ "sessionId": id, "requestId": "r1", "decision": "allow" }),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);

    // Renaming, and changing the model for later messages.
    let s: Session = serde_json::from_value(
        api.call(
            "update_session",
            json!({ "sessionId": id, "title": "Greeting", "modelChoice": "opus" }),
        )
        .await
        .unwrap(),
    )
    .unwrap();
    assert!(s.title_edited && s.title == "Greeting" && s.model_choice.as_deref() == Some("opus"));
    let s: Session = serde_json::from_value(
        api.call("update_session", json!({ "sessionId": id, "modelChoice": "default" }))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(s.model_choice, None);

    // A first message that can't reach an agent leaves nothing behind.
    let before = api.call("list_sessions", Value::Null).await.unwrap();
    let e = api
        .call(
            "start_session",
            json!({ "agent": "codex", "cwd": proj, "prompt": "hi", "modelChoice": "x" }),
        )
        .await;
    if which_codex_missing() {
        assert!(e.is_err());
        assert_eq!(api.call("list_sessions", Value::Null).await.unwrap(), before);
    }
    let e = api
        .call(
            "start_session",
            json!({ "agent": "claude", "cwd": dir.join("nope"), "prompt": "hi" }),
        )
        .await
        .unwrap_err();
    assert!(e.message.contains("nope"), "{e}");
    assert_eq!(api.call("list_sessions", Value::Null).await.unwrap(), before);

    // Deleting removes it and its transcript.
    drain(&mut events);
    api.call("delete_session", json!({ "sessionId": id }))
        .await
        .unwrap();
    assert_eq!(
        api.call("get_session", json!({ "sessionId": id }))
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    assert!(drain(&mut events)
        .iter()
        .any(|e| matches!(e, SessionEvent::Deleted { session_id } if *session_id == id)));
    let _ = std::fs::remove_dir_all(dir);
}

fn which_codex_missing() -> bool {
    !std::env::var("PATH")
        .unwrap()
        .split(':')
        .any(|d| Path::new(d).join("codex").exists())
}
