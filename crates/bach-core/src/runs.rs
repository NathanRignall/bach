use crate::{
    opencode_server,
    adapters::{codex_answer, opencode, permission_modes, AgentCli, AgentEvent, AgentKind, Conversation, Turn},
};
pub use bach_protocol::Decision;
use bach_protocol::ApiError;
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde::Serialize;
use satie::{Satie, Scope};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    os::unix::{fs::DirBuilderExt, process::ExitStatusExt},
    path::PathBuf,
    process::Stdio,
    sync::Arc,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{ChildStdin, Command},
    sync::{broadcast, oneshot, Mutex},
};

/// Where a run's events go (the session it belongs to).
pub type Emit = Arc<dyn Fn(RunEvent) + Send + Sync>;

/// One event from an agent run.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunEvent {
    pub run_id: String,
    #[serde(flatten)]
    pub event: AgentEvent,
}

/// Everything needed to start one turn of an agent.
pub struct RunRequest {
    pub agent: AgentKind,
    pub prompt: String,
    /// Images sent with the prompt, as `data:` URLs.
    pub images: Vec<String>,
    pub cwd: Option<String>,
    /// The agent's own session id, to continue an earlier conversation.
    pub session_id: Option<String>,
    pub model: Option<String>,
    /// Claude Code's `--permission-mode` (`acceptEdits`, `auto`, …); none for its default.
    pub permission_mode: Option<String>,
    /// Permission rules approved earlier in this session.
    pub allowed_tools: Vec<String>,
    /// The session this run belongs to, so deleting the session stops it.
    pub session_key: Option<String>,
    /// The id to give the run (a new one if not set).
    pub run_id: Option<String>,
}

struct PendingApproval {
    /// The rules an "allow for this session / always" answer grants.
    rules: Vec<String>,
    tool_name: String,
    input: Value,
    suggestions: Value,
}

/// A running agent process.
struct Live {
    agent: AgentKind,
    session_key: Option<String>,
    cancel: Option<oneshot::Sender<()>>,
    /// Open for agents driven over stdin, so approvals can be answered; closed when the run ends.
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    /// Requests the agent is waiting on. What was asked is kept here, not taken from the client.
    pending: Pending,
    /// For opencode, answered over HTTP: its server, and the project folder it runs in.
    opencode: Option<(opencode_server::Server, String)>,
}

type Pending = Arc<std::sync::Mutex<HashMap<String, PendingApproval>>>;

/// Notes what `ev` means for the run's open approvals (and trims an approval's folders to those
/// outside the project). False when it isn't worth passing on: cancelling an approval that was
/// already answered.
fn track(ev: &mut AgentEvent, pending: &Pending, project: Option<&std::path::Path>) -> bool {
    // The agent offers to "add" the file's folder even when it is the project itself; only
    // folders truly outside are worth showing.
    if let (AgentEvent::Approval { directories, .. }, Some(project)) = (&mut *ev, project) {
        directories.retain(|d| {
            let d = std::path::PathBuf::from(d);
            !d.canonicalize().unwrap_or(d).starts_with(project)
        });
    }
    match ev {
        AgentEvent::Approval { request_id, tool_name, input, suggestions, rules, .. } => {
            pending.lock().unwrap().insert(
                request_id.clone(),
                PendingApproval {
                    rules: rules.clone(),
                    tool_name: tool_name.clone(),
                    input: input.clone(),
                    suggestions: suggestions.clone(),
                },
            );
            true
        }
        AgentEvent::ApprovalCancelled { request_id } => {
            pending.lock().unwrap().remove(request_id.as_str()).is_some()
        }
        _ => true,
    }
}

/// Tracks live agent processes so they can be cancelled and their approvals answered.
#[derive(Clone, Default)]
pub struct Runs {
    live: Arc<Mutex<HashMap<String, Live>>>,
    /// Satie (background tasks), offered to Claude Code runs as an MCP server.
    satie: Option<Satie>,
}

/// A run's images written out for an agent that takes them as files, in a folder only this user
/// can read. The folder is removed when this is dropped (the run is over).
struct ImageFiles {
    dir: Option<PathBuf>,
    paths: Vec<String>,
}

impl ImageFiles {
    const NONE: Self = Self {
        dir: None,
        paths: vec![],
    };

    /// Writes `images` (`data:image/…;base64,` URLs) to `<temp>/bach-images/<run_id>/`.
    fn write(run_id: &str, images: &[String]) -> Result<Self, String> {
        if images.is_empty() {
            return Ok(Self::NONE);
        }
        let dir = std::env::temp_dir().join("bach-images").join(run_id);
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .map_err(|e| format!("couldn't save the images: {e}"))?;
        // From here on, dropping `files` cleans up whatever was written.
        let mut files = Self {
            dir: Some(dir.clone()),
            paths: vec![],
        };
        for (i, url) in images.iter().enumerate() {
            let (ext, bytes) = decode_image(url).ok_or("One of the images couldn't be read.")?;
            let path = dir.join(format!("image-{}.{ext}", i + 1));
            std::fs::write(&path, bytes).map_err(|e| format!("couldn't save the images: {e}"))?;
            files.paths.push(path.to_string_lossy().into_owned());
        }
        Ok(files)
    }
}

impl Drop for ImageFiles {
    fn drop(&mut self) {
        if let Some(dir) = &self.dir {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// A `data:image/png;base64,…` URL as a file extension and the image's bytes.
fn decode_image(url: &str) -> Option<(&'static str, Vec<u8>)> {
    let (media_type, data) = url.strip_prefix("data:")?.split_once(";base64,")?;
    let ext = match media_type {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => return None,
    };
    Some((ext, B64.decode(data).ok()?))
}

/// The permission modes Claude Code accepts (besides its default).
pub const PERMISSION_MODES: &[&str] = &[
    "acceptEdits",
    "auto",
    "plan",
    "bypassPermissions",
    "dontAsk",
];

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

/// Writes `lines` to the agent, if it is still listening.
async fn send(stdin: &Mutex<Option<ChildStdin>>, lines: &[String]) {
    if lines.is_empty() {
        return;
    }
    if let Some(w) = stdin.lock().await.as_mut() {
        for line in lines {
            let _ = w.write_all(format!("{line}\n").as_bytes()).await;
        }
        let _ = w.flush().await;
    }
}

/// One opencode turn: what to ask, and how.
struct OpencodeTurn {
    dir: String,
    session_id: Option<String>,
    prompt: String,
    model: Option<String>,
    mode: Option<String>,
    allowed: Vec<String>,
}

impl Runs {
    /// Starts a turn of an opencode session in the shared server: the session (created, or its
    /// permissions brought up to date), then the prompt, then its events until it goes idle.
    async fn start_opencode(
        &self,
        emit: Emit,
        run_id: String,
        session_key: Option<String>,
        turn: OpencodeTurn,
        image_files: ImageFiles,
        project: PathBuf,
    ) -> Result<String, String> {
        let server = opencode_server::server().await?;
        // Listening before prompting, so nothing is missed.
        let mut events = server.subscribe();
        let dir = turn.dir;
        let mode = turn.mode.as_deref();
        let permission = json!({ "permission": opencode::permission_rules(mode, &turn.allowed) });
        let emit = {
            let id = run_id.clone();
            move |event: AgentEvent| emit(RunEvent { run_id: id.clone(), event })
        };
        let session = match turn.session_id {
            Some(id) => {
                server.patch(&format!("/session/{id}"), &dir, &permission).await?;
                id
            }
            None => {
                let s = server.post("/session", &dir, &permission).await?;
                let id = s["id"].as_str().ok_or("opencode didn't make a session.")?.to_string();
                emit(AgentEvent::Session { id: id.clone(), model: None });
                id
            }
        };

        let pending: Pending = Arc::default();
        let (cancel_tx, mut cancel_rx) = oneshot::channel();
        self.live.lock().await.insert(
            run_id.clone(),
            Live {
                agent: AgentKind::Opencode,
                session_key,
                cancel: Some(cancel_tx),
                stdin: Arc::default(),
                pending: pending.clone(),
                opencode: Some((server.clone(), dir.clone())),
            },
        );

        let mut prompt = json!({
            "parts": opencode::prompt_parts(&turn.prompt, &image_files.paths),
            "agent": opencode::agent(mode),
        });
        if let Some(model) = turn.model.as_deref().and_then(opencode::model) {
            prompt["model"] = model;
        }
        if let Err(e) = server
            .post(&format!("/session/{session}/prompt_async"), &dir, &prompt)
            .await
        {
            self.live.lock().await.remove(&run_id);
            return Err(e);
        }

        let runs = self.live.clone();
        let id = run_id.clone();
        tokio::spawn(async move {
            let _image_files = image_files; // removed when the run ends
            let mut stream = opencode::Stream::new(&session, &turn.allowed);
            let path = format!("/session/{session}/abort");
            loop {
                tokio::select! {
                    _ = &mut cancel_rx => {
                        let _ = server.post(&path, &dir, &json!({})).await;
                        emit(AgentEvent::Cancelled);
                        break;
                    }
                    event = events.recv() => {
                        let event = match event {
                            Ok(e) => e,
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                eprintln!("run {id}: missed {n} opencode events");
                                continue;
                            }
                            Err(broadcast::error::RecvError::Closed) => json!({ "type": opencode_server::LOST }),
                        };
                        if event["type"] == opencode_server::LOST {
                            emit(AgentEvent::Error { message: "opencode's server stopped.".into() });
                            emit(AgentEvent::Done { cost_usd: None, is_error: true });
                            break;
                        }
                        let (evs, replies) = stream.on_event(&event);
                        for r in replies {
                            let _ = server.post(&r.path, &dir, &r.body).await;
                        }
                        let mut done = false;
                        for mut ev in evs {
                            if !track(&mut ev, &pending, Some(&project)) {
                                continue;
                            }
                            done |= matches!(ev, AgentEvent::Done { .. });
                            emit(ev);
                        }
                        if done {
                            break;
                        }
                    }
                }
            }
            runs.lock().await.remove(&id);
        });
        Ok(run_id)
    }

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
            images,
            cwd,
            session_id,
            model,
            permission_mode,
            allowed_tools,
            session_key,
            run_id,
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
        let permission_mode = permission_mode
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty() && m != "default");
        if let Some(m) = &permission_mode {
            if !permission_modes(agent).contains(&m.as_str()) {
                return Err(format!("`{m}` isn't a permission mode."));
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

        let run_id = run_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        // Agents that take images by path get files that last as long as the run.
        let image_files = if agent.images_as_files() {
            ImageFiles::write(&run_id, &images)?
        } else {
            ImageFiles::NONE
        };

        // opencode runs in its shared server rather than a process of its own.
        if agent == AgentKind::Opencode {
            let dir = project
                .clone()
                .ok_or("An opencode session needs its project folder.")?;
            let turn = OpencodeTurn {
                dir: dir.to_string_lossy().into_owned(),
                session_id,
                prompt,
                model,
                mode: permission_mode,
                allowed: allowed_tools,
            };
            return self
                .start_opencode(emit, run_id, session_key, turn, image_files, dir)
                .await;
        }
        let (mut conversation, opening) = Conversation::new(
            agent,
            &Turn {
                prompt: &prompt,
                images: &images,
                image_files: &image_files.paths,
                session_id: session_id.as_deref(),
                cwd: cwd.as_deref().and_then(|d| d.to_str()),
                allowed_tools: &allowed_tools,
                model: model.as_deref(),
                permission_mode: permission_mode.as_deref(),
            },
        );

        // Claude Code and Codex get Satie as an MCP server, with a token scoped to this project.
        // The grant is revoked when the run ends (or if launching fails below).
        let grant = match (&self.satie, agent) {
            (Some(satie), AgentKind::Claude | AgentKind::Codex) => Some(satie.grant(Scope {
                project: project.as_ref().map(|p| p.to_string_lossy().into_owned()),
                owner: Some(run_id.clone()),
            })),
            _ => None,
        };

        let mut cmd = Command::new(agent.binary());
        cmd.args(agent.args(
            session_id.as_deref(),
            model.as_deref(),
            permission_mode.as_deref(),
            &allowed_tools,
            grant.as_ref(),
        ))
        .envs(agent.env(grant.as_ref()))
        .stdin(if conversation.uses_stdin() {
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
        let stdin = Arc::new(Mutex::new(child.stdin.take()));
        send(&stdin, &opening).await;
        let pending = Arc::new(std::sync::Mutex::new(HashMap::new()));

        let (cancel_tx, mut cancel_rx) = oneshot::channel();
        self.live.lock().await.insert(
            run_id.clone(),
            Live {
                agent,
                opencode: None,
                session_key,
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
            let _grant = grant; // revoked when the run ends
            let _image_files = image_files; // removed when the run ends
            let mut lines = BufReader::new(stdout).lines();
            let mut done = false;
            // Sub-agents still working after the main agent's turn ended. Claude reports the
            // turn's result straight away, then takes another turn once they finish.
            let mut background = HashSet::new();
            loop {
                tokio::select! {
                    _ = &mut cancel_rx => {
                        // Agents that can stop a turn themselves get a moment to wrap it up.
                        if let Some(line) = conversation.interrupt() {
                            send(&stdin, &[line]).await;
                            stdin.lock().await.take();
                            let _ = tokio::time::timeout(
                                std::time::Duration::from_secs(3),
                                child.wait(),
                            )
                            .await;
                        }
                        let _ = child.kill().await;
                        emit(AgentEvent::Cancelled);
                        runs.lock().await.remove(&id);
                        return;
                    }
                    line = lines.next_line() => match line {
                        Ok(Some(line)) if !line.trim().is_empty() => {
                            let (events, replies) = conversation.on_line(&line);
                            send(&stdin, &replies).await;
                            for mut ev in events {
                                if !track(&mut ev, &pending, project.as_deref()) {
                                    continue;
                                }
                                match &ev {
                                    AgentEvent::Task { id, status: Some(status), background: bg, agent_type, .. } => {
                                        if status != "running" {
                                            background.remove(id);
                                        } else if *bg == Some(true) && agent_type.is_some() {
                                            background.insert(id.clone());
                                        }
                                    }
                                    // Not the end of the run yet: keep stdin open for the
                                    // sub-agents' approvals and wait for the next result.
                                    AgentEvent::Done { .. } if !background.is_empty() => continue,
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
                let failed = status.as_ref().map(|s| !s.success()).unwrap_or(true);
                if failed {
                    // Say how it ended: a run that just vanishes is impossible to diagnose.
                    let how = match &status {
                        Ok(s) => match (s.signal(), s.code()) {
                            (Some(sig), _) => format!("was killed by signal {sig}"),
                            (_, Some(code)) => format!("exited with code {code}"),
                            _ => "ended".into(),
                        },
                        Err(e) => format!("could not be waited on ({e})"),
                    };
                    let tail: Vec<&str> = stderr
                        .trim()
                        .lines()
                        .rev()
                        .take(8)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect();
                    let message = if tail.is_empty() {
                        format!("The agent process {how} before finishing.")
                    } else {
                        format!(
                            "The agent process {how} before finishing:\n{}",
                            tail.join("\n")
                        )
                    };
                    eprintln!("run {id}: {message}");
                    emit(AgentEvent::Error { message });
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

    pub async fn is_live(&self, run_id: &str) -> bool {
        self.live.lock().await.contains_key(run_id)
    }

    pub async fn cancel(&self, run_id: &str) {
        let live = self.live.lock().await.remove(run_id);
        if let Some(tx) = live.and_then(|l| l.cancel) {
            let _ = tx.send(());
        }
    }

    /// Stops every live run that belongs to a UI session. Returns how many there were.
    pub async fn cancel_session(&self, session_key: &str) -> usize {
        let ids: Vec<String> = self
            .live
            .lock()
            .await
            .iter()
            .filter(|(_, l)| l.session_key.as_deref() == Some(session_key))
            .map(|(id, _)| id.clone())
            .collect();
        for id in &ids {
            self.cancel(id).await;
        }
        ids.len()
    }

    /// Answers an approval request the agent is waiting on. Returns the rules the request
    /// offered (what an "allow for this session" answer grants).
    pub async fn respond_approval(
        &self,
        run_id: &str,
        request_id: &str,
        decision: Decision,
        message: Option<String>,
        answers: Option<HashMap<String, String>>,
    ) -> Result<Vec<String>, ApiError> {
        let finished = || ApiError::not_found("That run has already finished.");
        let (agent, stdin, pending, opencode) = {
            let runs = self.live.lock().await;
            let live = runs.get(run_id).ok_or_else(finished)?;
            (live.agent, live.stdin.clone(), live.pending.clone(), live.opencode.clone())
        };
        // Build (and so validate) the reply before taking the request off the list: a rejected
        // answer must leave the agent's question open to be answered properly.
        let response = {
            let mut list = pending.lock().unwrap();
            let approval = list
                .get(request_id)
                .ok_or_else(|| ApiError::not_found("That approval is no longer pending."))?;
            let line = match agent {
                AgentKind::Opencode => {
                    let reply = opencode::answer(&approval.suggestions, decision, message, answers.as_ref())
                        .map_err(ApiError::invalid)?;
                    serde_json::to_string(&json!({ "path": reply.path, "body": reply.body }))
                        .expect("json serializes")
                }
                AgentKind::Codex => codex_answer(&approval.suggestions, decision, answers.as_ref())
                    .map_err(ApiError::invalid)?,
                _ => {
                    let response = build_response(approval, decision, message, answers)
                        .map_err(ApiError::invalid)?;
                    json!({
                        "type": "control_response",
                        "response": { "subtype": "success", "request_id": request_id, "response": response },
                    })
                    .to_string()
                }
            };
            let rules = list.remove(request_id).map(|a| a.rules).unwrap_or_default();
            (line, rules)
        };
        let (line, rules) = response;

        if let Some((server, dir)) = opencode {
            let reply: Value = serde_json::from_str(&line).expect("built above");
            server
                .post(reply["path"].as_str().unwrap_or_default(), &dir, &reply["body"])
                .await
                .map_err(|e| ApiError::failed(format!("Couldn't answer the agent: {e}")))?;
            return Ok(rules);
        }

        let mut guard = stdin.lock().await;
        let w = guard.as_mut().ok_or_else(finished)?;
        let failed = |e: std::io::Error| ApiError::failed(format!("Couldn't answer the agent: {e}"));
        w.write_all(format!("{line}\n").as_bytes())
            .await
            .map_err(failed)?;
        w.flush().await.map_err(failed)?;
        Ok(rules)
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
            images: vec![],
            cwd: cwd.map(String::from),
            session_id: None,
            model: model.map(String::from),
            permission_mode: None,
            allowed_tools: rules.iter().map(|r| r.to_string()).collect(),
            session_key: None,
            run_id: None,
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
    async fn only_known_permission_modes_reach_the_cli() {
        let runs = Runs::default();
        for m in ["--dangerously-skip-permissions", "yolo", "Auto"] {
            let mut r = req(Some("/tmp"), None, &[]);
            r.permission_mode = Some(m.into());
            let err = runs.start(noop(), r).await.unwrap_err();
            assert!(err.contains("isn't a permission mode"), "{m}: {err}");
        }
        let a = AgentKind::Claude.args(None, None, Some("auto"), &[], None);
        let at = a.iter().position(|x| x == "--permission-mode").unwrap();
        assert_eq!(a[at + 1], "auto");
        assert!(!AgentKind::Claude
            .args(None, None, None, &[], None)
            .contains(&"--permission-mode".to_string()));
    }

    #[test]
    fn image_files_last_as_long_as_the_run() {
        let run_id = uuid::Uuid::new_v4().to_string();
        let files = ImageFiles::write(&run_id, &["data:image/png;base64,iVBORw0K".into()]).unwrap();
        let path = PathBuf::from(&files.paths[0]);
        assert!(path.ends_with(format!("bach-images/{run_id}/image-1.png")));
        assert_eq!(std::fs::read(&path).unwrap(), b"\x89PNG\r\n");
        drop(files);
        assert!(!path.parent().unwrap().exists());

        // Nothing is left behind when one can't be read.
        let bad = ImageFiles::write(
            &run_id,
            &[
                "data:image/png;base64,AAAA".into(),
                "data:text/plain;base64,AAAA".into(),
            ],
        );
        assert!(bad.is_err());
        assert!(!std::env::temp_dir()
            .join("bach-images")
            .join(&run_id)
            .exists());
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
