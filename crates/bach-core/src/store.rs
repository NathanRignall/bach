//! SQLite persistence for sessions: one row per [`Session`], and each session's transcript as
//! numbered [`LogEntry`] rows.
use bach_protocol::{
    AgentEvent, AgentKind, Entry, LogEntry, PlanUsage, SearchHit, SearchKind, SearchResult, Session,
    SnippetPart,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use std::{
    collections::HashMap,
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

/// What the search index holds (`meta`'s `search_index`). Change it when `searchable` does, and
/// the next start indexes every transcript again.
const SEARCH_INDEX: &str = "2";

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
             CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )
        .map_err(err)?;
        // An index built another way (see `SEARCH_INDEX`) can't be reused: start it over.
        let indexed: Option<String> = conn
            .query_row("SELECT value FROM meta WHERE key = 'search_index'", [], |r| r.get(0))
            .optional()
            .map_err(err)?;
        if indexed.as_deref() != Some(SEARCH_INDEX) {
            conn.execute_batch(
                "DROP TRIGGER IF EXISTS search_text_insert;
                 DROP TRIGGER IF EXISTS search_text_delete;
                 DROP TABLE IF EXISTS search_index;
                 DROP TABLE IF EXISTS search_text;",
            )
            .map_err(err)?;
        }
        conn.execute_batch(
            "
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
             );
             -- What search looks through: the text of transcript entries (see `searchable`), and a
             -- full-text index over it that the triggers keep current.
             CREATE TABLE IF NOT EXISTS search_text (
                 id         INTEGER PRIMARY KEY,
                 session_id TEXT NOT NULL,
                 seq        INTEGER NOT NULL,
                 kind       TEXT NOT NULL,
                 text       TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS search_text_session ON search_text (session_id);
             CREATE VIRTUAL TABLE IF NOT EXISTS search_index USING fts5(
                 text, content = 'search_text', content_rowid = 'id', tokenize = 'trigram'
             );
             CREATE TRIGGER IF NOT EXISTS search_text_insert AFTER INSERT ON search_text BEGIN
                 INSERT INTO search_index (rowid, text) VALUES (new.id, new.text);
             END;
             CREATE TRIGGER IF NOT EXISTS search_text_delete AFTER DELETE ON search_text BEGIN
                 INSERT INTO search_index (search_index, rowid, text) VALUES ('delete', old.id, old.text);
             END;",
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
        if indexed.as_deref() != Some(SEARCH_INDEX) {
            rebuild_search(&mut conn)?;
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

    /// Saves a new session together with transcript entries it starts with (numbered as they
    /// are, which must be 1, 2, 3, …).
    pub fn put_with_entries(&self, session: &Session, entries: &[LogEntry]) -> Result<(), String> {
        let mut conn = self.0.lock().unwrap();
        let tx = conn.transaction().map_err(err)?;
        for e in entries {
            tx.execute(
                "INSERT INTO entries (session_id, seq, at, data) VALUES (?1, ?2, ?3, ?4)",
                params![session.id, e.seq, e.at, serde_json::to_string(&e.entry).map_err(err)?],
            )
            .map_err(err)?;
        }
        let mut session = session.clone();
        session.last_seq = entries.last().map_or(0, |e| e.seq);
        put(&tx, &session)?;
        tx.commit().map_err(err)
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
        index_entry(&tx, id, log.seq, &log.entry)?;
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
        tx.execute("DELETE FROM search_text WHERE session_id = ?1", params![id])
            .map_err(err)?;
        tx.execute("DELETE FROM sessions WHERE id = ?1", params![id])
            .map_err(err)?;
        tx.commit().map_err(err)
    }

    /// Sessions whose title or transcript contain all the words of `query` (anywhere in a word),
    /// title matches first and then the most recently active, at most `limit`. Each has its best
    /// matching entries. The transcript needs a word of 3+ characters to be searched at all.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, String> {
        let title_words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        if title_words.is_empty() {
            return Ok(vec![]);
        }
        let words = words(query);
        let fts = match_expression(&words);
        let conn = self.0.lock().unwrap();

        // Transcript matches, best first. A word in every session ("the") can match far more
        // entries than anyone reads, so only the best are taken.
        let mut found: HashMap<String, (u32, Vec<SearchHit>)> = HashMap::new();
        if !fts.is_empty() {
            // Shorter words can't be looked up in the index; they only narrow what it finds.
            let short: Vec<&String> = words.iter().filter(|w| w.chars().count() < MIN_INDEXED_WORD).collect();
            let narrow: String = short.iter().map(|_| " AND instr(lower(t.text), ?) > 0").collect();
            let sql = format!(
                "SELECT t.session_id, t.seq, t.kind,
                        t.text
                 FROM search_index JOIN search_text t ON t.id = search_index.rowid
                 WHERE search_index MATCH ?{narrow}
                 ORDER BY rank LIMIT {SEARCH_ROWS}"
            );
            let mut stmt = conn.prepare(&sql).map_err(err)?;
            let args: Vec<&dyn rusqlite::ToSql> = std::iter::once(&fts as &dyn rusqlite::ToSql)
                .chain(short.iter().map(|w| *w as &dyn rusqlite::ToSql))
                .collect();
            let rows = stmt
                .query_map(args.as_slice(), |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, u64>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                })
                .map_err(err)?;
            for row in rows {
                let (session_id, seq, kind, text) = row.map_err(err)?;
                let (count, hits) = found.entry(session_id).or_default();
                *count += 1;
                if hits.len() < HITS_PER_SESSION {
                    let kind = match kind.as_str() {
                        "user" => SearchKind::User,
                        "tool" => SearchKind::Tool,
                        _ => SearchKind::Agent,
                    };
                    hits.push(SearchHit { seq, kind, snippet: snippet(&text, &words) });
                }
            }
        }

        let mut results = vec![];
        let mut stmt = conn
            .prepare("SELECT data FROM sessions ORDER BY updated_at DESC")
            .map_err(err)?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(err)?;
        for row in rows {
            let s: Session = serde_json::from_str(&row.map_err(err)?).map_err(err)?;
            let title = s.title.to_lowercase();
            let title_match = title_words.iter().all(|w| title.contains(w.as_str()));
            let (hit_count, hits) = found.remove(&s.id).unwrap_or_default();
            if title_match || hit_count > 0 {
                results.push((
                    title_match,
                    SearchResult { session_id: s.id, title_match, hits, hit_count },
                ));
            }
        }
        // Stable: the recency order from the query stays within each group.
        results.sort_by_key(|(title_match, _)| !title_match);
        Ok(results.into_iter().map(|(_, r)| r).take(limit).collect())
    }

    /// After a restart no run is live: forgets the runs sessions were in, and the approvals they
    /// were waiting on. Returns the sessions that changed.
    pub fn end_all_runs(&self) -> Result<Vec<Session>, String> {
        let mut changed = vec![];
        for s in self.list()? {
            if s.run_id.is_some() || !s.open_approvals.is_empty() {
                let updated = self.update::<String>(&s.id, |s| {
                    // A run lost to the restart never got to report; say it didn't end well.
                    if s.run_id.is_some() {
                        s.unseen = Some(bach_protocol::RunOutcome::Failed);
                    }
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

    /// Background tasks from before bach-tasks had its own database (the old `tasks` table), for
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

/// Transcript matches read per search, before they're grouped by session.
const SEARCH_ROWS: usize = 400;
const HITS_PER_SESSION: usize = 3;
/// An entry's text is indexed up to this many characters (a file an agent wrote can be huge).
const MAX_INDEXED: usize = 20_000;

/// Shortest word the trigram index can look for.
const MIN_INDEXED_WORD: usize = 3;

/// The words typed (letters and digits), lowercased.
fn words(query: &str) -> Vec<String> {
    query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// The FTS5 query for the words the index can look for (3+ characters), each quoted so it's
/// never query syntax. Every one must be in the entry, anywhere in a word. Empty if none.
fn match_expression(words: &[String]) -> String {
    words
        .iter()
        .filter(|w| w.chars().count() >= MIN_INDEXED_WORD)
        .map(|w| format!("\"{w}\""))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Characters of an entry shown around a match, and how many of them come before it.
const SNIPPET_LEN: usize = 150;
const SNIPPET_LEAD: usize = 40;

/// A stretch of `text` around the first of `words` (lowercase), with every occurrence marked.
fn snippet(text: &str, words: &[String]) -> Vec<SnippetPart> {
    let chars: Vec<char> = text.chars().collect();
    let lower: Vec<char> = chars.iter().map(|c| c.to_lowercase().next().unwrap_or(*c)).collect();
    let mut marked = vec![false; chars.len()];
    for w in words {
        let w: Vec<char> = w.chars().collect();
        for i in 0..(lower.len() + 1).saturating_sub(w.len()) {
            if lower[i..i + w.len()] == w[..] {
                marked[i..i + w.len()].fill(true);
            }
        }
    }
    let first = marked.iter().position(|m| *m).unwrap_or(0);
    let start = first.saturating_sub(SNIPPET_LEAD);
    let end = (start + SNIPPET_LEN).min(chars.len());
    let mut parts: Vec<SnippetPart> = vec![];
    let mut push = |c: char, matched: bool| match parts.last_mut() {
        Some(p) if p.matched == matched => p.text.push(c),
        _ => parts.push(SnippetPart { text: c.to_string(), matched }),
    };
    if start > 0 {
        push('…', false);
    }
    for i in start..end {
        push(chars[i], marked[i]);
    }
    if end < chars.len() {
        push('…', false);
    }
    parts
}

/// What of a transcript entry search looks through: what the user and the agent said, and the
/// inputs of the agent's tool calls (as the words in them, not JSON).
fn searchable(entry: &Entry) -> Vec<(&'static str, String)> {
    let mut found = vec![];
    match entry {
        Entry::User { text, .. } => found.push(("user", text.clone())),
        Entry::Agent { event: AgentEvent::Text { text, .. }, .. } => found.push(("agent", text.clone())),
        Entry::Agent { event: AgentEvent::ToolUse { name, input, .. }, .. } => {
            let mut text = name.clone();
            collect_strings(input, &mut text);
            found.push(("tool", text));
        }
        Entry::Imported { blocks } => {
            for b in blocks.as_array().into_iter().flatten() {
                let kind = match b["kind"].as_str() {
                    Some("user") => "user",
                    Some("text") => "agent",
                    _ => continue,
                };
                if let Some(text) = b["text"].as_str() {
                    found.push((kind, text.to_string()));
                }
            }
        }
        _ => {}
    }
    found.retain(|(_, t)| !t.trim().is_empty());
    for (_, t) in &mut found {
        if let Some((i, _)) = t.char_indices().nth(MAX_INDEXED) {
            t.truncate(i);
        }
    }
    found
}

/// Appends every string in `v` (values, not keys) to `out`, one per line.
fn collect_strings(v: &Value, out: &mut String) {
    match v {
        Value::String(s) => {
            out.push('\n');
            out.push_str(s);
        }
        Value::Array(a) => a.iter().for_each(|v| collect_strings(v, out)),
        Value::Object(o) => o.values().for_each(|v| collect_strings(v, out)),
        _ => {}
    }
}

fn index_entry(conn: &Connection, session_id: &str, seq: u64, entry: &Entry) -> Result<(), String> {
    for (kind, text) in searchable(entry) {
        conn.execute(
            "INSERT INTO search_text (session_id, seq, kind, text) VALUES (?1, ?2, ?3, ?4)",
            params![session_id, seq, kind, text],
        )
        .map_err(err)?;
    }
    Ok(())
}

/// Indexes every transcript, into an empty index (see `init`).
fn rebuild_search(conn: &mut Connection) -> Result<(), String> {
    let tx = conn.transaction().map_err(err)?;
    {
        let mut stmt = tx
            .prepare("SELECT session_id, seq, data FROM entries ORDER BY session_id, seq")
            .map_err(err)?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u64>(1)?, r.get::<_, String>(2)?)))
            .map_err(err)?;
        for row in rows {
            let (session_id, seq, data) = row.map_err(err)?;
            // An entry this version can't read has nothing to find in it.
            if let Ok(entry) = serde_json::from_str::<Entry>(&data) {
                index_entry(&tx, &session_id, seq, &entry)?;
            }
        }
    }
    tx.execute(
        "INSERT INTO meta (key, value) VALUES ('search_index', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![SEARCH_INDEX],
    )
    .map_err(err)?;
    tx.commit().map_err(err)
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
        effort: None,
        workdir: text("workdir"),
        git_branch: text("gitBranch"),
        workdir_removed: flag("workdirRemoved"),
        agent_session_id: text("agentSessionId"),
        allow_rules: serde_json::from_value(v["allowRules"].clone()).unwrap_or_default(),
        model: text("model"),
        context: None,
        run_id: None,
        archived: false,
        origin: None,
        pending_fork: None,
        open_approvals: vec![],
        unseen: None,
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
            effort: None,
            workdir: Some("/p".into()),
            git_branch: None,
            workdir_removed: false,
            agent_session_id: None,
            allow_rules: vec![],
            model: None,
            context: None,
            run_id: None,
            archived: false,
            origin: None,
            pending_fork: None,
            open_approvals: vec![],
            unseen: None,
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
        let (e1, a) = s.append("a", Entry::User { text: "hi".into(), images: vec![] }).unwrap().unwrap();
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
        assert!(s.append("nope", Entry::User { text: "x".into(), images: vec![] }).unwrap().is_none());

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

    fn say(s: &Store, id: &str, text: &str) -> u64 {
        let e = Entry::Agent {
            run_id: "r".into(),
            event: AgentEvent::Text { text: text.into(), parent: None },
        };
        s.append(id, e).unwrap().unwrap().0.seq
    }

    fn found(s: &Store, q: &str) -> Vec<(String, bool, Vec<u64>)> {
        s.search(q, 30)
            .unwrap()
            .into_iter()
            .map(|r| (r.session_id, r.title_match, r.hits.iter().map(|h| h.seq).collect()))
            .collect()
    }

    #[test]
    fn searches_titles_and_transcripts_across_sessions() {
        let s = Store::in_memory().unwrap();
        let mut a = session("a");
        a.title = "Fix the Sidebar".into();
        let mut b = session("b");
        b.title = "Other".into();
        s.put(&a).unwrap();
        s.put(&b).unwrap();
        s.append("a", Entry::User { text: "please rename the flux capacitor".into(), images: vec![] }).unwrap();
        let reply = say(&s, "a", "Renamed it; the capacitor works.");
        s.append(
            "b",
            Entry::Agent {
                run_id: "r".into(),
                event: AgentEvent::ToolUse {
                    id: "t".into(),
                    name: "Bash".into(),
                    input: json!({"command": "grep -r capacitor src", "description": "look"}),
                    parent: None,
                },
            },
        )
        .unwrap();
        // Not searched: reasoning and tool output.
        s.append("b", Entry::Agent { run_id: "r".into(), event: AgentEvent::Thinking { text: "capacitor?".into() } }).unwrap();

        // Titles: any case, in the middle of a word, every word needed.
        assert_eq!(found(&s, "SIDEBAR"), [("a".into(), true, vec![])]);
        assert_eq!(found(&s, "the side"), [("a".into(), true, vec![])]);
        assert!(found(&s, "sidebar nothing").is_empty());
        // Transcript: words match anywhere in a word; every word must be in the entry.
        let mut both = found(&s, "capac");
        both.iter_mut().for_each(|b| b.2.sort());
        both.sort();
        assert_eq!(both, [("a".into(), false, vec![1, reply]), ("b".into(), false, vec![1])]);
        assert_eq!(found(&s, "renamed capacitor"), [("a".into(), false, vec![reply])]);
        let mut inner = found(&s, "apacit");
        inner.sort();
        assert_eq!(inner.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        // Quotes and operators in what's typed are just punctuation; a short word only narrows.
        assert_eq!(found(&s, "\"flux\" OR capacitor -").len(), 1);
        assert!(found(&s, "flux zz").is_empty());
        assert!(found(&s, "fl").is_empty(), "too short to search transcripts");
        assert!(found(&s, "   ").is_empty() && found(&s, "\"").is_empty());

        let hit = &s.search("flux", 30).unwrap()[0].hits[0];
        assert_eq!(hit.kind, SearchKind::User);
        assert!(hit.snippet.iter().any(|p| p.matched && p.text == "flux"), "{:?}", hit.snippet);
        // A long entry is cut around the match, with every occurrence marked, in any case.
        let long = format!("{} The FLUX here and flux there. {}", "x ".repeat(100), "y ".repeat(100));
        let parts = snippet(&long, &["flux".into()]);
        let shown: String = parts.iter().map(|p| p.text.as_str()).collect();
        assert!(shown.starts_with('…') && shown.ends_with('…') && shown.chars().count() <= SNIPPET_LEN + 2);
        assert_eq!(parts.iter().filter(|p| p.matched).map(|p| p.text.as_str()).collect::<Vec<_>>(), ["FLUX", "flux"]);
        assert_eq!(s.search("capac", 1).unwrap().len(), 1);
    }

    #[test]
    fn the_search_index_follows_deletes_and_is_rebuilt_from_transcripts() {
        let dir = std::env::temp_dir().join(format!("bach-store-search-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("bach.db");
        {
            let s = Store::open(&path).unwrap();
            s.put(&session("a")).unwrap();
            s.put(&session("b")).unwrap();
            say(&s, "a", "needle in a");
            say(&s, "b", "needle in b");
            s.delete("b").unwrap();
            assert_eq!(found(&s, "needle").len(), 1);
            // An index from another version, or a lost one.
            let conn = s.0.lock().unwrap();
            conn.execute_batch("DELETE FROM search_text; DELETE FROM meta WHERE key = 'search_index';").unwrap();
        }
        let s = Store::open(&path).unwrap();
        assert_eq!(found(&s, "needle"), [("a".into(), false, vec![1])]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn replaces_an_index_built_the_old_way() {
        let dir = std::env::temp_dir().join(format!("bach-store-oldindex-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("bach.db");
        {
            let s = Store::open(&path).unwrap();
            s.put(&session("a")).unwrap();
            say(&s, "a", "websocket");
            // What an earlier version left: a word index, marked with its version.
            let conn = s.0.lock().unwrap();
            conn.execute_batch(
                "DROP TRIGGER search_text_insert; DROP TRIGGER search_text_delete; DROP TABLE search_index;
                 CREATE VIRTUAL TABLE search_index USING fts5(text, content = 'search_text', content_rowid = 'id');
                 UPDATE meta SET value = '1' WHERE key = 'search_index';",
            )
            .unwrap();
        }
        let s = Store::open(&path).unwrap();
        assert_eq!(found(&s, "sock"), [("a".into(), false, vec![1])]);
        say(&s, "a", "another websocket");
        assert_eq!(found(&s, "sock")[0].2.len(), 2);
        let _ = std::fs::remove_dir_all(dir);
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
