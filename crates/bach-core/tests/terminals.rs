//! Terminals through the API: a real shell in a PTY.
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use bach_core::Api;
use bach_protocol::{ErrorCode, ServerEvent, TerminalEvent};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::sync::broadcast;

/// Output of terminal `id` from events, until `done` says it's enough.
async fn read_until(
    events: &mut broadcast::Receiver<ServerEvent>,
    id: &str,
    last_seq: &mut u64,
    done: impl Fn(&str) -> bool,
) -> String {
    let mut text = String::new();
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(10), events.recv())
            .await
            .unwrap_or_else(|_| panic!("no more output; so far: {text:?}"))
            .unwrap();
        if let ServerEvent::Terminal(TerminalEvent::Output { terminal_id, seq, data }) = ev {
            if terminal_id == id {
                assert_eq!(seq, *last_seq + 1, "numbered without gaps");
                *last_seq = seq;
                text += &String::from_utf8_lossy(&B64.decode(data).unwrap());
                if done(&text) {
                    return text;
                }
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_shell_in_a_terminal() {
    std::env::set_var("SHELL", "/bin/sh");
    let dir = std::env::temp_dir().join(format!("bach-terminals-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("proj")).unwrap();
    let api = Api::open(&dir.join("data/bach.db")).await.unwrap();
    let mut events = api.subscribe();

    let t = api
        .call("open_terminal", json!({ "cwd": dir.join("proj"), "cols": 80, "rows": 24 }))
        .await
        .unwrap();
    let id = t["id"].as_str().unwrap().to_string();
    assert_eq!(t["shell"], "sh");
    assert_eq!(api.call("list_terminals", Value::Null).await.unwrap()[0]["id"], id);

    // Typing a command runs it in the folder asked for.
    let mut seq = 0;
    api.call("terminal_input", json!({ "terminalId": id, "data": "echo hi-$((40+2)) && pwd\n" }))
        .await
        .unwrap();
    let out = read_until(&mut events, &id, &mut seq, |t| t.contains("hi-42") && t.contains("/proj")).await;
    assert!(out.contains("hi-42"), "{out}");

    // The snapshot has it too, up to the latest event.
    let snap = api.call("terminal_snapshot", json!({ "terminalId": id })).await.unwrap();
    let data = String::from_utf8(B64.decode(snap["data"].as_str().unwrap()).unwrap()).unwrap();
    assert!(data.contains("hi-42"));
    assert!(snap["seq"].as_u64().unwrap() >= seq);

    api.call("resize_terminal", json!({ "terminalId": id, "cols": 120, "rows": 40 }))
        .await
        .unwrap();
    api.call("terminal_input", json!({ "terminalId": id, "data": "stty size\n" }))
        .await
        .unwrap();
    read_until(&mut events, &id, &mut seq, |t| t.contains("40 120")).await;

    // A flood of output: the scrollback keeps only the most recent, from the start of a line.
    api.call(
        "terminal_input",
        json!({ "terminalId": id, "data": "i=0; while [ $i -lt 15000 ]; do echo line-$i-0123456789012345678901234567890123456789; i=$((i+1)); done; echo flood-done\n" }),
    )
    .await
    .unwrap();
    // The line on its own (the typed command echoes "echo flood-done" first).
    read_until(&mut events, &id, &mut seq, |t| t.contains("\r\nflood-done\r\n")).await;
    let snap = api.call("terminal_snapshot", json!({ "terminalId": id })).await.unwrap();
    let data = B64.decode(snap["data"].as_str().unwrap()).unwrap();
    assert!(data.len() <= 512 * 1024, "{}", data.len());
    assert!(String::from_utf8_lossy(&data).starts_with("line-"), "starts at a line: {:?}", &String::from_utf8_lossy(&data)[..80]);
    assert!(!String::from_utf8_lossy(&data).contains("hi-42"), "the oldest output went");

    // The shell ending: the terminal stays, saying so, until closed.
    api.call("terminal_input", json!({ "terminalId": id, "data": "exit 3\n" }))
        .await
        .unwrap();
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(10), events.recv())
            .await
            .unwrap()
            .unwrap();
        if let ServerEvent::Terminal(TerminalEvent::Exited { terminal_id, status }) = ev {
            assert_eq!(terminal_id, id);
            assert_eq!(status.code, Some(3));
            break;
        }
    }
    let e = api
        .call("terminal_input", json!({ "terminalId": id, "data": "ls\n" }))
        .await
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid);
    assert_eq!(api.call("list_terminals", Value::Null).await.unwrap()[0]["exited"]["code"], 3);

    api.call("close_terminal", json!({ "terminalId": id })).await.unwrap();
    assert!(api.call("list_terminals", Value::Null).await.unwrap().as_array().unwrap().is_empty());
    let e = api
        .call("terminal_snapshot", json!({ "terminalId": id }))
        .await
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);

    // Closing a live one ends its shell.
    let t = api
        .call("open_terminal", json!({ "cols": 80, "rows": 24 }))
        .await
        .unwrap();
    api.call("close_terminal", json!({ "terminalId": t["id"] })).await.unwrap();
    let _ = std::fs::remove_dir_all(dir);
}
