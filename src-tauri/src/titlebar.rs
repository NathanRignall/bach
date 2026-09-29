//! The Mac app's title bar. The window draws under it (`titleBarStyle: Overlay`), and an empty
//! unified toolbar makes macOS lay the traffic lights out as it does for any app with a toolbar:
//! their size, spacing and inset for the running macOS version, and hover and the green button's
//! menu working. (Moving the buttons with `trafficLightPosition` leaves their hover tracking
//! behind.) The page reads the bar's height and where the buttons end with `title_bar`, and
//! sizes its top bar to match.

use serde_json::Value;

#[cfg(target_os = "macos")]
pub fn install(window: &tauri::WebviewWindow) -> tauri::Result<()> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSTitlebarSeparatorStyle, NSToolbar, NSWindowToolbarStyle};

    let mtm = MainThreadMarker::new().expect("the window is set up on the main thread");
    let toolbar = NSToolbar::new(mtm);
    let ns_window = ns_window(window)?;
    ns_window.setToolbar(Some(&toolbar));
    ns_window.setToolbarStyle(NSWindowToolbarStyle::Unified);
    ns_window.setTitlebarSeparatorStyle(NSTitlebarSeparatorStyle::None);
    Ok(())
}

/// The bar's height and where the traffic lights end, in points; `None` off the Mac.
#[tauri::command]
pub async fn title_bar(window: tauri::WebviewWindow) -> Option<Value> {
    #[cfg(target_os = "macos")]
    {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let w = window.clone();
        window
            .run_on_main_thread(move || {
                let _ = tx.send(measure(&w));
            })
            .ok()?;
        rx.await.ok().flatten()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = window;
        None
    }
}

#[cfg(target_os = "macos")]
fn measure(window: &tauri::WebviewWindow) -> Option<Value> {
    use objc2_app_kit::NSWindowButton;
    let ns_window = ns_window(window).ok()?;
    // Window coordinates start at the bottom: the bar is what's above the content layout rect.
    let frame = ns_window.frame();
    let content = ns_window.contentLayoutRect();
    let height = frame.size.height - (content.origin.y + content.size.height);
    let zoom = ns_window.standardWindowButton(NSWindowButton::ZoomButton)?;
    let r = zoom.convertRect_toView(zoom.bounds(), None);
    Some(serde_json::json!({ "height": height, "controlsEnd": r.origin.x + r.size.width }))
}

/// Only for use on the main thread.
#[cfg(target_os = "macos")]
fn ns_window(window: &tauri::WebviewWindow) -> tauri::Result<&objc2_app_kit::NSWindow> {
    // SAFETY: Tauri hands out the window's live NSWindow, which outlives `window`.
    Ok(unsafe { &*window.ns_window()?.cast() })
}
