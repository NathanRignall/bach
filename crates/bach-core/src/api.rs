//! The backend behind every transport: owns sessions, runs, git and Satie, implements each
//! command of the protocol, and broadcasts [`ServerEvent`]s. Transports only parse, call
//! [`Api::call`], and forward [`Api::subscribe`].
use crate::{
    adapters::list_agents,
    git::Git,
    runs::{Emit, RunRequest, Runs},
    sessions::{OnFinish, Sessions},
    terminals::Terminals,
    store::{now_ms, Store},
};
use bach_protocol::{commands::*, *};
use satie::{Satie, StartTask};
use satie_protocol::*;
use serde_json::Value;
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::{broadcast, mpsc},
    task::JoinHandle,
};

pub struct Api {
    runs: Runs,
    launcher: Launcher,
    git: Git,
    sessions: Sessions,
    terminals: Terminals,
    satie: Satie,
    events: broadcast::Sender<ServerEvent>,
    /// Passes Satie's task events on while anyone is subscribed.
    task_events: Mutex<Option<JoinHandle<()>>>,
}

impl Api {
    /// Opens (creating if needed) the session database at `db`. Worktrees and Satie's files go
    /// in folders next to it. Starts Satie's MCP server on a free loopback port.
    pub async fn open(db: &Path) -> Result<Api, String> {
        let dir = db.parent().unwrap_or(Path::new("."));
        let store = Store::open(db)?;
        // Nothing survives a restart of the backend: no run is live any more.
        store.end_all_runs()?;
        let satie = Satie::start("127.0.0.1:0".parse().unwrap(), dir.join("tasks"))
            .await
            .map_err(|e| format!("couldn't start Satie: {e}"))?;
        // Tasks used to live in the session database; Satie keeps its own now.
        let legacy = store.legacy_tasks()?;
        if !legacy.is_empty() {
            let n = satie.import(legacy.into_iter().filter_map(satie::parse_task).collect());
            eprintln!("moved {n} background task(s) to Satie's database");
        }
        store.drop_legacy_tasks()?;
        Ok(Self::new(store, Git::new(dir.join("worktrees")), satie))
    }

    pub fn new(store: Store, git: Git, satie: Satie) -> Api {
        let (events, _) = broadcast::channel(1024);
        let runs = Runs::with_satie(Some(satie.clone()));
        let sessions = Sessions::new(store, events.clone());
        Api {
            launcher: Launcher::new(sessions.clone(), runs.clone()),
            runs,
            git,
            sessions,
            terminals: Terminals::new(events.clone()),
            satie,
            events,
            task_events: Mutex::default(),
        }
    }

    pub fn satie(&self) -> &Satie {
        &self.satie
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
    /// listening: the forwarder stops (and unsubscribes from Satie) once nobody is.
    fn forward_task_events(&self) {
        let mut running = self.task_events.lock().unwrap();
        if running.as_ref().is_some_and(|h| !h.is_finished()) {
            return;
        }
        let (mut tasks, events) = (self.satie.subscribe(), self.events.clone());
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

fn title_for(prompt: &str) -> String {
    let title: String = prompt.trim().chars().take(40).collect();
    if title.is_empty() {
        "Image".into()
    } else {
        title
    }
}

/// A message needs text or images.
fn check_message(prompt: &str, images: &[String]) -> Result<(), ApiError> {
    if prompt.is_empty() && images.is_empty() {
        return Err(ApiError::invalid("Write a message first."));
    }
    if images.iter().any(|i| !i.starts_with("data:image/")) {
        return Err(ApiError::invalid("Images must be sent as `data:image/…` URLs."));
    }
    Ok(())
}

/// No choice, "", and "default" all mean the agent's default model.
fn model_choice(m: Option<String>) -> Option<String> {
    m.map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty() && m != "default")
}

/// No choice, "" and "default" all mean the agent's default permission mode.
fn permission_mode(m: Option<String>) -> Option<String> {
    m.map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty() && m != "default")
}

impl Api {
    /// Sends `prompt` (and `images`) to session `id`'s agent: records it, marks the session
    /// running and starts the run. A run that can't start is recorded as failed. While the agent
    /// is busy, the message is queued instead.
    async fn send(&self, id: &str, prompt: String, images: Vec<String>) -> Result<Session, ApiError> {
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
            // Talking to it again brings it back.
            s.archived = false;
            Ok(())
        })?;
        if queued {
            return Ok(s);
        }
        self.launcher.launch(id, s, run_id, prompt, images).await
    }
}

/// Starts agent runs for sessions, and the next queued message whenever a run finishes cleanly.
#[derive(Clone)]
struct Launcher {
    sessions: Sessions,
    runs: Runs,
    /// Queued messages claimed when a run finished, to launch once its process has exited.
    next: mpsc::UnboundedSender<Next>,
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
    fn new(sessions: Sessions, runs: Runs) -> Launcher {
        let (next, mut rx) = mpsc::unbounded_channel::<Next>();
        let this = Launcher {
            sessions,
            runs,
            next,
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
                    Ok(s) => worker.launch(&id, s, n.run_id, n.prompt, n.images).await,
                    Err(e) => Err(e),
                };
                if let Err(e) = launched {
                    eprintln!("couldn't send session {id}'s next queued message: {}", e.message);
                }
            }
        });
        this
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
            s.archived = false;
            Ok(())
        })?;
        let QueuedMessage { text, images, .. } = message.expect("taken above");
        self.launch(id, s, run_id, text, images).await
    }

    /// Records `prompt` (and `images`) and starts run `run_id` of session `s`, which is already
    /// marked running.
    async fn launch(
        &self,
        id: &str,
        s: Session,
        run_id: String,
        prompt: String,
        images: Vec<String>,
    ) -> Result<Session, ApiError> {
        self.sessions.append(
            id,
            Entry::User {
                text: prompt.clone(),
                images: images.clone(),
            },
        )?;
        let req = RunRequest {
            agent: s.agent,
            prompt: prompt.clone(),
            images,
            cwd: s.workdir.clone().or(Some(s.cwd.clone())),
            session_id: s.agent_session_id.clone(),
            model: s
                .model_choice
                .clone()
                .filter(|_| s.agent == AgentKind::Claude),
            permission_mode: s
                .permission_mode
                .clone()
                .filter(|_| s.agent == AgentKind::Claude),
            allowed_tools: s.allow_rules.clone(),
            session_key: Some(id.to_string()),
            run_id: Some(run_id.clone()),
        };
        match self.runs.start(self.recorder(id), req).await {
            Ok(_) => self.sessions.get(id),
            Err(message) => {
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

fn satie_error(e: satie::Error) -> ApiError {
    match e {
        satie::Error::NotFound(m) => ApiError::not_found(m),
        satie::Error::Invalid(m) => ApiError::invalid(m),
        satie::Error::Failed(m) => ApiError::failed(m),
    }
}

impl Handler for Api {
    async fn list_agents(&self, _: ListAgentsArgs) -> Result<Vec<AgentInfo>, ApiError> {
        Ok(list_agents())
    }

    async fn get_usage(&self, _: GetUsageArgs) -> Result<Option<PlanUsage>, ApiError> {
        self.sessions.plan_usage()
    }

    async fn list_sessions(&self, _: ListSessionsArgs) -> Result<Vec<Session>, ApiError> {
        self.sessions.list()
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
            title: title_for(&prompt),
            title_edited: false,
            agent: a.agent,
            cwd,
            branch: a.branch,
            worktree: ws.worktree,
            model_choice: model_choice(a.model_choice),
            permission_mode: permission_mode(a.permission_mode),
            workdir: Some(ws.workdir),
            git_branch: ws.branch,
            workdir_removed: false,
            agent_session_id: None,
            allow_rules: vec![],
            model: None,
            context: None,
            run_id: None,
            archived: false,
            open_approvals: vec![],
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

    async fn send_message(&self, a: SendMessageArgs) -> Result<Session, ApiError> {
        self.send(&a.session_id, a.prompt, a.images).await
    }

    async fn send_queued(&self, a: SendQueuedArgs) -> Result<Session, ApiError> {
        self.launcher
            .send_queued(&a.session_id, &a.message_id)
            .await
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
            Ok(())
        })
    }

    async fn delete_session(&self, a: DeleteSessionArgs) -> Result<(), ApiError> {
        // A deleted session's agent run must not outlive it (its background tasks do).
        self.runs.cancel_session(&a.session_id).await;
        self.sessions.delete(&a.session_id)
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

    async fn git_info(&self, a: GitInfoArgs) -> Result<GitInfo, ApiError> {
        Ok(self.git.info(a.path).await?)
    }

    async fn git_diff(&self, a: GitDiffArgs) -> Result<GitDiff, ApiError> {
        Ok(self.git.diff(a.path, a.base_branch).await?)
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
        Ok(self.satie.list(None))
    }

    async fn task_logs(&self, a: TaskLogsArgs) -> Result<String, ApiError> {
        self.satie
            .logs(
                &a.task_id,
                a.lines.unwrap_or(200).clamp(1, 2000),
                a.process.as_deref(),
            )
            .map_err(satie_error)
    }

    async fn task_log_chunk(&self, a: TaskLogChunkArgs) -> Result<LogChunk, ApiError> {
        self.satie
            .log_chunk(
                &a.task_id,
                a.process.as_deref(),
                a.from,
                a.max_bytes.unwrap_or(512 * 1024),
            )
            .map_err(satie_error)
    }

    async fn stop_task(&self, a: StopTaskArgs) -> Result<Task, ApiError> {
        self.satie.stop_task(&a.task_id).await.map_err(satie_error)
    }

    async fn remove_task(&self, a: RemoveTaskArgs) -> Result<(), ApiError> {
        self.satie.remove_task(&a.task_id).map_err(satie_error)
    }

    async fn start_task(&self, a: StartTaskArgs) -> Result<Task, ApiError> {
        self.satie
            .start_task(StartTask {
                command: a.command,
                cwd: Some(a.cwd.clone()),
                name: a.name,
                project: Some(a.cwd),
                owner: None,
                ports: vec![],
            })
            .map_err(satie_error)
    }

    async fn task_process(&self, a: TaskProcessArgs) -> Result<(), ApiError> {
        self.satie
            .process_action(&a.task_id, &a.process, a.action)
            .await
            .map_err(satie_error)
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
