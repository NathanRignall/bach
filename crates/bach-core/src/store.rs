//! SQLite persistence for chat sessions. Sessions are opaque JSON owned by the frontend
//! (only `id` is read here), so the UI can evolve its shape without migrations.
use rusqlite::{params, Connection};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone)]
pub struct Store(Arc<Mutex<Connection>>);

/// `$BACH_DB`, else `$XDG_DATA_HOME/bach/bach.db`, else `~/.local/share/bach/bach.db`.
pub fn default_db_path() -> PathBuf {
    if let Ok(p) = std::env::var("BACH_DB") {
        return p.into();
    }
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    data.join("bach/bach.db")
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        Self::init(Connection::open(path).map_err(|e| e.to_string())?)
    }

    pub fn in_memory() -> Result<Self, String> {
        Self::init(Connection::open_in_memory().map_err(|e| e.to_string())?)
    }

    fn init(conn: Connection) -> Result<Self, String> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS sessions (
                 id         TEXT PRIMARY KEY,
                 data       TEXT NOT NULL,
                 updated_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS tasks (
                 id         TEXT PRIMARY KEY,
                 data       TEXT NOT NULL,
                 started_at INTEGER NOT NULL
             );",
        )
        .map_err(|e| e.to_string())?;
        Ok(Self(Arc::new(Mutex::new(conn))))
    }

    /// All sessions, most recently saved first.
    pub fn list(&self) -> Result<Vec<Value>, String> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT data FROM sessions ORDER BY updated_at DESC")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        rows.map(|r| {
            let text = r.map_err(|e| e.to_string())?;
            serde_json::from_str(&text).map_err(|e| e.to_string())
        })
        .collect()
    }

    pub fn save(&self, session: &Value) -> Result<(), String> {
        let id = session["id"].as_str().ok_or("session has no string `id`")?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64);
        let conn = self.0.lock().unwrap();
        // Strictly increasing, so "most recently saved" is well defined within one millisecond.
        let newest: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(updated_at), 0) FROM sessions",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        let updated_at = now.max(newest + 1);
        conn.execute(
            "INSERT INTO sessions (id, data, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET data = excluded.data, updated_at = excluded.updated_at",
            params![id, session.to_string(), updated_at],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Background tasks (see `satie`), oldest first. Stored as JSON; only `id` is read here.
    pub fn list_tasks(&self) -> Result<Vec<Value>, String> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT data FROM tasks ORDER BY started_at ASC")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        rows.map(|r| {
            serde_json::from_str(&r.map_err(|e| e.to_string())?).map_err(|e| e.to_string())
        })
        .collect()
    }

    pub fn save_task(&self, task: &Value) -> Result<(), String> {
        let id = task["id"].as_str().ok_or("task has no string `id`")?;
        let started = task["startedAt"].as_i64().unwrap_or(0);
        self.0
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO tasks (id, data, started_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(id) DO UPDATE SET data = excluded.data",
                params![id, task.to_string(), started],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn delete_task(&self, id: &str) -> Result<(), String> {
        self.0
            .lock()
            .unwrap()
            .execute("DELETE FROM tasks WHERE id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn delete(&self, id: &str) -> Result<(), String> {
        self.0
            .lock()
            .unwrap()
            .execute("DELETE FROM sessions WHERE id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn save_list_update_delete() {
        let s = Store::in_memory().unwrap();
        s.save(&json!({"id": "a", "title": "one"})).unwrap();
        s.save(&json!({"id": "b", "title": "two"})).unwrap();
        s.save(&json!({"id": "a", "title": "one v2"})).unwrap(); // upsert, becomes newest
        let ids: Vec<_> = s
            .list()
            .unwrap()
            .iter()
            .map(|v| v["id"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(ids, ["a", "b"]);
        assert_eq!(s.list().unwrap()[0]["title"], "one v2");
        s.delete("a").unwrap();
        assert_eq!(s.list().unwrap().len(), 1);
        assert!(s.save(&json!({"title": "no id"})).is_err());
    }

    #[test]
    fn persists_across_reopen() {
        let dir = std::env::temp_dir().join(format!("bach-test-{}", std::process::id()));
        let path = dir.join("nested/bach.db");
        Store::open(&path)
            .unwrap()
            .save(&json!({"id": "x", "blocks": [1, 2]}))
            .unwrap();
        assert_eq!(
            Store::open(&path).unwrap().list().unwrap()[0]["blocks"],
            json!([1, 2])
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
