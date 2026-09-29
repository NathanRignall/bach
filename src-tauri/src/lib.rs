use bach_core::{
    adapters::AgentKind,
    git::{Git, GitInfo, Workspace, WorktreeEntry},
    list_agents as core_list_agents,
    runs::{Decision, Emit, RunRequest, Runs},
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
// Tauri command arguments are flat by design.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
async fn start_run(
    app: AppHandle,
    runs: State<'_, Runs>,
    agent: AgentKind,
    prompt: String,
    cwd: Option<String>,
    session_id: Option<String>,
    model: Option<String>,
    allowed_tools: Option<Vec<String>>,
) -> Result<String, String> {
    let emit: Emit = Arc::new(move |ev| {
        let _ = app.emit("agent-event", ev);
    });
    let req = RunRequest {
        agent,
        prompt,
        cwd,
        session_id,
        model,
        allowed_tools: allowed_tools.unwrap_or_default(),
    };
    runs.start(emit, req).await
}

#[tauri::command]
async fn respond_approval(
    runs: State<'_, Runs>,
    run_id: String,
    request_id: String,
    decision: Decision,
    message: Option<String>,
    answers: Option<std::collections::HashMap<String, String>>,
) -> Result<(), String> {
    runs.respond_approval(&run_id, &request_id, decision, message, answers)
        .await
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
async fn git_info(git: State<'_, Git>, path: String) -> Result<GitInfo, String> {
    git.info(path).await
}

#[tauri::command]
async fn prepare_workspace(
    git: State<'_, Git>,
    cwd: String,
    branch: Option<String>,
    worktree: Option<bool>,
    new_branch: Option<String>,
) -> Result<Workspace, String> {
    git.prepare(cwd, branch, worktree.unwrap_or(false), new_branch)
        .await
}

#[tauri::command]
async fn list_worktrees(git: State<'_, Git>) -> Result<Vec<WorktreeEntry>, String> {
    Ok(git.list_worktrees().await)
}

#[tauri::command]
async fn remove_worktree(
    git: State<'_, Git>,
    path: String,
    discard: Option<bool>,
    delete_branch: Option<bool>,
) -> Result<(), String> {
    git.remove_worktree(
        path,
        discard.unwrap_or(false),
        delete_branch.unwrap_or(false),
    )
    .await
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
            let dir = app.path().app_data_dir()?;
            app.manage(Store::open(&dir.join("bach.db"))?);
            app.manage(Git::new(dir.join("worktrees")));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_agents,
            start_run,
            cancel_run,
            respond_approval,
            list_dir,
            git_info,
            prepare_workspace,
            list_worktrees,
            remove_worktree,
            list_sessions,
            save_session,
            delete_session
        ])
        .run(tauri::generate_context!())
        .expect("error while running Bach");
}
