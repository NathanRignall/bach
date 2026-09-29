use bach_core::{
    adapters::AgentKind,
    list_agents as core_list_agents,
    runs::{Emit, Runs},
    store::Store,
    AgentInfo,
};
use serde_json::Value;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};

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

#[tauri::command]
fn list_dir(
    path: Option<String>,
    show_hidden: Option<bool>,
) -> Result<bach_core::fs::DirListing, String> {
    bach_core::fs::list_dir(path.as_deref(), show_hidden.unwrap_or(false))
}

#[tauri::command]
fn list_sessions(store: State<'_, Store>) -> Result<Vec<Value>, String> {
    store.list()
}

#[tauri::command]
fn save_session(store: State<'_, Store>, session: Value) -> Result<(), String> {
    store.save(&session)
}

#[tauri::command]
fn delete_session(store: State<'_, Store>, session_id: String) -> Result<(), String> {
    store.delete(&session_id)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(Runs::default())
        .setup(|app| {
            let db = app.path().app_data_dir()?.join("bach.db");
            app.manage(Store::open(&db)?);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_agents,
            start_run,
            cancel_run,
            list_dir,
            list_sessions,
            save_session,
            delete_session
        ])
        .run(tauri::generate_context!())
        .expect("error while running Bach");
}
