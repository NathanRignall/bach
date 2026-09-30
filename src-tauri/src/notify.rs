//! Native notifications for the Mac app. The page decides when one is due (a turn finished, an
//! approval or a question is waiting, and the user isn't looking at that session); this shows it
//! and, when it's clicked, brings the window forward and tells the page which session to open.
//! Only the Mac has this: elsewhere the page has nothing to call, so it falls back to nothing.

#[cfg(target_os = "macos")]
pub use mac::notify;

#[cfg(target_os = "macos")]
mod mac {
    use mac_notification_sys::{Notification, NotificationResponse};
    use tauri::{AppHandle, Emitter, Manager};

    /// Shows a notification, and on a click opens `session_id`'s session (the page listens for
    /// `bach-open-session`). Delivery blocks until the notification is clicked or dismissed, so it
    /// gets a thread of its own.
    #[tauri::command]
    pub fn notify(app: AppHandle, title: String, body: String, session_id: String) {
        // Notifications come from the app's own bundle id (falling back to a default one when
        // running unbundled, which `mac-notification-sys` picks itself). Already set is fine.
        let _ = mac_notification_sys::set_application(&app.config().identifier);
        std::thread::spawn(move || {
            let sent = Notification::new()
                .title(&title)
                .message(&body)
                .default_sound()
                .wait_for_click(true)
                .send();
            match sent {
                Ok(NotificationResponse::Click) => {
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.unminimize();
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                    let _ = app.emit("bach-open-session", session_id);
                }
                Ok(_) => {}
                Err(e) => eprintln!("couldn't show a notification: {e}"),
            }
        });
    }
}

/// Off the Mac there are no native notifications; the page doesn't call this.
#[cfg(not(target_os = "macos"))]
#[tauri::command]
pub fn notify(_title: String, _body: String, _session_id: String) {}
