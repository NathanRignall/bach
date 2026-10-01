//! The backend behind every transport: owns sessions, runs, git and bach-tasks, implements each
//! command of the protocol, and broadcasts [`ServerEvent`]s. Transports only parse, call
//! [`Api::call`], and forward [`Api::subscribe`].
use crate::{
    adapters::list_agents,
    attachments::Attachment,
    git::Git,
    runs::{Emit, RunRequest, Runs},
    sessions::{OnFinish, Sessions},
    terminals::Terminals,
    tools::SessionTools,
    store::{now_ms, Store},
};
use bach_protocol::{commands::*, *};
use bach_tasks::{Tasks, StartTask};
use bach_tasks_protocol::*;
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::{broadcast, mpsc},
    task::JoinHandle,
};

/// Each agent's models (opencode's per folder), and when they were listed.
type ModelLists =
    std::collections::HashMap<(AgentKind, Option<String>), (std::time::Instant, Vec<ModelInfo>)>;

/// Each agent's skills per project folder, and when they were listed.
type SkillLists =
    std::collections::HashMap<(AgentKind, Option<String>), (std::time::Instant, Vec<SkillInfo>)>;

pub struct Api {
    runs: Runs,
    launcher: Launcher,
    git: Git,
    sessions: Sessions,
    terminals: Terminals,
    tasks: Tasks,
    events: broadcast::Sender<ServerEvent>,
    /// Passes bach-tasks' task events on while anyone is subscribed.
    task_events: Mutex<Option<JoinHandle<()>>>,
    /// Each agent's models, as last listed, and when.
    models: Mutex<ModelLists>,
    /// Each agent's skills, as last listed, and when.
    skills: Mutex<SkillLists>,
}

impl Api {
    /// Opens (creating if needed) the session database at `db`. Worktrees and bach-tasks' files go
    /// in folders next to it. Starts the agents' MCP server (bach-tasks', with Bach's tools added) on
    /// a free loopback port.
    pub async fn open(db: &Path) -> Result<Api, String> {
        crate::wrapper::init()?;
        let dir = db.parent().unwrap_or(Path::new("."));
        let store = Store::open(db)?;
        // Nothing survives a restart of the backend: no run is live any more.
        store.end_all_runs()?;
        let tasks = Tasks::start("127.0.0.1:0".parse().unwrap(), dir.join("tasks"))
            .await
            .map_err(|e| format!("couldn't start the task launcher: {e}"))?;
        // Tasks used to live in the session database; bach-tasks keeps its own now.
        let legacy = store.legacy_tasks()?;
        if !legacy.is_empty() {
            let n = tasks.import(legacy.into_iter().filter_map(bach_tasks::parse_task).collect());
            eprintln!("moved {n} background task(s) to bach-tasks' database");
        }
        store.drop_legacy_tasks()?;
        Ok(Self::new(store, Git::new(dir.join("worktrees")), tasks))
    }

    pub fn new(store: Store, git: Git, tasks: Tasks) -> Api {
        let (events, _) = broadcast::channel(1024);
        let runs = Runs::with_tasks(Some(tasks.clone()));
        let sessions = Sessions::new(store, events.clone());
        let launcher = Launcher::new(sessions.clone(), runs.clone(), git.clone());
        // Agents see one `bach` server: bach-tasks' task tools and Bach's own.
        tasks.set_server_name(crate::adapters::MCP_SERVER);
        tasks.add_tools(Arc::new(SessionTools {
            launcher: launcher.clone(),
            sessions: sessions.clone(),
        }));
        Api {
            launcher,
            runs,
            git,
            sessions,
            terminals: Terminals::new(events.clone()),
            tasks,
            events,
            task_events: Mutex::default(),
            models: Mutex::default(),
            skills: Mutex::default(),
        }
    }

    pub fn tasks(&self) -> &Tasks {
        &self.tasks
    }

    /// Runs one command given by name and JSON arguments.
    pub async fn call(&self, cmd: &str, args: Value) -> Result<Value, ApiError> {
        Request::parse(cmd, args)?.dispatch(self).await
    }

    /// Every event from now on. A receiver that falls too far behind skips ahead.
    pub fn subscribe(&self) -> broadcast::Receiver<ServerEvent> {
        let rx = self.events.subscribe();
        self.forward_task_events();
        rx
    }

    /// Watching tasks means looking at the machine every second, so only do it while someone is
    /// listening: the forwarder stops (and unsubscribes from bach-tasks) once nobody is.
    fn forward_task_events(&self) {
        let mut running = self.task_events.lock().unwrap();
        if running.as_ref().is_some_and(|h| !h.is_finished()) {
            return;
        }
        let (mut tasks, events) = (self.tasks.subscribe(), self.events.clone());
        *running = Some(tokio::spawn(async move {
            loop {
                tokio::select! {
                    ev = tasks.recv() => match ev {
                        Ok(ev) => { let _ = events.send(ServerEvent::Task(ev)); }
                        Err(broadcast::error::RecvError::Lagged(_)) => {}
                        Err(broadcast::error::RecvError::Closed) => return,
                    },
                    _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                }
                if events.receiver_count() == 0 {
                    return;
                }
            }
        }));
    }
}

/// `bach/<first words of the prompt>-<random>`, e.g. "Fix the login bug!" -> `bach/fix-the-login-bug-a3f1`.
fn branch_name_for(prompt: &str) -> String {
    let lower = prompt.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .take(5)
        .collect();
    let slug = if words.is_empty() {
        "session".to_string()
    } else {
        words.join("-")
    };
    format!("bach/{slug}-{}", &uuid::Uuid::new_v4().simple().to_string()[..4])
}

/// The prompt's start or, without one, the first attachment's name.
fn title_for(prompt: &str, attachments: &[String]) -> String {
    let title: String = prompt.trim().chars().take(40).collect();
    if !title.is_empty() {
        return title;
    }
    match attachments.first().and_then(|a| Attachment::parse(a)) {
        Some(Attachment { name: Some(name), .. }) => name.chars().take(40).collect(),
        _ => "Image".into(),
    }
}

/// A message needs text or attachments.
fn check_message(prompt: &str, attachments: &[String]) -> Result<(), ApiError> {
    if prompt.is_empty() && attachments.is_empty() {
        return Err(ApiError::invalid("Write a message first."));
    }
    if attachments.iter().any(|a| Attachment::parse(a).is_none()) {
        return Err(ApiError::invalid(
            "Attachments must be images, PDFs or text files, sent as base64 `data:` URLs.",
        ));
    }
    Ok(())
}

/// `prompt`, led by a note of the files the user edited by hand since the agent's last message.
fn edit_note(edited: &[String], prompt: &str) -> String {
    const SHOWN: usize = 20;
    if edited.is_empty() {
        return prompt.to_string();
    }
    let mut files: Vec<_> = edited.iter().take(SHOWN).map(|f| format!("`{f}`")).collect();
    if edited.len() > SHOWN {
        files.push(format!("and {} more", edited.len() - SHOWN));
    }
    format!(
        "[Note from Bach: since your last message the user edited these files by hand, so read them again before relying on what you remember of them: {}.]\n\n{prompt}",
        files.join(", ")
    )
}

/// No choice, "", and "default" all mean the agent's default model.
fn model_choice(m: Option<String>) -> Option<String> {
    m.map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty() && m != "default")
}

/// No choice, "" and "default" all mean the model's default effort.
fn effort(e: Option<String>) -> Option<String> {
    model_choice(e)
}

/// No choice, "" and "default" all mean the agent's default permission mode.
fn permission_mode(m: Option<String>) -> Option<String> {
    m.map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty() && m != "default")
}

impl Api {
    /// The folder session `id` works in: its worktree, or its project folder.
    fn session_folder(&self, id: &str) -> Result<String, ApiError> {
        let s = self.sessions.get(id)?;
        if s.workdir_removed {
            return Err(ApiError::invalid("This session's worktree was removed."));
        }
        let folder = s.workdir.unwrap_or(s.cwd);
        if folder.trim().is_empty() {
            return Err(ApiError::invalid("This session has no project folder."));
        }
        Ok(folder)
    }
}

/// Starts sessions and their agent runs, and the next queued message whenever a run finishes
/// cleanly.
#[derive(Clone)]
pub(crate) struct Launcher {
    sessions: Sessions,
    runs: Runs,
    git: Git,
    /// Queued messages claimed when a run finished, to launch once its process has exited.
    next: mpsc::UnboundedSender<Next>,
    /// Files the user edited by hand in each session since its agent last heard of it, in order.
    /// Kept in memory only: the note is for the next message, so a restart may drop it.
    edited: Arc<Mutex<std::collections::HashMap<String, Vec<String>>>>,
}

/// A queued message taken off the queue as its session's run finished: the session is marked
/// running with it, to launch once the finished run's process has exited.
struct Next {
    session_id: String,
    run_id: String,
    prompt: String,
    images: Vec<String>,
    after: String,
}

impl Launcher {
    fn new(sessions: Sessions, runs: Runs, git: Git) -> Launcher {
        let (next, mut rx) = mpsc::unbounded_channel::<Next>();
        let this = Launcher {
            sessions,
            runs,
            git,
            next,
            edited: Arc::default(),
        };
        let worker = this.clone();
        tokio::spawn(async move {
            while let Some(n) = rx.recv().await {
                // `Done` comes with the agent's result, a moment before its process exits; the
                // next run resumes the same conversation, so let this one finish writing it.
                for _ in 0..100 {
                    if !worker.runs.is_live(&n.after).await {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                let id = n.session_id;
                let launched = match worker.sessions.get(&id) {
                    // Stopped while waiting: the message goes back to the head of the queue.
                    Ok(s) if s.run_id.as_deref() != Some(&n.run_id) => worker
                        .sessions
                        .update(&id, |s| {
                            let id = uuid::Uuid::new_v4().to_string();
                            let (text, images) = (n.prompt, n.images);
                            s.queued.insert(0, QueuedMessage { id, text, images });
                            Ok(())
                        }),
                    Ok(s) => worker.launch(&id, s, n.run_id, n.prompt, n.images, None).await,
                    Err(e) => Err(e),
                };
                if let Err(e) = launched {
                    eprintln!("couldn't send session {id}'s next queued message: {}", e.message);
                }
            }
        });
        this
    }

    /// Starts a new session: readies its folder (a worktree, maybe) and sends its first message.
    pub(crate) async fn start_session(&self, a: StartSessionArgs) -> Result<Session, ApiError> {
        let prompt = a.prompt.trim().to_string();
        check_message(&prompt, &a.images)?;
        let cwd = a.cwd.trim().to_string();
        if cwd.is_empty() {
            return Err(ApiError::invalid("Choose a project folder first."));
        }
        // Where it runs: the folder on the chosen branch, or a new worktree branched from it.
        let worktree = a.worktree.unwrap_or(false);
        let new_branch = a
            .new_branch
            .map(|b| b.trim().to_string())
            .filter(|b| !b.is_empty())
            .unwrap_or_else(|| branch_name_for(&prompt));
        let ws = self
            .git
            .prepare(
                cwd.clone(),
                a.branch.clone(),
                worktree,
                worktree.then_some(new_branch),
            )
            .await
            .map_err(ApiError::failed)?;
        let now = now_ms();
        let session = Session {
            id: uuid::Uuid::new_v4().to_string(),
            title: title_for(&prompt, &a.images),
            title_edited: false,
            agent: a.agent,
            cwd,
            branch: a.branch,
            worktree: ws.worktree,
            model_choice: model_choice(a.model_choice),
            permission_mode: permission_mode(a.permission_mode),
            effort: effort(a.effort),
            workdir: Some(ws.workdir),
            git_branch: ws.branch,
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
            created_at: now,
            updated_at: now,
            last_seq: 0,
        };
        self.sessions.put_quietly(&session)?;
        match self.send(&session.id, prompt, a.images).await {
            Ok(s) => Ok(s),
            Err(e) => {
                // A session is only kept once its first message reached the agent.
                self.sessions.delete(&session.id)?;
                Err(e)
            }
        }
    }

    /// Starts a session from an earlier message of another: its transcript up to there, and the
    /// worktree as it was when that message was sent. Its agent continues a copy of the
    /// conversation where it can do that itself (see [`NativeFork`]), and is otherwise told it.
    pub(crate) async fn fork_session(&self, a: ForkSessionArgs) -> Result<Session, ApiError> {
        let source = self.sessions.get(&a.session_id)?;
        let entries = self.sessions.entries(&a.session_id, 0)?;
        let at = entries
            .iter()
            .position(|e| e.seq == a.seq)
            .ok_or_else(|| ApiError::not_found("That message isn't in the session."))?;
        let Entry::User { text: original, images } = &entries[at].entry else {
            return Err(ApiError::invalid("Only a message you sent can be forked from."));
        };
        let before = &entries[..at];
        let prompt = a.prompt.map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).unwrap_or_else(|| original.trim().to_string());
        check_message(&prompt, images)?;
        if source.cwd.trim().is_empty() {
            return Err(ApiError::invalid("This session has no project folder."));
        }

        // The agent's own conversation: its turns before this message (a message that never
        // reached the agent isn't one).
        let turn = before
            .iter()
            .enumerate()
            .filter(|(i, e)| {
                matches!(e.entry, Entry::User { .. }) && !matches!(before.get(i + 1).map(|n| &n.entry), Some(Entry::Failed { .. }))
            })
            .count();
        let mut preamble = None;
        let pending_fork = if turn == 0 {
            None
        } else {
            let native = async {
                let agent_session_id = source.agent_session_id.clone().ok_or("the agent's session wasn't recorded")?;
                let (agent, id, text) = (source.agent, agent_session_id.clone(), original.clone());
                let at = tokio::task::spawn_blocking(move || crate::adapters::fork_point(agent, &id, turn, &text))
                    .await
                    .map_err(|e| e.to_string())??;
                Ok::<_, String>(NativeFork { agent_session_id, turn: turn as u32, at })
            };
            match native.await {
                Ok(f) => Some(f),
                // Nothing to fork natively (or it can't be found): the agent gets the transcript instead.
                Err(why) => {
                    eprintln!("forking session {} from its transcript instead: {why}", source.id);
                    preamble = Some(crate::handoff::transcript_seed(before));
                    None
                }
            }
        };

        // Where it runs: a worktree of its own with the files as they were, or (without git) the
        // same folder.
        let info = self.git.info(source.cwd.clone()).await.map_err(ApiError::failed)?;
        let (workdir, git_branch, worktree) = if let Some(root) = info.root.clone().filter(|_| info.is_repo) {
            let (root, id, seq) = (PathBuf::from(root), source.id.clone(), a.seq);
            let snapshot = tokio::task::spawn_blocking(move || crate::snapshots::find(&root, &id, seq))
                .await
                .map_err(|e| ApiError::failed(e.to_string()))?
                .ok_or_else(|| ApiError::invalid("Bach didn't save the files as they were before that message, so it can't be forked from."))?;
            let ws = self
                .git
                .prepare(source.cwd.clone(), None, true, Some(branch_name_for(&prompt)))
                .await
                .map_err(ApiError::failed)?;
            let dir = ws.workdir.clone();
            let restored = tokio::task::spawn_blocking(move || crate::snapshots::restore(Path::new(&dir), &snapshot))
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r);
            if let Err(e) = restored {
                let _ = self.git.remove_worktree(ws.workdir, true, true).await;
                return Err(ApiError::failed(format!("Couldn't restore the files as they were: {e}")));
            }
            (ws.workdir, ws.branch, true)
        } else {
            (source.workdir.clone().unwrap_or_else(|| source.cwd.clone()), None, false)
        };

        let now = now_ms();
        let session = Session {
            id: uuid::Uuid::new_v4().to_string(),
            title: format!("{} (fork)", source.title.trim_end_matches(" (fork)")),
            title_edited: false,
            workdir: Some(workdir),
            git_branch,
            worktree,
            workdir_removed: false,
            agent_session_id: None,
            model: None,
            context: None,
            run_id: None,
            archived: false,
            origin: Some(SessionOrigin { kind: OriginKind::Fork, session_id: source.id.clone(), seq: Some(a.seq) }),
            pending_fork,
            open_approvals: vec![],
            unseen: None,
            queued: vec![],
            created_at: now,
            updated_at: now,
            last_seq: 0,
            ..source.clone()
        };
        self.sessions.put_with_entries(&session, before)?;
        if info.is_repo {
            let (root, from, to, seq) = (PathBuf::from(info.root.unwrap_or_default()), source.id.clone(), session.id.clone(), a.seq);
            let _ = tokio::task::spawn_blocking(move || crate::snapshots::copy(&root, &from, &to, seq)).await;
        }
        match self.send_with(&session.id, prompt, images.clone(), preamble).await {
            Ok(s) => Ok(s),
            Err(e) => {
                // Like a new session, it is only kept once its first message reached the agent.
                self.sessions.delete(&session.id)?;
                Err(e)
            }
        }
    }

    /// A draft of the message that hands session `id`'s task to another agent.
    pub(crate) async fn handoff_summary(&self, id: &str) -> Result<String, ApiError> {
        let source = self.sessions.get(id)?;
        let entries = self.sessions.entries(id, 0)?;
        let dir = source.workdir.clone().filter(|_| !source.workdir_removed);
        let base = source.branch.clone();
        let files = match dir {
            Some(dir) => tokio::task::spawn_blocking(move || crate::snapshots::changed_files(Path::new(&dir), base.as_deref()))
                .await
                .unwrap_or_default(),
            None => vec![],
        };
        Ok(crate::handoff::summary(&source.title, source.agent, &entries, &source.queued, &files))
    }

    /// Starts a session with another agent in the same folder and worktree as session
    /// `a.session_id`, carrying its task on with `a.prompt`.
    pub(crate) async fn handoff_session(&self, a: HandoffSessionArgs) -> Result<Session, ApiError> {
        let source = self.sessions.get(&a.session_id)?;
        if source.workdir_removed {
            return Err(ApiError::invalid("This session's worktree was removed, so its work can't be handed on."));
        }
        if source.run_id.is_some() {
            return Err(ApiError::invalid("The agent is still working. Stop it or wait for it to finish first."));
        }
        let prompt = a.prompt.trim().to_string();
        check_message(&prompt, &[])?;
        let now = now_ms();
        let session = Session {
            id: uuid::Uuid::new_v4().to_string(),
            title_edited: false,
            agent: a.agent,
            model_choice: model_choice(a.model_choice),
            permission_mode: permission_mode(a.permission_mode),
            effort: effort(a.effort),
            // Rules the other agent approved are in its own words.
            allow_rules: vec![],
            agent_session_id: None,
            model: None,
            context: None,
            run_id: None,
            archived: false,
            origin: Some(SessionOrigin { kind: OriginKind::Handoff, session_id: source.id.clone(), seq: None }),
            pending_fork: None,
            open_approvals: vec![],
            unseen: None,
            queued: vec![],
            created_at: now,
            updated_at: now,
            last_seq: 0,
            ..source
        };
        self.sessions.put_quietly(&session)?;
        match self.send(&session.id, prompt, vec![]).await {
            Ok(s) => Ok(s),
            Err(e) => {
                self.sessions.delete(&session.id)?;
                Err(e)
            }
        }
    }

    /// Sends `prompt` (and `images`) to session `id`'s agent: records it, marks the session
    /// running and starts the run. A run that can't start is recorded as failed. While the agent
    /// is busy, the message is queued instead.
    async fn send(&self, id: &str, prompt: String, images: Vec<String>) -> Result<Session, ApiError> {
        self.send_with(id, prompt, images, None).await
    }

    /// Like [`send`](Self::send), with `preamble` (context for a session continuing another's
    /// work) put before the prompt for the agent, though not in the transcript.
    async fn send_with(
        &self,
        id: &str,
        prompt: String,
        images: Vec<String>,
        preamble: Option<String>,
    ) -> Result<Session, ApiError> {
        let prompt = prompt.trim().to_string();
        check_message(&prompt, &images)?;
        let run_id = uuid::Uuid::new_v4().to_string();
        let mut queued = false;
        let s = self.sessions.update(id, |s| {
            if s.workdir_removed {
                return Err(ApiError::invalid(
                    "This session's worktree was removed, so it can't be continued.",
                ));
            }
            if s.run_id.is_some() {
                s.queued.push(QueuedMessage {
                    id: uuid::Uuid::new_v4().to_string(),
                    text: prompt.clone(),
                    images: images.clone(),
                });
                queued = true;
                return Ok(());
            }
            s.run_id = Some(run_id.clone());
            s.unseen = None;
            // Talking to it again brings it back.
            s.archived = false;
            Ok(())
        })?;
        if queued {
            return Ok(s);
        }
        self.launch(id, s, run_id, prompt, images, preamble).await
    }

    /// Sends queued message `message_id` now. The session must be idle.
    async fn send_queued(&self, id: &str, message_id: &str) -> Result<Session, ApiError> {
        let run_id = uuid::Uuid::new_v4().to_string();
        let mut message = None;
        let s = self.sessions.update(id, |s| {
            if s.workdir_removed {
                return Err(ApiError::invalid(
                    "This session's worktree was removed, so it can't be continued.",
                ));
            }
            if s.run_id.is_some() {
                return Err(ApiError::invalid(
                    "The agent is still working; queued messages go once it's done.",
                ));
            }
            let i = s
                .queued
                .iter()
                .position(|q| q.id == message_id)
                .ok_or_else(|| ApiError::not_found("That message isn't queued any more."))?;
            message = Some(s.queued.remove(i));
            s.run_id = Some(run_id.clone());
            s.unseen = None;
            s.archived = false;
            Ok(())
        })?;
        let QueuedMessage { text, images, .. } = message.expect("taken above");
        self.launch(id, s, run_id, text, images, None).await
    }

    /// Remembers that the user edited `path` by hand in session `id`.
    fn note_edit(&self, id: &str, path: &str) {
        let mut all = self.edited.lock().unwrap();
        let files = all.entry(id.to_string()).or_default();
        if !files.iter().any(|f| f == path) {
            files.push(path.to_string());
        }
    }

    /// Records `prompt` (and `images`) and starts run `run_id` of session `s`, which is already
    /// marked running. Before the agent touches anything, the worktree is saved as it is (see
    /// [`crate::snapshots`]), so the session can be forked from this message.
    async fn launch(
        &self,
        id: &str,
        s: Session,
        run_id: String,
        prompt: String,
        images: Vec<String>,
        preamble: Option<String>,
    ) -> Result<Session, ApiError> {
        let logged = self.sessions.append(
            id,
            Entry::User {
                text: prompt.clone(),
                images: images.clone(),
            },
        )?;
        if let Some(dir) = s.workdir.clone().filter(|_| !s.workdir_removed) {
            let (session_id, seq) = (id.to_string(), logged.seq);
            let taken = tokio::task::spawn_blocking(move || crate::snapshots::take(Path::new(&dir), &session_id, seq))
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r);
            // A fork from this message just won't be possible; the turn goes on.
            if let Err(e) = taken {
                eprintln!("couldn't save session {id}'s worktree before message {seq}: {e}", seq = logged.seq);
            }
        }
        let fork = s.pending_fork.clone();
        // What the agent would otherwise miss: it reads files as they are, not what changed.
        let edited = self.edited.lock().unwrap().remove(id).unwrap_or_default();
        let req = RunRequest {
            agent: s.agent,
            prompt: edit_note(
                &edited,
                &match &preamble {
                    Some(p) => format!("{p}\n\n{prompt}"),
                    None => prompt.clone(),
                },
            ),
            images,
            cwd: s.workdir.clone().or(Some(s.cwd.clone())),
            session_id: s.agent_session_id.clone().filter(|_| fork.is_none()),
            fork,
            model: s.model_choice.clone(),
            permission_mode: s.permission_mode.clone(),
            effort: s.effort.clone(),
            allowed_tools: s.allow_rules.clone(),
            session_key: Some(id.to_string()),
            run_id: Some(run_id.clone()),
        };
        match self.runs.start(self.recorder(id), req).await {
            Ok(_) => self.sessions.get(id),
            Err(message) => {
                // The agent still hasn't been told.
                for path in &edited {
                    self.note_edit(id, path);
                }
                self.sessions.append(
                    id,
                    Entry::Failed {
                        message: message.clone(),
                        retry_text: Some(prompt),
                    },
                )?;
                self.sessions.update(id, |s| {
                    if s.run_id.as_deref() == Some(&run_id) {
                        s.run_id = None;
                        s.unseen = Some(RunOutcome::Failed);
                    }
                    Ok(())
                })?;
                Err(ApiError::failed(message))
            }
        }
    }

    /// Records the run's events and, once it finishes cleanly, hands the session to its next
    /// queued message. A stopped or failed run leaves the queue waiting: the user may not want
    /// what comes next any more.
    fn recorder(&self, id: &str) -> Emit {
        let next = self.next.clone();
        let on_finish: OnFinish = Arc::new(move |s: &mut Session, finished: &str| {
            if s.queued.is_empty() || s.workdir_removed {
                return;
            }
            let (run_id, message) = (uuid::Uuid::new_v4().to_string(), s.queued.remove(0));
            s.run_id = Some(run_id.clone());
            let _ = next.send(Next {
                session_id: s.id.clone(),
                run_id,
                prompt: message.text,
                images: message.images,
                after: finished.to_string(),
            });
        });
        self.sessions.recorder(id, Some(on_finish))
    }
}

fn task_error(e: bach_tasks::Error) -> ApiError {
    match e {
        bach_tasks::Error::NotFound(m) => ApiError::not_found(m),
        bach_tasks::Error::Invalid(m) => ApiError::invalid(m),
        bach_tasks::Error::Failed(m) => ApiError::failed(m),
    }
}

impl Handler for Api {
    async fn list_agents(&self, _: ListAgentsArgs) -> Result<Vec<AgentInfo>, ApiError> {
        Ok(list_agents())
    }

    async fn list_models(&self, a: ListModelsArgs) -> Result<Vec<ModelInfo>, ApiError> {
        // Asking takes a second or so, and the lists rarely change. opencode's depend on the
        // project (it may configure providers of its own), and it runs in the session's
        // folder, so that's where it's asked.
        const FRESH: Duration = Duration::from_secs(30 * 60);
        let cwd = match a.agent {
            AgentKind::Opencode => a
                .cwd
                .map(|d| crate::fs::expand_home(d.trim()))
                .map(|d| d.canonicalize().unwrap_or(d).to_string_lossy().into_owned()),
            _ => None,
        };
        let key = (a.agent, cwd.clone());
        if let Some((at, models)) = self.models.lock().unwrap().get(&key) {
            if at.elapsed() < FRESH {
                return Ok(models.clone());
            }
        }
        let models = crate::adapters::list_models(a.agent, cwd.as_deref(), Some(&self.tasks))
            .await
            .map_err(ApiError::failed)?;
        self.models
            .lock()
            .unwrap()
            .insert(key, (std::time::Instant::now(), models.clone()));
        Ok(models)
    }

    async fn list_skills(&self, a: ListSkillsArgs) -> Result<Vec<SkillInfo>, ApiError> {
        // Asking starts the agent, so like the models the lists are kept for a while. Project
        // skills come with the folder, so they're kept per folder.
        const FRESH: Duration = Duration::from_secs(5 * 60);
        if a.agent != AgentKind::Claude {
            return Ok(vec![]);
        }
        let cwd = a
            .cwd
            .filter(|d| !d.trim().is_empty())
            .map(|d| crate::fs::expand_home(d.trim()))
            .map(|d| d.canonicalize().unwrap_or(d).to_string_lossy().into_owned());
        let key = (a.agent, cwd.clone());
        if let Some((at, skills)) = self.skills.lock().unwrap().get(&key) {
            if at.elapsed() < FRESH {
                return Ok(skills.clone());
            }
        }
        let skills = crate::adapters::list_skills(a.agent, cwd.as_deref())
            .await
            .map_err(ApiError::failed)?;
        self.skills
            .lock()
            .unwrap()
            .insert(key, (std::time::Instant::now(), skills.clone()));
        Ok(skills)
    }

    async fn get_usage(&self, _: GetUsageArgs) -> Result<Option<PlanUsage>, ApiError> {
        self.sessions.plan_usage()
    }

    async fn list_sessions(&self, _: ListSessionsArgs) -> Result<Vec<Session>, ApiError> {
        self.sessions.list()
    }

    async fn search_sessions(&self, a: SearchSessionsArgs) -> Result<Vec<SearchResult>, ApiError> {
        self.sessions.search(&a.query, a.limit.map_or(30, |n| n as usize))
    }

    async fn get_session(&self, a: GetSessionArgs) -> Result<SessionLog, ApiError> {
        Ok(SessionLog {
            session: self.sessions.get(&a.session_id)?,
            entries: self
                .sessions
                .entries(&a.session_id, a.after_seq.unwrap_or(0))?,
        })
    }

    async fn start_session(&self, a: StartSessionArgs) -> Result<Session, ApiError> {
        self.launcher.start_session(a).await
    }

    async fn send_message(&self, a: SendMessageArgs) -> Result<Session, ApiError> {
        self.launcher.send(&a.session_id, a.prompt, a.images).await
    }

    async fn send_queued(&self, a: SendQueuedArgs) -> Result<Session, ApiError> {
        self.launcher
            .send_queued(&a.session_id, &a.message_id)
            .await
    }

    async fn mark_seen(&self, a: MarkSeenArgs) -> Result<Session, ApiError> {
        if self.sessions.get(&a.session_id)?.unseen.is_none() {
            return self.sessions.get(&a.session_id);
        }
        self.sessions.update(&a.session_id, |s| {
            s.unseen = None;
            Ok(())
        })
    }

    async fn remove_queued(&self, a: RemoveQueuedArgs) -> Result<Session, ApiError> {
        self.sessions.update(&a.session_id, |s| {
            s.queued.retain(|q| q.id != a.message_id);
            Ok(())
        })
    }

    async fn stop_session(&self, a: StopSessionArgs) -> Result<(), ApiError> {
        let Some(run_id) = self.sessions.get(&a.session_id)?.run_id else {
            return Ok(());
        };
        if self.runs.is_live(&run_id).await {
            // The run ends with a `cancelled` event, which marks the session stopped.
            self.runs.cancel(&run_id).await;
        } else {
            self.sessions.update(&a.session_id, |s| {
                s.run_id = None;
                s.open_approvals.clear();
                Ok(())
            })?;
        }
        Ok(())
    }

    async fn answer_approval(&self, a: AnswerApprovalArgs) -> Result<(), ApiError> {
        let id = &a.session_id;
        let run_id = self
            .sessions
            .get(id)?
            .run_id
            .ok_or_else(|| ApiError::not_found("That run has already finished."))?;
        let rules = self
            .runs
            .respond_approval(&run_id, &a.request_id, a.decision, a.message, a.answers.clone())
            .await?;
        self.sessions.append(
            id,
            Entry::Decision {
                request_id: a.request_id.clone(),
                decision: a.decision,
                answers: a.answers,
            },
        )?;
        self.sessions.update(id, |s| {
            s.open_approvals.retain(|r| *r != a.request_id);
            // "For this session" outlasts the run: later runs start with these rules.
            if a.decision == Decision::AllowSession {
                for r in rules {
                    if !s.allow_rules.contains(&r) {
                        s.allow_rules.push(r);
                    }
                }
            }
            Ok(())
        })?;
        Ok(())
    }

    async fn update_session(&self, a: UpdateSessionArgs) -> Result<Session, ApiError> {
        let title = a.title.map(|t| t.trim().to_string());
        if title.as_ref().is_some_and(|t| t.is_empty()) {
            return Err(ApiError::invalid("A session needs a name."));
        }
        if a.archived == Some(true) {
            // An archived session isn't running anything.
            self.runs.cancel_session(&a.session_id).await;
        }
        self.sessions.update(&a.session_id, |s| {
            if let Some(archived) = a.archived {
                s.archived = archived;
            }
            if let Some(t) = title {
                s.title = t;
                s.title_edited = true;
            }
            if a.model_choice.is_some() {
                s.model_choice = model_choice(a.model_choice);
            }
            if a.permission_mode.is_some() {
                s.permission_mode = permission_mode(a.permission_mode);
            }
            if a.effort.is_some() {
                s.effort = effort(a.effort);
            }
            Ok(())
        })
    }

    async fn delete_session(&self, a: DeleteSessionArgs) -> Result<(), ApiError> {
        // A deleted session's agent run must not outlive it (its background tasks do).
        self.runs.cancel_session(&a.session_id).await;
        let project = self.sessions.get(&a.session_id).map(|s| s.cwd).unwrap_or_default();
        self.sessions.delete(&a.session_id)?;
        // Its saved worktree states go with it (forks keep their own).
        let id = a.session_id;
        let _ = tokio::task::spawn_blocking(move || {
            if std::path::Path::new(&project).is_dir() {
                crate::snapshots::remove(Path::new(&project), &id);
            }
        })
        .await;
        Ok(())
    }

    async fn fork_session(&self, a: ForkSessionArgs) -> Result<Session, ApiError> {
        self.launcher.fork_session(a).await
    }

    async fn handoff_summary(&self, a: HandoffSummaryArgs) -> Result<String, ApiError> {
        self.launcher.handoff_summary(&a.session_id).await
    }

    async fn handoff_session(&self, a: HandoffSessionArgs) -> Result<Session, ApiError> {
        self.launcher.handoff_session(a).await
    }

    async fn list_dir(&self, a: ListDirArgs) -> Result<DirListing, ApiError> {
        Ok(crate::fs::list_dir(
            a.path.as_deref(),
            a.show_hidden.unwrap_or(false),
        )?)
    }

    async fn read_image(&self, a: ReadImageArgs) -> Result<String, ApiError> {
        let path = a.path;
        tokio::task::spawn_blocking(move || crate::fs::read_image(&path))
            .await
            .map_err(|e| ApiError::failed(e.to_string()))?
    }

    async fn list_files(&self, a: ListFilesArgs) -> Result<FileList, ApiError> {
        let root = self.session_folder(&a.session_id)?;
        tokio::task::spawn_blocking(move || crate::fs::list_files(&root))
            .await
            .map_err(|e| ApiError::failed(e.to_string()))?
    }

    async fn read_file(&self, a: ReadFileArgs) -> Result<FileContent, ApiError> {
        let root = self.session_folder(&a.session_id)?;
        tokio::task::spawn_blocking(move || crate::fs::read_file(&root, &a.path))
            .await
            .map_err(|e| ApiError::failed(e.to_string()))?
    }

    async fn write_file(&self, a: WriteFileArgs) -> Result<FileContent, ApiError> {
        let root = self.session_folder(&a.session_id)?;
        let path = a.path.clone();
        let file = tokio::task::spawn_blocking(move || {
            crate::fs::write_file(&root, &a.path, &a.text, &a.expected_version, a.overwrite.unwrap_or(false))
        })
        .await
        .map_err(|e| ApiError::failed(e.to_string()))??;
        self.launcher.note_edit(&a.session_id, &path);
        Ok(file)
    }

    async fn git_info(&self, a: GitInfoArgs) -> Result<GitInfo, ApiError> {
        Ok(self.git.info(a.path).await?)
    }

    async fn git_diff(&self, a: GitDiffArgs) -> Result<GitDiff, ApiError> {
        Ok(self.git.diff(a.path, a.base_branch, a.commit).await?)
    }

    async fn git_log(&self, a: GitLogArgs) -> Result<GitLog, ApiError> {
        Ok(self
            .git
            .log(
                a.path,
                a.base_branch,
                a.older.unwrap_or(false),
                a.skip.unwrap_or(0),
                a.limit.unwrap_or(0),
            )
            .await?)
    }

    async fn git_status(&self, a: GitStatusArgs) -> Result<BranchStatus, ApiError> {
        Ok(self.git.status(a.path).await?)
    }

    async fn git_stage(&self, a: GitStageArgs) -> Result<(), ApiError> {
        Ok(self.git.stage(a.path, a.files, a.stage).await?)
    }

    async fn git_stage_hunk(&self, a: GitStageHunkArgs) -> Result<(), ApiError> {
        Ok(self
            .git
            .stage_hunk(a.path, a.file, a.hunk, a.header, a.stage)
            .await?)
    }

    async fn git_commit(&self, a: GitCommitArgs) -> Result<CommitInfo, ApiError> {
        Ok(self.git.commit(a.path, a.message).await?)
    }

    async fn git_push(&self, a: GitPushArgs) -> Result<BranchStatus, ApiError> {
        Ok(self.git.push(a.path).await?)
    }

    async fn list_worktrees(&self, _: ListWorktreesArgs) -> Result<Vec<WorktreeEntry>, ApiError> {
        Ok(self.git.list_worktrees().await)
    }

    async fn remove_worktree(&self, a: RemoveWorktreeArgs) -> Result<(), ApiError> {
        self.git
            .remove_worktree(
                a.path.clone(),
                a.discard.unwrap_or(false),
                a.delete_branch.unwrap_or(false),
            )
            .await?;
        // Their folder is gone: keep the transcripts readable, but they can't be continued.
        for s in self.sessions.list()? {
            if s.workdir.as_deref() == Some(&a.path) && !s.workdir_removed {
                self.sessions.update(&s.id, |s| {
                    s.workdir_removed = true;
                    Ok(())
                })?;
            }
        }
        Ok(())
    }

    async fn list_tasks(&self, _: ListTasksArgs) -> Result<Vec<TaskView>, ApiError> {
        Ok(self.tasks.list(None))
    }

    async fn task_logs(&self, a: TaskLogsArgs) -> Result<String, ApiError> {
        self.tasks
            .logs(
                &a.task_id,
                a.lines.unwrap_or(200).clamp(1, 2000),
                a.process.as_deref(),
            )
            .map_err(task_error)
    }

    async fn task_log_chunk(&self, a: TaskLogChunkArgs) -> Result<LogChunk, ApiError> {
        self.tasks
            .log_chunk(
                &a.task_id,
                a.process.as_deref(),
                a.from,
                a.max_bytes.unwrap_or(512 * 1024),
            )
            .map_err(task_error)
    }

    async fn stop_task(&self, a: StopTaskArgs) -> Result<Task, ApiError> {
        self.tasks.stop_task(&a.task_id).await.map_err(task_error)
    }

    async fn remove_task(&self, a: RemoveTaskArgs) -> Result<(), ApiError> {
        self.tasks.remove_task(&a.task_id).map_err(task_error)
    }

    async fn start_task(&self, a: StartTaskArgs) -> Result<Task, ApiError> {
        self.tasks
            .start_task(StartTask {
                command: a.command,
                cwd: Some(a.cwd.clone()),
                name: a.name,
                project: Some(a.cwd),
                owner: None,
                ports: vec![],
                interactive: None,
            })
            .map_err(task_error)
    }

    async fn task_process(&self, a: TaskProcessArgs) -> Result<(), ApiError> {
        self.tasks
            .process_action(&a.task_id, &a.process, a.action)
            .await
            .map_err(task_error)
    }

    async fn list_terminals(&self, _: ListTerminalsArgs) -> Result<Vec<TerminalInfo>, ApiError> {
        Ok(self.terminals.list())
    }

    async fn open_terminal(&self, a: OpenTerminalArgs) -> Result<TerminalInfo, ApiError> {
        self.terminals.open(a.cwd, a.cols, a.rows)
    }

    async fn terminal_snapshot(&self, a: TerminalSnapshotArgs) -> Result<TerminalSnapshot, ApiError> {
        self.terminals.snapshot(&a.terminal_id)
    }

    async fn terminal_input(&self, a: TerminalInputArgs) -> Result<(), ApiError> {
        self.terminals.input(&a.terminal_id, &a.data)
    }

    async fn resize_terminal(&self, a: ResizeTerminalArgs) -> Result<(), ApiError> {
        self.terminals.resize(&a.terminal_id, a.cols, a.rows)
    }

    async fn close_terminal(&self, a: CloseTerminalArgs) -> Result<(), ApiError> {
        self.terminals.close(&a.terminal_id)
    }
}
