//! Satie: Bach's launcher for long-running background processes.
//!
//! Agents can't keep background work alive themselves: Claude Code stops its background tasks
//! when its process exits, which Bach's does after every turn. Satie starts processes
//! *detached* (own session, output to a log file, exit code to a file), remembers them in the
//! database, and finds them again after a restart. Agents reach it as an MCP server, so it works
//! for any agent that speaks MCP; the UI reaches it through the same backend commands as
//! everything else.
//!
//! Unix only (`setsid`, `/proc` for ports).
use crate::{fs::expand_home, store::Store};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs::OpenOptions,
    io::{Read, Seek, SeekFrom, Write},
    net::SocketAddr,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::net::TcpListener;

// ---------------------------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Running,
    /// Finished with exit code 0.
    Exited,
    /// Finished with a non-zero exit code.
    Failed,
    /// Stopped from Bach.
    Stopped,
    /// Gone without leaving an exit code (killed from outside, machine rebooted).
    Lost,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub name: String,
    pub command: String,
    /// Where it runs.
    pub cwd: String,
    /// The project (agent run folder) that started it; agents only see their own project's tasks.
    pub project: Option<String>,
    pub run_id: Option<String>,
    pub pid: u32,
    /// Process start time from `/proc`, so a reused pid isn't mistaken for this task.
    pub start_ticks: Option<u64>,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub status: TaskStatus,
    pub exit_code: Option<i32>,
    pub log_path: String,
    /// Ports the task should come up on; a missing one is a sign a part of it failed.
    #[serde(default)]
    pub expected_ports: Vec<u16>,
}

/// A task plus what is only known by looking at the machine right now.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskView {
    #[serde(flatten)]
    pub task: Task,
    /// TCP ports the task's processes are listening on (recognisable ones; see `presentable_ports`).
    pub ports: Vec<u16>,
    /// Expected ports that are listening right now.
    pub up_ports: Vec<u16>,
    /// Expected ports that are not listening, once the task has had a moment to start.
    pub missing_ports: Vec<u16>,
    /// What looks wrong: ports already taken by something else, expected ports that never came up.
    pub problems: Vec<String>,
    /// What is running in it, e.g. `workerd ×8`.
    pub processes: Vec<String>,
}

#[derive(Default)]
pub struct StartTask {
    pub command: String,
    pub cwd: Option<String>,
    pub name: Option<String>,
    pub project: Option<String>,
    pub run_id: Option<String>,
    /// Ports it is expected to listen on.
    pub ports: Vec<u16>,
}

/// An HTTP check a task must pass to count as ready.
pub struct Ready {
    pub url: String,
    /// Required status; by default anything below 500 counts as the server answering.
    pub status: Option<u16>,
}

/// Who an MCP token belongs to.
#[derive(Clone, Debug)]
pub struct RunInfo {
    pub run_id: String,
    pub cwd: Option<String>,
}

/// What a Claude Code run is given so it can use Satie.
#[derive(Clone, Debug)]
pub struct SatieArgs {
    /// `--mcp-config` JSON.
    pub mcp_config: String,
    /// `--append-system-prompt` text steering agents to Satie for anything long-running.
    pub system_prompt: String,
    /// `--settings` JSON with a hook that stops `run_in_background` and points at Satie instead.
    pub settings: String,
}

// ---------------------------------------------------------------------------------------------
// Process helpers
// ---------------------------------------------------------------------------------------------

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// `(state, start time in ticks)` from `/proc/<pid>/stat`, if the process exists.
pub(crate) fn proc_stat(pid: u32) -> Option<(char, u64)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name is in parentheses and may contain spaces; fields follow the last `)`.
    let rest = &stat[stat.rfind(')')? + 2..];
    let fields: Vec<&str> = rest.split(' ').collect();
    Some((
        fields.first()?.chars().next()?,
        fields.get(19)?.parse().ok()?,
    ))
}

pub(crate) fn process_group_of(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = &stat[stat.rfind(')')? + 2..];
    rest.split(' ').nth(2)?.parse().ok() // field 5, pgrp
}

fn task_alive(pid: u32, ticks: Option<u64>) -> bool {
    match proc_stat(pid) {
        Some((state, t)) => state != 'Z' && ticks.is_none_or(|want| want == t),
        None => false,
    }
}

fn signal_group(pgid: u32, sig: i32) {
    // SAFETY: plain kill(2) on a negative pid, i.e. the whole process group.
    unsafe {
        libc::kill(-(pgid as i32), sig);
    }
}

/// Listening TCP ports of every process in group `pgid` (Linux; empty elsewhere).
fn ports_of_group(pgid: u32) -> Vec<u16> {
    let mut group_of: std::collections::HashMap<u32, Option<u32>> =
        std::collections::HashMap::new();
    let mut ports: Vec<u16> = crate::probe::socket_owners()
        .into_iter()
        .filter_map(|(port, pid)| {
            let pid = pid?;
            (*group_of.entry(pid).or_insert_with(|| process_group_of(pid)) == Some(pgid))
                .then_some(port)
        })
        .collect();
    ports.sort_unstable();
    ports.dedup();
    ports
}

/// A task must be this old before a port that isn't listening yet counts as a problem.
const SETTLE_MS: i64 = 10_000;

/// Who holds a port, in words an agent (or a person) can act on.
fn holder_text(
    holder: &crate::probe::Listener,
    tasks_by_group: &std::collections::HashMap<u32, (String, String)>,
) -> String {
    if let Some((id, name)) = holder.pgid.and_then(|g| tasks_by_group.get(&g)) {
        return format!("Satie task {id} \"{name}\"");
    }
    let Some(pid) = holder.pid else {
        return holder.command.clone();
    };
    let mut w = format!("pid {pid} (`{}`", holder.command);
    if let Some(cwd) = &holder.cwd {
        w += &format!(", in {cwd}");
    }
    if let Some(up) = holder.up_secs {
        w += &format!(", up {}", crate::probe::age(up));
    }
    w + "), not a Satie task"
}

/// Says what looks wrong with a task: ports it needs that something else already holds, and
/// expected ports that never came up. Returns `(problems, missing expected ports)`.
fn diagnose(
    task: &Task,
    own: &[u16],
    listening: &[crate::probe::Listener],
    tasks_by_group: &std::collections::HashMap<u32, (String, String)>,
    settle_ms: i64,
) -> (Vec<String>, Vec<u16>) {
    let foreign = |port: u16| {
        listening
            .iter()
            .find(|l| l.port == port && l.pgid != Some(task.pid))
    };
    let mut problems = vec![];
    let mut explained: Vec<u16> = vec![];

    // Ports the log complains are taken, and who really holds them right now.
    let log = crate::probe::log_for_scanning(Path::new(&task.log_path));
    for port in crate::probe::ports_named_in_conflicts(&log) {
        if let Some(holder) = foreign(port) {
            problems.push(format!(
                "Port {port} is already in use by {}.",
                holder_text(holder, tasks_by_group)
            ));
            explained.push(port);
        }
    }

    // Ports it should be serving that it isn't: taken by something else, or just not up. A task
    // that is still starting gets a moment; one that already failed is judged straight away.
    let unmet: Vec<u16> = match task.status {
        TaskStatus::Running if now_ms() - task.started_at >= settle_ms => task
            .expected_ports
            .iter()
            .copied()
            .filter(|p| !own.contains(p))
            .collect(),
        TaskStatus::Failed => task
            .expected_ports
            .iter()
            .copied()
            .filter(|p| !own.contains(p))
            .collect(),
        _ => vec![],
    };
    for p in &unmet {
        if explained.contains(p) {
            continue;
        }
        problems.push(match foreign(*p) {
            Some(holder) => format!(
                "Port {p} is already in use by {}.",
                holder_text(holder, tasks_by_group)
            ),
            None => format!("Expected port {p} is not listening; check the task's logs."),
        });
    }
    // Only a running task can still bring its ports up.
    let missing = if task.status == TaskStatus::Running {
        unmet
    } else {
        vec![]
    };
    (problems, missing)
}

/// Dev stacks open dozens of sockets: inspector ports, random high ports for internal IPC.
/// Show the ones a person would recognise; only if a task listens on nothing else, show those.
pub fn presentable_ports(all: Vec<u16>) -> Vec<u16> {
    // Above this is Linux's ephemeral range, where programs get *arbitrary* ports.
    const EPHEMERAL: u16 = 32768;
    let named: Vec<u16> = all.iter().copied().filter(|p| *p < EPHEMERAL).collect();
    if named.is_empty() {
        all
    } else {
        named
    }
}

// ---------------------------------------------------------------------------------------------
// The launcher and its MCP endpoint
// ---------------------------------------------------------------------------------------------

struct Inner {
    /// MCP bearer tokens of runs that are in progress.
    runs: Mutex<HashMap<String, RunInfo>>,
    tasks: Mutex<HashMap<String, Task>>,
    store: Store,
    dir: PathBuf,
    /// How long a task gets to open its ports before a missing one is called a problem.
    settle_ms: std::sync::atomic::AtomicI64,
}

/// Handle to Satie. Cheap to clone.
#[derive(Clone)]
pub struct Satie {
    url: String,
    inner: Arc<Inner>,
}

/// Revokes a run's token when dropped, so a finished run can't keep calling tools.
pub struct TokenGuard {
    satie: Satie,
    token: String,
}

impl Drop for TokenGuard {
    fn drop(&mut self) {
        self.satie.inner.runs.lock().unwrap().remove(&self.token);
    }
}

const GUIDANCE: &str = "Anything that must keep running after your turn ends (dev servers, simulations, watchers, \
long jobs) has to be started with the `satie` MCP tool `task_start`, not with Bash `run_in_background`, `nohup`, a trailing \
`&` or tmux: processes started those ways are stopped when the turn ends. `task_start` keeps the process running on its own, \
shows it to the user in the Tasks panel, and `task_logs`, `task_list` and `task_stop` manage it. Pass `port` when the process \
serves on one, so the call waits until it is up.";

const HOOK_DENY: &str = "Background commands are stopped when this turn ends. Start it with the satie MCP tool `task_start` \
instead: it keeps running independently and the user can see and stop it in the Tasks panel.";

/// A hook that denies Bash calls with `run_in_background: true` (reads the hook input on stdin).
fn hook_command() -> String {
    let deny = json!({ "hookSpecificOutput": {
        "hookEventName": "PreToolUse",
        "permissionDecision": "deny",
        "permissionDecisionReason": HOOK_DENY,
    }});
    format!(
        "if grep -Eq '\"run_in_background\"[[:space:]]*:[[:space:]]*true'; then printf '%s' '{deny}'; fi"
    )
}

impl Satie {
    /// Starts Satie: loads and reconciles known tasks, starts the monitor, and serves MCP on
    /// `addr` (loopback only; use port 0 for any free port).
    pub async fn start(addr: SocketAddr, store: Store, dir: PathBuf) -> std::io::Result<Satie> {
        Self::start_with(addr, store, dir, Duration::from_secs(1)).await
    }

    pub async fn start_with(
        addr: SocketAddr,
        store: Store,
        dir: PathBuf,
        tick: Duration,
    ) -> std::io::Result<Satie> {
        assert!(addr.ip().is_loopback(), "Satie only listens on loopback");
        std::fs::create_dir_all(&dir)?;
        let tasks = store
            .list_tasks()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|v| serde_json::from_value::<Task>(v).ok())
            .map(|t| (t.id.clone(), t))
            .collect();
        let listener = TcpListener::bind(addr).await?;
        let satie = Satie {
            url: format!("http://{}/mcp", listener.local_addr()?),
            inner: Arc::new(Inner {
                runs: Mutex::default(),
                tasks: Mutex::new(tasks),
                store,
                dir,
                settle_ms: std::sync::atomic::AtomicI64::new(SETTLE_MS),
            }),
        };
        satie.tick(); // reconcile: what ran while we were down?

        let monitor = satie.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(tick).await;
                monitor.tick();
            }
        });
        let app = Router::new()
            .route(
                "/mcp",
                post(mcp).get(|| async { StatusCode::METHOD_NOT_ALLOWED }),
            )
            .with_state(satie.clone());
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(satie)
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    /// Runs that currently hold a token.
    pub fn active_runs(&self) -> usize {
        self.inner.runs.lock().unwrap().len()
    }

    /// Gives a run its own token. Returns what to hand the agent and a guard that revokes the
    /// token when the run ends.
    pub fn register_run(&self, info: RunInfo) -> (SatieArgs, TokenGuard) {
        let token = uuid::Uuid::new_v4().simple().to_string();
        self.inner.runs.lock().unwrap().insert(token.clone(), info);
        let mcp_config = json!({
            "mcpServers": { "satie": {
                "type": "http",
                "url": self.url,
                "headers": { "Authorization": format!("Bearer {token}") },
            }}
        })
        .to_string();
        let settings = json!({ "hooks": { "PreToolUse": [{
            "matcher": "Bash",
            "hooks": [{ "type": "command", "command": hook_command() }],
        }]}})
        .to_string();
        let args = SatieArgs {
            mcp_config,
            system_prompt: GUIDANCE.to_string(),
            settings,
        };
        (
            args,
            TokenGuard {
                satie: self.clone(),
                token,
            },
        )
    }

    // ----- state -----------------------------------------------------------------------------

    fn save(&self, task: &Task) {
        let _ = self
            .inner
            .store
            .save_task(&serde_json::to_value(task).unwrap());
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
        self.list_settled(
            project,
            self.inner
                .settle_ms
                .load(std::sync::atomic::Ordering::Relaxed),
        )
    }

    /// How long a task gets to open its expected ports before a missing one counts as a problem.
    pub fn set_settle_ms(&self, ms: i64) {
        self.inner
            .settle_ms
            .store(ms, std::sync::atomic::Ordering::Relaxed);
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
        let listening = crate::probe::listeners();
        let by_group: std::collections::HashMap<u32, (String, String)> = all
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
                        crate::probe::process_summary(task.pid)
                    } else {
                        vec![]
                    },
                    task,
                }
            })
            .collect()
    }

    /// Starts a detached process. It keeps running if this process, or the agent, exits.
    pub fn start_task(&self, req: StartTask) -> Result<Task, String> {
        let command = req.command.trim().to_string();
        if command.is_empty() {
            return Err("`command` is required".into());
        }
        let cwd = req
            .cwd
            .as_deref()
            .or(req.project.as_deref())
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .ok_or("A working directory is required.")?;
        let cwd = expand_home(cwd);
        if !cwd.is_dir() {
            return Err(format!("{} is not a folder.", cwd.display()));
        }

        let id = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
        let log_path = self.inner.dir.join(format!("{id}.log"));
        let exit_path = self.exit_path(&id);
        let mut log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map_err(|e| e.to_string())?;
        let _ = writeln!(log, "$ {command}");
        let err = log.try_clone().map_err(|e| e.to_string())?;

        // The outer shell records the inner command's exit code, so the result is known even
        // if Bach was restarted in the meantime.
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
        let mut child = cmd.spawn().map_err(|e| format!("failed to start: {e}"))?;
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
            run_id: req.run_id,
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
        Ok(task)
    }

    /// Stops a running task and everything it started.
    pub async fn stop_task(&self, id: &str) -> Result<Task, String> {
        let task = self.get(id).ok_or("No such task.")?;
        if task.status != TaskStatus::Running {
            return Ok(task);
        }
        // Mark first: the monitor only watches running tasks, so it can't call this "lost".
        let stopped = self
            .update(id, |t| {
                t.status = TaskStatus::Stopped;
                t.ended_at = Some(now_ms());
            })
            .ok_or("No such task.")?;
        signal_group(task.pid, libc::SIGTERM);
        for _ in 0..60 {
            if !task_alive(task.pid, task.start_ticks) {
                return Ok(stopped);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        signal_group(task.pid, libc::SIGKILL);
        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(stopped)
    }

    /// Forgets a finished task and its files.
    pub fn remove_task(&self, id: &str) -> Result<(), String> {
        let task = self.get(id).ok_or("No such task.")?;
        if task.status == TaskStatus::Running {
            return Err("Stop the task before removing it.".into());
        }
        self.inner.tasks.lock().unwrap().remove(id);
        let _ = self.inner.store.delete_task(id);
        let _ = std::fs::remove_file(&task.log_path);
        let _ = std::fs::remove_file(self.exit_path(id));
        Ok(())
    }

    /// The last `lines` lines of a task's output.
    pub fn logs(&self, id: &str, lines: usize) -> Result<String, String> {
        let task = self.get(id).ok_or("No such task.")?;
        let mut f = std::fs::File::open(&task.log_path).map_err(|e| e.to_string())?;
        let len = f.metadata().map_err(|e| e.to_string())?.len();
        let take = len.min(256 * 1024);
        f.seek(SeekFrom::Start(len - take))
            .map_err(|e| e.to_string())?;
        let mut buf = String::new();
        f.take(take).read_to_string(&mut buf).ok(); // may cut a multibyte char at the start: best effort
        let all: Vec<&str> = buf.lines().collect();
        Ok(all[all.len().saturating_sub(lines)..].join("\n"))
    }

    /// Waits until the task listens on `port` (if given), ends, or `max` passes. Without a port,
    /// gives it a moment to fail fast.
    pub async fn wait_ready(
        &self,
        id: &str,
        ports: &[u16],
        ready: Option<&Ready>,
        max: Duration,
    ) -> Option<Result<crate::probe::HttpProbe, String>> {
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
                    let probe = crate::probe::http_get(&r.url, Duration::from_millis(1500)).await;
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
                let now = crate::probe::listeners();
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

    /// A piece of a task's log, for viewers that follow it: see [`crate::probe::read_chunk`].
    pub fn log_chunk(
        &self,
        id: &str,
        from: Option<u64>,
        max_bytes: u64,
    ) -> Result<crate::probe::Chunk, String> {
        let task = self.get(id).ok_or("No such task.")?;
        crate::probe::read_chunk(
            Path::new(&task.log_path),
            from,
            max_bytes.clamp(1024, 8 * 1024 * 1024),
            task.status == TaskStatus::Running,
        )
        .map_err(|e| e.to_string())
    }

    // ----- MCP -------------------------------------------------------------------------------

    fn tools() -> Value {
        let id = json!({ "type": "object", "properties": { "id": { "type": "string" } }, "required": ["id"] });
        json!([
            {
                "name": "task_start",
                "description": "Start a long-running background process (dev server, simulation, watcher, long job) that keeps running after this turn ends. Use this instead of running such commands in the background yourself, which are stopped when the turn ends. The user sees and can stop it in Bach's Tasks panel. Give every port it should serve on in `ports`: the call waits for them and reports any that do not come up, and why if another process holds the port. Returns the task id, its state and the first output.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "Shell command to run" },
                        "name": { "type": "string", "description": "Short label shown to the user" },
                        "cwd": { "type": "string", "description": "Working directory; defaults to the project folder" },
                        "ports": { "type": "array", "items": { "type": "integer" }, "description": "TCP ports the task should be listening on; wait (up to 30s) for all of them" },
                        "port": { "type": "integer", "description": "Shorthand for a single entry in `ports`" },
                        "ready_url": { "type": "string", "description": "A local http://localhost:PORT/... URL that must answer before the task counts as ready; the response status is reported" },
                        "ready_status": { "type": "integer", "description": "Exact status `ready_url` must return (default: anything below 500)" },
                        "timeout_seconds": { "type": "integer", "description": "How long to wait for `ports` / `ready_url` (default 30, at most 120)" }
                    },
                    "required": ["command"]
                }
            },
            { "name": "task_list", "description": "List this project's background tasks with their state, ports, running processes and any problems found (a port already taken, an expected port not listening).", "inputSchema": { "type": "object", "properties": {} } },
            {
                "name": "task_logs",
                "description": "Show the latest output of a background task.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "id": { "type": "string" }, "lines": { "type": "integer", "description": "How many lines (default 40)" } },
                    "required": ["id"]
                }
            },
            { "name": "task_stop", "description": "Stop a background task and everything it started.", "inputSchema": id },
            {
                "name": "port_info",
                "description": "Say what is listening on TCP ports on this machine: the process, its command line, folder and age, and whether it is one of Bach's background tasks. Use it to find out who holds a port before deciding what to do about a conflict. Without `ports`, lists the recognisable listeners.",
                "inputSchema": { "type": "object", "properties": { "ports": { "type": "array", "items": { "type": "integer" } } } }
            },
            {
                "name": "http_check",
                "description": "Request a local http://localhost:PORT/... URL and report the HTTP status. Only localhost can be checked.",
                "inputSchema": { "type": "object", "properties": { "url": { "type": "string" } }, "required": ["url"] }
            }
        ])
    }

    fn describe(view: &TaskView) -> String {
        let t = &view.task;
        // First line: state and ports. Details, if any, follow on their own lines.
        let mut s = format!(
            "Task {} \"{}\": {}",
            t.id,
            t.name,
            format!("{:?}", t.status).to_lowercase()
        );
        if let Some(code) = t.exit_code {
            s += &format!(" (exit code {code})");
        }
        if t.status == TaskStatus::Running {
            s += &format!(", pid {}", t.pid);
        }
        if !view.ports.is_empty() {
            let ports: Vec<String> = view.ports.iter().map(|p| p.to_string()).collect();
            s += &format!(", listening on {}", ports.join(", "));
        }
        if t.status == TaskStatus::Running && !t.expected_ports.is_empty() {
            let parts: Vec<String> = t
                .expected_ports
                .iter()
                .map(|p| {
                    let state = if view.up_ports.contains(p) {
                        "up"
                    } else if view.missing_ports.contains(p) {
                        "NOT listening"
                    } else {
                        "not up yet"
                    };
                    format!("{p} {state}")
                })
                .collect();
            s += &format!("\n  Expected ports: {}", parts.join(", "));
        }
        if !view.processes.is_empty() {
            s += &format!("\n  Processes: {}", view.processes.join(", "));
        }
        for p in &view.problems {
            s += &format!("\n  Problem: {p}");
        }
        s
    }

    /// Who is listening on `wanted` ports (or, with none given, on the recognisable ones).
    fn port_report(&self, wanted: &[u16]) -> String {
        let all = crate::probe::listeners();
        let tasks: std::collections::HashMap<u32, (String, String)> = self
            .inner
            .tasks
            .lock()
            .unwrap()
            .values()
            .map(|t| (t.pid, (t.id.clone(), t.name.clone())))
            .collect();
        let line = |l: &crate::probe::Listener| {
            let owner = match l.pgid.and_then(|g| tasks.get(&g)) {
                Some((id, name)) => format!("Satie task {id} \"{name}\""),
                None => "not a Satie task".to_string(),
            };
            match l.pid {
                Some(pid) => format!(
                    "Port {}: pid {pid} `{}`{}{} - {owner}",
                    l.port,
                    l.command,
                    l.cwd
                        .as_ref()
                        .map(|c| format!(", in {c}"))
                        .unwrap_or_default(),
                    l.up_secs
                        .map(|u| format!(", up {}", crate::probe::age(u)))
                        .unwrap_or_default(),
                ),
                None => format!("Port {}: {}", l.port, l.command),
            }
        };
        let mut lines = vec![];
        if wanted.is_empty() {
            let mut shown = all.clone();
            shown.retain(|l| l.port < 32768);
            shown.dedup_by_key(|l| (l.port, l.pid));
            lines.extend(shown.iter().take(60).map(line));
            if lines.is_empty() {
                lines.push("Nothing is listening on a recognisable port.".into());
            }
        } else {
            for p in wanted {
                let mut holders: Vec<&crate::probe::Listener> =
                    all.iter().filter(|l| l.port == *p).collect();
                holders.dedup_by_key(|l| l.pid);
                if holders.is_empty() {
                    lines.push(format!("Port {p}: nothing is listening."));
                }
                lines.extend(holders.into_iter().map(line));
            }
        }
        lines.join("\n")
    }

    /// A task, checked to belong to the calling run's project.
    fn owned(&self, run: &RunInfo, id: &str) -> Result<Task, String> {
        let t = self
            .get(id)
            .ok_or_else(|| format!("No task with id `{id}`."))?;
        match (&run.cwd, &t.project) {
            (Some(mine), Some(theirs)) if mine != theirs => Err(format!("No task with id `{id}`.")),
            _ => Ok(t),
        }
    }

    fn view(&self, id: &str) -> Option<TaskView> {
        self.list(None).into_iter().find(|v| v.task.id == id)
    }

    async fn call(&self, run: &RunInfo, name: &str, args: &Value) -> Result<String, String> {
        match name {
            "task_start" => {
                let mut ports: Vec<u16> = args["ports"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .chain(args.get("port"))
                    .filter_map(|p| {
                        p.as_u64()
                            .and_then(|p| u16::try_from(p).ok())
                            .filter(|p| *p > 0)
                    })
                    .collect();
                ports.sort_unstable();
                ports.dedup();
                if let Some(url) = args["ready_url"].as_str() {
                    // Fail before launching anything: a URL we would never request can't become ready.
                    crate::probe::validate_local_url(url)
                        .map_err(|e| format!("`ready_url`: {e}"))?;
                }
                let wait = Duration::from_secs(
                    args["timeout_seconds"].as_u64().unwrap_or(30).clamp(1, 120),
                );
                let ready = args["ready_url"].as_str().map(|url| Ready {
                    url: url.to_string(),
                    status: args["ready_status"]
                        .as_u64()
                        .and_then(|p| u16::try_from(p).ok()),
                });
                let task = self.start_task(StartTask {
                    command: args["command"].as_str().unwrap_or_default().to_string(),
                    cwd: args["cwd"].as_str().map(String::from),
                    name: args["name"].as_str().map(String::from),
                    project: run.cwd.clone(),
                    run_id: Some(run.run_id.clone()),
                    ports: ports.clone(),
                })?;
                let probe = self
                    .wait_ready(&task.id, &ports, ready.as_ref(), wait)
                    .await;
                // The wait is over: anything still not listening is a problem now, not "still starting".
                let view = self
                    .list_settled(None, 0)
                    .into_iter()
                    .find(|v| v.task.id == task.id)
                    .ok_or("The task disappeared.")?;
                let mut text = Self::describe(&view);
                if let Some(r) = &ready {
                    match probe {
                        Some(Ok(p)) => {
                            text += &format!(
                                "\n  HTTP check: GET {} -> {} ({} ms)",
                                r.url, p.status, p.ms
                            )
                        }
                        Some(Err(e)) => {
                            text += &format!("\n  HTTP check: GET {} did not answer: {e}", r.url)
                        }
                        None => {
                            text += &format!(
                            "\n  HTTP check: GET {} was not attempted (the ports did not come up)",
                            r.url
                        )
                        }
                    }
                }
                let out = self.logs(&task.id, 15).unwrap_or_default();
                if !out.is_empty() {
                    text += &format!("\nOutput so far:\n{out}");
                }
                Ok(text)
            }
            "port_info" => {
                let wanted: Vec<u16> = args["ports"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|p| p.as_u64().and_then(|p| u16::try_from(p).ok()))
                    .collect();
                Ok(self.port_report(&wanted))
            }
            "http_check" => {
                let url = args["url"].as_str().unwrap_or_default();
                Ok(
                    match crate::probe::http_get(url, Duration::from_secs(4)).await {
                        Ok(p) => format!("GET {url} -> {} ({} ms)", p.status, p.ms),
                        Err(e) => format!("GET {url} did not answer: {e}"),
                    },
                )
            }
            "task_list" => {
                let views = self.list(run.cwd.as_deref());
                Ok(if views.is_empty() {
                    "No background tasks.".into()
                } else {
                    views
                        .iter()
                        .map(Self::describe)
                        .collect::<Vec<_>>()
                        .join("\n")
                })
            }
            "task_logs" => {
                let id = args["id"].as_str().unwrap_or_default();
                self.owned(run, id)?;
                let lines = args["lines"].as_u64().unwrap_or(40).clamp(1, 500) as usize;
                self.logs(id, lines)
            }
            "task_stop" => {
                let id = args["id"].as_str().unwrap_or_default();
                self.owned(run, id)?;
                self.stop_task(id).await?;
                Ok(self
                    .view(id)
                    .map(|v| Self::describe(&v))
                    .unwrap_or_else(|| "Stopped.".into()))
            }
            other => Err(format!("Unknown tool `{other}`")),
        }
    }

    /// One JSON-RPC message. `None` for notifications, which get no reply.
    async fn handle(&self, run: &RunInfo, msg: &Value) -> Option<Value> {
        let id = msg.get("id")?.clone();
        let reply = |result: Value| json!({ "jsonrpc": "2.0", "id": id, "result": result });
        Some(match msg["method"].as_str().unwrap_or_default() {
            "initialize" => reply(json!({
                // Echo the client's protocol version: nothing here is version-specific.
                "protocolVersion": msg["params"]["protocolVersion"].as_str().unwrap_or("2025-03-26"),
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "satie", "version": env!("CARGO_PKG_VERSION") },
            })),
            "ping" => reply(json!({})),
            "tools/list" => reply(json!({ "tools": Self::tools() })),
            "tools/call" => {
                let name = msg["params"]["name"].as_str().unwrap_or_default();
                let (text, is_error) = match self.call(run, name, &msg["params"]["arguments"]).await
                {
                    Ok(t) => (t, false),
                    Err(e) => (e, true),
                };
                reply(json!({ "content": [{ "type": "text", "text": text }], "isError": is_error }))
            }
            other => json!({
                "jsonrpc": "2.0", "id": id,
                "error": { "code": -32601, "message": format!("Method not found: {other}") },
            }),
        })
    }
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("authorization")?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

async fn mcp(State(satie): State<Satie>, headers: HeaderMap, Json(body): Json<Value>) -> Response {
    let run = bearer(&headers).and_then(|t| satie.inner.runs.lock().unwrap().get(t).cloned());
    let Some(run) = run else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    match body {
        Value::Array(msgs) => {
            let mut replies = vec![];
            for m in &msgs {
                replies.extend(satie.handle(&run, m).await);
            }
            if replies.is_empty() {
                StatusCode::ACCEPTED.into_response()
            } else {
                Json(replies).into_response()
            }
        }
        msg => match satie.handle(&run, &msg).await {
            Some(reply) => Json(reply).into_response(),
            None => StatusCode::ACCEPTED.into_response(), // a notification
        },
    }
}

// ---------------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    fn tmp(label: &str) -> PathBuf {
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let d = std::env::temp_dir().join(format!("bach-satie-{label}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    async fn satie_in(store: &Store, dir: &Path) -> Satie {
        Satie::start_with(
            "127.0.0.1:0".parse().unwrap(),
            store.clone(),
            dir.join("tasks"),
            Duration::from_millis(50),
        )
        .await
        .unwrap()
    }

    fn req(command: &str, cwd: &Path) -> StartTask {
        StartTask {
            command: command.into(),
            cwd: Some(cwd.to_string_lossy().into()),
            ..Default::default()
        }
    }

    async fn until(what: &str, mut cond: impl FnMut() -> bool) {
        for _ in 0..100 {
            if cond() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("timed out waiting for: {what}");
    }

    fn status(s: &Satie, id: &str) -> TaskStatus {
        s.get(id).unwrap().status
    }

    /// Any process whose command line contains `needle`.
    fn procs_matching(needle: &str) -> Vec<u32> {
        std::fs::read_dir("/proc")
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
            .filter(|pid| {
                std::fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|c| {
                    String::from_utf8_lossy(&c)
                        .replace('\0', " ")
                        .contains(needle)
                }) && proc_stat(*pid).is_some_and(|(st, _)| st != 'Z')
                    && *pid != std::process::id()
            })
            .collect()
    }

    #[test]
    fn presents_recognisable_ports_first() {
        // A dev stack: named ports plus random internal ones -> only the named ones.
        assert_eq!(
            presentable_ports(vec![4001, 8787, 9229, 34865, 40000]),
            vec![4001, 8787, 9229]
        );
        // Only random ports: those are the point of the task, so they stay.
        assert_eq!(presentable_ports(vec![40001, 40000]), vec![40001, 40000]);
        assert_eq!(presentable_ports(vec![]), Vec::<u16>::new());
    }

    #[tokio::test]
    async fn runs_detached_and_records_output_and_exit_code() {
        let dir = tmp("exit");
        let satie = satie_in(&Store::in_memory().unwrap(), &dir).await;

        let ok = satie
            .start_task(req("echo hello; echo oops >&2", &dir))
            .unwrap();
        until("exit 0", || status(&satie, &ok.id) == TaskStatus::Exited).await;
        let out = satie.logs(&ok.id, 10).unwrap();
        assert!(
            out.contains("$ echo hello") && out.contains("hello") && out.contains("oops"),
            "{out}"
        );
        assert_eq!(satie.get(&ok.id).unwrap().exit_code, Some(0));

        let bad = satie.start_task(req("exit 3", &dir)).unwrap();
        until("exit 3", || status(&satie, &bad.id) == TaskStatus::Failed).await;
        assert_eq!(satie.get(&bad.id).unwrap().exit_code, Some(3));

        // Only the last lines are returned.
        let many = satie.start_task(req("seq 1 100", &dir)).unwrap();
        until("seq", || status(&satie, &many.id) == TaskStatus::Exited).await;
        assert_eq!(satie.logs(&many.id, 3).unwrap(), "98\n99\n100");

        // It ran in its own session, not ours.
        let pid = satie.start_task(req("sleep 4701", &dir)).unwrap();
        let (sid_of_task, sid_ours) = (
            process_group_of(pid.pid).unwrap(),
            process_group_of(std::process::id()).unwrap(),
        );
        assert_ne!(sid_of_task, sid_ours);
        assert_eq!(sid_of_task, pid.pid, "it leads its own process group");
        satie.stop_task(&pid.id).await.unwrap();

        assert!(satie.start_task(req("", &dir)).is_err());
        assert!(satie
            .start_task(req("true", Path::new("/definitely/not/here")))
            .is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn stop_kills_everything_the_task_started() {
        let dir = tmp("stop");
        let satie = satie_in(&Store::in_memory().unwrap(), &dir).await;
        let t = satie
            .start_task(req("sleep 4702 & sleep 4702 & wait", &dir))
            .unwrap();
        until("children up", || procs_matching("sleep 4702").len() >= 2).await;

        let stopped = satie.stop_task(&t.id).await.unwrap();
        assert_eq!(stopped.status, TaskStatus::Stopped);
        until("all gone", || procs_matching("sleep 4702").is_empty()).await;
        // The monitor doesn't reclassify a stopped task as lost.
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(status(&satie, &t.id), TaskStatus::Stopped);
        assert!(
            satie.stop_task(&t.id).await.is_ok(),
            "stopping twice is harmless"
        );

        satie.remove_task(&t.id).unwrap();
        assert!(satie.get(&t.id).is_none() && !Path::new(&t.log_path).exists());
        let running = satie.start_task(req("sleep 4703", &dir)).unwrap();
        assert!(
            satie.remove_task(&running.id).is_err(),
            "can't remove a running task"
        );
        satie.stop_task(&running.id).await.unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn tasks_survive_a_restart_and_are_reconciled() {
        let dir = tmp("restart");
        let store = Store::in_memory().unwrap();
        let before = satie_in(&store, &dir).await;
        let long = before.start_task(req("sleep 4704", &dir)).unwrap();
        let short = before.start_task(req("sleep 0.3; exit 7", &dir)).unwrap();
        // A task whose process vanished without a trace.
        let gone = before.start_task(req("sleep 4705", &dir)).unwrap();
        // Simulate a restart: nothing of `before` is consulted from here on.
        let running_pids = (long.pid, gone.pid);
        drop(before);
        signal_group(running_pids.1, libc::SIGKILL);
        std::fs::remove_file(dir.join("tasks").join(format!("{}.exit", gone.id))).ok();
        tokio::time::sleep(Duration::from_millis(600)).await;

        let after = satie_in(&store, &dir).await;
        assert_eq!(
            status(&after, &long.id),
            TaskStatus::Running,
            "still running, and found again"
        );
        assert!(task_alive(
            long.pid,
            after.get(&long.id).unwrap().start_ticks
        ));
        assert_eq!(status(&after, &short.id), TaskStatus::Failed);
        assert_eq!(
            after.get(&short.id).unwrap().exit_code,
            Some(7),
            "exit code read from its file"
        );
        until("lost", || status(&after, &gone.id) == TaskStatus::Lost).await;

        // The new instance can stop what the old one started.
        after.stop_task(&long.id).await.unwrap();
        until("stopped", || !task_alive(long.pid, None)).await;
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn reports_listening_ports_and_waits_for_them() {
        let dir = tmp("ports");
        let satie = satie_in(&Store::in_memory().unwrap(), &dir).await;
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let t = satie
            .start_task(req(
                &format!("python3 -m http.server {port} --bind 127.0.0.1"),
                &dir,
            ))
            .unwrap();
        satie
            .wait_ready(&t.id, &[port], None, Duration::from_secs(10))
            .await;
        let view = satie.view(&t.id).unwrap();
        assert_eq!(view.ports, vec![port], "{view:?}");
        assert!(Satie::describe(&view).contains(&format!("listening on {port}")));
        satie.stop_task(&t.id).await.unwrap();
        assert!(satie.view(&t.id).unwrap().ports.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn the_task_view_hides_random_ports_but_keeps_named_ones() {
        let dir = tmp("presentable");
        let satie = satie_in(&Store::in_memory().unwrap(), &dir).await;
        // A named port (below the ephemeral range) that is free right now.
        let named = (20000u16..30000)
            .find(|p| std::net::TcpListener::bind(("127.0.0.1", *p)).is_ok())
            .expect("a free port");
        let script = format!(
            "import socket,time\na=socket.socket(); a.bind(('127.0.0.1',{named})); a.listen()\nb=socket.socket(); b.bind(('127.0.0.1',0)); b.listen()\nc=socket.socket(); c.bind(('127.0.0.1',0)); c.listen()\ntime.sleep(60)\n"
        );
        std::fs::write(dir.join("two.py"), script).unwrap();
        let t = satie.start_task(req("python3 two.py", &dir)).unwrap();

        // The process really has three listeners...
        until("three listeners", || ports_of_group(t.pid).len() == 3).await;
        // ...but only the recognisable one is presented, in the view and in what the agent is told.
        let view = satie.view(&t.id).unwrap();
        assert_eq!(view.ports, vec![named], "{view:?}");
        assert!(Satie::describe(&view).contains(&format!("listening on {named}")));
        // The first line is state + ports; any details follow on their own lines.
        let first = Satie::describe(&view);
        let first = first.lines().next().unwrap();
        assert!(
            first.ends_with(&format!("listening on {named}")),
            "the description lists just the named port: {first}"
        );
        satie.stop_task(&t.id).await.unwrap();

        // A task that only has random ports keeps them: they are the point of it.
        std::fs::write(dir.join("rand.py"), "import socket,time\nb=socket.socket(); b.bind(('127.0.0.1',0)); b.listen()\ntime.sleep(60)\n").unwrap();
        let r = satie.start_task(req("python3 rand.py", &dir)).unwrap();
        until("random listener", || ports_of_group(r.pid).len() == 1).await;
        assert_eq!(satie.view(&r.id).unwrap().ports.len(), 1);
        satie.stop_task(&r.id).await.unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    // ----- MCP over HTTP -------------------------------------------------------------------

    /// Minimal HTTP/1.1 client: (status, body).
    async fn post(url: &str, token: Option<&str>, body: &Value) -> (u16, String) {
        let addr = url.trim_start_matches("http://").trim_end_matches("/mcp");
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        let body = body.to_string();
        let auth = token
            .map(|t| format!("Authorization: Bearer {t}\r\n"))
            .unwrap_or_default();
        let req = format!(
            "POST /mcp HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\n{auth}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        s.write_all(req.as_bytes()).await.unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).await.unwrap();
        let status = out.split_whitespace().nth(1).unwrap().parse().unwrap();
        (
            status,
            out.split("\r\n\r\n").nth(1).unwrap_or_default().to_string(),
        )
    }

    fn rpc(id: u32, method: &str, params: Value) -> Value {
        json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
    }

    fn token_of(args: &SatieArgs) -> String {
        let cfg: Value = serde_json::from_str(&args.mcp_config).unwrap();
        cfg["mcpServers"]["satie"]["headers"]["Authorization"]
            .as_str()
            .unwrap()
            .trim_start_matches("Bearer ")
            .to_string()
    }

    async fn call(satie: &Satie, token: &str, id: u32, tool: &str, args: Value) -> (bool, String) {
        let (st, body) = post(
            satie.url(),
            Some(token),
            &rpc(id, "tools/call", json!({ "name": tool, "arguments": args })),
        )
        .await;
        assert_eq!(st, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        (
            v["result"]["isError"].as_bool().unwrap(),
            v["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .to_string(),
        )
    }

    #[tokio::test]
    async fn speaks_mcp_and_manages_tasks_for_its_own_project() {
        let dir = tmp("mcp");
        let proj_a = dir.join("a");
        let proj_b = dir.join("b");
        std::fs::create_dir_all(&proj_a).unwrap();
        std::fs::create_dir_all(&proj_b).unwrap();
        let satie = satie_in(&Store::in_memory().unwrap(), &dir).await;
        let (cfg_a, guard_a) = satie.register_run(RunInfo {
            run_id: "run-a".into(),
            cwd: Some(proj_a.to_string_lossy().into()),
        });
        let (cfg_b, _guard_b) = satie.register_run(RunInfo {
            run_id: "run-b".into(),
            cwd: Some(proj_b.to_string_lossy().into()),
        });
        let (ta, tb) = (token_of(&cfg_a), token_of(&cfg_b));

        // What the agent is handed.
        let cfg: Value = serde_json::from_str(&cfg_a.mcp_config).unwrap();
        assert_eq!(cfg["mcpServers"]["satie"]["type"], "http");
        assert_eq!(cfg["mcpServers"]["satie"]["url"], satie.url());
        assert!(
            cfg_a.system_prompt.contains("task_start")
                && cfg_a.system_prompt.contains("run_in_background")
        );

        // No token, or a wrong one: refused.
        assert_eq!(
            post(satie.url(), None, &rpc(1, "ping", json!({}))).await.0,
            401
        );
        assert_eq!(
            post(satie.url(), Some("nope"), &rpc(1, "ping", json!({})))
                .await
                .0,
            401
        );

        let (st, body) = post(
            satie.url(),
            Some(&ta),
            &rpc(1, "initialize", json!({ "protocolVersion": "2025-06-18" })),
        )
        .await;
        assert_eq!(st, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            (
                v["result"]["serverInfo"]["name"].as_str(),
                v["result"]["protocolVersion"].as_str()
            ),
            (Some("satie"), Some("2025-06-18"))
        );
        let note = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        assert_eq!(post(satie.url(), Some(&ta), &note).await.0, 202);
        let (_, body) = post(satie.url(), Some(&ta), &rpc(2, "tools/list", json!({}))).await;
        let v: Value = serde_json::from_str(&body).unwrap();
        let names: Vec<_> = v["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "task_start",
                "task_list",
                "task_logs",
                "task_stop",
                "port_info",
                "http_check"
            ]
        );

        // Start: defaults to the run's project, waits briefly, reports state and first output.
        let (err, text) = call(
            &satie,
            &ta,
            3,
            "task_start",
            json!({ "command": "echo up; sleep 4706", "name": "web" }),
        )
        .await;
        assert!(!err, "{text}");
        assert!(
            text.contains("\"web\": running")
                && text.contains("Output so far:")
                && text.contains("up"),
            "{text}"
        );
        let id = satie.list(None)[0].task.id.clone();
        let t = satie.get(&id).unwrap();
        assert_eq!(
            (t.cwd.as_str(), t.run_id.as_deref()),
            (proj_a.to_str().unwrap(), Some("run-a"))
        );

        // List/logs/stop see it from project A, but project B's run does not.
        let (_, list) = call(&satie, &ta, 4, "task_list", json!({})).await;
        assert!(list.contains(&id) && list.contains("running"), "{list}");
        assert_eq!(
            call(&satie, &tb, 5, "task_list", json!({})).await.1,
            "No background tasks."
        );
        for tool in ["task_logs", "task_stop"] {
            let (err, text) = call(&satie, &tb, 6, tool, json!({ "id": id })).await;
            assert!(err && text.contains("No task"), "{tool}: {text}");
        }
        assert_eq!(
            status(&satie, &id),
            TaskStatus::Running,
            "project B could not stop it"
        );

        let (_, logs) = call(&satie, &ta, 7, "task_logs", json!({ "id": id, "lines": 5 })).await;
        assert!(logs.contains("up"), "{logs}");
        let (err, text) = call(&satie, &ta, 8, "task_stop", json!({ "id": id })).await;
        assert!(!err && text.contains("stopped"), "{text}");
        until("stopped", || procs_matching("sleep 4706").is_empty()).await;

        // A bad call is a tool error the agent can read, not a transport failure.
        assert!(call(&satie, &ta, 9, "task_start", json!({})).await.0);
        let v: Value = serde_json::from_str(
            &post(satie.url(), Some(&ta), &rpc(10, "nope", json!({})))
                .await
                .1,
        )
        .unwrap();
        assert_eq!(v["error"]["code"], -32601);

        // Once the run is over its token stops working.
        assert_eq!(satie.active_runs(), 2);
        drop(guard_a);
        assert_eq!(satie.active_runs(), 1);
        assert_eq!(
            post(satie.url(), Some(&ta), &rpc(11, "ping", json!({})))
                .await
                .0,
            401
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    // ----- diagnosing conflicts, several ports, HTTP readiness ------------------------------

    /// A process that is not a Satie task, listening on `port`.
    struct Foreign(std::process::Child);
    impl Foreign {
        async fn listening_on(port: u16, dir: &Path) -> Foreign {
            let child = std::process::Command::new("python3")
                .args([
                    "-m",
                    "http.server",
                    &port.to_string(),
                    "--bind",
                    "127.0.0.1",
                ])
                .current_dir(dir)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            until("foreign listener", || {
                std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
            })
            .await;
            Foreign(child)
        }
        fn pid(&self) -> u32 {
            self.0.id()
        }
    }
    impl Drop for Foreign {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    fn run_in(dir: &Path) -> RunInfo {
        RunInfo {
            run_id: "r".into(),
            cwd: Some(dir.to_string_lossy().into()),
        }
    }

    #[tokio::test]
    async fn explains_who_holds_a_port_a_task_could_not_get() {
        let dir = tmp("conflict");
        let satie = satie_in(&Store::in_memory().unwrap(), &dir).await;
        let port = free_port();
        let squatter = Foreign::listening_on(port, &dir).await;

        // 1) The log names the port (as vite/node do): the holder is found and described.
        let t = satie
            .start_task(req(
                &format!("echo 'Error: Port {port} is already in use'; exit 1"),
                &dir,
            ))
            .unwrap();
        until("failed", || status(&satie, &t.id) == TaskStatus::Failed).await;
        let view = satie.view(&t.id).unwrap();
        let text = view.problems.join("\n");
        assert!(
            text.contains(&format!(
                "Port {port} is already in use by pid {}",
                squatter.pid()
            )),
            "{text}"
        );
        assert!(
            text.contains("not a Satie task") && text.contains(dir.to_str().unwrap()),
            "{text}"
        );
        assert!(
            Satie::describe(&view).contains("Problem: Port"),
            "{}",
            Satie::describe(&view)
        );

        // 2) The holder can be one of our own tasks, and is named as such.
        let other = satie
            .start_task(req(
                &format!("python3 -m http.server {} --bind 127.0.0.1", free_port()),
                &dir,
            ))
            .unwrap();
        let ported = satie.start_task(req("sleep 60", &dir)).unwrap();
        let _ = (other, ported);

        // 3) port_info answers the question directly, for one port and for a port nobody has.
        let report = satie.port_report(&[port, free_port()]);
        assert!(
            report.contains(&format!("Port {port}: pid {}", squatter.pid())),
            "{report}"
        );
        assert!(
            report.contains("not a Satie task") && report.contains("up "),
            "{report}"
        );
        assert!(report.contains("nothing is listening"), "{report}");
        for t in satie.list(None) {
            let _ = satie.stop_task(&t.task.id).await;
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn port_info_recognises_its_own_tasks() {
        let dir = tmp("portinfo");
        let satie = satie_in(&Store::in_memory().unwrap(), &dir).await;
        let port = free_port();
        let t = satie
            .start_task(req(
                &format!("python3 -m http.server {port} --bind 127.0.0.1"),
                &dir,
            ))
            .unwrap();
        satie
            .wait_ready(&t.id, &[port], None, Duration::from_secs(10))
            .await;
        let report = satie.port_report(&[port]);
        assert!(report.contains(&format!("Satie task {}", t.id)), "{report}");
        assert!(!report.contains("not a Satie task"), "{report}");

        // A second task that wants the same port is told exactly which task has it.
        let clash = satie
            .start_task(StartTask {
                ports: vec![port],
                ..req(
                    &format!("python3 -m http.server {port} --bind 127.0.0.1"),
                    &dir,
                )
            })
            .unwrap();
        until("clash exits", || {
            status(&satie, &clash.id) != TaskStatus::Running
        })
        .await;
        let view = satie
            .list_settled(None, 0)
            .into_iter()
            .find(|v| v.task.id == clash.id)
            .unwrap();
        assert!(
            view.problems.iter().any(|p| p.contains(&format!(
                "Port {port} is already in use by Satie task {}",
                t.id
            ))),
            "{:?}",
            view.problems
        );
        satie.stop_task(&t.id).await.unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn task_start_reports_which_expected_ports_came_up_and_gives_up_early() {
        let dir = tmp("expected");
        let satie = satie_in(&Store::in_memory().unwrap(), &dir).await;
        satie.set_settle_ms(3000); // a short grace period so the test can see both sides of it
        let (good, taken) = (free_port(), free_port());
        let squatter = Foreign::listening_on(taken, &dir).await;
        let a = format!("python3 -m http.server {good} --bind 127.0.0.1");
        let b = format!("python3 -m http.server {taken} --bind 127.0.0.1");

        // Two services, one of which cannot get its port: like `just dev` with a stale vite.
        let started = std::time::Instant::now();
        let (err, text) = {
            let run = run_in(&dir);
            let r = satie
                .call(&run, "task_start", &json!({ "command": format!("{a} & {b} & wait"), "name": "stack", "ports": [good, taken] }))
                .await;
            (r.is_err(), r.unwrap_or_else(|e| e))
        };
        let waited = started.elapsed();
        assert!(!err, "{text}");
        assert!(
            waited < Duration::from_secs(12),
            "must not sit out the full 30s timeout: {waited:?}"
        );
        let expected_line = text
            .lines()
            .find(|l| l.contains("Expected ports:"))
            .expect("an Expected ports line");
        assert!(
            expected_line.contains(&format!("{good} up"))
                && expected_line.contains(&format!("{taken} NOT listening")),
            "{text}"
        );
        assert!(
            text.contains(&format!(
                "Problem: Port {taken} is already in use by pid {}",
                squatter.pid()
            )),
            "{text}"
        );
        assert!(
            text.contains("Processes:") && text.contains("python3"),
            "which processes are alive: {text}"
        );

        // The same picture is available later, from task_list.
        let (_, list) = (
            0,
            satie
                .call(&run_in(&dir), "task_list", &json!({}))
                .await
                .unwrap(),
        );
        // (still inside the grace period: not yet called a failure, and not claimed to be up)
        assert!(list.contains(&format!("{taken} not up yet")), "{list}");
        assert!(!list.contains(&format!("{taken} up")), "{list}");
        tokio::time::sleep(Duration::from_millis(3200)).await;
        let later = satie
            .call(&run_in(&dir), "task_list", &json!({}))
            .await
            .unwrap();
        assert!(later.contains(&format!("{taken} NOT listening")), "{later}");
        assert!(later.contains("Problem: Port"), "{later}");
        // ...but a task that is simply still starting is not reported as broken.
        let slow = satie
            .start_task(StartTask {
                ports: vec![free_port()],
                ..req("sleep 30", &dir)
            })
            .unwrap();
        let view = satie.view(&slow.id).unwrap();
        assert!(
            view.problems.is_empty() && view.missing_ports.is_empty(),
            "{:?}",
            view.problems
        );

        for t in satie.list(None) {
            satie.stop_task(&t.task.id).await.unwrap();
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn waits_for_all_expected_ports_and_an_http_answer() {
        let dir = tmp("ready");
        std::fs::write(dir.join("index.html"), "hi").unwrap();
        let satie = satie_in(&Store::in_memory().unwrap(), &dir).await;
        let (p1, p2) = (free_port(), free_port());
        let cmd = format!("python3 -m http.server {p1} --bind 127.0.0.1 & python3 -m http.server {p2} --bind 127.0.0.1 & wait");
        let run = run_in(&dir);
        let text = satie
            .call(&run, "task_start", &json!({ "command": cmd, "ports": [p1, p2], "ready_url": format!("http://localhost:{p2}/index.html") }))
            .await
            .unwrap();
        let expected_line = text
            .lines()
            .find(|l| l.contains("Expected ports:"))
            .expect("an Expected ports line");
        assert!(
            expected_line.contains(&format!("{p1} up"))
                && expected_line.contains(&format!("{p2} up")),
            "{text}"
        );
        assert!(
            text.contains(&format!(
                "HTTP check: GET http://localhost:{p2}/index.html -> 200"
            )),
            "{text}"
        );

        // A required status that is not what the server returns is reported, not hidden.
        let text2 = satie
            .call(
                &run,
                "task_start",
                &json!({ "command": "sleep 30", "ready_url": format!("http://localhost:{p1}/nope.html"), "ready_status": 200, "ports": [], "timeout_seconds": 2 }),
            )
            .await
            .unwrap();
        assert!(
            text2.contains("HTTP check: GET") && text2.contains("-> 404"),
            "{text2}"
        );

        // Only local URLs are ever requested, and a bad one is refused before anything is launched.
        let before = satie.list(None).len();
        let started = std::time::Instant::now();
        let bad = satie
            .call(
                &run,
                "task_start",
                &json!({ "command": "sleep 30", "ready_url": "http://example.com/" }),
            )
            .await
            .unwrap_err();
        assert!(
            bad.contains("ready_url") && bad.contains("Only localhost"),
            "{bad}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "must fail at once, not time out"
        );
        assert_eq!(satie.list(None).len(), before, "no task was started");
        let check = satie
            .call(
                &run,
                "http_check",
                &json!({ "url": format!("http://127.0.0.1:{p1}/index.html") }),
            )
            .await
            .unwrap();
        assert!(check.contains("-> 200"), "{check}");
        let closed = satie
            .call(
                &run,
                "http_check",
                &json!({ "url": format!("http://127.0.0.1:{}/", free_port()) }),
            )
            .await
            .unwrap();
        assert!(closed.contains("did not answer"), "{closed}");
        for t in satie.list(None) {
            satie.stop_task(&t.task.id).await.unwrap();
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn serves_a_task_log_in_chunks_that_follow_it() {
        let dir = tmp("chunk");
        let satie = satie_in(&Store::in_memory().unwrap(), &dir).await;
        let t = satie
            .start_task(req("echo first; sleep 1; echo second; sleep 30", &dir))
            .unwrap();
        // (The log's first line echoes the command, so match whole lines.)
        let has_line = |c: &crate::probe::Chunk, l: &str| c.text.lines().any(|x| x == l);
        until("first line", || {
            satie
                .log_chunk(&t.id, None, 4096)
                .is_ok_and(|c| has_line(&c, "first"))
        })
        .await;
        let c1 = satie.log_chunk(&t.id, None, 4096).unwrap();
        until("second line", || {
            satie
                .log_chunk(&t.id, Some(c1.next), 4096)
                .is_ok_and(|c| has_line(&c, "second"))
        })
        .await;
        let c2 = satie.log_chunk(&t.id, Some(c1.next), 4096).unwrap();
        assert!(
            has_line(&c2, "second") && !has_line(&c2, "first"),
            "a continuation, not a re-read: {c2:?}"
        );
        assert_eq!(c2.offset, c1.next);
        assert!(satie.log_chunk("nope", None, 100).is_err());
        satie.stop_task(&t.id).await.unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn hook_denies_only_background_bash_calls() {
        let (dir, satie_args) = {
            // Only the settings are needed; a token guard isn't.
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let dir = tmp("hook");
            let s = rt.block_on(satie_in(&Store::in_memory().unwrap(), &dir));
            (
                dir,
                s.register_run(RunInfo {
                    run_id: "r".into(),
                    cwd: None,
                })
                .0,
            )
        };
        let settings: Value = serde_json::from_str(&satie_args.settings).unwrap();
        let hook = &settings["hooks"]["PreToolUse"][0];
        assert_eq!(hook["matcher"], "Bash");
        let cmd = hook["hooks"][0]["command"].as_str().unwrap();

        let run = |input: &str| {
            let mut child = Command::new("sh")
                .arg("-c")
                .arg(cmd)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.as_bytes())
                .unwrap();
            String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap()
        };
        let bg = run(
            r#"{"tool_name":"Bash","tool_input":{"command":"sleep 9","run_in_background":true}}"#,
        );
        let v: Value = serde_json::from_str(&bg).unwrap();
        assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "deny");
        assert!(v["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .unwrap()
            .contains("task_start"));
        let spaced = run(r#"{"tool_input": {"run_in_background" : true}}"#);
        assert!(spaced.contains("deny"), "whitespace variants too");
        assert_eq!(
            run(r#"{"tool_name":"Bash","tool_input":{"command":"ls","run_in_background":false}}"#),
            ""
        );
        assert_eq!(
            run(r#"{"tool_name":"Bash","tool_input":{"command":"ls"}}"#),
            ""
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
