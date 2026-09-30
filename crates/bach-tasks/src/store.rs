//! bach-tasks' own SQLite database: one row per task, stored as JSON.
use crate::Task;
use rusqlite::{params, Connection};
use serde_json::Value;
use std::{path::Path, sync::Mutex};

/// Reads a task saved by any version of bach-tasks (before tasks had owners, they had run ids).
pub fn parse_task(mut v: Value) -> Option<Task> {
    if let Some(obj) = v.as_object_mut() {
        if let Some(run) = obj.remove("runId") {
            obj.entry("owner").or_insert(run);
        }
    }
    serde_json::from_value(v).ok()
}

pub(crate) struct TaskStore(Mutex<Connection>);

impl TaskStore {
    pub fn open(path: &Path) -> Result<Self, String> {
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS tasks (
                 id         TEXT PRIMARY KEY,
                 data       TEXT NOT NULL,
                 started_at INTEGER NOT NULL
             );",
        )
        .map_err(|e| e.to_string())?;
        Ok(Self(Mutex::new(conn)))
    }

    /// Every task, oldest first. Rows that no longer parse are skipped.
    pub fn list(&self) -> Result<Vec<Task>, String> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT data FROM tasks ORDER BY started_at ASC")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        Ok(rows
            .filter_map(|r| parse_task(serde_json::from_str(&r.ok()?).ok()?))
            .collect())
    }

    pub fn save(&self, task: &Task) -> Result<(), String> {
        let data = serde_json::to_string(task).map_err(|e| e.to_string())?;
        self.0
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO tasks (id, data, started_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(id) DO UPDATE SET data = excluded.data",
                params![task.id, data, task.started_at],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn delete(&self, id: &str) -> Result<(), String> {
        self.0
            .lock()
            .unwrap()
            .execute("DELETE FROM tasks WHERE id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}
