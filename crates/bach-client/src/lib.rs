//! A client for a Bach backend on another machine, reached through a command whose stdin and
//! stdout carry the protocol: in the desktop app, `ssh <host> bach-server attach`. SSH does the
//! authentication (keys, agent, `~/.ssh/config`), so this needs nothing of its own.
//!
//! [`Remote`] keeps the connection up: it checks the server speaks the same protocol, matches
//! replies to calls, hands events on, and reconnects (with backoff) when the command ends.
use bach_protocol::{fingerprint, ApiError, ClientFrame, ErrorCode};
use serde_json::Value;
use std::{
    collections::HashMap,
    process::Stdio,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::Command,
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
};

/// Where the connection stands.
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Connecting,
    /// `version` is the server's.
    Connected { version: String },
    /// Why it isn't connected. Retried automatically unless `retrying` is false.
    Disconnected { error: String, retrying: bool },
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, ApiError>>>>>;

struct Link {
    lines: mpsc::UnboundedSender<String>,
    pending: Pending,
}

pub struct Remote {
    label: String,
    status: watch::Receiver<Status>,
    link: Arc<Mutex<Option<Link>>>,
    next_id: AtomicU64,
    supervisor: JoinHandle<()>,
}

impl Drop for Remote {
    fn drop(&mut self) {
        // Ends the connection too: the child is killed when its handle is dropped.
        self.supervisor.abort();
    }
}

/// How long a call waits for a connection that is being (re)established.
const CONNECT_WAIT: Duration = Duration::from_secs(20);

impl Remote {
    /// Starts connecting with `command` (program and arguments). `label` names the other end in
    /// messages ("orion"). Events go to `on_event` (a `ServerEvent` as JSON), status changes to
    /// `on_status`.
    pub fn connect(
        label: impl Into<String>,
        command: Vec<String>,
        on_event: impl Fn(Value) + Send + Sync + 'static,
        on_status: impl Fn(Status) + Send + Sync + 'static,
    ) -> Remote {
        let label = label.into();
        let (status_tx, status) = watch::channel(Status::Connecting);
        let link: Arc<Mutex<Option<Link>>> = Arc::default();
        let supervisor = tokio::spawn(supervise(
            label.clone(),
            command,
            link.clone(),
            Arc::new(on_event),
            Arc::new(move |s: Status| {
                let _ = status_tx.send(s.clone());
                on_status(s);
            }),
        ));
        Remote {
            label,
            status,
            link,
            next_id: AtomicU64::new(1),
            supervisor,
        }
    }

    pub fn status(&self) -> Status {
        self.status.borrow().clone()
    }

    /// Runs a command on the backend. Waits a while for a connection attempt in progress; fails
    /// straight away (saying why) while disconnected.
    pub async fn call(&self, cmd: &str, args: Value) -> Result<Value, ApiError> {
        let mut status = self.status.clone();
        let ready = tokio::time::timeout(CONNECT_WAIT, async {
            loop {
                match &*status.borrow_and_update() {
                    Status::Connected { .. } => return Ok(()),
                    // Between attempts: say why now rather than make the caller wait.
                    Status::Disconnected { error, .. } => return Err(error.clone()),
                    Status::Connecting => {}
                }
                if status.changed().await.is_err() {
                    return Err("The connection was closed.".to_string());
                }
            }
        })
        .await;
        let unavailable = |why: &str| {
            ApiError::new(
                ErrorCode::Unavailable,
                format!("Not connected to {}: {why}", self.label),
            )
        };
        match ready {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(unavailable(&e)),
            Err(_) => {
                return Err(unavailable(match &*status.borrow() {
                    Status::Disconnected { error, .. } => error,
                    _ => "still connecting",
                }))
            }
        }

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        {
            let link = self.link.lock().unwrap();
            let link = link.as_ref().ok_or_else(|| unavailable("the connection dropped"))?;
            link.pending.lock().unwrap().insert(id, tx);
            let frame = ClientFrame {
                id,
                cmd: cmd.into(),
                args: Some(args),
            };
            if link
                .lines
                .send(serde_json::to_string(&frame).expect("frames serialize"))
                .is_err()
            {
                link.pending.lock().unwrap().remove(&id);
                return Err(unavailable("the connection dropped"));
            }
        }
        rx.await
            .unwrap_or_else(|_| Err(unavailable("the connection dropped")))
    }
}

type OnEvent = Arc<dyn Fn(Value) + Send + Sync>;
type OnStatus = Arc<dyn Fn(Status) + Send + Sync>;

/// Connects, serves the connection until it ends, and tries again.
async fn supervise(
    label: String,
    command: Vec<String>,
    link: Arc<Mutex<Option<Link>>>,
    on_event: OnEvent,
    on_status: OnStatus,
) {
    let mut backoff = Duration::from_secs(1);
    loop {
        on_status(Status::Connecting);
        let started = Instant::now();
        let outcome = session(&label, &command, &link, &on_event, &on_status).await;
        // Whatever was waiting on this connection won't get an answer.
        if let Some(l) = link.lock().unwrap().take() {
            for (_, tx) in l.pending.lock().unwrap().drain() {
                let _ = tx.send(Err(ApiError::new(
                    ErrorCode::Unavailable,
                    format!("Lost the connection to {label}."),
                )));
            }
        }
        match outcome {
            End::Retry(error) => {
                // A connection that held for a while starts the backoff over.
                if started.elapsed() > Duration::from_secs(30) {
                    backoff = Duration::from_secs(1);
                }
                on_status(Status::Disconnected {
                    error,
                    retrying: true,
                });
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
            End::GiveUp(error) => {
                on_status(Status::Disconnected {
                    error,
                    retrying: false,
                });
                return;
            }
        }
    }
}

enum End {
    Retry(String),
    /// Retrying can't help (e.g. the versions differ).
    GiveUp(String),
}

/// One run of the command, until it ends.
async fn session(
    label: &str,
    command: &[String],
    link: &Arc<Mutex<Option<Link>>>,
    on_event: &OnEvent,
    on_status: &OnStatus,
) -> End {
    let Some((program, args)) = command.split_first() else {
        return End::GiveUp("No command to connect with.".into());
    };
    let mut child = match Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return End::GiveUp(format!("Couldn't run `{program}`: {e}")),
    };
    let mut stdin = child.stdin.take().expect("piped");
    let mut lines = BufReader::new(child.stdout.take().expect("piped")).lines();
    // What went wrong is usually in stderr (ssh's own errors, a missing bach-server).
    let stderr = {
        let mut err = child.stderr.take().expect("piped");
        tokio::spawn(async move {
            let mut buf = String::new();
            let _ = err.read_to_string(&mut buf).await;
            buf
        })
    };
    let failure = |stderr: String, fallback: &str| {
        let text = stderr.trim();
        if text.is_empty() {
            fallback.to_string()
        } else {
            // The last few lines say why; ssh can be chatty before that.
            let tail: Vec<&str> = text.lines().rev().take(4).collect();
            tail.into_iter().rev().collect::<Vec<_>>().join("\n")
        }
    };

    // The server speaks first.
    let hello = match tokio::time::timeout(CONNECT_WAIT, lines.next_line()).await {
        Ok(Ok(Some(line))) => serde_json::from_str::<Value>(&line).unwrap_or(Value::Null),
        Ok(_) => {
            let _ = child.wait().await;
            let err = stderr.await.unwrap_or_default();
            return End::Retry(failure(err, &format!("{label} closed the connection.")));
        }
        Err(_) => return End::Retry(format!("{label} didn't answer.")),
    };
    if hello["kind"] != "hello" {
        return End::GiveUp(format!("{label} doesn't look like a Bach server."));
    }
    if hello["protocol"] != fingerprint().as_str() {
        return End::GiveUp(format!(
            "Bach on {label} ({}) is a different version from this app; update one of them.",
            hello["version"].as_str().unwrap_or("unknown version")
        ));
    }

    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let pending: Pending = Arc::default();
    *link.lock().unwrap() = Some(Link {
        lines: tx,
        pending: pending.clone(),
    });
    on_status(Status::Connected {
        version: hello["version"].as_str().unwrap_or_default().to_string(),
    });

    let writer = tokio::spawn(async move {
        while let Some(mut line) = rx.recv().await {
            line.push('\n');
            if stdin.write_all(line.as_bytes()).await.is_err() {
                break;
            }
            let _ = stdin.flush().await;
        }
    });
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(frame) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let reply = |result| {
            if let Some(tx) = frame["id"]
                .as_u64()
                .and_then(|id| pending.lock().unwrap().remove(&id))
            {
                let _ = tx.send(result);
            }
        };
        match frame["kind"].as_str() {
            Some("event") => on_event(frame["event"].clone()),
            Some("reply") => reply(Ok(frame["result"].clone())),
            Some("error") => reply(Err(serde_json::from_value(frame["error"].clone())
                .unwrap_or_else(|_| ApiError::failed("The server sent an unreadable error.")))),
            _ => {}
        }
    }
    writer.abort();
    let _ = child.wait().await;
    let err = stderr.await.unwrap_or_default();
    End::Retry(failure(err, &format!("Lost the connection to {label}.")))
}
