//! Wire format for the WebSocket bridge. Field names match the arguments the frontend
//! passes to Tauri's `invoke`, so both transports share one call shape.
use crate::adapters::AgentKind;
use serde::Deserialize;

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
    },
    CancelRun {
        #[serde(rename = "runId")]
        run_id: String,
    },
}
