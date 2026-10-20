//! Keeping the floating panel out of screenshots and screen recordings.
//!
//! Port of the original's `AppSettings.hidesFromScreenCapture` and its
//! `FloatingPanelController.applyScreenCapture`, which set
//! `NSWindow.sharingType = .none` on the panel.
//!
//! No FFI of our own is needed: Tauri's `set_content_protected` is exactly this
//! call on both platforms. On Windows `tao` passes `WDA_EXCLUDEFROMCAPTURE` (or
//! `WDA_NONE`) to `SetWindowDisplayAffinity`; on macOS it sets the same
//! `NSWindow.sharingType` the original sets. This module exists to name the
//! preference's effect in one place, apply it both at launch and when the
//! preference changes, and say something when the system refuses.
//!
//! **A request, not a guarantee**, exactly as the original documents it. A
//! capture tool that asks the documented APIs leaves the panel out; one that
//! reads the screen buffer directly, or records the desktop through a driver,
//! can still include it. Off by default, because it hides the panel from the
//! reader's own pictures of it too — which is also what keeps a README image or
//! a bug report's screenshot from showing it.

/// Apply a preference to the panel window.
///
/// Returns the reason the system refused, so the caller can say so; a refusal is
/// not fatal, because the panel still draws and only its capture behaviour is
/// unchanged.
pub fn apply(window: &tauri::WebviewWindow, hide_from_capture: bool) -> Result<(), String> {
    window
        .set_content_protected(hide_from_capture)
        .map_err(|e| e.to_string())
}

/// Apply the stored preference to the panel, if there is one.
///
/// Called on launch and whenever the preference changes. A missing window is
/// not an error: the panel is created from `tauri.conf.json` before the frontend
/// runs, but a preference saved from the settings window can arrive while the
/// panel is being recreated.
pub fn apply_to_panel(app: &tauri::AppHandle, hide_from_capture: bool) {
    use tauri::Manager;
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    if let Err(reason) = apply(&window, hide_from_capture) {
        eprintln!("PulseWin: 无法应用“截图时隐藏”：{reason}");
    }
}
