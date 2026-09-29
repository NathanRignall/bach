use crate::{
    adapters::{AgentEvent, AgentKind},
    satie::{RunInfo, Satie},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::HashMap, process::Stdio, sync::Arc};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{ChildStdin, Command},
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

/// Everything needed to start one turn of an agent.
pub struct RunRequest {
    pub agent: AgentKind,
    pub prompt: String,
    pub cwd: Option<String>,
    /// The agent's own session id, to continue an earlier conversation.
    pub session_id: Option<String>,
    pub model: Option<String>,
    /// Permission rules approved earlier in this session.
    pub allowed_tools: Vec<String>,
}

/// How the user answered an approval request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    /// Just this once.
    Allow,
    /// Also for the rest of this run; the UI remembers the rule for later runs.
    AllowSession,
    /// Save the agent's suggested rule to the project's settings.
    AllowAlways,
    Deny,
}

struct PendingApproval {
    tool_name: String,
    input: Value,
    suggestions: Value,
}

/// A running agent process.
struct Live {
    cancel: Option<oneshot::Sender<()>>,
    /// Open for agents driven over stdin, so approvals can be answered; closed when the run ends.
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    /// Requests the agent is waiting on. What was asked is kept here, not taken from the client.
    pending: Arc<std::sync::Mutex<HashMap<String, PendingApproval>>>,
}

/// Tracks live agent processes so they can be cancelled and their approvals answered.
#[derive(Default)]
pub struct Runs {
    live: Arc<Mutex<HashMap<String, Live>>>,
    /// Bach's own MCP server (background tasks), offered to Claude Code runs.
    satie: Option<Satie>,
}

fn valid_rule(r: &str) -> bool {
    !r.is_empty() && !r.starts_with('-') && r.len() < 500
}

/// The reply to a permission request. Errors leave the request unanswered.
fn build_response(
    approval: &PendingApproval,
    decision: Decision,
    message: Option<String>,
    answers: Option<HashMap<String, String>>,
) -> Result<Value, String> {
    if decision == Decision::Deny {
        return Ok(json!({
            "behavior": "deny",
            "message": message.unwrap_or_else(|| "The user denied this request.".into()),
        }));
    }

    let mut input = approval.input.clone();
    // A question for the user (AskUserQuestion) is answered by allowing it with the answers
    // filled in, keyed by the question text. Only questions actually asked can be answered,
    // and the questions themselves can't be altered.
    if approval.tool_name == "AskUserQuestion" {
        let answers = answers.ok_or("Answer the question first.")?;
        let asked: Vec<&str> = input["questions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|q| q["question"].as_str())
            .collect();
        if let Some(unknown) = answers.keys().find(|k| !asked.contains(&k.as_str())) {
            return Err(format!("`{unknown}` isn't one of the questions asked."));
        }
        input["answers"] = json!(answers);
    } else if answers.is_some() {
        return Err("That request doesn't take answers.".into());
    }

    let mut r = json!({ "behavior": "allow", "updatedInput": input });
    // Only ever the rule itself. The agent also offers to widen directory access or switch
    // permission mode; "allow this command" must not quietly grant those.
    let rules: Vec<Value> = approval
        .suggestions
        .as_array()
        .into_iter()
        .flatten()
        .filter(|s| s["type"] == "addRules" && s["behavior"] == "allow")
        .cloned()
        .collect();
    let updates = match decision {
        Decision::AllowSession => Some(
            rules
                .into_iter()
                .map(|mut s| {
                    s["destination"] = json!("session");
                    s
                })
                .collect::<Vec<_>>(),
        ),
        Decision::AllowAlways => Some(rules),
        _ => None,
    };
    if let Some(u) = updates.filter(|u| !u.is_empty()) {
        r["updatedPermissions"] = json!(u);
    }
    Ok(r)
}

impl Runs {
    pub fn with_satie(satie: Option<Satie>) -> Self {
        Self {
            satie,
            ..Default::default()
        }
    }

    pub async fn start(&self, emit: Emit, req: RunRequest) -> Result<String, String> {
        let RunRequest {
            agent,
            prompt,
            cwd,
            session_id,
            model,
            allowed_tools,
        } = req;

        // Passed straight to the CLI, so keep to plausible model names (aliases like `opus`,
        // ids like `claude-opus-5-5`, `sonnet[1m]`) and never anything that looks like a flag.
        let model = model
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty() && m != "default");
        if let Some(m) = &model {
            let ok = m.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
                && m.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-._[]/:".contains(c));
            if !ok {
                return Err(format!("`{m}` isn't a valid model name."));
            }
        }
        if let Some(bad) = allowed_tools.iter().find(|r| !valid_rule(r)) {
            return Err(format!("`{bad}` isn't a valid permission rule."));
        }

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

        // Where the agent runs, for telling "inside the project" from "reaches outside it".
        let project = cwd
            .as_ref()
            .map(|d| d.canonicalize().unwrap_or_else(|_| d.clone()));

        let stdin_prompt = agent.stdin_prompt(&prompt);

        // Claude Code gets Bach's MCP server, with a token that lets us tell which run calls it.
        // The guard revokes the token when the run ends (or if launching fails below).
        let run_id = uuid::Uuid::new_v4().to_string();
        let (satie_args, token) = match (&self.satie, agent) {
            (Some(satie), AgentKind::Claude) => {
                let (args, guard) = satie.register_run(RunInfo {
                    run_id: run_id.clone(),
                    cwd: project.as_ref().map(|p| p.to_string_lossy().into_owned()),
                });
                (Some(args), Some(guard))
            }
            _ => (None, None),
        };

        let mut cmd = Command::new(agent.binary());
        cmd.args(agent.args(
            &prompt,
            session_id.as_deref(),
            model.as_deref(),
            &allowed_tools,
            satie_args.as_ref(),
        ))
        .stdin(if stdin_prompt.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("failed to launch `{}`: {e}", agent.binary()))?;

        // Deliver the prompt, leaving stdin open for the agent's requests until the run ends.
        let mut stdin = child.stdin.take();
        if let (Some(line), Some(w)) = (&stdin_prompt, stdin.as_mut()) {
            let _ = w.write_all(format!("{line}\n").as_bytes()).await;
            let _ = w.flush().await;
        }
        let stdin = Arc::new(Mutex::new(stdin));
        let pending = Arc::new(std::sync::Mutex::new(HashMap::new()));

        let (cancel_tx, mut cancel_rx) = oneshot::channel();
        self.live.lock().await.insert(
            run_id.clone(),
            Live {
                cancel: Some(cancel_tx),
                stdin: stdin.clone(),
                pending: pending.clone(),
            },
        );

        let stdout = child.stdout.take().expect("piped");
        let stderr = child.stderr.take().expect("piped");
        let runs = self.live.clone();
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
            let _token = token; // revoked when the run ends
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
                            for mut ev in agent.parse_line(&line) {
                                // The agent offers to "add" the file's folder even when it is the
                                // project itself; only folders truly outside are worth showing.
                                if let (AgentEvent::Approval { directories, .. }, Some(project)) = (&mut ev, &project) {
                                    directories.retain(|d| {
                                        let d = std::path::PathBuf::from(d);
                                        !d.canonicalize().unwrap_or(d).starts_with(project)
                                    });
                                }
                                match &ev {
                                    AgentEvent::Approval { request_id, tool_name, input, suggestions, .. } => {
                                        pending.lock().unwrap().insert(
                                            request_id.clone(),
                                            PendingApproval {
                                                tool_name: tool_name.clone(),
                                                input: input.clone(),
                                                suggestions: suggestions.clone(),
                                            },
                                        );
                                    }
                                    AgentEvent::ApprovalCancelled { request_id } => {
                                        pending.lock().unwrap().remove(request_id);
                                    }
                                    AgentEvent::Done { .. } => {
                                        done = true;
                                        // The turn is over; closing stdin lets the process exit.
                                        stdin.lock().await.take();
                                    }
                                    _ => {}
                                }
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
        let live = self.live.lock().await.remove(run_id);
        if let Some(tx) = live.and_then(|l| l.cancel) {
            let _ = tx.send(());
        }
    }

    /// Answers an approval request the agent is waiting on.
    pub async fn respond_approval(
        &self,
        run_id: &str,
        request_id: &str,
        decision: Decision,
        message: Option<String>,
        answers: Option<HashMap<String, String>>,
    ) -> Result<(), String> {
        let (stdin, pending) = {
            let runs = self.live.lock().await;
            let live = runs.get(run_id).ok_or("That run has already finished.")?;
            (live.stdin.clone(), live.pending.clone())
        };
        // Build (and so validate) the reply before taking the request off the list: a rejected
        // answer must leave the agent's question open to be answered properly.
        let response = {
            let mut list = pending.lock().unwrap();
            let approval = list
                .get(request_id)
                .ok_or("That approval is no longer pending.")?;
            let response = build_response(approval, decision, message, answers)?;
            list.remove(request_id);
            response
        };
        let line = json!({
            "type": "control_response",
            "response": { "subtype": "success", "request_id": request_id, "response": response },
        });

        let mut guard = stdin.lock().await;
        let w = guard.as_mut().ok_or("That run has already finished.")?;
        w.write_all(format!("{line}\n").as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        w.flush().await.map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noop() -> Emit {
        Arc::new(|_| {})
    }

    fn req(cwd: Option<&str>, model: Option<&str>, rules: &[&str]) -> RunRequest {
        RunRequest {
            agent: AgentKind::Claude,
            prompt: "hi".into(),
            cwd: cwd.map(String::from),
            session_id: None,
            model: model.map(String::from),
            allowed_tools: rules.iter().map(|r| r.to_string()).collect(),
        }
    }

    #[tokio::test]
    async fn new_runs_require_an_existing_folder() {
        let runs = Runs::default();
        for cwd in [None, Some("  ")] {
            let err = runs.start(noop(), req(cwd, None, &[])).await.unwrap_err();
            assert!(err.contains("Choose a project folder"), "{err}");
        }
        let err = runs
            .start(noop(), req(Some("/definitely/not/here"), None, &[]))
            .await
            .unwrap_err();
        assert!(err.contains("not a folder"), "{err}");
    }

    #[tokio::test]
    async fn rejects_flag_like_model_names_and_rules() {
        let runs = Runs::default();
        for m in ["--dangerously-skip-permissions", "-x", "opus; rm", "a b"] {
            let err = runs
                .start(noop(), req(Some("/tmp"), Some(m), &[]))
                .await
                .unwrap_err();
            assert!(err.contains("valid model"), "{m}: {err}");
        }
        for r in ["--dangerously-skip-permissions", "", "-x"] {
            let err = runs
                .start(noop(), req(Some("/tmp"), None, &[r]))
                .await
                .unwrap_err();
            assert!(err.contains("valid permission rule"), "{r:?}: {err}");
        }
    }
}
