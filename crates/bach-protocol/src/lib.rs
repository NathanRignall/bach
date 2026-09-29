//! Bach's wire protocol, shared by every transport (Tauri IPC, the WebSocket bridge) and, through
//! the generated `src/api/generated/protocol.ts`, by the frontend.
//!
//! - A **command** is a name plus JSON arguments, answered with a result or an [`ApiError`].
//!   The whole list is the table in [`commands`].
//! - An **event** ([`ServerEvent`]) is pushed from the backend to every client.
//! - [`ClientFrame`] / [`ServerFrame`] are how the WebSocket bridge carries both. Tauri carries
//!   the same `cmd` + `args` through one `rpc` command and events on the `bach` channel.
mod agent;
pub mod commands;
mod error;
mod events;
mod typescript;
mod workspace;

pub use agent::*;
pub use commands::{Handler, Request};
pub use error::*;
pub use events::*;
pub use typescript::typescript;
pub use workspace::*;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

/// Tauri's event channel for [`ServerEvent`]s.
pub const EVENT_CHANNEL: &str = "bach";

/// A command sent over the WebSocket bridge. `id` is echoed back in the reply.
#[derive(Debug, Serialize, Deserialize, TS)]
pub struct ClientFrame {
    pub id: u64,
    pub cmd: String,
    #[serde(default)]
    #[ts(optional)]
    pub args: Option<Value>,
}

/// What the WebSocket bridge sends: replies to commands, and events.
#[derive(Debug, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ServerFrame {
    Reply { id: u64, result: Value },
    Error { id: u64, error: ApiError },
    Event { event: ServerEvent },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_commands_and_reports_bad_ones() {
        let r = Request::parse("delete_session", json!({"sessionId": "abc"})).unwrap();
        assert!(
            matches!(&r, Request::DeleteSessionArgs(a) if a.session_id == "abc"),
            "{r:?}"
        );
        // No arguments at all is the same as `{}`.
        assert_eq!(
            Request::parse("list_tasks", Value::Null).unwrap().name(),
            "list_tasks"
        );
        let e = Request::parse("nope", json!({})).unwrap_err();
        assert_eq!(e.code, ErrorCode::Invalid);
        let e = Request::parse("start_run", json!({"agent": "claude"})).unwrap_err();
        assert_eq!(e.code, ErrorCode::Invalid);
        assert!(e.message.contains("prompt"), "{e}");
    }

    #[test]
    fn arguments_live_apart_from_the_frame() {
        // An argument named `id` or `cmd` can't collide with the frame's own fields.
        let f: ClientFrame =
            serde_json::from_str(r#"{"id":7,"cmd":"stop_task","args":{"taskId":"t","id":1}}"#)
                .unwrap();
        assert_eq!(f.id, 7);
        let r = Request::parse(&f.cmd, f.args.unwrap_or_default()).unwrap();
        assert!(matches!(r, Request::StopTaskArgs(a) if a.task_id == "t"));
    }

    #[test]
    fn events_use_camel_case_fields_and_snake_case_names() {
        let ev = ServerEvent::Run(RunEvent {
            run_id: "r".into(),
            event: AgentEvent::ToolResult {
                id: "t".into(),
                output: "ok".into(),
                is_error: false,
                parent: None,
            },
        });
        assert_eq!(
            serde_json::to_value(&ev).unwrap(),
            json!({"topic": "run", "data": {"runId": "r", "type": "tool_result", "id": "t", "output": "ok", "isError": false}})
        );
        let err = ServerFrame::Error {
            id: 3,
            error: ApiError::not_found("gone"),
        };
        assert_eq!(
            serde_json::to_value(&err).unwrap(),
            json!({"kind": "error", "id": 3, "error": {"code": "not_found", "message": "gone"}})
        );
    }

    #[test]
    fn typescript_bindings_are_current() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../src/api/generated/protocol.ts");
        let fresh = typescript();
        let old = std::fs::read_to_string(&path).unwrap_or_default();
        if old != fresh {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &fresh).unwrap();
            panic!(
                "{} was out of date and has been regenerated; re-run the tests and commit it",
                path.display()
            );
        }
    }
}
