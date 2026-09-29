pub mod adapters;
pub mod fs;
pub mod git;
pub mod protocol;
pub mod runs;
pub mod satie;
pub mod server;
pub mod store;

use adapters::AgentKind;
use serde::Serialize;

#[derive(Serialize)]
pub struct AgentInfo {
    pub kind: AgentKind,
    pub name: &'static str,
    pub installed: bool,
}

pub fn list_agents() -> Vec<AgentInfo> {
    AgentKind::ALL
        .iter()
        .map(|&kind| AgentInfo {
            kind,
            name: kind.display_name(),
            installed: which::which(kind.binary()).is_ok(),
        })
        .collect()
}
