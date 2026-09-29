//! process-compose, natively. [`Satie::start_compose`] runs a compose file as a task, in the
//! project's environment, with process-compose's API on a socket of the task's own. While the
//! task runs, Satie polls that API for the state of each process and streams each process's output
//! into a file of its own. Those files are read like any task log, and they outlive
//! process-compose's in-memory buffer and the project itself.
use crate::{probe::Listener, process::parent_of, Error, Satie, StartTask, Task, TaskStatus};
use futures_util::StreamExt;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use satie_protocol::{ComposeProcess, ProcessAction};
use serde::Deserialize;
use std::{
    collections::HashMap,
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
};
use tokio_tungstenite::tungstenite::Message;

/// Longer socket paths don't fit in `sockaddr_un`; tasks in such a folder go without.
const MAX_SOCKET_PATH: usize = 100;
/// Lines back from the end of process-compose's buffer to start a new log at: all of them.
const WHOLE_BUFFER: usize = 1_000_000;

/// Where a task's process-compose serves its API: next to its other files, or, where that path
/// is too long for a socket, in a private folder in /tmp.
pub(crate) fn socket_path(dir: &Path, id: &str) -> PathBuf {
    let next_to_files = dir.join(format!("{id}.pc.sock"));
    if next_to_files.as_os_str().len() <= MAX_SOCKET_PATH {
        return next_to_files;
    }
    shared_socket_dir().join(format!("{id}.pc.sock"))
}

fn shared_socket_dir() -> PathBuf {
    // SAFETY: getuid has no preconditions.
    PathBuf::from(format!("/tmp/satie-{}", unsafe { libc::getuid() }))
}

/// Makes the folder a socket goes in. The one in /tmp must be this user's alone.
fn make_socket_dir(sock: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    let Some(dir) = sock.parent() else {
        return Ok(());
    };
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    if dir == shared_socket_dir() {
        let meta = std::fs::metadata(dir)?;
        // SAFETY: getuid has no preconditions.
        if meta.uid() != unsafe { libc::getuid() } || meta.mode() & 0o077 != 0 {
            return Err(std::io::Error::other(format!("{} isn't private to this user", dir.display())));
        }
    }
    Ok(())
}

/// Where a task's per-process logs and last known process states live.
pub(crate) fn files_dir(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.pc"))
}

fn state_path(dir: &Path, id: &str) -> PathBuf {
    files_dir(dir, id).join("processes.json")
}

pub(crate) fn process_log(dir: &Path, id: &str, process: &str) -> PathBuf {
    let safe: String = process
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '_' })
        .collect();
    files_dir(dir, id).join(format!("{safe}.log"))
}

/// Quotes `s` for `sh`.
fn quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:@%+,".contains(c)) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// How to get into the project's environment in `cwd`: its direnv environment when it has an
/// `.envrc`, else its flake's devShell, else none. A command prefix, possibly empty.
pub fn project_shell(cwd: &Path) -> String {
    if cwd.join(".envrc").is_file() {
        "direnv exec .".into()
    } else if cwd.join("flake.nix").is_file() {
        "nix develop --command".into()
    } else {
        String::new()
    }
}

/// A process-compose project to run as a task.
#[derive(Default)]
pub struct StartCompose {
    /// The compose file, relative to the working directory or absolute.
    pub file: String,
    /// Command prefix that enters the project's environment; by default [`project_shell`].
    pub shell: Option<String>,
    /// Everything but `command`, as for any task.
    pub task: StartTask,
}

// ---------------------------------------------------------------------------------------------
// The API
// ---------------------------------------------------------------------------------------------

/// One request, `(status, body)`. HTTP/1.0, so the answer is never chunked and ends with the
/// connection.
async fn request(sock: &Path, method: &str, path: &str) -> Result<(u16, String), String> {
    let attempt = async {
        let mut s = UnixStream::connect(sock).await.map_err(|e| e.to_string())?;
        let req = format!("{method} {path} HTTP/1.0\r\nHost: localhost\r\nContent-Length: 0\r\n\r\n");
        s.write_all(req.as_bytes()).await.map_err(|e| e.to_string())?;
        let mut buf = vec![];
        s.read_to_end(&mut buf).await.map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&buf);
        let (head, body) = text.split_once("\r\n\r\n").ok_or("not an HTTP answer")?;
        let status = head
            .split_whitespace()
            .nth(1)
            .and_then(|c| c.parse().ok())
            .ok_or("not an HTTP answer")?;
        Ok((status, body.to_string()))
    };
    tokio::time::timeout(Duration::from_secs(3), attempt)
        .await
        .map_err(|_| "process-compose did not answer".to_string())?
}

/// process-compose's error text (`{"error": "..."}`), or the body as it is.
fn error_text(body: &str) -> String {
    #[derive(Deserialize)]
    struct Error {
        error: String,
    }
    serde_json::from_str::<Error>(body).map_or_else(|_| body.trim().to_string(), |e| e.error)
}

#[derive(Deserialize)]
struct WireProcess {
    name: String,
    status: String,
    is_running: bool,
    has_ready_probe: bool,
    is_ready: String,
    restarts: u32,
    exit_code: i32,
    pid: u32,
}

/// Every process of the project, by name.
pub(crate) async fn processes(sock: &Path) -> Result<Vec<ComposeProcess>, String> {
    #[derive(Deserialize)]
    struct List {
        data: Vec<WireProcess>,
    }
    let (status, body) = request(sock, "GET", "/processes").await?;
    if status != 200 {
        return Err(error_text(&body));
    }
    let list: List = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    let mut all: Vec<ComposeProcess> = list
        .data
        .into_iter()
        .map(|w| ComposeProcess {
            name: w.name,
            status: w.status,
            running: w.is_running,
            ready: w.has_ready_probe.then(|| w.is_ready == "Ready"),
            restarts: w.restarts,
            exit_code: w.exit_code,
            pid: w.pid,
            ports: vec![],
        })
        .collect();
    all.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(all)
}

pub(crate) async fn act(sock: &Path, process: &str, action: ProcessAction) -> Result<(), String> {
    let name = utf8_percent_encode(process, NON_ALPHANUMERIC);
    let (method, path) = match action {
        ProcessAction::Start => ("POST", format!("/process/start/{name}")),
        ProcessAction::Stop => ("PATCH", format!("/process/stop/{name}")),
        ProcessAction::Restart => ("POST", format!("/process/restart/{name}")),
    };
    match request(sock, method, &path).await? {
        (200, _) => Ok(()),
        (_, body) => Err(error_text(&body)),
    }
}

/// Appends a process's output to `file`, one line per message, until process-compose closes the
/// stream. Starts `offset` lines back from the end of what process-compose still holds.
async fn stream_logs(sock: &Path, process: &str, offset: usize, file: &Path) -> Result<(), String> {
    #[derive(Deserialize)]
    struct Line {
        message: String,
    }
    let stream = UnixStream::connect(sock).await.map_err(|e| e.to_string())?;
    let name = utf8_percent_encode(process, NON_ALPHANUMERIC);
    let url = format!("ws://localhost/process/logs/ws?name={name}&offset={offset}&follow=true");
    let (mut ws, _) = tokio_tungstenite::client_async(url, stream)
        .await
        .map_err(|e| e.to_string())?;
    let mut out = OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
        .map_err(|e| e.to_string())?;
    while let Some(msg) = ws.next().await {
        match msg.map_err(|e| e.to_string())? {
            Message::Text(t) => {
                if let Ok(line) = serde_json::from_str::<Line>(&t) {
                    writeln!(out, "{}", line.message).map_err(|e| e.to_string())?;
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
    Ok(())
}

/// Gives each process the ports that it, or anything it started, listens on.
fn assign_ports(procs: &mut [ComposeProcess], listeners: &[Listener], sid: u32) {
    let by_pid: HashMap<u32, usize> = procs
        .iter()
        .enumerate()
        .filter(|(_, p)| p.running && p.pid != 0)
        .map(|(i, p)| (p.pid, i))
        .collect();
    for l in listeners.iter().filter(|l| l.sid == Some(sid)) {
        let mut pid = l.pid;
        // Up the tree from the listener to the process process-compose started.
        for _ in 0..64 {
            let Some(p) = pid else { break };
            if let Some(i) = by_pid.get(&p) {
                procs[*i].ports.push(l.port);
                break;
            }
            pid = parent_of(p).filter(|pp| *pp > 1);
        }
    }
    for p in procs {
        p.ports.sort_unstable();
        p.ports.dedup();
        p.ports = crate::presentable_ports(std::mem::take(&mut p.ports));
    }
}

/// Whether a process is where it's going to stay for now: not on its way up or down, and ready if
/// it has a readiness probe.
fn settled(p: &ComposeProcess) -> bool {
    !moving(p) && (!p.running || p.ready != Some(false))
}

/// On its way up or down (or waiting for its dependencies).
fn moving(p: &ComposeProcess) -> bool {
    matches!(
        p.status.as_str(),
        "Pending" | "Launching" | "Restarting" | "Terminating" | "Scheduled"
    )
}

/// Whether a process has failed: stopped with a non-zero exit code (and not being restarted), or
/// in process-compose's error state.
pub(crate) fn failed(p: &ComposeProcess) -> bool {
    (!p.running && p.exit_code != 0 && !moving(p)) || p.status == "Error"
}

// ---------------------------------------------------------------------------------------------
// Following the projects in running tasks
// ---------------------------------------------------------------------------------------------

impl Satie {
    /// Runs a process-compose project as a task, with its API on the task's own socket.
    pub fn start_compose(&self, req: StartCompose) -> Result<Task, Error> {
        let file = req.file.trim();
        if file.is_empty() {
            return Err(Error::Invalid("`file` is required".into()));
        }
        let cwd = req
            .task
            .cwd
            .as_deref()
            .or(req.task.project.as_deref())
            .map(|c| crate::process::expand_home(c.trim()))
            .ok_or_else(|| Error::Invalid("A working directory is required.".into()))?;
        if !cwd.join(file).is_file() {
            return Err(Error::Invalid(format!("There is no compose file {file} in {}.", cwd.display())));
        }
        let shell = req.shell.unwrap_or_else(|| project_shell(&cwd));
        let id = crate::new_task_id();
        let sock = socket_path(&self.inner.dir, &id);
        make_socket_dir(&sock).map_err(|e| Error::Failed(format!("Couldn't make a place for the socket: {e}")))?;
        let sock = sock.to_string_lossy();
        let command = format!(
            "{shell} process-compose up -f {} -t=false -U -u {}",
            quote(file),
            quote(&sock)
        );
        let name = req.task.name.clone().or_else(|| Some(file.to_string()));
        self.launch(
            id,
            StartTask {
                command: command.trim().to_string(),
                cwd: Some(cwd.to_string_lossy().into_owned()),
                name,
                ..req.task
            },
            Some(file.to_string()),
        )
    }

    /// Waits until every process of a compose task has settled (see [`settled`]), one has failed,
    /// the task ends, or `max` passes.
    pub(crate) async fn wait_compose(&self, id: &str, max: Duration) {
        let started = std::time::Instant::now();
        let sock = socket_path(&self.inner.dir, id);
        while started.elapsed() < max {
            if self.get(id).is_none_or(|t| t.status != TaskStatus::Running) {
                return;
            }
            if let Ok(procs) = processes(&sock).await {
                if !procs.is_empty() && (procs.iter().all(settled) || procs.iter().any(failed)) {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        self.refresh_compose(id).await;
    }

    /// Asks for a task's process states now, rather than waiting for the next tick.
    async fn refresh_compose(&self, id: &str) {
        let Some(task) = self.get(id).filter(|t| t.status == TaskStatus::Running) else {
            return;
        };
        if let Ok(mut procs) = processes(&socket_path(&self.inner.dir, id)).await {
            assign_ports(&mut procs, &crate::probe::listeners(), task.pid);
            if self.set_compose(id, procs) {
                self.publish();
            }
        }
    }

    /// Every `tick`: for each running task with a process-compose project, the state of its
    /// processes, and a log stream for each process.
    pub(crate) async fn watch_compose(self, tick: Duration) {
        loop {
            tokio::time::sleep(tick).await;
            let running: Vec<(String, u32)> = self
                .inner
                .tasks
                .lock()
                .unwrap()
                .values()
                .filter(|t| t.status == TaskStatus::Running && t.compose_file.is_some())
                .map(|t| (t.id.clone(), t.pid))
                .collect();
            let mut listeners = None;
            let mut changed = false;
            for (id, pid) in running {
                let sock = socket_path(&self.inner.dir, &id);
                if !sock.exists() {
                    continue;
                }
                let Ok(mut procs) = processes(&sock).await else {
                    continue;
                };
                let listeners = listeners.get_or_insert_with(crate::probe::listeners);
                assign_ports(&mut procs, listeners, pid);
                for p in &procs {
                    self.follow_logs(&id, &p.name);
                }
                changed |= self.set_compose(&id, procs);
            }
            if changed {
                self.publish();
            }
        }
    }

    /// Records a task's process states; true if they changed.
    fn set_compose(&self, id: &str, procs: Vec<ComposeProcess>) -> bool {
        let mut known = self.inner.compose.lock().unwrap();
        if known.get(id) == Some(&procs) {
            return false;
        }
        let dir = files_dir(&self.inner.dir, id);
        let saved = std::fs::create_dir_all(&dir)
            .and_then(|_| std::fs::write(state_path(&self.inner.dir, id), serde_json::to_vec(&procs)?));
        if let Err(e) = saved {
            eprintln!("satie: couldn't save the processes of task {id}: {e}");
        }
        known.insert(id.to_string(), procs);
        true
    }

    /// The processes of a task's process-compose project, if it has one: as last reported while
    /// it runs, and as they were left once it has ended.
    pub(crate) fn compose_of(&self, task: &Task) -> Option<Vec<ComposeProcess>> {
        task.compose_file.as_ref()?;
        let (id, running) = (task.id.as_str(), task.status == TaskStatus::Running);
        let mut known = self.inner.compose.lock().unwrap();
        if !known.contains_key(id) {
            let saved = std::fs::read(state_path(&self.inner.dir, id)).ok()?;
            known.insert(id.to_string(), serde_json::from_slice(&saved).ok()?);
        }
        let mut procs = known.get(id)?.clone();
        if !running {
            // Its last report may predate the end; nothing in it runs any more.
            for p in &mut procs {
                if p.running {
                    p.running = false;
                    p.status = "Stopped".into();
                }
                p.ports.clear();
            }
        }
        Some(procs)
    }

    pub(crate) fn forget_compose(&self, id: &str) {
        self.inner.compose.lock().unwrap().remove(id);
        let _ = std::fs::remove_dir_all(files_dir(&self.inner.dir, id));
        let _ = std::fs::remove_file(socket_path(&self.inner.dir, id));
    }

    /// Starts copying a process's output to its own log file, unless that is already happening.
    /// Keeps at it (reconnecting if the stream drops) while the task runs and has the process.
    fn follow_logs(&self, id: &str, process: &str) {
        let key = (id.to_string(), process.to_string());
        if !self.inner.log_streams.lock().unwrap().insert(key.clone()) {
            return;
        }
        let satie = self.clone();
        tokio::spawn(async move {
            let (id, process) = &key;
            let sock = socket_path(&satie.inner.dir, id);
            let file = process_log(&satie.inner.dir, id, process);
            let _ = std::fs::create_dir_all(files_dir(&satie.inner.dir, id));
            // A new log starts with everything process-compose still holds. An existing one (Satie
            // was restarted) carries on from now: replaying the buffer would repeat lines.
            let mut offset = WHOLE_BUFFER;
            if file.exists() {
                offset = 0;
                if let Ok(mut f) = OpenOptions::new().append(true).open(&file) {
                    let _ = writeln!(f, "… (output while Satie was not watching may be missing)");
                }
            }
            loop {
                let _ = stream_logs(&sock, process, offset, &file).await;
                offset = 0;
                tokio::time::sleep(Duration::from_secs(1)).await;
                let still_there = satie.get(id).is_some_and(|t| t.status == TaskStatus::Running)
                    && sock.exists()
                    && satie
                        .inner
                        .compose
                        .lock()
                        .unwrap()
                        .get(id)
                        .is_some_and(|procs| procs.iter().any(|p| &p.name == process));
                if !still_there {
                    break;
                }
            }
            satie.inner.log_streams.lock().unwrap().remove(&key);
        });
    }

    /// Starts, stops or restarts one process of a running task's process-compose project.
    pub async fn process_action(
        &self,
        id: &str,
        process: &str,
        action: ProcessAction,
    ) -> Result<(), Error> {
        let task = self.get(id).ok_or_else(crate::no_such_task)?;
        if task.compose_file.is_none() {
            return Err(Error::Invalid("This task isn't a process-compose project.".into()));
        }
        if task.status != TaskStatus::Running {
            return Err(Error::Invalid("The task isn't running.".into()));
        }
        act(&socket_path(&self.inner.dir, id), process, action)
            .await
            .map_err(|e| Error::Failed(format!("process-compose: {e}")))?;
        self.refresh_compose(id).await;
        Ok(())
    }
}
