//! The desktop app's own commands (not the backend's): where its agents run, and how that
//! connection is doing.
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Where the desktop app's agents run.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "mode", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum Connection {
    /// On this computer, inside the app.
    Local,
    /// On another machine, through `ssh <host> <command> attach`.
    Ssh {
        /// As you would pass it to `ssh`: a `~/.ssh/config` alias, `host`, or `user@host`.
        host: String,
        /// How to run bach-server there (default `bach-server`).
        #[serde(default = "default_command")]
        command: String,
    },
}

pub fn default_command() -> String {
    "bach-server".into()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    Connecting,
    Connected,
    Disconnected,
}

/// How the desktop app's connection is doing. Sent on the `bach-connection` channel as it changes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionStatus {
    pub connection: Connection,
    pub state: ConnectionState,
    /// Why it isn't connected.
    pub error: Option<String>,
    /// Whether it will try again by itself.
    pub retrying: bool,
    /// The server's version, once connected.
    pub version: Option<String>,
}
