//! How much of its limits the Claude account has used, and how full a session's context is.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use ts_rs::TS;

/// One usage-limit window, e.g. the 5-hour or the weekly limit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LimitWindow {
    /// Share used, 0.0 to 1.0.
    pub utilization: f64,
    /// When the window resets (ms since the epoch).
    pub resets_at: Option<i64>,
}

/// The account's usage limits as the agent last reported them. Only updated while agents run.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PlanUsage {
    /// `allowed`, or why requests are being refused.
    pub status: String,
    /// By the agent's name for the window: `five_hour`, `seven_day`, ...
    pub windows: BTreeMap<String, LimitWindow>,
    /// When this was reported (ms since the epoch).
    pub observed_at: i64,
}

/// How much of the model's context window a session's conversation fills.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ContextUsage {
    /// Tokens in the conversation after the agent's latest message.
    pub used: u64,
    /// The model's context window, once the agent has reported it.
    pub window: Option<u64>,
}
