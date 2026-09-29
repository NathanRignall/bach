//! Port forwarding from this computer to the machine agents run on, over the same SSH connection
//! the app already has open.
//!
//! The app's `ssh <host> bach-server attach` is started as a connection master
//! ([`master_options`]); each forward is added to it with
//! `ssh -S <control> -O forward -L 127.0.0.1:<local>:localhost:<remote> <host>`. So there's no
//! second login, forwards listen on this computer's loopback only, and nothing new listens on
//! the remote.
//!
//! Forwards are either *auto* (the ports of running background tasks, kept in step by
//! [`Forwards::set_auto`]) or added by hand. They end with the SSH connection;
//! [`Forwards::reapply`] brings them back after a reconnect.
use std::{
    collections::{BTreeMap, BTreeSet},
    net::TcpListener,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
};
use tokio::{process::Command, sync::Mutex};

/// `ssh` options that make a connection the master others (forwards) go through.
pub fn master_options(control: &Path) -> Vec<String> {
    vec![
        "-o".into(),
        "ControlMaster=auto".into(),
        "-o".into(),
        format!("ControlPath={}", control.display()),
        "-o".into(),
        "ControlPersist=no".into(),
    ]
}

/// Where the control socket goes. Kept short: macOS allows only 104 bytes for a socket path, and
/// its per-user temp dirs are long. `%C` is ssh's hash of the connection.
pub fn control_path() -> PathBuf {
    // SAFETY: getuid can't fail.
    let uid = unsafe { libc_getuid() };
    PathBuf::from(format!("/tmp/bach-{uid}")).join("cm-%C")
}

extern "C" {
    #[link_name = "getuid"]
    fn libc_getuid() -> u32;
}

/// Creates the control socket's folder, private to this user (the socket gives access to the
/// SSH connection).
pub fn prepare_control_dir(control: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(dir) = control.parent() {
        std::fs::create_dir_all(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Forward {
    pub remote: u16,
    pub local: u16,
    /// Added for a background task (and removed when no running task uses the port).
    pub auto: bool,
}

pub struct Forwards {
    ssh: String,
    /// Options that pick the connection (e.g. `-F <config>`), before the control options.
    ssh_options: Vec<String>,
    host: String,
    control: PathBuf,
    state: Mutex<BTreeMap<u16, Forward>>,
    on_change: Arc<dyn Fn(Vec<Forward>) + Send + Sync>,
}

/// A port this computer can listen on right now: `want` if it's free (and not privileged),
/// otherwise any free one.
fn free_local_port(want: u16) -> Result<u16, String> {
    if want >= 1024 && TcpListener::bind(("127.0.0.1", want)).is_ok() {
        return Ok(want);
    }
    TcpListener::bind(("127.0.0.1", 0))
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .map_err(|e| format!("no free local port: {e}"))
}

impl Forwards {
    /// `ssh` is the program (normally "ssh"); `ssh_options` go before everything else.
    pub fn new(
        ssh: impl Into<String>,
        ssh_options: Vec<String>,
        host: impl Into<String>,
        control: PathBuf,
        on_change: impl Fn(Vec<Forward>) + Send + Sync + 'static,
    ) -> Self {
        Forwards {
            ssh: ssh.into(),
            ssh_options,
            host: host.into(),
            control,
            state: Mutex::default(),
            on_change: Arc::new(on_change),
        }
    }

    pub async fn list(&self) -> Vec<Forward> {
        self.state.lock().await.values().cloned().collect()
    }

    /// Runs `ssh -S <control> -O <op> -L <spec> <host>`.
    async fn control(&self, op: &str, local: u16, remote: u16) -> Result<(), String> {
        let spec = format!("127.0.0.1:{local}:localhost:{remote}");
        let out = Command::new(&self.ssh)
            .args(&self.ssh_options)
            .arg("-S")
            .arg(&self.control)
            .args(["-o", "BatchMode=yes", "-O", op, "-L", &spec, &self.host])
            .stdin(Stdio::null())
            .output()
            .await
            .map_err(|e| format!("couldn't run {}: {e}", self.ssh))?;
        if out.status.success() {
            Ok(())
        } else {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            Err(if err.is_empty() {
                format!("ssh -O {op} failed")
            } else {
                err
            })
        }
    }

    async fn changed(&self, state: &BTreeMap<u16, Forward>) {
        (self.on_change)(state.values().cloned().collect());
    }

    /// Forwards `remote`, or returns the existing forward. A forward added by hand stays until
    /// removed by hand, even if a task added it first.
    pub async fn add(&self, remote: u16, auto: bool) -> Result<Forward, String> {
        let mut state = self.state.lock().await;
        if let Some(f) = state.get_mut(&remote) {
            if !auto && f.auto {
                f.auto = false;
                let f = f.clone();
                self.changed(&state).await;
                return Ok(f);
            }
            return Ok(f.clone());
        }
        let f = self.open(remote, auto).await?;
        state.insert(remote, f.clone());
        self.changed(&state).await;
        Ok(f)
    }

    async fn open(&self, remote: u16, auto: bool) -> Result<Forward, String> {
        let local = free_local_port(remote)?;
        self.control("forward", local, remote)
            .await
            .map_err(|e| format!("Couldn't forward port {remote}: {e}"))?;
        Ok(Forward {
            remote,
            local,
            auto,
        })
    }

    pub async fn remove(&self, remote: u16) -> Result<(), String> {
        let mut state = self.state.lock().await;
        if let Some(f) = state.remove(&remote) {
            // Gone already if the connection dropped; either way it's no longer ours.
            let _ = self.control("cancel", f.local, f.remote).await;
            self.changed(&state).await;
        }
        Ok(())
    }

    /// Makes the automatic forwards exactly `ports` (forwards added by hand are left alone).
    /// Returns the ports that couldn't be forwarded, with why.
    pub async fn set_auto(&self, ports: &BTreeSet<u16>) -> Vec<(u16, String)> {
        let mut state = self.state.lock().await;
        let stale: Vec<Forward> = state
            .values()
            .filter(|f| f.auto && !ports.contains(&f.remote))
            .cloned()
            .collect();
        for f in stale {
            let _ = self.control("cancel", f.local, f.remote).await;
            state.remove(&f.remote);
        }
        let mut failed = vec![];
        for &remote in ports {
            if state.contains_key(&remote) {
                continue;
            }
            match self.open(remote, true).await {
                Ok(f) => {
                    state.insert(remote, f);
                }
                Err(e) => failed.push((remote, e)),
            }
        }
        self.changed(&state).await;
        failed
    }

    /// After the SSH connection came back: its forwards went with the old one, so open them
    /// again (on the same local ports where they're still free). Ones that can't be reopened
    /// are dropped.
    pub async fn reapply(&self) -> Vec<(u16, String)> {
        let mut state = self.state.lock().await;
        let mut failed = vec![];
        let old: Vec<Forward> = state.values().cloned().collect();
        for f in old {
            let local = if TcpListener::bind(("127.0.0.1", f.local)).is_ok() {
                f.local
            } else {
                match free_local_port(f.remote) {
                    Ok(p) => p,
                    Err(e) => {
                        failed.push((f.remote, e));
                        state.remove(&f.remote);
                        continue;
                    }
                }
            };
            match self.control("forward", local, f.remote).await {
                Ok(()) => {
                    state.insert(f.remote, Forward { local, ..f });
                }
                Err(e) => {
                    failed.push((f.remote, e));
                    state.remove(&f.remote);
                }
            }
        }
        self.changed(&state).await;
        failed
    }
}
