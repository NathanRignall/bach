use crate::AgentEvent;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// One event from an agent run.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RunEvent {
    pub run_id: String,
    #[serde(flatten)]
    pub event: AgentEvent,
}

/// Everything the backend pushes to clients, whichever transport carries it.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[serde(tag = "topic", content = "data", rename_all = "snake_case")]
pub enum ServerEvent {
    Run(RunEvent),
}
