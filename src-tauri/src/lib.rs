mod adapters;
mod runs;

use adapters::AgentKind;
use runs::Runs;
use serde::Serialize;
use tauri::{AppHandle, State};

#[derive(Serialize)]
struct AgentInfo {
    kind: AgentKind,
    name: &'static str,
    installed: bool,
}

#[tauri::command]
fn list_agents() -> Vec<AgentInfo> {
    AgentKind::ALL
        .iter()
        .map(|&kind| AgentInfo {
            kind,
            name: kind.display_name(),
            installed: which::which(kind.binary()).is_ok(),
        })
        .collect()
}

/// Starts a run and returns its run id. Events arrive on the `agent-event` channel.
#[tauri::command]
async fn start_run(
    app: AppHandle,
    runs: State<'_, Runs>,
    agent: AgentKind,
    prompt: String,
    cwd: Option<String>,
    session_id: Option<String>,
) -> Result<String, String> {
    runs.start(app, agent, prompt, cwd, session_id).await
}

#[tauri::command]
async fn cancel_run(runs: State<'_, Runs>, run_id: String) -> Result<(), String> {
    runs.cancel(&run_id).await;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(Runs::default())
        .invoke_handler(tauri::generate_handler![list_agents, start_run, cancel_run])
        .run(tauri::generate_context!())
        .expect("error while running Bach");
}
