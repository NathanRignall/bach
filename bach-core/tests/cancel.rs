//! Stopping a run must kill the process and tell the UI the run is over.
use bach_core::{adapters::AgentKind, runs::Runs};
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
            "#!/bin/sh\necho $$ > {pid}\necho '{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s1\"}}'\nexec sleep 60\n",
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
            AgentKind::Claude,
            "hi".into(),
            Some(dir.to_string_lossy().into()),
            None,
            None,
        )
        .await
        .unwrap();

    let first = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(first["type"], "session");

    runs.cancel(&run_id).await;
    let next = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(next["type"], "cancelled");
    assert_eq!(next["run_id"], run_id.as_str());

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
    let _ = std::fs::remove_dir_all(dir);
}
