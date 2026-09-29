//! The backend behind every transport: owns runs, git, storage and Satie, implements each
//! command of the protocol, and broadcasts [`ServerEvent`]s. Transports only parse, call
//! [`Api::call`], and forward [`Api::subscribe`].
use crate::{
    adapters::list_agents,
    git::Git,
    runs::{Emit, RunRequest, Runs},
    store::Store,
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
use tokio::{sync::broadcast, task::JoinHandle};

pub struct Api {
    runs: Runs,
    git: Git,
    store: Store,
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
        Api {
            runs: Runs::with_satie(Some(satie.clone())),
            git,
            store,
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

    fn emit(&self) -> Emit {
        let events = self.events.clone();
        Arc::new(move |ev| {
            // No subscribers is fine: nobody is watching.
            let _ = events.send(ServerEvent::Run(ev));
        })
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

    async fn start_run(&self, a: StartRunArgs) -> Result<String, ApiError> {
        let req = RunRequest {
            agent: a.agent,
            prompt: a.prompt,
            cwd: a.cwd,
            session_id: a.session_id,
            model: a.model,
            allowed_tools: a.allowed_tools.unwrap_or_default(),
            session_key: a.session_key,
        };
        Ok(self.runs.start(self.emit(), req).await?)
    }

    async fn cancel_run(&self, a: CancelRunArgs) -> Result<(), ApiError> {
        self.runs.cancel(&a.run_id).await;
        Ok(())
    }

    async fn respond_approval(&self, a: RespondApprovalArgs) -> Result<(), ApiError> {
        self.runs
            .respond_approval(&a.run_id, &a.request_id, a.decision, a.message, a.answers)
            .await
    }

    async fn list_dir(&self, a: ListDirArgs) -> Result<DirListing, ApiError> {
        Ok(crate::fs::list_dir(
            a.path.as_deref(),
            a.show_hidden.unwrap_or(false),
        )?)
    }

    async fn git_info(&self, a: GitInfoArgs) -> Result<GitInfo, ApiError> {
        Ok(self.git.info(a.path).await?)
    }

    async fn prepare_workspace(&self, a: PrepareWorkspaceArgs) -> Result<Workspace, ApiError> {
        Ok(self
            .git
            .prepare(a.cwd, a.branch, a.worktree.unwrap_or(false), a.new_branch)
            .await?)
    }

    async fn list_worktrees(&self, _: ListWorktreesArgs) -> Result<Vec<WorktreeEntry>, ApiError> {
        Ok(self.git.list_worktrees().await)
    }

    async fn remove_worktree(&self, a: RemoveWorktreeArgs) -> Result<(), ApiError> {
        Ok(self
            .git
            .remove_worktree(
                a.path,
                a.discard.unwrap_or(false),
                a.delete_branch.unwrap_or(false),
            )
            .await?)
    }

    async fn list_tasks(&self, _: ListTasksArgs) -> Result<Vec<TaskView>, ApiError> {
        Ok(self.satie.list(None))
    }

    async fn task_logs(&self, a: TaskLogsArgs) -> Result<String, ApiError> {
        self.satie
            .logs(&a.task_id, a.lines.unwrap_or(200).clamp(1, 2000))
            .map_err(satie_error)
    }

    async fn task_log_chunk(&self, a: TaskLogChunkArgs) -> Result<LogChunk, ApiError> {
        self.satie
            .log_chunk(&a.task_id, a.from, a.max_bytes.unwrap_or(512 * 1024))
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

    async fn list_sessions(&self, _: ListSessionsArgs) -> Result<Vec<Value>, ApiError> {
        Ok(self.store.list()?)
    }

    async fn save_session(&self, a: SaveSessionArgs) -> Result<(), ApiError> {
        self.store.save(&a.session).map_err(ApiError::invalid)
    }

    async fn delete_session(&self, a: DeleteSessionArgs) -> Result<(), ApiError> {
        // A deleted session's agent runs must not outlive it (its background tasks do).
        self.runs.cancel_session(&a.session_id).await;
        Ok(self.store.delete(&a.session_id)?)
    }
}
