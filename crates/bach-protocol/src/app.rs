//! The desktop app's own commands (not the backend's): where its agents run, and how that
//! connection is doing.
use crate::AgentWrapper;
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
        /// Given to bach-server there when the app starts or restarts it. An empty command leaves
        /// the server's own (`BACH_AGENT_WRAPPER`): the app never takes a wrapper away.
        #[serde(default)]
        wrapper: AgentWrapper,
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
    /// The server is there but speaks another protocol than this app: restarting it (or the app)
    /// is the way out.
    pub incompatible: bool,
    /// The server's version, once connected.
    pub version: Option<String>,
    /// How the server starts agents, once connected.
    pub wrapper: Option<AgentWrapper>,
    /// The server was started with another agent wrapper than the one set here (and only a new
    /// server takes it): new agent turns are refused until it restarts.
    pub wrapper_mismatch: bool,
}

/// A port on the agents' machine reachable on this computer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PortForward {
    /// The port there.
    pub remote: u16,
    /// Where it is here (`localhost:<local>`); the same number when that was free.
    pub local: u16,
    /// Forwarded because a background task listens on it; ends when no running task does.
    pub auto: bool,
}

/// Port forwarding in the desktop app. Sent on the `bach-forwards` channel as it changes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Forwarding {
    /// Only connections over SSH forward; on this computer, ports are already local.
    pub available: bool,
    /// Forward the ports background tasks listen on without being asked.
    pub auto: bool,
    pub forwards: Vec<PortForward>,
    /// The latest thing that went wrong, if anything.
    pub error: Option<String>,
}
