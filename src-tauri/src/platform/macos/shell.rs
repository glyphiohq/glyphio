//! How the app presents itself to macOS: Dock/menu-bar activation policy, hiding the app, and
//! letting a window follow the user across Spaces.

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, Manager, WebviewWindow};

/// Start as a menu-bar agent (no Dock icon). Unconditional; called once at launch.
pub fn init_agent(app: &AppHandle) {
    if let Err(e) = app.set_activation_policy(tauri::ActivationPolicy::Accessory) {
        log::warn!("could not set activation policy: {e}");
    }
}

/// Set the policy, remembering what it is. The transition is visible — the Dock icon appears
/// or disappears — so it is only applied on an actual change; calling it on every window event
/// otherwise flickers. See `windows::summoned` for why summoned surfaces demote first.
pub fn set_regular(app: &AppHandle, regular: bool) {
    static IS_REGULAR: AtomicBool = AtomicBool::new(false);

    if regular == IS_REGULAR.swap(regular, Ordering::SeqCst) {
        return;
    }
    let policy = if regular {
        tauri::ActivationPolicy::Regular
    } else {
        tauri::ActivationPolicy::Accessory
    };
    if let Err(e) = app.set_activation_policy(policy) {
        log::warn!("could not set activation policy: {e}");
    }
}

/// Deactivate the whole app, not just one window, so key focus returns to the app the user was in.
pub fn hide_app(app: &AppHandle) {
    let _ = app.hide();
}

/// Make `win` follow the user across Spaces. AppKit objects must be touched on the main thread,
/// so the hop is done here, once, rather than trusted at each call site;
/// `run_on_main_thread` runs it inline when already on the main thread.
pub fn follow_user_across_spaces(win: &WebviewWindow) {
    use objc2_app_kit::{NSWindow, NSWindowCollectionBehavior};

    let win = win.clone();
    let app = win.app_handle().clone();
    let _ = app.run_on_main_thread(move || {
        let Ok(ptr) = win.ns_window() else { return };
        if ptr.is_null() {
            return;
        }
        // Safety: `ns_window` hands back this window's live NSWindow, and this block only runs
        // on the main thread.
        unsafe {
            let ns: &NSWindow = &*(ptr as *const NSWindow);
            let behavior = ns.collectionBehavior() | NSWindowCollectionBehavior::MoveToActiveSpace;
            ns.setCollectionBehavior(behavior);
        }
    });
}
