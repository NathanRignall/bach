//! The API as a transport sees it: named commands with JSON arguments, and pushed events.
use bach_core::Api;
use bach_protocol::{ErrorCode, ServerEvent};
use satie_protocol::{TaskEvent, TaskStatus};
use serde_json::{json, Value};
use std::time::Duration;

fn tmp(label: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("bach-api-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[tokio::test]
async fn background_tasks_move_to_satie_and_push_their_changes() {
    let dir = tmp("tasks");
    let db = dir.join("bach.db");

    // A database from before Satie kept its own, with one (long finished) task in it.
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        let old = json!({
            "id": "old1", "name": "serve", "command": "make serve", "cwd": dir, "project": dir,
            "runId": "run-9", "pid": 999_999_999u32, "startTicks": null, "startedAt": 1,
            "endedAt": null, "status": "running", "exitCode": null, "logPath": dir.join("old.log"),
        });
        conn.execute_batch(&format!(
            "CREATE TABLE tasks (id TEXT PRIMARY KEY, data TEXT NOT NULL, started_at INTEGER NOT NULL);
             INSERT INTO tasks VALUES ('old1', '{old}', 1);"
        ))
        .unwrap();
    }

    let api = Api::open(&db).await.unwrap();
    let tasks = api.call("list_tasks", Value::Null).await.unwrap();
    assert_eq!(tasks[0]["id"], "old1", "{tasks}");
    assert_eq!(tasks[0]["owner"], "run-9");
    assert_eq!(tasks[0]["status"], "lost", "its process is long gone");

    // Clients hear about tasks as they change, without asking.
    let mut events = api.subscribe();
    let started = api
        .call(
            "start_task",
            json!({ "command": "sleep 4709", "cwd": dir, "name": "sleeper" }),
        )
        .await
        .unwrap();
    let id = started["id"].as_str().unwrap().to_string();
    let event = loop {
        let ev = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("a task event")
            .unwrap();
        if let ServerEvent::Task(TaskEvent::Changed { task }) = ev {
            if task.task.id == id {
                break task;
            }
        }
    };
    assert_eq!(event.task.status, TaskStatus::Running);
    assert_eq!(event.task.name, "sleeper");

    let e = api
        .call("remove_task", json!({ "taskId": id }))
        .await
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::Invalid, "{e}");
    api.call("stop_task", json!({ "taskId": id })).await.unwrap();
    let e = api
        .call("stop_task", json!({ "taskId": "nope" }))
        .await
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::NotFound);
    let _ = std::fs::remove_dir_all(dir);
}
