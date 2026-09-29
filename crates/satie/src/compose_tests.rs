//! process-compose projects run by Satie: real process-compose (from the devShell), real ports.
use crate::{
    compose::{files_dir, project_shell},
    tests::{free_port, run_in, satie_in, tmp},
    Satie, StartCompose, StartTask, TaskStatus, TaskView,
};
use serde_json::json;
use std::time::Duration;

async fn eventually(what: &str, secs: u64, mut cond: impl FnMut() -> bool) {
    for _ in 0..secs * 10 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("timed out waiting for: {what}");
}

/// Kills whatever the tests' tasks still run when it goes, even if an assertion failed first.
struct Cleanup(Satie);
impl Drop for Cleanup {
    fn drop(&mut self) {
        for v in self.0.list(None) {
            if v.task.status == TaskStatus::Running {
                crate::process::signal_session(v.task.pid, libc::SIGKILL);
            }
        }
    }
}

fn only_view(satie: &Satie) -> TaskView {
    let mut all = satie.list(None);
    assert_eq!(all.len(), 1, "{all:?}");
    all.remove(0)
}

#[tokio::test]
async fn runs_a_compose_project_and_follows_each_process() {
    let dir = tmp("compose");
    let satie = satie_in(&dir).await;
    let _cleanup = Cleanup(satie.clone());
    let port = free_port();
    std::fs::write(
        dir.join("pc.yaml"),
        format!(
            r#"version: "0.5"
processes:
  web:
    command: "python3 -m http.server {port} --bind 127.0.0.1"
    readiness_probe:
      http_get: {{ host: 127.0.0.1, port: {port}, path: / }}
      period_seconds: 1
  ticker:
    command: "i=0; while true; do i=$((i+1)); echo tick $i; printf '\e[32mgreen\e[0m\n'; sleep 0.2; done"
"#
        ),
    )
    .unwrap();

    let scope = run_in(&dir);
    let text = satie
        .call(&scope, "compose_start", &json!({ "file": "pc.yaml", "shell": "" }))
        .await
        .unwrap();
    assert!(text.contains(&format!("web: Running, ready, listening on {port}")), "{text}");
    assert!(text.contains("ticker: Running"), "{text}");
    assert!(!text.contains("Processes:"), "compose tasks list their processes instead: {text}");

    let view = only_view(&satie);
    let id = view.task.id.clone();
    assert_eq!(view.task.compose_file.as_deref(), Some("pc.yaml"));
    assert!(view.task.command.contains("process-compose up -f pc.yaml -t=false -U -u "), "{}", view.task.command);
    let names: Vec<&str> = view.compose.iter().flatten().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["ticker", "web"]);

    // Each process has a log of its own: no prefixes, colours kept.
    eventually("ticker output", 10, || {
        satie.logs(&id, 50, Some("ticker")).unwrap().matches("tick").count() >= 3
    })
    .await;
    let ticker = satie.logs(&id, 50, Some("ticker")).unwrap();
    assert!(ticker.starts_with("tick 1\n"), "the log starts at the beginning: {ticker}");
    assert!(ticker.contains("\x1b[32mgreen\x1b[0m") && !ticker.contains("[ticker"), "{ticker}");
    let chunk = satie.log_chunk(&id, Some("ticker"), None, 4096).unwrap();
    assert!(chunk.text.contains("tick"), "{chunk:?}");
    assert!(!satie.logs(&id, 50, Some("web")).unwrap().contains("tick"));
    assert!(satie.logs(&id, 5, Some("nope")).is_err());
    // The whole task's output is still there too.
    assert!(satie.logs(&id, 50, None).unwrap().contains("tick"));

    // One process restarts; the rest keep going, and its log carries on.
    let text = satie
        .call(&scope, "task_process", &json!({ "id": id, "process": "ticker", "action": "restart" }))
        .await
        .unwrap();
    assert!(text.contains("ticker: Running"), "{text}");
    assert!(text.contains("web: Running, ready"), "{text}");
    eventually("ticker starts over", 10, || {
        satie
            .logs(&id, 2000, Some("ticker"))
            .unwrap()
            .lines()
            .filter(|l| *l == "tick 1")
            .count()
            >= 2
    })
    .await;
    let err = satie
        .call(&scope, "task_process", &json!({ "id": id, "process": "nope", "action": "stop" }))
        .await
        .unwrap_err();
    assert!(err.contains("nope"), "{err}");
    let err = satie
        .call(&scope, "task_process", &json!({ "id": id, "process": "web", "action": "explode" }))
        .await
        .unwrap_err();
    assert!(err.contains("start, stop or restart"), "{err}");

    // Stopping one process from the API side: it shows as stopped, the project runs on.
    satie.process_action(&id, "web", crate::ProcessAction::Stop).await.unwrap();
    let web = || only_view(&satie).compose.unwrap().into_iter().find(|p| p.name == "web").unwrap();
    eventually("web stopped", 10, || !web().running).await;
    assert!(web().ports.is_empty(), "{:?}", web());
    assert_eq!(only_view(&satie).task.status, TaskStatus::Running);

    // Stopping the task ends everything, and the processes are shown as they were left.
    satie.stop_task(&id).await.unwrap();
    let view = only_view(&satie);
    assert!(view.compose.iter().flatten().all(|p| !p.running), "{view:?}");
    assert!(satie.logs(&id, 50, Some("ticker")).unwrap().contains("tick"), "logs outlive the project");

    satie.remove_task(&id).unwrap();
    assert!(!files_dir(&dir.join("tasks"), &id).exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn reports_a_process_that_fails_with_its_output() {
    // Deep enough that a socket next to Satie's files wouldn't fit in `sockaddr_un`.
    let dir = tmp("compose-fail").join("a-folder-name-long-enough-to-push-the-socket-path-past-the-limit");
    std::fs::create_dir_all(&dir).unwrap();
    let satie = satie_in(&dir).await;
    let _cleanup = Cleanup(satie.clone());
    // A file name that needs quoting.
    std::fs::write(
        dir.join("fail case.yaml"),
        r#"version: "0.5"
processes:
  steady:
    command: "sleep 60"
  broken:
    command: "echo boom >&2; exit 3"
"#,
    )
    .unwrap();
    let text = satie
        .call(&run_in(&dir), "compose_start", &json!({ "file": "fail case.yaml", "shell": "" }))
        .await
        .unwrap();
    assert!(text.contains("broken: Completed (exit code 3)"), "{text}");
    assert!(text.contains("Last output of broken:\nboom"), "{text}");
    assert!(text.contains("steady: Running"), "{text}");
    let task = &satie.list(None)[0].task;
    assert!(task.command.contains("-u /tmp/satie-"), "{}", task.command);
    for v in satie.list(None) {
        satie.stop_task(&v.task.id).await.unwrap();
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn starts_in_the_project_environment_and_checks_the_file() {
    let dir = tmp("compose-env");
    assert_eq!(project_shell(&dir), "");
    std::fs::write(dir.join("flake.nix"), "{}").unwrap();
    assert_eq!(project_shell(&dir), "nix develop --command");
    std::fs::write(dir.join(".envrc"), "use flake").unwrap();
    assert_eq!(project_shell(&dir), "direnv exec .");

    let satie = satie_in(&dir).await;
    let _cleanup = Cleanup(satie.clone());
    let missing = satie.start_compose(StartCompose {
        file: "nope.yaml".into(),
        task: StartTask { cwd: Some(dir.to_string_lossy().into()), ..Default::default() },
        ..Default::default()
    });
    assert!(matches!(missing, Err(crate::Error::Invalid(m)) if m.contains("nope.yaml")));

    // Any other task isn't a compose project.
    let t = satie.start_task(StartTask { command: "true".into(), cwd: Some(dir.to_string_lossy().into()), ..Default::default() }).unwrap();
    assert!(satie.list(None).iter().all(|v| v.compose.is_none()));
    assert!(satie.process_action(&t.id, "x", crate::ProcessAction::Stop).await.is_err());
    let _ = std::fs::remove_dir_all(dir);
}
