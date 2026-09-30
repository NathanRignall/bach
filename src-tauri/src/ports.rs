//! The desktop app's port forwarding: over SSH, the ports background tasks listen on are
//! forwarded to this computer as they come and go (unless switched off), and any port can be
//! forwarded by hand. See `bach_client::forward` for how.
use bach_client::forward::{Forward, Forwards};
use bach_protocol::{
    app::{Forwarding, PortForward},
    FORWARDS_CHANNEL,
};
use serde_json::Value;
use std::{
    collections::{BTreeSet, HashMap},
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tauri::{AppHandle, Emitter};

pub struct Ports {
    handle: AppHandle,
    /// Where the automatic-forwarding setting is saved.
    settings: PathBuf,
    auto: Mutex<bool>,
    /// Set while connected over SSH.
    forwards: Mutex<Option<Arc<Forwards>>>,
    /// Ports each running task listens on.
    tasks: Mutex<HashMap<String, BTreeSet<u16>>>,
    /// Automatic forwards stopped by hand: not reopened while a task still uses the port.
    dismissed: Mutex<BTreeSet<u16>>,
    error: Mutex<Option<String>>,
}

fn convert(f: Forward) -> PortForward {
    PortForward {
        remote: f.remote,
        local: f.local,
        auto: f.auto,
    }
}

/// Opens a URL in the default browser.
pub fn open_url(url: &str) -> Result<(), String> {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program)
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("couldn't open {url}: {e}"))
}

impl Ports {
    pub fn new(handle: AppHandle, settings: PathBuf) -> Self {
        let auto = std::fs::read_to_string(&settings)
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .and_then(|v| v["auto"].as_bool())
            .unwrap_or(true);
        Ports {
            handle,
            settings,
            auto: Mutex::new(auto),
            forwards: Mutex::default(),
            tasks: Mutex::default(),
            dismissed: Mutex::default(),
            error: Mutex::default(),
        }
    }

    fn current(&self) -> Option<Arc<Forwards>> {
        self.forwards.lock().unwrap().clone()
    }

    pub async fn snapshot(&self) -> Forwarding {
        let forwards = match self.current() {
            Some(f) => f.list().await.into_iter().map(convert).collect(),
            None => vec![],
        };
        Forwarding {
            available: self.current().is_some(),
            auto: *self.auto.lock().unwrap(),
            forwards,
            error: self.error.lock().unwrap().clone(),
        }
    }

    async fn announce(&self) {
        let _ = self.handle.emit(FORWARDS_CHANNEL, self.snapshot().await);
    }

    /// A new connection: over SSH (`Some`), or on this computer (`None`, nothing to forward).
    pub async fn use_forwards(&self, forwards: Option<Arc<Forwards>>) {
        *self.forwards.lock().unwrap() = forwards;
        self.tasks.lock().unwrap().clear();
        self.dismissed.lock().unwrap().clear();
        *self.error.lock().unwrap() = None;
        self.announce().await;
    }

    /// The SSH connection is (back) up: reopen the forwards it had, and catch up on tasks.
    pub async fn connected(&self, running_tasks: Vec<Value>) {
        let Some(forwards) = self.current() else {
            return;
        };
        let failed = forwards.reapply().await;
        self.note(failed);
        let mut tasks = HashMap::new();
        for t in running_tasks {
            track(&mut tasks, &t);
        }
        *self.tasks.lock().unwrap() = tasks;
        self.sync().await;
    }

    /// A `task` event from the backend.
    pub async fn task_event(&self, event: &Value) {
        {
            let mut tasks = self.tasks.lock().unwrap();
            match event["type"].as_str() {
                Some("changed") => track(&mut tasks, &event["task"]),
                Some("removed") => {
                    tasks.remove(event["id"].as_str().unwrap_or_default());
                }
                _ => return,
            }
        }
        self.sync().await;
    }

    fn note(&self, failed: Vec<(u16, String)>) {
        if let Some((_, why)) = failed.into_iter().last() {
            *self.error.lock().unwrap() = Some(why);
        }
    }

    /// Makes the automatic forwards match the running tasks' ports.
    async fn sync(&self) {
        let Some(forwards) = self.current() else {
            return;
        };
        let wanted: BTreeSet<u16> = if *self.auto.lock().unwrap() {
            let used: BTreeSet<u16> = self
                .tasks
                .lock()
                .unwrap()
                .values()
                .flatten()
                .copied()
                .collect();
            let mut dismissed = self.dismissed.lock().unwrap();
            // A port no task uses any more may be forwarded again when one does.
            dismissed.retain(|p| used.contains(p));
            used.difference(&dismissed).copied().collect()
        } else {
            BTreeSet::new()
        };
        let failed = forwards.set_auto(&wanted).await;
        self.note(failed);
        self.announce().await;
    }

    pub async fn set_auto(&self, auto: bool) -> Result<(), String> {
        *self.auto.lock().unwrap() = auto;
        if let Some(dir) = self.settings.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(
            &self.settings,
            serde_json::json!({ "auto": auto }).to_string(),
        )
        .map_err(|e| e.to_string())?;
        self.sync().await;
        Ok(())
    }

    /// Forwards `port` (if it isn't already) and returns where it is on this computer.
    pub async fn forward(&self, port: u16) -> Result<PortForward, String> {
        let forwards = self
            .current()
            .ok_or("Ports are only forwarded when agents run on another machine.")?;
        self.dismissed.lock().unwrap().remove(&port);
        let f = forwards.add(port, false).await.map_err(|e| {
            *self.error.lock().unwrap() = Some(e.clone());
            e
        })?;
        *self.error.lock().unwrap() = None;
        self.announce().await;
        Ok(convert(f))
    }

    pub async fn stop(&self, port: u16) -> Result<(), String> {
        let Some(forwards) = self.current() else {
            return Ok(());
        };
        if forwards
            .list()
            .await
            .iter()
            .any(|f| f.remote == port && f.auto)
        {
            self.dismissed.lock().unwrap().insert(port);
        }
        forwards.remove(port).await?;
        self.announce().await;
        Ok(())
    }

    /// Opens `http://localhost:<port>` for a port on the agents' machine: forwarded first when
    /// that's another machine.
    pub async fn open(&self, port: u16) -> Result<(), String> {
        let local = match self.current() {
            None => port,
            Some(forwards) => match forwards.list().await.into_iter().find(|f| f.remote == port) {
                Some(f) => f.local,
                None => self.forward(port).await?.local,
            },
        };
        open_url(&format!("http://localhost:{local}"))
    }
}

/// Records a task's listening ports while it runs, if the user is meant to open them: a task
/// only the agent uses (`interactive: false`, e.g. a server for tests) isn't forwarded.
fn track(tasks: &mut HashMap<String, BTreeSet<u16>>, task: &Value) {
    let Some(id) = task["id"].as_str() else {
        return;
    };
    if task["status"] == "running" && task["interactive"] != false {
        let ports = task["ports"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| p.as_u64().and_then(|p| u16::try_from(p).ok()))
            .collect();
        tasks.insert(id.to_string(), ports);
    } else {
        tasks.remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_interactive_running_tasks_are_forwarded() {
        let mut tasks = HashMap::new();
        track(&mut tasks, &json!({ "id": "dev", "status": "running", "ports": [5173] }));
        track(&mut tasks, &json!({ "id": "tests", "status": "running", "interactive": false, "ports": [5174] }));
        track(&mut tasks, &json!({ "id": "old", "status": "running", "interactive": true, "ports": [3000] }));
        assert_eq!(tasks.keys().collect::<BTreeSet<_>>(), BTreeSet::from([&"dev".to_string(), &"old".to_string()]));

        track(&mut tasks, &json!({ "id": "dev", "status": "stopped", "ports": [] }));
        assert!(!tasks.contains_key("dev"));
    }
}
