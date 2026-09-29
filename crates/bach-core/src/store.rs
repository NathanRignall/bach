//! SQLite persistence for sessions: one row per [`Session`], and each session's transcript as
//! numbered [`LogEntry`] rows.
use bach_protocol::{AgentKind, Entry, LogEntry, PlanUsage, Session};
use rusqlite::{params, Connection, OptionalExtension};
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

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// The schema this code writes (`PRAGMA user_version`). 0 was sessions as the frontend's own
/// JSON, transcript included.
const SCHEMA: i64 = 1;

impl Store {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(err)?;
        }
        Self::init(Connection::open(path).map_err(err)?)
    }

    pub fn in_memory() -> Result<Self, String> {
        Self::init(Connection::open_in_memory().map_err(err)?)
    }

    fn init(mut conn: Connection) -> Result<Self, String> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS sessions (
                 id         TEXT PRIMARY KEY,
                 data       TEXT NOT NULL,
                 updated_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS entries (
                 session_id TEXT NOT NULL,
                 seq        INTEGER NOT NULL,
                 at         INTEGER NOT NULL,
                 data       TEXT NOT NULL,
                 PRIMARY KEY (session_id, seq)
             );
             CREATE TABLE IF NOT EXISTS meta (
                 key   TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );",
        )
        .map_err(err)?;
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(err)?;
        if version < 1 {
            let tx = conn.transaction().map_err(err)?;
            migrate_frontend_sessions(&tx)?;
            tx.pragma_update(None, "user_version", SCHEMA).map_err(err)?;
            tx.commit().map_err(err)?;
        }
        Ok(Self(Arc::new(Mutex::new(conn))))
    }

    /// All sessions, most recently active first.
    pub fn list(&self) -> Result<Vec<Session>, String> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT data FROM sessions ORDER BY updated_at DESC")
            .map_err(err)?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(err)?;
        rows.map(|r| serde_json::from_str(&r.map_err(err)?).map_err(err))
            .collect()
    }

    pub fn get(&self, id: &str) -> Result<Option<Session>, String> {
        get(&self.0.lock().unwrap(), id)
    }

    /// Saves a new session (or replaces one).
    pub fn put(&self, session: &Session) -> Result<(), String> {
        put(&self.0.lock().unwrap(), session)
    }

    /// Changes a session in place. `f` can refuse the change by returning an error, which is
    /// passed on; `Ok(None)` means there is no such session.
    pub fn update<E: From<String>>(
        &self,
        id: &str,
        f: impl FnOnce(&mut Session) -> Result<(), E>,
    ) -> Result<Option<Session>, E> {
        let conn = self.0.lock().unwrap();
        let Some(mut s) = get(&conn, id)? else {
            return Ok(None);
        };
        f(&mut s)?;
        put(&conn, &s)?;
        Ok(Some(s))
    }

    /// Adds an entry to a session's transcript, which also counts as activity. Returns the entry
    /// and the session as it now stands; `None` if there is no such session.
    pub fn append(&self, id: &str, entry: Entry) -> Result<Option<(LogEntry, Session)>, String> {
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction().map_err(err)?;
        let Some(mut s) = get(&tx, id)? else {
            return Ok(None);
        };
        let log = LogEntry {
            seq: s.last_seq + 1,
            at: now_ms(),
            entry,
        };
        tx.execute(
            "INSERT INTO entries (session_id, seq, at, data) VALUES (?1, ?2, ?3, ?4)",
            params![id, log.seq, log.at, serde_json::to_string(&log.entry).map_err(err)?],
        )
        .map_err(err)?;
        s.last_seq = log.seq;
        s.updated_at = s.updated_at.max(log.at);
        put(&tx, &s)?;
        tx.commit().map_err(err)?;
        Ok(Some((log, s)))
    }

    /// A session's transcript after `after` (0 for all of it).
    pub fn entries(&self, id: &str, after: u64) -> Result<Vec<LogEntry>, String> {
        let conn = self.0.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT seq, at, data FROM entries WHERE session_id = ?1 AND seq > ?2 ORDER BY seq")
            .map_err(err)?;
        let rows = stmt
            .query_map(params![id, after], |r| {
                Ok((r.get::<_, u64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?))
            })
            .map_err(err)?;
        rows.map(|r| {
            let (seq, at, data) = r.map_err(err)?;
            Ok(LogEntry {
                seq,
                at,
                entry: serde_json::from_str(&data).map_err(err)?,
            })
        })
        .collect()
    }

    pub fn delete(&self, id: &str) -> Result<(), String> {
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction().map_err(err)?;
        tx.execute("DELETE FROM entries WHERE session_id = ?1", params![id])
            .map_err(err)?;
        tx.execute("DELETE FROM sessions WHERE id = ?1", params![id])
            .map_err(err)?;
        tx.commit().map_err(err)
    }

    /// After a restart no run is live: forgets the runs sessions were in, and the approvals they
    /// were waiting on. Returns the sessions that changed.
    pub fn end_all_runs(&self) -> Result<Vec<Session>, String> {
        let mut changed = vec![];
        for s in self.list()? {
            if s.run_id.is_some() || !s.open_approvals.is_empty() {
                let updated = self.update::<String>(&s.id, |s| {
                    s.run_id = None;
                    s.open_approvals.clear();
                    Ok(())
                })?;
                changed.extend(updated);
            }
        }
        Ok(changed)
    }

    /// The account's usage limits as last reported.
    pub fn plan_usage(&self) -> Result<Option<PlanUsage>, String> {
        let conn = self.0.lock().unwrap();
        conn.query_row("SELECT value FROM meta WHERE key = 'plan_usage'", [], |r| {
            r.get::<_, String>(0)
        })
        .optional()
        .map_err(err)?
        .map(|v| serde_json::from_str(&v).map_err(err))
        .transpose()
    }

    pub fn set_plan_usage(&self, usage: &PlanUsage) -> Result<(), String> {
        self.0
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO meta (key, value) VALUES ('plan_usage', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![serde_json::to_string(usage).map_err(err)?],
            )
            .map_err(err)?;
        Ok(())
    }

    /// Background tasks from before Satie had its own database (the old `tasks` table), for
    /// handing over to it. Empty once [`drop_legacy_tasks`](Self::drop_legacy_tasks) has run.
    pub fn legacy_tasks(&self) -> Result<Vec<Value>, String> {
        let conn = self.0.lock().unwrap();
        let exists: bool = conn
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'tasks')",
                [],
                |r| r.get(0),
            )
            .map_err(err)?;
        if !exists {
            return Ok(vec![]);
        }
        let mut stmt = conn
            .prepare("SELECT data FROM tasks ORDER BY started_at ASC")
            .map_err(err)?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(err)?;
        Ok(rows
            .filter_map(|r| serde_json::from_str(&r.ok()?).ok())
            .collect())
    }

    pub fn drop_legacy_tasks(&self) -> Result<(), String> {
        self.0
            .lock()
            .unwrap()
            .execute_batch("DROP TABLE IF EXISTS tasks")
            .map_err(err)
    }
}

fn get(conn: &Connection, id: &str) -> Result<Option<Session>, String> {
    conn.query_row("SELECT data FROM sessions WHERE id = ?1", params![id], |r| {
        r.get::<_, String>(0)
    })
    .optional()
    .map_err(err)?
    .map(|data| serde_json::from_str(&data).map_err(err))
    .transpose()
}

fn put(conn: &Connection, s: &Session) -> Result<(), String> {
    conn.execute(
        "INSERT INTO sessions (id, data, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(id) DO UPDATE SET data = excluded.data, updated_at = excluded.updated_at",
        params![s.id, serde_json::to_string(s).map_err(err)?, s.updated_at],
    )
    .map_err(err)?;
    Ok(())
}

/// Sessions used to be the frontend's JSON, transcript included as rendered blocks. Each becomes
/// a [`Session`] whose transcript is one `imported` entry holding those blocks.
fn migrate_frontend_sessions(conn: &Connection) -> Result<(), String> {
    let old: Vec<(String, i64)> = {
        let mut stmt = conn
            .prepare("SELECT data, updated_at FROM sessions")
            .map_err(err)?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(err)?;
        rows.collect::<Result<_, _>>().map_err(err)?
    };
    for (data, updated_at) in old {
        let Ok(v) = serde_json::from_str::<Value>(&data) else {
            continue;
        };
        if v.get("createdAt").is_some() {
            continue; // already a Session
        }
        let Some(s) = session_from_frontend(&v, updated_at) else {
            continue;
        };
        let blocks = v.get("blocks").cloned().unwrap_or(Value::Array(vec![]));
        if blocks.as_array().is_some_and(|b| !b.is_empty()) {
            conn.execute(
                "INSERT OR REPLACE INTO entries (session_id, seq, at, data) VALUES (?1, 1, ?2, ?3)",
                params![s.id, updated_at, serde_json::to_string(&Entry::Imported { blocks }).map_err(err)?],
            )
            .map_err(err)?;
        }
        put(conn, &s)?;
    }
    Ok(())
}

fn session_from_frontend(v: &Value, updated_at: i64) -> Option<Session> {
    let text = |k: &str| v.get(k).and_then(Value::as_str).map(String::from);
    let flag = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
    let has_blocks = v["blocks"].as_array().is_some_and(|b| !b.is_empty());
    Some(Session {
        id: text("id")?,
        title: text("title").unwrap_or_else(|| "Session".into()),
        title_edited: flag("titleEdited"),
        agent: serde_json::from_value(v["agent"].clone()).unwrap_or(AgentKind::Claude),
        cwd: text("cwd").unwrap_or_default(),
        branch: text("branch"),
        worktree: flag("worktree"),
        model_choice: text("modelChoice").filter(|m| m != "default"),
        permission_mode: None,
        workdir: text("workdir"),
        git_branch: text("gitBranch"),
        workdir_removed: flag("workdirRemoved"),
        agent_session_id: text("agentSessionId"),
        allow_rules: serde_json::from_value(v["allowRules"].clone()).unwrap_or_default(),
        model: text("model"),
        context: None,
        run_id: None,
        archived: false,
        open_approvals: vec![],
        queued: vec![],
        created_at: updated_at,
        updated_at,
        last_seq: u64::from(has_blocks),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bach_protocol::AgentEvent;
    use serde_json::json;

    fn session(id: &str) -> Session {
        Session {
            id: id.into(),
            title: "t".into(),
            title_edited: false,
            agent: AgentKind::Claude,
            cwd: "/p".into(),
            branch: None,
            worktree: false,
            model_choice: None,
            permission_mode: None,
            workdir: Some("/p".into()),
            git_branch: None,
            workdir_removed: false,
            agent_session_id: None,
            allow_rules: vec![],
            model: None,
            context: None,
            run_id: None,
            archived: false,
            open_approvals: vec![],
            queued: vec![],
            created_at: 1,
            updated_at: 1,
            last_seq: 0,
        }
    }

    #[test]
    fn sessions_and_their_transcripts() {
        let s = Store::in_memory().unwrap();
        s.put(&session("a")).unwrap();
        s.put(&session("b")).unwrap();
        // Activity makes a session the most recent.
        let (e1, a) = s.append("a", Entry::User { text: "hi".into() }).unwrap().unwrap();
        let (e2, _) = s
            .append(
                "a",
                Entry::Agent {
                    run_id: "r".into(),
                    event: AgentEvent::Text { text: "hello".into(), parent: None },
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!((e1.seq, e2.seq, a.last_seq), (1, 2, 1));
        let ids: Vec<_> = s.list().unwrap().into_iter().map(|s| s.id).collect();
        assert_eq!(ids, ["a", "b"]);
        assert_eq!(s.entries("a", 0).unwrap().len(), 2);
        assert_eq!(s.entries("a", 1).unwrap()[0].seq, 2);
        assert!(s.append("nope", Entry::User { text: "x".into() }).unwrap().is_none());

        // Updates can refuse.
        let r = s.update("a", |s| {
            if s.run_id.is_none() {
                return Err("not running".to_string());
            }
            Ok(())
        });
        assert_eq!(r.unwrap_err(), "not running");
        let r = s.update::<String>("a", |s| {
            s.run_id = Some("r".into());
            s.open_approvals.push("q".into());
            Ok(())
        });
        assert_eq!(r.unwrap().unwrap().run_id.as_deref(), Some("r"));
        assert_eq!(s.end_all_runs().unwrap().len(), 1);
        let a = s.get("a").unwrap().unwrap();
        assert!(a.run_id.is_none() && a.open_approvals.is_empty());

        s.delete("a").unwrap();
        assert!(s.get("a").unwrap().is_none() && s.entries("a", 0).unwrap().is_empty());
    }

    #[test]
    fn turns_frontend_sessions_into_sessions_with_an_imported_transcript() {
        let dir = std::env::temp_dir().join(format!("bach-store-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bach.db");
        {
            // A database written by the previous version.
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE sessions (id TEXT PRIMARY KEY, data TEXT NOT NULL, updated_at INTEGER NOT NULL);",
            )
            .unwrap();
            let old = json!({
                "id": "s1", "title": "Fix it", "agent": "claude", "cwd": "/p", "worktree": true,
                "workdir": "/w", "gitBranch": "bach/fix", "agentSessionId": "cs", "modelChoice": "default",
                "allowRules": ["Bash(ls)"], "runId": "stale",
                "blocks": [{"kind": "user", "text": "fix it"}, {"kind": "text", "text": "done"}],
            });
            conn.execute(
                "INSERT INTO sessions VALUES ('s1', ?1, 42)",
                params![old.to_string()],
            )
            .unwrap();
        }
        let s = Store::open(&path).unwrap();
        let got = s.get("s1").unwrap().unwrap();
        assert_eq!(
            (got.title.as_str(), got.worktree, got.git_branch.as_deref(), got.agent_session_id.as_deref()),
            ("Fix it", true, Some("bach/fix"), Some("cs"))
        );
        assert_eq!(got.allow_rules, ["Bash(ls)"]);
        assert_eq!((got.model_choice, got.run_id, got.last_seq, got.updated_at), (None, None, 1, 42));
        let entries = s.entries("s1", 0).unwrap();
        assert!(
            matches!(&entries[0].entry, Entry::Imported { blocks } if blocks[1]["text"] == "done"),
            "{entries:?}"
        );
        // Opening again changes nothing.
        drop(s);
        let s = Store::open(&path).unwrap();
        assert_eq!(s.entries("s1", 0).unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn hands_over_tasks_from_the_old_table_once() {
        let s = Store::in_memory().unwrap();
        assert!(s.legacy_tasks().unwrap().is_empty());
        s.0.lock()
            .unwrap()
            .execute_batch(
                r#"CREATE TABLE tasks (id TEXT PRIMARY KEY, data TEXT NOT NULL, started_at INTEGER NOT NULL);
                   INSERT INTO tasks VALUES ('t1', '{"id":"t1","runId":"r"}', 1);"#,
            )
            .unwrap();
        assert_eq!(s.legacy_tasks().unwrap()[0]["runId"], "r");
        s.drop_legacy_tasks().unwrap();
        assert!(s.legacy_tasks().unwrap().is_empty());
    }
}
