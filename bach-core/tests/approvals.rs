//! Approval requests: the agent asks over stdout and waits for an answer on stdin.
//! A stand-in `claude` script speaks the same protocol as the real CLI (recorded in
//! tests/fixtures/claude_approval.jsonl) so the exchange is checked end to end.
use bach_core::{
    adapters::AgentKind,
    runs::{Decision, RunRequest, Runs},
};
use serde_json::Value;
use std::{os::unix::fs::PermissionsExt, path::Path, sync::mpsc, time::Duration};

const FAKE_CLAUDE: &str = r#"#!/bin/sh
out="$BACH_TEST_OUT"
echo $$ > "$out/pid"
echo "$@" > "$out/args"
read first
echo "$first" > "$out/first"
echo '{"type":"system","subtype":"init","session_id":"s1","model":"claude-test"}'
echo '{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"tmux ls"},"description":"List sessions","decision_reason":"This command requires approval","permission_suggestions":[{"type":"addRules","rules":[{"toolName":"Bash","ruleContent":"tmux ls *"}],"behavior":"allow","destination":"localSettings"},{"type":"addDirectories","directories":["/tmp","'"$PWD"'"],"destination":"session"},{"type":"setMode","mode":"acceptEdits","destination":"session"}],"tool_use_id":"tu1"}}'
read answer
echo "$answer" > "$out/answer"
echo '{"type":"result","subtype":"success","is_error":false,"total_cost_usd":0}'
# Like the real CLI, keep running until stdin is closed.
cat > /dev/null
"#;

struct Outcome {
    events: Vec<Value>,
    answer: Value,
    args: String,
    first: Value,
    pid: String,
}

async fn scenario(
    dir: &Path,
    decision: Decision,
    message: Option<&str>,
    rules: &[&str],
) -> Outcome {
    let out = dir.join(format!("out-{decision:?}"));
    std::fs::create_dir_all(&out).unwrap();
    std::env::set_var("BACH_TEST_OUT", &out);

    let (tx, rx) = mpsc::channel();
    let runs = Runs::default();
    let run_id = runs
        .start(
            std::sync::Arc::new(move |ev| {
                let _ = tx.send(serde_json::to_value(&ev).unwrap());
            }),
            RunRequest {
                agent: AgentKind::Claude,
                prompt: "list tmux".into(),
                cwd: Some(dir.to_string_lossy().into()),
                session_id: None,
                model: None,
                allowed_tools: rules.iter().map(|r| r.to_string()).collect(),
            },
        )
        .await
        .unwrap();

    let mut events = vec![];
    let recv = || rx.recv_timeout(Duration::from_secs(5)).unwrap();
    loop {
        let ev = recv();
        let is_approval = ev["type"] == "approval";
        events.push(ev);
        if is_approval {
            break;
        }
    }

    // Answering something that isn't pending is an error, not a hang or a bad write.
    assert!(runs
        .respond_approval(&run_id, "nope", decision, None)
        .await
        .is_err());
    runs.respond_approval(&run_id, "r1", decision, message.map(String::from))
        .await
        .unwrap();
    assert!(
        runs.respond_approval(&run_id, "r1", decision, None)
            .await
            .is_err(),
        "an approval can only be answered once"
    );

    loop {
        let ev = recv();
        let done = ev["type"] == "done";
        events.push(ev);
        if done {
            break;
        }
    }

    // The process must exit on its own once the turn is over (stdin closed), not linger.
    let pid = std::fs::read_to_string(out.join("pid"))
        .unwrap()
        .trim()
        .to_string();
    for _ in 0..40 {
        if !Path::new(&format!("/proc/{pid}")).exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        !Path::new(&format!("/proc/{pid}")).exists(),
        "agent {pid} still running after the turn"
    );

    let read = |f: &str| std::fs::read_to_string(out.join(f)).unwrap();
    Outcome {
        events,
        answer: serde_json::from_str(&read("answer")).unwrap(),
        args: read("args"),
        first: serde_json::from_str(&read("first")).unwrap(),
        pid,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn approvals_round_trip() {
    let dir = std::env::temp_dir().join(format!("bach-approvals-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("claude");
    std::fs::write(&script, FAKE_CLAUDE).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::env::set_var(
        "PATH",
        format!("{}:{}", dir.display(), std::env::var("PATH").unwrap()),
    );

    // The prompt travels over stdin, and the CLI is told to ask us for permissions.
    let o = scenario(&dir, Decision::Allow, None, &["Bash(git status *)", "Read"]).await;
    assert_eq!(o.first["type"], "user");
    assert_eq!(o.first["message"]["content"], "list tmux");
    for flag in [
        "--input-format stream-json",
        "--permission-prompt-tool stdio",
        "--allowedTools Bash(git status *) Read",
    ] {
        assert!(o.args.contains(flag), "missing `{flag}` in: {}", o.args);
    }
    assert!(
        !o.args.contains("list tmux"),
        "the prompt belongs on stdin, not argv: {}",
        o.args
    );

    // What the UI sees.
    let approval = o.events.iter().find(|e| e["type"] == "approval").unwrap();
    assert_eq!(approval["tool_name"], "Bash");
    assert_eq!(approval["input"]["command"], "tmux ls");
    assert_eq!(approval["reason"], "This command requires approval");
    assert_eq!(approval["rules"], serde_json::json!(["Bash(tmux ls *)"]));
    // The project folder itself is not "outside"; /tmp is.
    assert_eq!(approval["directories"], serde_json::json!(["/tmp"]));
    assert!(
        approval.get("suggestions").is_none(),
        "raw suggestions stay on the backend"
    );

    // Allow once: the original input is echoed back, and no permanent change is requested.
    let r = &o.answer;
    assert_eq!(r["type"], "control_response");
    assert_eq!(r["response"]["request_id"], "r1");
    assert_eq!(r["response"]["response"]["behavior"], "allow");
    assert_eq!(
        r["response"]["response"]["updatedInput"]["command"],
        "tmux ls"
    );
    assert!(r["response"]["response"]
        .get("updatedPermissions")
        .is_none());

    // Allow for this session: the agent's own suggestion, retargeted at the session.
    let o = scenario(&dir, Decision::AllowSession, None, &[]).await;
    let up = &o.answer["response"]["response"]["updatedPermissions"];
    assert_eq!(up[0]["rules"][0]["ruleContent"], "tmux ls *");
    assert_eq!(up[0]["destination"], "session");
    // ...and only the rule: no directory access, no permission-mode change.
    assert_eq!(
        up.as_array().unwrap().len(),
        1,
        "extra suggestions leaked: {up}"
    );
    assert!(!o.args.contains("--allowedTools"));

    // Always allow: the suggestion as given (saved to the project's settings).
    let o = scenario(&dir, Decision::AllowAlways, None, &[]).await;
    assert_eq!(
        o.answer["response"]["response"]["updatedPermissions"][0]["destination"],
        "localSettings"
    );

    // Deny: the agent is told the user said no.
    let o = scenario(&dir, Decision::Deny, Some("Not on this machine."), &[]).await;
    assert_eq!(o.answer["response"]["response"]["behavior"], "deny");
    assert_eq!(
        o.answer["response"]["response"]["message"],
        "Not on this machine."
    );
    assert!(o.answer["response"]["response"]
        .get("updatedInput")
        .is_none());
    let _ = o.pid;
    let _ = std::fs::remove_dir_all(dir);
}
