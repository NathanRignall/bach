//! Stopping a run must kill the process and tell the UI the run is over.
use bach_core::{
    adapters::AgentKind,
    runs::{RunRequest, Runs},
};
use std::{os::unix::fs::PermissionsExt, sync::mpsc, time::Duration};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_kills_the_agent_and_ends_the_run() {
    // A stand-in `claude` that reports a session and then hangs.
    let dir = std::env::temp_dir().join(format!("bach-cancel-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("claude");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\necho $$ >> {pid}\necho '{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s1\"}}'\nexec sleep 60\n",
            pid = dir.join("pid").display()
        ),
    )
    .unwrap();
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
                cwd: Some(dir.to_string_lossy().into()),
                session_id: None,
                model: None,
                allowed_tools: vec![],
                session_key: None,
                run_id: None,
            },
        )
        .await
        .unwrap();

    let first = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(first["type"], "session");

    runs.cancel(&run_id).await;
    let next = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(next["type"], "cancelled");
    assert_eq!(next["runId"], run_id.as_str());

    // The process is really gone, not just abandoned.
    let pid = std::fs::read_to_string(dir.join("pid"))
        .unwrap()
        .trim()
        .to_string();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !std::path::Path::new(&format!("/proc/{pid}")).exists(),
        "agent process {pid} still running"
    );

    // Stopping a whole UI session's runs, as deleting the session does: only its own runs
    // go, whether or not the page still remembers their ids.
    let _ = std::fs::remove_file(dir.join("pid"));
    let (tx, rx) = mpsc::channel();
    let runs = Runs::default();
    let start = |key: &str| {
        let tx = tx.clone();
        let req = RunRequest {
            agent: AgentKind::Claude,
            prompt: "hi".into(),
            cwd: Some(dir.to_string_lossy().into()),
            session_id: None,
            model: None,
            allowed_tools: vec![],
            session_key: Some(key.to_string()),
            run_id: None,
        };
        let runs = &runs;
        async move {
            runs.start(
                std::sync::Arc::new(move |ev| {
                    let _ = tx.send(serde_json::to_value(&ev).unwrap());
                }),
                req,
            )
            .await
            .unwrap()
        }
    };
    let (a1, a2, b1) = (start("a").await, start("a").await, start("b").await);
    for _ in 0..3 {
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap()["type"],
            "session"
        );
    }
    let pids: Vec<String> = std::fs::read_to_string(dir.join("pid"))
        .unwrap()
        .lines()
        .map(String::from)
        .collect();
    assert_eq!(pids.len(), 3);
    let alive = |pids: &[String]| {
        pids.iter()
            .filter(|p| std::path::Path::new(&format!("/proc/{p}")).exists())
            .count()
    };

    assert_eq!(
        runs.cancel_session("nope").await,
        0,
        "an unknown session has nothing to stop"
    );
    assert_eq!(alive(&pids), 3);

    assert_eq!(runs.cancel_session("a").await, 2);
    let mut cancelled: Vec<String> = (0..2)
        .map(|_| {
            let ev = rx.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(ev["type"], "cancelled");
            ev["runId"].as_str().unwrap().to_string()
        })
        .collect();
    cancelled.sort();
    let mut expect = vec![a1.clone(), a2.clone()];
    expect.sort();
    assert_eq!(cancelled, expect, "exactly session a's runs were stopped");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(alive(&pids), 1, "session b's run is untouched");

    assert_eq!(runs.cancel_session("b").await, 1);
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(5)).unwrap()["runId"],
        b1.as_str()
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(alive(&pids), 0);
    let _ = std::fs::remove_dir_all(dir);
}
