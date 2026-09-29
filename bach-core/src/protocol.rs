//! Wire format for the WebSocket bridge. Field names match the arguments the frontend
//! passes to Tauri's `invoke`, so both transports share one call shape.
use crate::adapters::AgentKind;
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
pub struct Envelope {
    pub id: u64,
    #[serde(flatten)]
    pub request: Request,
}

#[derive(Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    ListAgents,
    StartRun {
        agent: AgentKind,
        prompt: String,
        cwd: Option<String>,
        #[serde(rename = "sessionId")]
        session_id: Option<String>,
        model: Option<String>,
        /// Permission rules approved earlier in the session.
        #[serde(rename = "allowedTools", default)]
        allowed_tools: Vec<String>,
        /// The UI session this run belongs to (`sessionId` above is the agent's own session).
        #[serde(rename = "sessionKey")]
        session_key: Option<String>,
    },
    RespondApproval {
        #[serde(rename = "runId")]
        run_id: String,
        #[serde(rename = "requestId")]
        request_id: String,
        decision: crate::runs::Decision,
        message: Option<String>,
        /// For a question from the agent: the chosen answer per question text.
        answers: Option<std::collections::HashMap<String, String>>,
    },
    ListDir {
        path: Option<String>,
        #[serde(rename = "showHidden", default)]
        show_hidden: bool,
    },
    GitInfo {
        path: String,
    },
    PrepareWorkspace {
        cwd: String,
        branch: Option<String>,
        #[serde(default)]
        worktree: bool,
        #[serde(rename = "newBranch")]
        new_branch: Option<String>,
    },
    ListWorktrees,
    RemoveWorktree {
        path: String,
        #[serde(default)]
        discard: bool,
        #[serde(rename = "deleteBranch", default)]
        delete_branch: bool,
    },
    ListTasks,
    // `taskId`, not `id`: that name is the envelope's request id.
    TaskLogs {
        #[serde(rename = "taskId")]
        task_id: String,
        lines: Option<usize>,
    },
    StopTask {
        #[serde(rename = "taskId")]
        task_id: String,
    },
    RemoveTask {
        #[serde(rename = "taskId")]
        task_id: String,
    },
    StartTask {
        command: String,
        cwd: String,
        name: Option<String>,
    },
    ListSessions,
    SaveSession {
        session: Value,
    },
    DeleteSession {
        // Not `id`: that name is the envelope's request id.
        #[serde(rename = "sessionId")]
        session_id: String,
    },
    CancelRun {
        #[serde(rename = "runId")]
        run_id: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_args_do_not_collide_with_envelope_id() {
        let env: Envelope =
            serde_json::from_str(r#"{"id":7,"cmd":"delete_session","sessionId":"abc"}"#).unwrap();
        assert_eq!(env.id, 7);
        assert!(
            matches!(env.request, Request::DeleteSession { session_id } if session_id == "abc")
        );
        let env: Envelope = serde_json::from_str(
            r#"{"id":8,"cmd":"start_run","agent":"claude","prompt":"hi","sessionId":"s"}"#,
        )
        .unwrap();
        assert!(matches!(env.request, Request::StartRun { session_id: Some(s), .. } if s == "s"));
    }

    #[test]
    fn no_request_reuses_the_envelope_id_name() {
        // Every command's arguments must coexist with the envelope's numeric `id`.
        for body in [
            r#"{"id":1,"cmd":"task_logs","taskId":"abc","lines":50}"#,
            r#"{"id":2,"cmd":"stop_task","taskId":"abc"}"#,
            r#"{"id":3,"cmd":"remove_task","taskId":"abc"}"#,
            r#"{"id":4,"cmd":"start_task","command":"ls","cwd":"/tmp"}"#,
            r#"{"id":5,"cmd":"list_tasks"}"#,
            r#"{"id":6,"cmd":"delete_session","sessionId":"s"}"#,
        ] {
            let env: Envelope =
                serde_json::from_str(body).unwrap_or_else(|e| panic!("{body}: {e}"));
            assert!(env.id >= 1, "{body}");
        }
        // The old spelling is refused rather than silently mis-parsed.
        assert!(serde_json::from_str::<Envelope>(r#"{"id":"abc","cmd":"stop_task"}"#).is_err());
    }
}
