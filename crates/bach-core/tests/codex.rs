//! Codex through `codex app-server`: a stand-in `codex` speaks the protocol as recorded from the
//! real one (tests/fixtures/codex_*.jsonl), asks for one approval, and reports what it was sent.
use bach_core::{
    adapters::AgentKind,
    runs::{Decision, RunRequest, Runs},
};
use serde_json::Value;
use std::{os::unix::fs::PermissionsExt, path::Path, sync::mpsc, time::Duration};

const FAKE_CODEX: &str = r#"#!/bin/sh
out="$BACH_TEST_OUT"
echo $$ > "$out/pid"
echo "$@" > "$out/args"
echo "$BACH_MCP_TOKEN" > "$out/token"
read init
echo '{"id":1,"result":{"userAgent":"test","platformFamily":"unix","platformOs":"linux"}}'
read initialized
read thread
echo "$thread" > "$out/thread"
echo '{"id":2,"result":{"thread":{"id":"t1","turns":[]},"model":"gpt-test"}}'
read turn
echo "$turn" > "$out/turn"
echo '{"id":3,"result":{"turn":{"id":"u1","status":"inProgress"}}}'
echo '{"method":"item/started","params":{"item":{"type":"commandExecution","id":"c1","command":"/bin/zsh -lc '"'"'git push'"'"'","status":"inProgress"}}}'
echo '{"method":"item/commandExecution/requestApproval","id":0,"params":{"threadId":"t1","turnId":"u1","itemId":"c1","command":"/bin/zsh -lc '"'"'git push'"'"'","cwd":"/somewhere/else","reason":"needs network","proposedExecpolicyAmendment":["git","push"]}}'
# The answer, or a request to stop the turn.
read received
echo "$received" > "$out/received"
case "$received" in
  *turn/interrupt*) status=interrupted ;;
  *) status=completed
     echo '{"method":"item/completed","params":{"item":{"type":"commandExecution","id":"c1","command":"git push","status":"completed","aggregatedOutput":"pushed","exitCode":0}}}'
     echo '{"method":"item/completed","params":{"item":{"type":"agentMessage","id":"m1","text":"Pushed."}}}' ;;
esac
echo '{"method":"turn/completed","params":{"threadId":"t1","turn":{"id":"u1","status":"'$status'","error":null}}}'
# Like the real server, keep running until stdin is closed.
cat > /dev/null
"#;

struct Run {
    tasks: bach_tasks::Tasks,
    runs: Runs,
    run_id: String,
    rx: mpsc::Receiver<Value>,
    events: Vec<Value>,
    out: std::path::PathBuf,
}

static SCENARIO: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Starts a turn and waits for its approval request.
async fn begin(dir: &Path, session_id: Option<&str>) -> Run {
    let n = SCENARIO.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let out = dir.join(format!("out-{n}"));
    std::fs::create_dir_all(&out).unwrap();
    std::env::set_var("BACH_TEST_OUT", &out);

    let (tx, rx) = mpsc::channel();
    let tasks = bach_tasks::Tasks::start("127.0.0.1:0".parse().unwrap(), out.join("tasks")).await.unwrap();
    let runs = Runs::with_tasks(Some(tasks.clone()));
    let run_id = runs
        .start(
            std::sync::Arc::new(move |ev| {
                let _ = tx.send(serde_json::to_value(&ev).unwrap());
            }),
            RunRequest {
                agent: AgentKind::Codex,
                prompt: "push it".into(),
                images: vec![],
                cwd: Some(dir.to_string_lossy().into()),
                session_id: session_id.map(String::from),
                fork: None,
                model: None,
                permission_mode: None,
                effort: None,
                allowed_tools: vec![],
                session_key: None,
                run_id: None,
            },
        )
        .await
        .unwrap();
    let mut run = Run {
        tasks,
        runs,
        run_id,
        rx,
        events: vec![],
        out,
    };
    run.until("approval");
    run
}

impl Run {
    fn until(&mut self, kind: &str) {
        loop {
            let ev = self.rx.recv_timeout(Duration::from_secs(5)).unwrap();
            let found = ev["type"] == kind;
            self.events.push(ev);
            if found {
                return;
            }
        }
    }

    /// What the stand-in wrote down, once it has.
    fn file(&self, name: &str) -> Value {
        for _ in 0..100 {
            if let Some(v) = std::fs::read_to_string(self.out.join(name))
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
            {
                return v;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("the stand-in never wrote `{name}`");
    }

    /// The process exits by itself once the turn is over (stdin closed), rather than linger.
    async fn exited(&self) {
        let pid = std::fs::read_to_string(self.out.join("pid")).unwrap();
        for _ in 0..40 {
            if !Path::new(&format!("/proc/{}", pid.trim())).exists() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("the codex process is still running");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn codex_app_server_round_trip() {
    let dir = std::env::temp_dir().join(format!("bach-codex-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("codex");
    std::fs::write(&script, FAKE_CODEX).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::env::set_var(
        "PATH",
        format!("{}:{}", dir.display(), std::env::var("PATH").unwrap()),
    );

    // A new thread that may write to the project, and asks before going further.
    let mut run = begin(&dir, None).await;
    // Bach's MCP server is offered; its token comes through the environment, never the
    // command line.
    let args = std::fs::read_to_string(run.out.join("args")).unwrap();
    assert!(args.starts_with("app-server --enable default_mode_request_user_input "), "{args}");
    assert!(args.contains(&format!("mcp_servers.bach.url=\"{}\"", run.tasks.url())), "{args}");
    assert!(args.contains("mcp_servers.bach.bearer_token_env_var=\"BACH_MCP_TOKEN\""), "{args}");
    assert!(args.contains("developer_instructions=") && args.contains("task_start"), "{args}");
    let token = std::fs::read_to_string(run.out.join("token")).unwrap();
    assert!(token.trim().len() >= 16 && !args.contains(token.trim()), "token: {token}");
    assert_eq!(run.tasks.active_grants(), 1);
    let thread = run.file("thread");
    assert_eq!(thread["method"], "thread/start");
    assert_eq!(thread["params"]["sandbox"], "workspace-write");
    assert_eq!(thread["params"]["approvalPolicy"], "on-request");
    assert_eq!(thread["params"]["cwd"], dir.to_string_lossy().as_ref());
    assert_eq!(run.file("turn")["params"]["input"][0]["text"], "push it");

    let approval = run.events.last().unwrap().clone();
    assert_eq!(approval["toolName"], "Shell");
    assert_eq!(approval["input"]["command"], "git push");
    assert_eq!(approval["reason"], "needs network");
    assert_eq!(approval["rules"][0], "Shell(git push)");
    // The command runs outside the project, so the card says so.
    assert_eq!(approval["directories"][0], "/somewhere/else");
    assert!(approval.get("suggestions").is_none(), "stays on the backend");

    let request_id = approval["requestId"].as_str().unwrap();
    let rules = run
        .runs
        .respond_approval(&run.run_id, request_id, Decision::AllowSession, None, None)
        .await
        .unwrap();
    assert_eq!(rules, ["Shell(git push)"]);
    // No `availableDecisions` in the request, so Codex takes its own session answer.
    let answer = run.file("received");
    assert_eq!(answer["id"], 0);
    assert_eq!(answer["result"]["decision"], "acceptForSession");
    run.until("done");
    assert!(run.events.iter().any(|e| e["type"] == "text" && e["text"] == "Pushed."));
    assert_eq!(run.events.last().unwrap()["isError"], false);
    run.exited().await;
    // The token goes with the run.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(run.tasks.active_grants(), 0);

    // Stopping a resumed turn: Codex is asked to interrupt it, then the process goes.
    let mut run = begin(&dir, Some("t1")).await;
    let thread = run.file("thread");
    assert_eq!(thread["method"], "thread/resume");
    assert_eq!(thread["params"]["threadId"], "t1");
    // A thread first run by `codex exec` would otherwise stay read-only.
    assert_eq!(thread["params"]["sandbox"], "workspace-write");
    run.runs.cancel(&run.run_id).await;
    run.until("cancelled");
    let interrupt = run.file("received");
    assert_eq!(interrupt["method"], "turn/interrupt");
    assert_eq!(interrupt["params"]["threadId"], "t1");
    assert_eq!(interrupt["params"]["turnId"], "u1");
    run.exited().await;
}

/// The real Codex (and the user's Codex account): edits and commits in a scratch repo,
/// approving whatever it asks. `cargo test -p bach-core --test codex -- --ignored --nocapture`
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn real_codex_edits_and_commits() {
    let dir = std::env::temp_dir().join(format!("bach-real-codex-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(&dir)
            .output()
            .unwrap()
    };
    git(&["init", "-q"]);
    git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "start"]);

    let (tx, rx) = mpsc::channel();
    let runs = Runs::default();
    let run_id = runs
        .start(
            std::sync::Arc::new(move |ev| {
                let _ = tx.send(serde_json::to_value(&ev).unwrap());
            }),
            RunRequest {
                agent: AgentKind::Codex,
                prompt: "Create hello.txt containing hi, then commit it with git (message: add hello, author t <t@t>). Be brief.".into(),
                images: vec![],
                cwd: Some(dir.to_string_lossy().into()),
                session_id: None,
                fork: None,
                model: None,
                permission_mode: None,
                effort: None,
                allowed_tools: vec![],
                session_key: None,
                run_id: None,
            },
        )
        .await
        .unwrap();
    loop {
        let ev = rx.recv_timeout(Duration::from_secs(180)).unwrap();
        println!("{ev}");
        if ev["type"] == "approval" {
            runs.respond_approval(&run_id, ev["requestId"].as_str().unwrap(), Decision::Allow, None, None)
                .await
                .unwrap();
        }
        if ev["type"] == "done" {
            assert_eq!(ev["isError"], false);
            break;
        }
    }
    assert_eq!(std::fs::read_to_string(dir.join("hello.txt")).unwrap().trim(), "hi");
    let log = String::from_utf8(git(&["log", "--oneline"]).stdout).unwrap();
    assert!(log.contains("add hello"), "{log}");
}

/// The real Codex asking a question, answered with its first option. Whether it can ask
/// depends on its version and mode. `cargo test -p bach-core --test codex real_codex_question
/// -- --ignored --nocapture`
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn real_codex_question() {
    let dir = std::env::temp_dir().join(format!("bach-real-codex-q-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (tx, rx) = mpsc::channel();
    let runs = Runs::default();
    let run_id = runs
        .start(
            std::sync::Arc::new(move |ev| {
                let _ = tx.send(serde_json::to_value(&ev).unwrap());
            }),
            RunRequest {
                agent: AgentKind::Codex,
                prompt: "Use your request_user_input tool (not plain text) to ask me whether I prefer red or blue, then reply with just my answer.".into(),
                images: vec![],
                cwd: Some(dir.to_string_lossy().into()),
                session_id: None,
                fork: None,
                model: None,
                permission_mode: None,
                effort: None,
                allowed_tools: vec![],
                session_key: None,
                run_id: None,
            },
        )
        .await
        .unwrap();
    let mut deltas = 0;
    loop {
        let ev = rx.recv_timeout(Duration::from_secs(180)).unwrap();
        if ev["type"] == "text_delta" {
            deltas += 1;
            continue;
        }
        println!("{ev}");
        if ev["type"] == "approval" {
            let q = &ev["input"]["questions"][0];
            let answers = q["options"][0]["label"]
                .as_str()
                .map(|a| std::collections::HashMap::from([(q["question"].as_str().unwrap().to_string(), a.to_string())]));
            runs.respond_approval(&run_id, ev["requestId"].as_str().unwrap(), Decision::Allow, None, answers)
                .await
                .unwrap();
        }
        if ev["type"] == "done" {
            break;
        }
    }
    println!("{deltas} text deltas");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Runs one real Codex turn in `dir`, approving everything; returns the events, deltas included.
async fn real_turn(dir: &Path, prompt: &str, mode: Option<&str>, session_id: Option<&str>) -> Vec<Value> {
    let (tx, rx) = mpsc::channel();
    let runs = Runs::default();
    let run_id = runs
        .start(
            std::sync::Arc::new(move |ev| {
                let _ = tx.send(serde_json::to_value(&ev).unwrap());
            }),
            RunRequest {
                agent: AgentKind::Codex,
                prompt: prompt.into(),
                images: vec![],
                cwd: Some(dir.to_string_lossy().into()),
                session_id: session_id.map(String::from),
                fork: None,
                model: None,
                permission_mode: mode.map(String::from),
                effort: None,
                allowed_tools: vec![],
                session_key: None,
                run_id: None,
            },
        )
        .await
        .unwrap();
    let mut events = vec![];
    loop {
        let ev = rx.recv_timeout(Duration::from_secs(180)).unwrap();
        if ev["type"] == "approval" {
            runs.respond_approval(&run_id, ev["requestId"].as_str().unwrap(), Decision::Allow, None, None)
                .await
                .unwrap();
        }
        let done = ev["type"] == "done";
        events.push(ev);
        if done {
            return events;
        }
    }
}

/// The real Codex: its model list, command output as it runs, and plan mode then back.
/// `cargo test -p bach-core --test codex real_codex_settings -- --ignored --nocapture`
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn real_codex_settings_and_streams() {
    let models = bach_core::adapters::list_models(AgentKind::Codex, None, None).await.unwrap();
    println!("models: {:?}", models.iter().map(|m| (&m.id, m.is_default)).collect::<Vec<_>>());
    assert!(models.iter().any(|m| m.is_default));

    let dir = std::env::temp_dir().join(format!("bach-real-codex-s-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let ev = real_turn(&dir, "Run exactly this shell command, nothing else: `for i in 1 2 3; do echo line $i; sleep 1; done`", None, None).await;
    let outputs: Vec<&Value> = ev.iter().filter(|e| e["type"] == "delta" && e["kind"] == "output").collect();
    let thinking = ev.iter().filter(|e| e["type"] == "delta" && e["kind"] == "thinking").count();
    println!("{} output deltas: {:?}; {thinking} thinking deltas", outputs.len(), outputs.iter().map(|e| &e["text"]).collect::<Vec<_>>());
    assert!(!outputs.is_empty(), "no command output streamed");
    let thread = ev.iter().find(|e| e["type"] == "session").unwrap()["id"].as_str().unwrap().to_string();

    // Plan mode: it plans rather than writes. Then the next turn, back in the default mode, does.
    let ev = real_turn(&dir, "Create plan.txt containing hi.", Some("plan"), Some(&thread)).await;
    let texts: Vec<&str> = ev.iter().filter(|e| e["type"] == "text").filter_map(|e| e["text"].as_str()).collect();
    println!("plan turn said: {texts:#?}");
    assert!(texts.iter().any(|t| t.contains("plan.txt")), "no plan shown");
    assert!(!dir.join("plan.txt").exists(), "wrote a file in plan mode");
    let ev = real_turn(&dir, "Go ahead and create plan.txt containing hi now.", None, Some(&thread)).await;
    assert_eq!(ev.last().unwrap()["isError"], false);
    assert!(dir.join("plan.txt").exists(), "didn't write after leaving plan mode");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The real Codex with background tasks: it starts a server through `task_start`, which outlives the turn.
/// `cargo test -p bach-core --test codex real_codex_tasks -- --ignored --nocapture`
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn real_codex_tasks() {
    let dir = std::env::temp_dir().join(format!("bach-real-codex-tasks-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("proj")).unwrap();
    let project = dir.join("proj");
    let tasks = bach_tasks::Tasks::start("127.0.0.1:0".parse().unwrap(), dir.join("tasks")).await.unwrap();
    let (tx, rx) = mpsc::channel();
    let runs = Runs::with_tasks(Some(tasks.clone()));
    let run_id = runs
        .start(
            std::sync::Arc::new(move |ev| {
                let _ = tx.send(serde_json::to_value(&ev).unwrap());
            }),
            RunRequest {
                agent: AgentKind::Codex,
                prompt: "Start two web servers serving this folder, one on port 3977 and one on 3978 (python3 -m http.server <port>), as two separate tasks, so they keep running after you finish. Then list the running tasks and reply done.".into(),
                images: vec![],
                cwd: Some(project.to_string_lossy().into()),
                session_id: None,
                fork: None,
                model: None,
                permission_mode: None,
                effort: None,
                allowed_tools: vec![],
                session_key: None,
                run_id: None,
            },
        )
        .await
        .unwrap();
    // "For this session" on the first card; Codex shouldn't ask again for the second server.
    let mut cards = vec![];
    loop {
        let ev = rx.recv_timeout(Duration::from_secs(180)).unwrap();
        if ev["type"] == "delta" {
            continue;
        }
        println!("{ev}");
        if ev["type"] == "approval" {
            cards.push(ev["toolName"].as_str().unwrap().to_string());
            let rules = runs
                .respond_approval(&run_id, ev["requestId"].as_str().unwrap(), Decision::AllowSession, None, None)
                .await
                .unwrap();
            assert_eq!(rules, [ev["toolName"].as_str().unwrap()]);
        }
        if ev["type"] == "done" {
            break;
        }
    }
    // The token dies with the run; the task doesn't.
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(tasks.active_grants(), 0);
    let started = tasks.list(None);
    println!("tasks: {:?}", started.iter().map(|t| (&t.task.id, &t.task.name, format!("{:?}", t.task.status))).collect::<Vec<_>>());
    let up = tokio::net::TcpStream::connect("127.0.0.1:3977").await.is_ok();
    for t in &started {
        let _ = tasks.stop_task(&t.task.id).await;
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(started.len(), 2, "two background tasks");
    assert!(up, "the server isn't up after the turn");
    // One card for starting (listing is harmless), and none for the second start.
    assert_eq!(cards, ["mcp__bach__task_start"]);
}
