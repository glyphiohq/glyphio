/*
 * This file is part of espanso (Glyphio fork).
 *
 * GLYPHIO DEVIATION: this extension is Glyphio-specific. It bridges the engine to the
 * Glyphio desktop app over a unix socket (env `GLYPHIO_IPC_SOCKET`, exported by the app
 * when it spawns the engine sidecar), so interactive snippet kinds can use native Tauri
 * windows instead of modulo/wxWidgets (which this fork does not build):
 *
 *   * `glyphio_popup` — the app shows the snippet body in a popup window (cheatsheet).
 *     Resolves immediately to an empty string, so the typed trigger just disappears.
 *   * `glyphio_form`  — the app shows a form window, waits for the user to submit, and
 *     returns the filled body; cancelling aborts the expansion.
 *
 * Protocol: one newline-delimited JSON request, one newline-delimited JSON response:
 *   -> {"op":"popup"|"form","snippetId":"<uuid>"}
 *   <- {"ok":true,"text":"..."} | {"ok":false,"cancelled":true} | {"ok":false,"error":"..."}
 *
 * espanso is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 */

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use thiserror::Error;

use crate::{Extension, ExtensionOutput, ExtensionResult, Params, Value};

/// Forms can sit open while the user types — allow plenty of time before giving up.
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(120);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

pub struct GlyphioExtension {
    alias: &'static str,
    op: &'static str,
}

impl GlyphioExtension {
    pub fn new_form() -> Self {
        Self {
            alias: "glyphio_form",
            op: "form",
        }
    }

    pub fn new_popup() -> Self {
        Self {
            alias: "glyphio_popup",
            op: "popup",
        }
    }
}

impl Extension for GlyphioExtension {
    fn name(&self) -> &str {
        self.alias
    }

    fn calculate(
        &self,
        _: &crate::Context,
        _: &crate::Scope,
        params: &Params,
    ) -> crate::ExtensionResult {
        let Some(Value::String(snippet_id)) = params.get("snippet_id") else {
            return ExtensionResult::Error(GlyphioExtensionError::MissingSnippetId.into());
        };

        match request(self.op, snippet_id) {
            Ok(Response::Text(text)) => ExtensionResult::Success(ExtensionOutput::Single(text)),
            Ok(Response::Cancelled) => ExtensionResult::Aborted,
            Err(err) => ExtensionResult::Error(err),
        }
    }
}

enum Response {
    Text(String),
    Cancelled,
}

fn request(op: &str, snippet_id: &str) -> anyhow::Result<Response> {
    let socket_path = std::env::var("GLYPHIO_IPC_SOCKET")
        .map_err(|_| GlyphioExtensionError::SocketNotConfigured)?;

    let mut stream = UnixStream::connect(&socket_path)
        .map_err(|e| GlyphioExtensionError::ConnectFailed(e.to_string()))?;
    stream.set_write_timeout(Some(CONNECT_TIMEOUT))?;
    stream.set_read_timeout(Some(RESPONSE_TIMEOUT))?;

    let req = serde_json::json!({ "op": op, "snippetId": snippet_id });
    stream.write_all(req.to_string().as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;

    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    let resp: serde_json::Value = serde_json::from_str(line.trim())
        .map_err(|e| GlyphioExtensionError::BadResponse(e.to_string()))?;

    if resp.get("ok").and_then(|v| v.as_bool()) == Some(true) {
        let text = resp
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        Ok(Response::Text(text))
    } else if resp.get("cancelled").and_then(|v| v.as_bool()) == Some(true) {
        Ok(Response::Cancelled)
    } else {
        let msg = resp
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error")
            .to_string();
        Err(GlyphioExtensionError::AppError(msg).into())
    }
}

#[derive(Error, Debug)]
pub enum GlyphioExtensionError {
    #[error("missing 'snippet_id' parameter")]
    MissingSnippetId,
    #[error("GLYPHIO_IPC_SOCKET is not set — is the engine running outside the Glyphio app?")]
    SocketNotConfigured,
    #[error("could not reach the Glyphio app: {0}")]
    ConnectFailed(String),
    #[error("malformed response from the Glyphio app: {0}")]
    BadResponse(String),
    #[error("{0}")]
    AppError(String),
}
