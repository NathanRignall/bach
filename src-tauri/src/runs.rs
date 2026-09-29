use crate::adapters::{AgentEvent, AgentKind};
use serde::Serialize;
use std::{collections::HashMap, process::Stdio, sync::Arc};
use tauri::{AppHandle, Emitter};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    sync::{oneshot, Mutex},
};

#[derive(Clone, Serialize)]
struct RunEvent {
    run_id: String,
    #[serde(flatten)]
    event: AgentEvent,
}

/// Tracks live agent processes so they can be cancelled.
#[derive(Default)]
pub struct Runs(Arc<Mutex<HashMap<String, oneshot::Sender<()>>>>);

impl Runs {
    pub async fn start(
        &self,
        app: AppHandle,
        agent: AgentKind,
        prompt: String,
        cwd: Option<String>,
        session_id: Option<String>,
    ) -> Result<String, String> {
        let mut cmd = Command::new(agent.binary());
        cmd.args(agent.args(&prompt, session_id.as_deref()))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(dir) = cwd.filter(|d| !d.is_empty()) {
            cmd.current_dir(dir);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("failed to launch `{}`: {e}", agent.binary()))?;

        let run_id = uuid::Uuid::new_v4().to_string();
        let (cancel_tx, mut cancel_rx) = oneshot::channel();
        self.0.lock().await.insert(run_id.clone(), cancel_tx);

        let stdout = child.stdout.take().expect("piped");
        let stderr = child.stderr.take().expect("piped");
        let runs = self.0.clone();
        let id = run_id.clone();

        let emit = {
            let (app, id) = (app.clone(), run_id.clone());
            move |event: AgentEvent| {
                let _ = app.emit("agent-event", RunEvent { run_id: id.clone(), event });
            }
        };

        // Collect stderr so a failed launch can be explained.
        let stderr_task = tokio::spawn(async move {
            let mut buf = String::new();
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                buf.push_str(&l);
                buf.push('\n');
            }
            buf
        });

        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            let mut done = false;
            loop {
                tokio::select! {
                    _ = &mut cancel_rx => {
                        let _ = child.kill().await;
                        emit(AgentEvent::Error { message: "cancelled".into() });
                        runs.lock().await.remove(&id);
                        return;
                    }
                    line = lines.next_line() => match line {
                        Ok(Some(line)) if !line.trim().is_empty() => {
                            for ev in agent.parse_line(&line) {
                                done |= matches!(ev, AgentEvent::Done { .. });
                                emit(ev);
                            }
                        }
                        Ok(Some(_)) => {}
                        _ => break,
                    }
                }
            }
            let status = child.wait().await;
            let stderr = stderr_task.await.unwrap_or_default();
            if !done {
                let failed = status.map(|s| !s.success()).unwrap_or(true);
                if failed {
                    emit(AgentEvent::Error { message: stderr.trim().to_string() });
                }
                emit(AgentEvent::Done { cost_usd: None, is_error: failed });
            }
            runs.lock().await.remove(&id);
        });

        Ok(run_id)
    }

    pub async fn cancel(&self, run_id: &str) {
        if let Some(tx) = self.0.lock().await.remove(run_id) {
            let _ = tx.send(());
        }
    }
}
