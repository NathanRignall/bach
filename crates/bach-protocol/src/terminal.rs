//! Terminals: shells on the machine the agents run on, kept by the backend so they outlive the
//! window that opened them. Output is bytes (it can split a UTF-8 character), sent as base64.
use serde::Serialize;
use ts_rs::TS;

#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TerminalInfo {
    pub id: String,
    /// The folder it started in.
    pub cwd: String,
    /// The shell it runs, e.g. `zsh`.
    pub shell: String,
    pub cols: u16,
    pub rows: u16,
    pub created_at: i64,
    /// Set once the shell has ended (its exit code, if it had one). It stays listed until closed.
    pub exited: Option<ExitStatus>,
}

#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ExitStatus {
    pub code: Option<u32>,
}

/// A terminal and its recent output, to draw it from scratch.
#[derive(Clone, Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSnapshot {
    pub terminal: TerminalInfo,
    /// The scrollback kept by the backend, base64.
    pub data: String,
    /// The `seq` of the last output included; later output events follow on from it.
    pub seq: u64,
}

#[derive(Clone, Debug, Serialize, TS)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum TerminalEvent {
    Opened { terminal: TerminalInfo },
    /// Output, base64. `seq` goes up by one per event, so a gap means some was missed.
    Output { terminal_id: String, seq: u64, data: String },
    /// The shell ended; the terminal stays until closed.
    Exited { terminal_id: String, status: ExitStatus },
    Closed { terminal_id: String },
}
