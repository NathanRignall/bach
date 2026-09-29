use crate::adapters::{AgentEvent, AgentKind};
use serde::Serialize;
use std::{collections::HashMap, process::Stdio, sync::Arc};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    sync::{oneshot, Mutex},
};

/// Delivers events to whichever transport (Tauri window, WebSocket) is listening.
pub type Emit = Arc<dyn Fn(RunEvent) + Send + Sync>;

#[derive(Clone, Serialize)]
pub struct RunEvent {
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
        emit: Emit,
        agent: AgentKind,
        prompt: String,
        cwd: Option<String>,
        session_id: Option<String>,
    ) -> Result<String, String> {
        // No implicit "wherever the backend happens to be": a new session must name its folder.
        // (A resumed session may predate this rule and have none saved.)
        let cwd = cwd.map(|d| d.trim().to_string()).filter(|d| !d.is_empty());
        if cwd.is_none() && session_id.is_none() {
            return Err("Choose a project folder first.".into());
        }
        let cwd = cwd.map(|d| crate::fs::expand_home(&d));
        if let Some(dir) = &cwd {
            if !dir.is_dir() {
                return Err(format!(
                    "{} is not a folder on the machine running the agents.",
                    dir.display()
                ));
            }
        }

        let mut cmd = Command::new(agent.binary());
        cmd.args(agent.args(&prompt, session_id.as_deref()))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(dir) = cwd {
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
            let id = run_id.clone();
            move |event: AgentEvent| {
                emit(RunEvent {
                    run_id: id.clone(),
                    event,
                })
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
                        emit(AgentEvent::Cancelled);
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
                    emit(AgentEvent::Error {
                        message: stderr.trim().to_string(),
                    });
                }
                emit(AgentEvent::Done {
                    cost_usd: None,
                    is_error: failed,
                });
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

#[cfg(test)]
mod tests {
    use super::*;

    fn noop() -> Emit {
        Arc::new(|_| {})
    }

    #[tokio::test]
    async fn new_runs_require_an_existing_folder() {
        let runs = Runs::default();
        for cwd in [None, Some("  ".to_string())] {
            let err = runs
                .start(noop(), AgentKind::Claude, "hi".into(), cwd, None)
                .await
                .unwrap_err();
            assert!(err.contains("Choose a project folder"), "{err}");
        }
        let err = runs
            .start(
                noop(),
                AgentKind::Claude,
                "hi".into(),
                Some("/definitely/not/here".into()),
                None,
            )
            .await
            .unwrap_err();
        assert!(err.contains("not a folder"), "{err}");
    }
}
