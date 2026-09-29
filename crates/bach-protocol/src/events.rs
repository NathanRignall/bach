use crate::SessionEvent;
use satie_protocol::TaskEvent;
use serde::Serialize;
use ts_rs::TS;

/// Everything the backend pushes to clients, whichever transport carries it.
#[derive(Clone, Debug, Serialize, TS)]
#[serde(tag = "topic", content = "data", rename_all = "snake_case")]
pub enum ServerEvent {
    Session(SessionEvent),
    /// A background task changed or was removed.
    Task(TaskEvent),
}
