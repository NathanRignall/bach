//! Bach's own tools on the agents' MCP server, served next to bach-tasks' `task_*` ones (see
//! [`bach_tasks::Tools`]). A call's grant is owned by the run that makes it, which leads back to the
//! calling session.
use crate::{api::Launcher, sessions::Sessions};
use bach_protocol::{commands::StartSessionArgs, AgentKind, Session};
use bach_tasks::Scope;
use serde_json::{json, Value};
use std::{future::Future, pin::Pin};

pub(crate) struct SessionTools {
    pub launcher: Launcher,
    pub sessions: Sessions,
}

impl bach_tasks::Tools for SessionTools {
    fn list(&self) -> Vec<Value> {
        vec![json!({
            "name": "start_session",
            "description": "Start a new Bach session in this project: another agent that works on `prompt` on its own, \
shown to the user in their sidebar next to this session. Use it for a separate task you notice (an unrelated bug, a \
follow-up, a side investigation) so it doesn't derail the current one. The new agent doesn't see this conversation, so \
make the prompt self-contained. You won't hear back from it. Returns the new session's id, folder and branch.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "prompt": { "type": "string", "description": "The new session's first message: the whole task, with the context it needs" },
                    "agent": { "type": "string", "enum": ["claude", "codex", "opencode"], "description": "Which agent runs it (default: the same as this session)" },
                    "model": { "type": "string", "description": "The agent's model, e.g. `opus` or `sonnet` for Claude (default: this session's model when the agent is the same, else the agent's default)" },
                    "worktree": { "type": "boolean", "description": "Work in a new git worktree on its own branch, branched from the branch this session started from (default true in a git repository). With false it works in the project folder itself, alongside whatever else runs there" }
                },
                "required": ["prompt"]
            }
        })]
    }

    fn instructions(&self) -> Option<String> {
        Some(
            "When you notice a separate task that shouldn't derail this one (an unrelated bug, a follow-up), \
`start_session` hands it to a new agent session in this project, which the user sees in their sidebar."
                .into(),
        )
    }

    fn call<'a>(
        &'a self,
        scope: &'a Scope,
        name: &'a str,
        args: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Option<Result<String, String>>> + Send + 'a>> {
        Box::pin(async move {
            match name {
                "start_session" => Some(self.start_session(scope, args).await),
                _ => None,
            }
        })
    }
}

impl SessionTools {
    /// The session whose run holds the grant. opencode's grant is its folder's, not a run's
    /// (its server outlives runs), so there it is the one opencode session working in that folder.
    fn caller(&self, scope: &Scope) -> Result<Session, String> {
        let sessions = self.sessions.list().map_err(|e| e.message)?;
        let found = match scope.owner.as_deref() {
            Some(run) => sessions.into_iter().find(|s| s.run_id.as_deref() == Some(run)),
            None => {
                let mut working = sessions.into_iter().filter(|s| {
                    s.agent == AgentKind::Opencode
                        && s.run_id.is_some()
                        && scope.project.is_some()
                        && s.workdir.as_deref().or(Some(&s.cwd)) == scope.project.as_deref()
                });
                match (working.next(), working.next()) {
                    (Some(s), None) => Some(s),
                    _ => None,
                }
            }
        };
        found.ok_or_else(|| "Only an agent working in a Bach session can start sessions.".into())
    }

    async fn start_session(&self, scope: &Scope, args: &Value) -> Result<String, String> {
        let prompt = args["prompt"].as_str().unwrap_or_default().trim().to_string();
        if prompt.is_empty() {
            return Err("`prompt` is required: the task for the new session.".into());
        }
        let parent = self.caller(scope)?;
        let agent = match args.get("agent").filter(|a| !a.is_null()) {
            Some(a) => serde_json::from_value::<AgentKind>(a.clone())
                .map_err(|_| "`agent` must be one of claude, codex or opencode.".to_string())?,
            None => parent.agent,
        };
        let same_agent = agent == parent.agent;
        let model = match args["model"].as_str() {
            Some(m) => Some(m.to_string()),
            None if same_agent => parent.model_choice.clone(),
            None => None,
        };
        // Permission modes and effort levels differ between agents (and effort between models),
        // so they carry over only where they still mean the same thing.
        let permission_mode = parent.permission_mode.clone().filter(|_| same_agent);
        let effort = parent
            .effort
            .clone()
            .filter(|_| same_agent && args["model"].as_str().is_none());
        let in_repo = parent.git_branch.is_some();
        let worktree = args["worktree"].as_bool().unwrap_or(in_repo);
        let s = self
            .launcher
            .start_session(StartSessionArgs {
                agent,
                cwd: parent.cwd.clone(),
                // In the project folder itself, never switch branches under whoever works there.
                branch: parent.branch.clone().filter(|_| worktree),
                worktree: Some(worktree),
                new_branch: None,
                model_choice: model,
                permission_mode,
                effort,
                prompt,
                images: vec![],
            })
            .await
            .map_err(|e| e.message)?;
        let mut text = format!(
            "Started session {} \"{}\" with {}",
            s.id,
            s.title,
            agent.display_name()
        );
        if let Some(m) = &s.model_choice {
            text += &format!(" ({m})");
        }
        text += &format!(", in {}", s.workdir.as_deref().unwrap_or(&s.cwd));
        if let Some(b) = &s.git_branch {
            text += &format!(" on branch {b}");
        }
        text += ". The user sees it in their sidebar; it works on its own.";
        Ok(text)
    }
}
