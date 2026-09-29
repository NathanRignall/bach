//! The desktop shell: one `rpc` command carries every protocol command to [`Api`], and events
//! go out on the `bach` channel. The frontend can instead talk to a remote `bach-server`.
use bach_core::Api;
use bach_protocol::{ApiError, EVENT_CHANNEL};
use serde_json::Value;
use tauri::{Emitter, Manager, State};
use tokio::sync::broadcast::error::RecvError;

#[tauri::command]
async fn rpc(api: State<'_, Api>, name: String, args: Option<Value>) -> Result<Value, ApiError> {
    api.call(&name, args.unwrap_or_default()).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let db = app.path().app_data_dir()?.join("bach.db");
            let api = tauri::async_runtime::block_on(Api::open(&db))?;
            let mut events = api.subscribe();
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    match events.recv().await {
                        Ok(ev) => {
                            let _ = handle.emit(EVENT_CHANNEL, ev);
                        }
                        Err(RecvError::Lagged(_)) => continue,
                        Err(RecvError::Closed) => break,
                    }
                }
            });
            app.manage(api);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![rpc])
        .run(tauri::generate_context!())
        .expect("error while running Bach");
}
