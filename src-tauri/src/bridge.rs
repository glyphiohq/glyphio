//! Unix-socket bridge between the engine sidecar and the app.
//!
//! The fork's `glyphio` render extension connects here mid-expansion (one newline-delimited
//! JSON request per connection):
//! * `{"op":"popup","snippetId":X}` — open the popup (cheatsheet) window, reply immediately
//!   with empty text so the typed trigger just disappears.
//! * `{"op":"form","snippetId":X}` — open the form window and **block** until the user
//!   submits (reply carries the filled body) or cancels/times out (expansion aborts).
//!
//! Only snippets that exist, are live, and are enabled resolve — the engine can never use
//! the bridge to run anything the store doesn't currently expose.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::json;
use tauri::{AppHandle, Manager};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::AppState;

/// Slightly under the extension's 120s read timeout, so the engine sees a clean
/// "cancelled" instead of a socket timeout.
const FORM_TIMEOUT: Duration = Duration::from_secs(110);

pub enum FormReply {
    Submitted(String),
    Cancelled,
}

/// Form requests waiting on user input, keyed by request id. Kept in [`AppState`].
#[derive(Default)]
pub struct BridgeState {
    pending_forms: Mutex<HashMap<String, oneshot::Sender<FormReply>>>,
}

impl BridgeState {
    /// Complete a pending form request (from the `form_submit` / `form_cancel` commands).
    pub fn resolve(&self, request_id: &str, reply: FormReply) {
        if let Some(tx) = self.pending_forms.lock().unwrap().remove(request_id) {
            let _ = tx.send(reply);
        }
    }
}

/// Bind the socket and serve engine callbacks for the life of the app.
pub fn start(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = serve(app).await {
            log::error!("engine bridge failed: {e}");
        }
    });
}

async fn serve(app: AppHandle) -> anyhow::Result<()> {
    let path = app.state::<AppState>().paths.bridge_socket();
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    // The socket accepts commands that pop windows over the user's screen — user-only access.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    log::info!("engine bridge listening on {}", path.display());
    loop {
        let (stream, _) = listener.accept().await?;
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = handle(app, stream).await {
                log::warn!("bridge request failed: {e}");
            }
        });
    }
}

async fn handle(app: AppHandle, stream: tokio::net::UnixStream) -> anyhow::Result<()> {
    let (read, mut write) = stream.into_split();
    let mut line = String::new();
    BufReader::new(read).read_line(&mut line).await?;
    let req: serde_json::Value = serde_json::from_str(line.trim())?;
    let op = req.get("op").and_then(|v| v.as_str()).unwrap_or("");
    let snippet_id = req.get("snippetId").and_then(|v| v.as_str()).unwrap_or("");

    let resp = process(&app, op, snippet_id).await;
    write.write_all(resp.to_string().as_bytes()).await?;
    write.write_all(b"\n").await?;
    Ok(())
}

async fn process(app: &AppHandle, op: &str, snippet_id: &str) -> serde_json::Value {
    let snippet = {
        let state = app.state::<AppState>();
        match state.snippets.get(snippet_id) {
            Ok(Some(s)) if s.deleted_at.is_none() && s.enabled => s,
            _ => return json!({ "ok": false, "error": "unknown or disabled snippet" }),
        }
    };

    match op {
        "popup" => {
            let payload = json!({
                "snippet": {
                    "id": snippet.id,
                    "trigger": snippet.trigger,
                    "replacement": snippet.replacement,
                    "format": snippet.format,
                },
            });
            open_with_payload(app, "popup", payload).await;
            json!({ "ok": true, "text": "" })
        }
        "form" => {
            let request_id = Uuid::new_v4().to_string();
            let (tx, rx) = oneshot::channel();
            {
                let state = app.state::<AppState>();
                // A previous form left open gets its sender dropped here → it cancels.
                state.bridge.pending_forms.lock().unwrap().insert(request_id.clone(), tx);
            }
            let payload = json!({
                "requestId": request_id,
                "snippet": {
                    "id": snippet.id,
                    "trigger": snippet.trigger,
                    "replacement": snippet.replacement,
                    "format": snippet.format,
                    "variables": snippet.variables,
                },
            });
            open_with_payload(app, "form", payload).await;

            match tokio::time::timeout(FORM_TIMEOUT, rx).await {
                Ok(Ok(FormReply::Submitted(text))) => json!({ "ok": true, "text": text }),
                Ok(Ok(FormReply::Cancelled)) | Ok(Err(_)) => {
                    json!({ "ok": false, "cancelled": true })
                }
                Err(_timeout) => {
                    let state = app.state::<AppState>();
                    state.bridge.pending_forms.lock().unwrap().remove(&request_id);
                    json!({ "ok": false, "cancelled": true })
                }
            }
        }
        other => json!({ "ok": false, "error": format!("unknown op {other:?}") }),
    }
}

/// Stash the payload for the surface, then (re)open its window on the main thread. An
/// already-open window is recreated so it always shows the fresh payload.
///
/// Recreating means the old one has to be *gone*, not merely asked to go: `close()` posts a
/// request to the event loop, and building a webview whose label is still taken fails
/// outright — which is how a second form trigger used to do nothing at all.
async fn open_with_payload(app: &AppHandle, surface: &'static str, payload: serde_json::Value) {
    {
        let state = app.state::<AppState>();
        state.pending_payloads.lock().unwrap().insert(surface.to_string(), payload);
    }
    if let Some(win) = app.get_webview_window(surface) {
        let _ = win.close();
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(25)).await;
            if app.get_webview_window(surface).is_none() {
                break;
            }
        }
    }
    let app = app.clone();
    let _ = app.clone().run_on_main_thread(move || {
        if let Err(e) = crate::windows::open_surface(&app, surface) {
            log::error!("could not open {surface} window: {e}");
        }
    });
}
