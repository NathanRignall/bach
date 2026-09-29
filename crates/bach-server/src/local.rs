//! The server's private Unix socket, and `bach-server attach`, which connects its own stdin and
//! stdout to it. That is how the desktop app reaches a server on another machine: it runs
//! `ssh <host> bach-server attach`, so SSH does the authentication and nothing listens on the
//! network. Frames are one JSON object per line (see [`crate::connection`]).
//!
//! The socket, a lock and the log live next to the session database. The lock keeps it to one
//! server per database.
use bach_core::Api;
use bach_protocol::ServerFrame;
use futures_util::StreamExt;
use std::{
    fs::File,
    os::unix::{io::AsRawFd, process::CommandExt},
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    sync::mpsc,
};
use tokio_stream::wrappers::LinesStream;

pub struct Paths {
    pub db: PathBuf,
    pub socket: PathBuf,
    pub lock: PathBuf,
    pub log: PathBuf,
}

impl Paths {
    pub fn for_db(db: PathBuf) -> Self {
        let dir = db.parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        Paths {
            socket: dir.join("bach.sock"),
            lock: dir.join("bach.lock"),
            log: dir.join("server.log"),
            db,
        }
    }
}

/// Holds the server lock for as long as it lives. `None` if another server has it.
pub fn lock(paths: &Paths) -> std::io::Result<Option<File>> {
    if let Some(dir) = paths.lock.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let file = File::create(&paths.lock)?;
    // SAFETY: flock on a descriptor we own; released when the file is closed.
    let taken = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
    Ok(taken.then_some(file))
}

/// Serves connections on the socket forever. Call only while holding the [`lock`].
pub async fn listen(paths: &Paths, api: Arc<Api>) -> std::io::Result<()> {
    // Whatever is there is left over from a server that is gone (we hold the lock).
    let _ = std::fs::remove_file(&paths.socket);
    let listener = UnixListener::bind(&paths.socket)?;
    // Only this user may connect: the socket runs agents as them.
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&paths.socket, std::fs::Permissions::from_mode(0o600))?;
    loop {
        let (stream, _) = listener.accept().await?;
        let api = api.clone();
        tokio::spawn(async move {
            let (read, mut write) = stream.into_split();
            let (out, mut out_rx) = mpsc::unbounded_channel::<ServerFrame>();
            let writer = tokio::spawn(async move {
                while let Some(frame) = out_rx.recv().await {
                    let mut line = serde_json::to_string(&frame).expect("frames serialize");
                    line.push('\n');
                    if write.write_all(line.as_bytes()).await.is_err() {
                        break;
                    }
                }
            });
            let lines = LinesStream::new(BufReader::new(read).lines())
                .take_while(|l| std::future::ready(l.is_ok()))
                .filter_map(|l| async move { l.ok().filter(|l| !l.trim().is_empty()) });
            crate::connection::serve(api, Box::pin(lines), out).await;
            writer.abort();
        });
    }
}

/// Starts `bach-server serve` detached from us (own session, output to the log), so it outlives
/// the SSH connection that started it.
fn start_server(paths: &Paths) -> std::io::Result<()> {
    if let Some(dir) = paths.log.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.log)?;
    let mut cmd = std::process::Command::new(std::env::current_exe()?);
    cmd.arg("serve")
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    // SAFETY: setsid is async-signal-safe and the only call between fork and exec.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = cmd.spawn()?;
    // Reap it if it exits early (another server won the race, or it failed to start).
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Connects stdin and stdout to the server, starting it first if it isn't running.
pub async fn attach(paths: &Paths) -> Result<(), String> {
    let stream = match UnixStream::connect(&paths.socket).await {
        Ok(s) => s,
        Err(_) => {
            start_server(paths).map_err(|e| format!("couldn't start bach-server: {e}"))?;
            let mut connected = None;
            for _ in 0..150 {
                tokio::time::sleep(Duration::from_millis(100)).await;
                if let Ok(s) = UnixStream::connect(&paths.socket).await {
                    connected = Some(s);
                    break;
                }
            }
            connected.ok_or_else(|| {
                let log = std::fs::read_to_string(&paths.log).unwrap_or_default();
                let tail: Vec<&str> = log.lines().rev().take(10).collect();
                format!(
                    "bach-server didn't start (log: {}):\n{}",
                    paths.log.display(),
                    tail.into_iter().rev().collect::<Vec<_>>().join("\n")
                )
            })?
        }
    };
    let (mut from_server, mut to_server) = stream.into_split();
    let (mut stdin, mut stdout) = (tokio::io::stdin(), tokio::io::stdout());
    // Until either side hangs up.
    tokio::select! {
        r = tokio::io::copy(&mut stdin, &mut to_server) => { r.map_err(|e| e.to_string())?; }
        r = async {
            let r = tokio::io::copy(&mut from_server, &mut stdout).await;
            let _ = stdout.flush().await;
            r
        } => { r.map_err(|e| e.to_string())?; }
    }
    Ok(())
}
