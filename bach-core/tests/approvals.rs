//! Approval requests: the agent asks over stdout and waits for an answer on stdin.
//! A stand-in `claude` script speaks the same protocol as the real CLI (recorded in
//! tests/fixtures/claude_approval.jsonl) so the exchange is checked end to end.
use bach_core::{
    adapters::AgentKind,
    runs::{Decision, RunRequest, Runs},
    satie::Satie,
};
use serde_json::Value;
use std::{os::unix::fs::PermissionsExt, path::Path, sync::mpsc, time::Duration};

/// What the CLI sends for a shell command it wants to run (shape recorded from the real one).
const BASH_REQUEST: &str = r#"{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"tmux ls"},"description":"List sessions","decision_reason":"This command requires approval","permission_suggestions":[{"type":"addRules","rules":[{"toolName":"Bash","ruleContent":"tmux ls *"}],"behavior":"allow","destination":"localSettings"},{"type":"addDirectories","directories":["/tmp","__PWD__"],"destination":"session"},{"type":"setMode","mode":"acceptEdits","destination":"session"}],"tool_use_id":"tu1"}}"#;

/// ...and for a question it wants answered (also recorded from the real CLI).
const QUESTION_REQUEST: &str = r#"{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":{"questions":[{"question":"Which colour do you prefer?","header":"Colour","options":[{"label":"Red","description":"The colour red"},{"label":"Blue","description":"The colour blue"}],"multiSelect":false}]},"tool_use_id":"tu1"}}"#;

const FAKE_CLAUDE: &str = r#"#!/bin/sh
out="$BACH_TEST_OUT"
echo $$ > "$out/pid"
echo "$@" > "$out/args"
read first
echo "$first" > "$out/first"
echo '{"type":"system","subtype":"init","session_id":"s1","model":"claude-test"}'
echo "$BACH_TEST_REQUEST"
read answer
echo "$answer" > "$out/answer"
echo '{"type":"result","subtype":"success","is_error":false,"total_cost_usd":0}'
# Like the real CLI, keep running until stdin is closed.
cat > /dev/null
"#;

struct Outcome {
    /// Runs still holding an MCP token once the agent had exited.
    tokens_left: usize,
    events: Vec<Value>,
    answer: Value,
    args: String,
    first: Value,
}

type Answers = Vec<(&'static str, &'static str)>;

fn answers_map(a: &Answers) -> Option<std::collections::HashMap<String, String>> {
    (!a.is_empty()).then(|| {
        a.iter()
            .map(|(q, v)| (q.to_string(), v.to_string()))
            .collect()
    })
}

/// A run that has been started and is waiting on the request the stand-in agent sent.
struct Waiting {
    satie: Satie,
    runs: Runs,
    run_id: String,
    rx: mpsc::Receiver<Value>,
    events: Vec<Value>,
    out: std::path::PathBuf,
}

static SCENARIO: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

async fn begin(dir: &Path, request: &str, rules: &[&str]) -> Waiting {
    let n = SCENARIO.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let out = dir.join(format!("out-{n}"));
    std::fs::create_dir_all(&out).unwrap();
    std::env::set_var("BACH_TEST_OUT", &out);
    std::env::set_var(
        "BACH_TEST_REQUEST",
        request.replace("__PWD__", &dir.to_string_lossy()),
    );

    let (tx, rx) = mpsc::channel();
    let satie = Satie::start(
        "127.0.0.1:0".parse().unwrap(),
        bach_core::store::Store::in_memory().unwrap(),
        dir.join("tasks"),
    )
    .await
    .unwrap();
    let runs = Runs::with_satie(Some(satie.clone()));
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
                session_key: None,
            },
        )
        .await
        .unwrap();

    let mut events = vec![];
    loop {
        let ev = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let is_approval = ev["type"] == "approval";
        events.push(ev);
        if is_approval {
            break;
        }
    }
    Waiting {
        satie,
        runs,
        run_id,
        rx,
        events,
        out,
    }
}

/// Reads until the turn ends, checks the agent then exits by itself, and collects what it saw.
async fn finish(mut w: Waiting) -> Outcome {
    loop {
        let ev = w.rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let done = ev["type"] == "done";
        w.events.push(ev);
        if done {
            break;
        }
    }
    // The process must exit on its own once the turn is over (stdin closed), not linger.
    let pid = std::fs::read_to_string(w.out.join("pid"))
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

    // The run's MCP token is revoked when the run is over.
    for _ in 0..40 {
        if w.satie.active_runs() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let read = |f: &str| std::fs::read_to_string(w.out.join(f)).unwrap();
    Outcome {
        tokens_left: w.satie.active_runs(),
        answer: serde_json::from_str(&read("answer")).unwrap(),
        args: read("args"),
        first: serde_json::from_str(&read("first")).unwrap(),
        events: w.events,
    }
}

async fn scenario(
    dir: &Path,
    request: &str,
    decision: Decision,
    message: Option<&str>,
    answers: Answers,
    rules: &[&str],
) -> Outcome {
    let w = begin(dir, request, rules).await;

    // Answering something that isn't pending is an error, not a hang or a bad write.
    assert!(w
        .runs
        .respond_approval(&w.run_id, "nope", decision, None, answers_map(&answers))
        .await
        .is_err());
    w.runs
        .respond_approval(
            &w.run_id,
            "r1",
            decision,
            message.map(String::from),
            answers_map(&answers),
        )
        .await
        .unwrap();
    assert!(
        w.runs
            .respond_approval(&w.run_id, "r1", decision, None, answers_map(&answers))
            .await
            .is_err(),
        "an approval can only be answered once"
    );
    finish(w).await
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
    let o = scenario(
        &dir,
        BASH_REQUEST,
        Decision::Allow,
        None,
        vec![],
        &["Bash(git status *)", "Read"],
    )
    .await;
    assert_eq!(o.first["type"], "user");
    assert_eq!(o.first["message"]["content"], "list tmux");
    for flag in [
        "--input-format stream-json",
        "--permission-prompt-tool stdio",
        "--allowedTools Bash(git status *) Read mcp__satie__task_list mcp__satie__task_logs",
    ] {
        assert!(o.args.contains(flag), "missing `{flag}` in: {}", o.args);
    }
    // Claude Code is handed Bach's MCP server (Satie), with a per-run token, and it is revoked after.
    assert!(
        o.args.contains("--append-system-prompt ") && o.args.contains("task_start"),
        "guidance missing"
    );
    assert!(
        o.args.contains("--settings ") && o.args.contains("PreToolUse"),
        "hook settings missing"
    );
    let cfg = &o.args[o.args.find("--mcp-config ").expect("--mcp-config missing") + 13..];
    assert!(cfg.starts_with(r#"{"mcpServers":{"satie":{"#), "{cfg}");
    assert!(
        cfg.contains(r#""type":"http""#) && cfg.contains("Bearer "),
        "{cfg}"
    );
    assert_eq!(
        o.tokens_left, 0,
        "the run's MCP token must be revoked once it ends"
    );
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
    let o = scenario(
        &dir,
        BASH_REQUEST,
        Decision::AllowSession,
        None,
        vec![],
        &[],
    )
    .await;
    let up = &o.answer["response"]["response"]["updatedPermissions"];
    assert_eq!(up[0]["rules"][0]["ruleContent"], "tmux ls *");
    assert_eq!(up[0]["destination"], "session");
    // ...and only the rule: no directory access, no permission-mode change.
    assert_eq!(
        up.as_array().unwrap().len(),
        1,
        "extra suggestions leaked: {up}"
    );
    // Only Satie's read-only tools are pre-approved; starting and stopping tasks still ask.
    assert!(
        o.args
            .contains("--allowedTools mcp__satie__task_list mcp__satie__task_logs"),
        "{}",
        o.args
    );
    assert!(
        !o.args.contains("task_start") || o.args.contains("--append-system-prompt"),
        "task_start must not be pre-approved"
    );
    assert!(
        !o.args.contains("--allowedTools mcp__satie__task_start")
            && !o.args.contains("mcp__satie__task_stop"),
        "{}",
        o.args
    );

    // Always allow: the suggestion as given (saved to the project's settings).
    let o = scenario(&dir, BASH_REQUEST, Decision::AllowAlways, None, vec![], &[]).await;
    assert_eq!(
        o.answer["response"]["response"]["updatedPermissions"][0]["destination"],
        "localSettings"
    );

    // Deny: the agent is told the user said no.
    let o = scenario(
        &dir,
        BASH_REQUEST,
        Decision::Deny,
        Some("Not on this machine."),
        vec![],
        &[],
    )
    .await;
    assert_eq!(o.answer["response"]["response"]["behavior"], "deny");
    assert_eq!(
        o.answer["response"]["response"]["message"],
        "Not on this machine."
    );
    assert!(o.answer["response"]["response"]
        .get("updatedInput")
        .is_none());

    // A question from the agent: allowing it with answers filled in, keyed by question text.
    let q = "Which colour do you prefer?";
    let o = scenario(
        &dir,
        QUESTION_REQUEST,
        Decision::Allow,
        None,
        vec![(q, "Blue")],
        &[],
    )
    .await;
    let ask = o.events.iter().find(|e| e["type"] == "approval").unwrap();
    assert_eq!(ask["tool_name"], "AskUserQuestion");
    assert_eq!(ask["input"]["questions"][0]["options"][1]["label"], "Blue");
    let input = &o.answer["response"]["response"]["updatedInput"];
    assert_eq!(input["answers"][q], "Blue");
    assert_eq!(
        input["questions"][0]["question"], q,
        "the questions come back unchanged"
    );
    assert!(o.answer["response"]["response"]
        .get("updatedPermissions")
        .is_none());

    // Declining to answer is a plain deny.
    let o = scenario(
        &dir,
        QUESTION_REQUEST,
        Decision::Deny,
        Some("Skipped."),
        vec![],
        &[],
    )
    .await;
    assert_eq!(o.answer["response"]["response"]["behavior"], "deny");

    // Bad answers are refused without consuming the request, so the right answer still works.
    let w = begin(&dir, QUESTION_REQUEST, &[]).await;
    let ask = |a: Answers| {
        w.runs
            .respond_approval(&w.run_id, "r1", Decision::Allow, None, answers_map(&a))
    };
    assert!(
        ask(vec![])
            .await
            .unwrap_err()
            .contains("Answer the question"),
        "no answers"
    );
    assert!(ask(vec![("Some other question?", "x")])
        .await
        .unwrap_err()
        .contains("isn't one of the questions"));
    ask(vec![(q, "Red")]).await.unwrap();
    let o = finish(w).await;
    assert_eq!(
        o.answer["response"]["response"]["updatedInput"]["answers"][q],
        "Red"
    );

    // Answers make no sense for an ordinary tool request.
    let w = begin(&dir, BASH_REQUEST, &[]).await;
    let err = w
        .runs
        .respond_approval(
            &w.run_id,
            "r1",
            Decision::Allow,
            None,
            answers_map(&vec![(q, "x")]),
        )
        .await;
    assert!(err.unwrap_err().contains("doesn't take answers"));
    w.runs
        .respond_approval(&w.run_id, "r1", Decision::Deny, None, None)
        .await
        .unwrap();
    finish(w).await;
    let _ = std::fs::remove_dir_all(dir);
}
