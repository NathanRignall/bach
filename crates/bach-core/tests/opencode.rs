//! opencode through its server, for real (it uses the user's opencode providers).
//! `cargo test -p bach-core --test opencode -- --ignored --nocapture --test-threads 1`
use bach_core::{
    adapters::AgentKind,
    runs::{Decision, RunRequest, Runs},
};
use serde_json::Value;
use std::{collections::HashMap, path::Path, sync::mpsc, time::Duration};

fn request(dir: &Path, prompt: &str, session_id: Option<&str>, rules: &[&str]) -> RunRequest {
    RunRequest {
        agent: AgentKind::Opencode,
        prompt: prompt.into(),
        images: vec![],
        cwd: Some(dir.to_string_lossy().into()),
        session_id: session_id.map(String::from),
        model: None,
        permission_mode: None,
        effort: None,
        allowed_tools: rules.iter().map(|r| r.to_string()).collect(),
        session_key: None,
        run_id: None,
    }
}

/// Runs one turn, answering questions with their first option and allowing everything else.
/// Returns its events (deltas left out).
async fn turn(runs: &Runs, req: RunRequest) -> Vec<Value> {
    let (tx, rx) = mpsc::channel();
    let run_id = runs
        .start(std::sync::Arc::new(move |ev| drop(tx.send(serde_json::to_value(&ev).unwrap()))), req)
        .await
        .unwrap();
    let mut events = vec![];
    loop {
        let ev = rx.recv_timeout(Duration::from_secs(240)).unwrap();
        if ev["type"] == "delta" {
            continue;
        }
        println!("{}", ev.to_string().chars().take(300).collect::<String>());
        if ev["type"] == "approval" {
            let q = &ev["input"]["questions"][0];
            let answers = q["options"][0]["label"].as_str().map(|a| {
                HashMap::from([(q["question"].as_str().unwrap().to_string(), a.to_string())])
            });
            runs.respond_approval(&run_id, ev["requestId"].as_str().unwrap(), Decision::Allow, None, answers)
                .await
                .unwrap();
        }
        let end = ev["type"] == "done" || ev["type"] == "cancelled";
        events.push(ev);
        if end {
            return events;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn real_opencode_turns() {
    let dir = std::env::temp_dir().join(format!("bach-real-opencode-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let runs = Runs::default();

    // A question, then a shell command, which asks first in the default mode.
    let ev = turn(&runs, request(&dir, "First use your question tool to ask me whether I prefer red or blue. Then run the shell command: echo <colour> > colour.txt (with my answer). Then say done.", None, &[])).await;
    let session = ev.iter().find(|e| e["type"] == "session").unwrap()["id"].as_str().unwrap().to_string();
    assert!(ev.iter().any(|e| e["type"] == "approval" && e["toolName"] == "AskUserQuestion"));
    assert!(ev.iter().any(|e| e["type"] == "approval" && e["toolName"] == "Shell"));
    assert_eq!(ev.last().unwrap()["isError"], false);
    assert_eq!(std::fs::read_to_string(dir.join("colour.txt")).unwrap().trim().to_lowercase(), "red");

    // opencode may be a wrapper that sandboxes it to where it starts: its server runs in the
    // session's folder.
    let folder = dir.canonicalize().unwrap();
    let in_folder = std::fs::read_dir("/proc").unwrap().flatten().any(|p| {
        let cmd = std::fs::read(p.path().join("cmdline")).unwrap_or_default();
        String::from_utf8_lossy(&cmd).contains("opencode\0serve")
            && std::fs::read_link(p.path().join("cwd")).ok().as_deref() == Some(folder.as_path())
    });
    assert!(in_folder, "no opencode server running in {}", folder.display());

    // The same session again, with a rule approved earlier: no card this time.
    let ev = turn(&runs, request(&dir, "Run the shell command: echo again >> colour.txt. Then say done.", Some(&session), &["bash(echo *)"])).await;
    assert!(!ev.iter().any(|e| e["type"] == "approval"), "asked despite the rule");
    assert!(std::fs::read_to_string(dir.join("colour.txt")).unwrap().contains("again"));

    // Stopping a turn mid-command.
    let (tx, rx) = mpsc::channel();
    let run_id = runs
        .start(
            std::sync::Arc::new(move |ev| drop(tx.send(serde_json::to_value(&ev).unwrap()))),
            request(&dir, "Run the shell command: sleep 60", Some(&session), &["bash(sleep *)"]),
        )
        .await
        .unwrap();
    loop {
        let ev = rx.recv_timeout(Duration::from_secs(120)).unwrap();
        if ev["type"] == "tool_use" {
            break;
        }
    }
    runs.cancel(&run_id).await;
    loop {
        let ev = rx.recv_timeout(Duration::from_secs(30)).unwrap();
        if ev["type"] == "cancelled" {
            break;
        }
    }
    assert!(!runs.is_live(&run_id).await);
    let _ = std::fs::remove_dir_all(&dir);
}
