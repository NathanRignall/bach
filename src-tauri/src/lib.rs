use bach_core::{
    adapters::AgentKind,
    list_agents as core_list_agents,
    runs::{Emit, Runs},
    AgentInfo,
};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

#[tauri::command]
fn list_agents() -> Vec<AgentInfo> {
    core_list_agents()
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
    let emit: Emit = Arc::new(move |ev| {
        let _ = app.emit("agent-event", ev);
    });
    runs.start(emit, agent, prompt, cwd, session_id).await
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
