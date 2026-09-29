//! Satie: a launcher for long-running background processes.
//!
//! Agents can't keep background work alive themselves: Claude Code stops its background tasks
//! when its process exits, which Bach's does after every turn. Satie starts processes
//! *detached* (own session, output to a log file, exit code to a file), remembers them in its own
//! database, and finds them again after a restart. Agents reach it as an MCP server (see [`mcp`]),
//! so it works for any agent that speaks MCP; apps drive it through [`Satie`]'s methods and follow
//! it through [`Satie::subscribe`].
//!
//! Satie knows nothing about who embeds it. Callers get an MCP token by [`Satie::grant`]ing a
//! [`Scope`]: the project whose tasks it may see and manage, and an opaque owner recorded on the
//! tasks it starts.
//!
//! Unix only (`setsid`, `/proc` for ports).
mod diagnose;
mod mcp;
pub mod probe;
mod process;
pub(crate) mod store;
#[cfg(test)]
mod tests;

pub use diagnose::presentable_ports;
pub use store::parse_task;
pub use satie_protocol::{LogChunk, Task, TaskEvent, TaskStatus, TaskView};

use diagnose::{diagnose, SETTLE_MS};
use process::{now_ms, ports_of_group, proc_stat, signal_group, task_alive};
use std::{
    collections::HashMap,
    fmt,
    fs::OpenOptions,
    io::{Read, Seek, SeekFrom, Write},
    net::SocketAddr,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{atomic::AtomicI64, atomic::Ordering, Arc, Mutex},
    time::Duration,
};
use store::TaskStore;
use tokio::{net::TcpListener, sync::broadcast};

// ---------------------------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------------------------

/// Why an operation failed. The text is written for the person (or agent) who asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// No such task (or not one the caller may see).
    NotFound(String),
    /// The request can't be carried out as asked.
    Invalid(String),
    /// It was tried and didn't work.
    Failed(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotFound(m) | Error::Invalid(m) | Error::Failed(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for Error {}

fn no_such_task() -> Error {
    Error::NotFound("No such task.".into())
}

#[derive(Default)]
pub struct StartTask {
    pub command: String,
    pub cwd: Option<String>,
    pub name: Option<String>,
    pub project: Option<String>,
    /// Who started it, as the caller names them (e.g. a Bach run id).
    pub owner: Option<String>,
    /// Ports it is expected to listen on.
    pub ports: Vec<u16>,
}

/// An HTTP check a task must pass to count as ready.
pub struct Ready {
    pub url: String,
    /// Required status; by default anything below 500 counts as the server answering.
    pub status: Option<u16>,
}

/// What an MCP caller may do: see and manage one project's tasks (every task, without one).
#[derive(Clone, Debug, Default)]
pub struct Scope {
    pub project: Option<String>,
    /// Recorded as the owner of tasks started through this grant.
    pub owner: Option<String>,
}

/// Access to Satie's MCP server for one [`Scope`]. The token stops working when this is dropped,
/// so a finished agent run can't keep calling tools.
pub struct Grant {
    /// The MCP endpoint (streamable HTTP, loopback only).
    pub url: String,
    /// Sent as `Authorization: Bearer <token>`.
    pub token: String,
    satie: Satie,
}

impl Drop for Grant {
    fn drop(&mut self) {
        self.satie.inner.grants.lock().unwrap().remove(&self.token);
    }
}

// ---------------------------------------------------------------------------------------------
// The launcher
// ---------------------------------------------------------------------------------------------

struct Inner {
    /// MCP bearer tokens in use, and what each may do.
    grants: Mutex<HashMap<String, Scope>>,
    tasks: Mutex<HashMap<String, Task>>,
    store: TaskStore,
    dir: PathBuf,
    /// How long a task gets to open its ports before a missing one is called a problem.
    settle_ms: AtomicI64,
    events: broadcast::Sender<TaskEvent>,
    /// What subscribers were last told about each task, to send only what changed.
    published: Mutex<HashMap<String, serde_json::Value>>,
}

/// Handle to Satie. Cheap to clone.
#[derive(Clone)]
pub struct Satie {
    url: String,
    inner: Arc<Inner>,
}

impl Satie {
    /// Starts Satie with its files (database, logs) in `dir`: loads and reconciles known tasks,
    /// starts the monitor, and serves MCP on `addr` (loopback only; port 0 for any free port).
    pub async fn start(addr: SocketAddr, dir: PathBuf) -> std::io::Result<Satie> {
        Self::start_with(addr, dir, Duration::from_secs(1)).await
    }

    /// [`start`](Self::start), with the monitor checking tasks every `tick`.
    pub async fn start_with(addr: SocketAddr, dir: PathBuf, tick: Duration) -> std::io::Result<Satie> {
        assert!(addr.ip().is_loopback(), "Satie only listens on loopback");
        std::fs::create_dir_all(&dir)?;
        let store = TaskStore::open(&dir.join("satie.db")).map_err(std::io::Error::other)?;
        let tasks = store
            .list()
            .unwrap_or_default()
            .into_iter()
            .map(|t| (t.id.clone(), t))
            .collect();
        let listener = TcpListener::bind(addr).await?;
        let satie = Satie {
            url: format!("http://{}/mcp", listener.local_addr()?),
            inner: Arc::new(Inner {
                grants: Mutex::default(),
                tasks: Mutex::new(tasks),
                store,
                dir,
                settle_ms: AtomicI64::new(SETTLE_MS),
                events: broadcast::channel(256).0,
                published: Mutex::default(),
            }),
        };
        satie.tick(); // reconcile: what ran while we were down?

        let monitor = satie.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(tick).await;
                monitor.tick();
                monitor.publish();
            }
        });
        let app = mcp::router(satie.clone());
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(satie)
    }

    /// The MCP endpoint.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Grants that are still held.
    pub fn active_grants(&self) -> usize {
        self.inner.grants.lock().unwrap().len()
    }

    /// A token for MCP callers limited to `scope`, valid until the [`Grant`] is dropped.
    pub fn grant(&self, scope: Scope) -> Grant {
        let token = uuid::Uuid::new_v4().simple().to_string();
        self.inner
            .grants
            .lock()
            .unwrap()
            .insert(token.clone(), scope);
        Grant {
            url: self.url.clone(),
            token,
            satie: self.clone(),
        }
    }

    /// Adopts tasks recorded elsewhere (an older database), skipping ones already known.
    /// Returns how many were new.
    pub fn import(&self, tasks: Vec<Task>) -> usize {
        let mut added = 0;
        for task in tasks {
            let mut known = self.inner.tasks.lock().unwrap();
            if known.contains_key(&task.id) {
                continue;
            }
            known.insert(task.id.clone(), task.clone());
            drop(known);
            self.save(&task);
            added += 1;
        }
        self.tick();
        added
    }

    /// Changes to tasks from now on: every change to a task's view (status, ports, problems,
    /// processes), and removals. Checked every monitor tick and after each operation.
    pub fn subscribe(&self) -> broadcast::Receiver<TaskEvent> {
        self.inner.events.subscribe()
    }

    /// Tells subscribers about every task whose view changed since they were last told.
    fn publish(&self) {
        let mut published = self.inner.published.lock().unwrap();
        if self.inner.events.receiver_count() == 0 {
            // Nobody to tell; whoever subscribes next gets everything once.
            published.clear();
            return;
        }
        for view in self.list(None) {
            let value = serde_json::to_value(&view).expect("task views serialize");
            if published.get(&view.task.id) != Some(&value) {
                published.insert(view.task.id.clone(), value);
                let _ = self.inner.events.send(TaskEvent::Changed { task: view });
            }
        }
    }

    // ----- state -----------------------------------------------------------------------------

    fn save(&self, task: &Task) {
        if let Err(e) = self.inner.store.save(task) {
            eprintln!("satie: couldn't save task {}: {e}", task.id);
        }
    }

    fn update(&self, id: &str, f: impl FnOnce(&mut Task)) -> Option<Task> {
        let mut tasks = self.inner.tasks.lock().unwrap();
        let t = tasks.get_mut(id)?;
        f(t);
        let t = t.clone();
        drop(tasks);
        self.save(&t);
        Some(t)
    }

    fn get(&self, id: &str) -> Option<Task> {
        self.inner.tasks.lock().unwrap().get(id).cloned()
    }

    fn exit_path(&self, id: &str) -> PathBuf {
        self.inner.dir.join(format!("{id}.exit"))
    }

    /// Notices tasks that ended: by their exit-code file, or by their process being gone.
    fn tick(&self) {
        let running: Vec<Task> = self
            .inner
            .tasks
            .lock()
            .unwrap()
            .values()
            .filter(|t| t.status == TaskStatus::Running)
            .cloned()
            .collect();
        for t in running {
            let code = std::fs::read_to_string(self.exit_path(&t.id))
                .ok()
                .and_then(|s| s.trim().parse::<i32>().ok());
            if let Some(code) = code {
                self.update(&t.id, |t| {
                    t.status = if code == 0 {
                        TaskStatus::Exited
                    } else {
                        TaskStatus::Failed
                    };
                    t.exit_code = Some(code);
                    t.ended_at = Some(now_ms());
                });
            } else if !task_alive(t.pid, t.start_ticks) {
                self.update(&t.id, |t| {
                    t.status = TaskStatus::Lost;
                    t.ended_at = Some(now_ms());
                });
            }
        }
    }

    // ----- operations ------------------------------------------------------------------------

    /// Tasks (newest first), optionally only those started from `project`.
    pub fn list(&self, project: Option<&str>) -> Vec<TaskView> {
        self.list_settled(project, self.inner.settle_ms.load(Ordering::Relaxed))
    }

    /// How long a task gets to open its expected ports before a missing one counts as a problem.
    pub fn set_settle_ms(&self, ms: i64) {
        self.inner.settle_ms.store(ms, Ordering::Relaxed);
    }

    /// `settle_ms`: how old a task must be before a not-yet-listening port counts as a problem.
    fn list_settled(&self, project: Option<&str>, settle_ms: i64) -> Vec<TaskView> {
        let all: Vec<Task> = self.inner.tasks.lock().unwrap().values().cloned().collect();
        let mut tasks: Vec<Task> = all
            .iter()
            .filter(|t| project.is_none_or(|p| t.project.as_deref() == Some(p)))
            .cloned()
            .collect();
        tasks.sort_by_key(|t| std::cmp::Reverse(t.started_at));

        // One look at the machine serves every task.
        let listening = probe::listeners();
        let by_group: HashMap<u32, (String, String)> = all
            .iter()
            .map(|t| (t.pid, (t.id.clone(), t.name.clone())))
            .collect();
        tasks
            .into_iter()
            .map(|task| {
                let running = task.status == TaskStatus::Running;
                let mut own: Vec<u16> = listening
                    .iter()
                    .filter(|l| l.pgid == Some(task.pid))
                    .map(|l| l.port)
                    .collect();
                own.sort_unstable();
                own.dedup();
                let (problems, missing_ports) =
                    if matches!(task.status, TaskStatus::Running | TaskStatus::Failed) {
                        diagnose(&task, &own, &listening, &by_group, settle_ms)
                    } else {
                        (vec![], vec![])
                    };
                let up_ports: Vec<u16> = task
                    .expected_ports
                    .iter()
                    .copied()
                    .filter(|p| own.contains(p))
                    .collect();
                TaskView {
                    up_ports,
                    ports: if running {
                        presentable_ports(own)
                    } else {
                        vec![]
                    },
                    missing_ports,
                    problems,
                    processes: if running {
                        probe::process_summary(task.pid)
                    } else {
                        vec![]
                    },
                    task,
                }
            })
            .collect()
    }

    /// Starts a detached process. It keeps running if this process, or the agent, exits.
    pub fn start_task(&self, req: StartTask) -> Result<Task, Error> {
        let command = req.command.trim().to_string();
        if command.is_empty() {
            return Err(Error::Invalid("`command` is required".into()));
        }
        let cwd = req
            .cwd
            .as_deref()
            .or(req.project.as_deref())
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .ok_or_else(|| Error::Invalid("A working directory is required.".into()))?;
        let cwd = process::expand_home(cwd);
        if !cwd.is_dir() {
            return Err(Error::Invalid(format!("{} is not a folder.", cwd.display())));
        }
        let failed = |e: std::io::Error| Error::Failed(format!("failed to start: {e}"));

        let id = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
        let log_path = self.inner.dir.join(format!("{id}.log"));
        let exit_path = self.exit_path(&id);
        let mut log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map_err(failed)?;
        let _ = writeln!(log, "$ {command}");
        let err = log.try_clone().map_err(failed)?;

        // The outer shell records the inner command's exit code, so the result is known even
        // if Satie was restarted in the meantime.
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(r#"sh -c "$1"; code=$?; echo "$code" > "$2"; exit "$code""#)
            .arg("satie")
            .arg(&command)
            .arg(&exit_path)
            .current_dir(&cwd)
            .stdin(Stdio::null())
            .stdout(log)
            .stderr(err);
        // SAFETY: setsid is async-signal-safe and is the only call made between fork and exec.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = cmd.spawn().map_err(failed)?;
        let pid = child.id();
        // Reap it when it ends, so it never lingers as a zombie of ours.
        std::thread::spawn(move || {
            let _ = child.wait();
        });

        let task = Task {
            name: req
                .name
                .map(|n| n.trim().to_string())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| command.clone()),
            command,
            cwd: cwd.to_string_lossy().into_owned(),
            project: req.project,
            owner: req.owner,
            pid,
            start_ticks: proc_stat(pid).map(|(_, t)| t),
            started_at: now_ms(),
            ended_at: None,
            status: TaskStatus::Running,
            exit_code: None,
            log_path: log_path.to_string_lossy().into_owned(),
            expected_ports: req.ports,
            id: id.clone(),
        };
        self.inner.tasks.lock().unwrap().insert(id, task.clone());
        self.save(&task);
        self.publish();
        Ok(task)
    }

    /// Stops a running task and everything it started.
    pub async fn stop_task(&self, id: &str) -> Result<Task, Error> {
        let task = self.get(id).ok_or_else(no_such_task)?;
        if task.status != TaskStatus::Running {
            return Ok(task);
        }
        // Mark first: the monitor only watches running tasks, so it can't call this "lost".
        let stopped = self
            .update(id, |t| {
                t.status = TaskStatus::Stopped;
                t.ended_at = Some(now_ms());
            })
            .ok_or_else(no_such_task)?;
        signal_group(task.pid, libc::SIGTERM);
        let mut gone = false;
        for _ in 0..60 {
            if !task_alive(task.pid, task.start_ticks) {
                gone = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        if !gone {
            signal_group(task.pid, libc::SIGKILL);
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        self.publish();
        Ok(stopped)
    }

    /// Forgets a finished task and its files.
    pub fn remove_task(&self, id: &str) -> Result<(), Error> {
        let task = self.get(id).ok_or_else(no_such_task)?;
        if task.status == TaskStatus::Running {
            return Err(Error::Invalid("Stop the task before removing it.".into()));
        }
        self.inner.tasks.lock().unwrap().remove(id);
        if let Err(e) = self.inner.store.delete(id) {
            eprintln!("satie: couldn't delete task {id}: {e}");
        }
        let _ = std::fs::remove_file(&task.log_path);
        let _ = std::fs::remove_file(self.exit_path(id));
        self.inner.published.lock().unwrap().remove(id);
        let _ = self.inner.events.send(TaskEvent::Removed { id: id.into() });
        Ok(())
    }

    /// The last `lines` lines of a task's output.
    pub fn logs(&self, id: &str, lines: usize) -> Result<String, Error> {
        let task = self.get(id).ok_or_else(no_such_task)?;
        let failed = |e: std::io::Error| Error::Failed(format!("Couldn't read the log: {e}"));
        let mut f = std::fs::File::open(&task.log_path).map_err(failed)?;
        let len = f.metadata().map_err(failed)?.len();
        let take = len.min(256 * 1024);
        f.seek(SeekFrom::Start(len - take)).map_err(failed)?;
        let mut buf = String::new();
        f.take(take).read_to_string(&mut buf).ok(); // may cut a multibyte char at the start: best effort
        let all: Vec<&str> = buf.lines().collect();
        Ok(all[all.len().saturating_sub(lines)..].join("\n"))
    }

    /// Waits until the task listens on `ports` (and answers `ready`, if given), ends, or `max`
    /// passes. Without expectations, gives it a moment to fail fast.
    pub async fn wait_ready(
        &self,
        id: &str,
        ports: &[u16],
        ready: Option<&Ready>,
        max: Duration,
    ) -> Option<Result<probe::HttpProbe, String>> {
        let started = std::time::Instant::now();
        let mut last_check = std::time::Instant::now();
        let mut last = None;
        loop {
            let Some(t) = self.get(id) else { return last };
            if t.status != TaskStatus::Running {
                return last;
            }
            let listening = ports_of_group(t.pid);
            let ports_ok = ports.iter().all(|p| listening.contains(p));
            let mut http_ok = true;
            if let Some(r) = ready {
                http_ok = false;
                if ports_ok {
                    let probe = probe::http_get(&r.url, Duration::from_millis(1500)).await;
                    http_ok = matches!(&probe, Ok(p) if r.status.map_or(p.status < 500, |want| p.status == want));
                    last = Some(probe);
                }
            }
            let has_expectations = !ports.is_empty() || ready.is_some();
            if ports_ok
                && http_ok
                && (has_expectations || started.elapsed() >= Duration::from_millis(1500))
            {
                return last;
            }
            // Waiting won't help if every missing port is held by another process right now.
            if !ports_ok
                && started.elapsed() >= Duration::from_secs(1)
                && last_check.elapsed() >= Duration::from_secs(1)
            {
                last_check = std::time::Instant::now();
                let now = probe::listeners();
                let held = |p: &u16| now.iter().any(|l| l.port == *p && l.pgid != Some(t.pid));
                if ports.iter().filter(|p| !listening.contains(p)).all(held) {
                    return last;
                }
            }
            if started.elapsed() >= max {
                return last;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    /// A piece of a task's log, for viewers that follow it: see [`probe::read_chunk`].
    pub fn log_chunk(&self, id: &str, from: Option<u64>, max_bytes: u64) -> Result<LogChunk, Error> {
        let task = self.get(id).ok_or_else(no_such_task)?;
        probe::read_chunk(
            Path::new(&task.log_path),
            from,
            max_bytes.clamp(1024, 8 * 1024 * 1024),
            task.status == TaskStatus::Running,
        )
        .map_err(|e| Error::Failed(format!("Couldn't read the log: {e}")))
    }
}
