//! Image and file bytes over IPC without base64.
//!
//! A page sends the bytes as the raw invoke body and the other arguments as one JSON object in
//! the [`ARGS_HEADER`] header, percent-encoded so a note in any language survives the trip
//! (`ui/shared/ipc-bytes.mjs` is the other half). A base64 string body still works, for a page
//! that has not caught up.

use serde::de::DeserializeOwned;
use tauri::ipc::{InvokeBody, Request};

/// The header that carries a binary command's other arguments.
pub const ARGS_HEADER: &str = "x-glyphio-args";

/// The bytes in an invoke body: raw as sent, or decoded from a base64 string or data URL. An
/// empty body (or JSON null) is no bytes.
pub fn body_bytes(body: &InvokeBody) -> anyhow::Result<Vec<u8>> {
    use base64::Engine;
    match body {
        InvokeBody::Raw(bytes) => Ok(bytes.clone()),
        InvokeBody::Json(serde_json::Value::String(s)) => {
            let b64 = s.split_once(',').map_or(s.as_str(), |(_, rest)| rest);
            Ok(base64::engine::general_purpose::STANDARD.decode(b64)?)
        }
        InvokeBody::Json(serde_json::Value::Null) => Ok(Vec::new()),
        InvokeBody::Json(serde_json::Value::Object(o)) if o.is_empty() => Ok(Vec::new()),
        InvokeBody::Json(other) => anyhow::bail!("expected bytes, got {other}"),
    }
}

/// The other arguments, from the header's percent-encoded JSON. No header means `{}`.
pub fn header_args<T: DeserializeOwned>(header: Option<&str>) -> anyhow::Result<T> {
    let json = match header {
        Some(raw) => percent_encoding::percent_decode_str(raw).decode_utf8()?.into_owned(),
        None => "{}".to_string(),
    };
    Ok(serde_json::from_str(&json)?)
}

/// Both halves of a binary command's input.
pub fn read<T: DeserializeOwned>(request: &Request<'_>) -> Result<(Vec<u8>, T), String> {
    let header = request
        .headers()
        .get(ARGS_HEADER)
        .map(|v| v.to_str().map_err(|e| format!("bad {ARGS_HEADER} header: {e}")))
        .transpose()?;
    let args = header_args(header).map_err(|e| format!("bad {ARGS_HEADER} header: {e:#}"))?;
    let bytes = body_bytes(request.body()).map_err(|e| format!("bad body: {e:#}"))?;
    Ok((bytes, args))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Deserialize, Debug, PartialEq)]
    #[serde(rename_all = "camelCase")]
    struct Args {
        id: String,
        #[serde(default)]
        note: String,
    }

    #[test]
    fn raw_bytes_pass_through_and_base64_still_decodes() {
        assert_eq!(body_bytes(&InvokeBody::Raw(vec![1, 2, 3])).unwrap(), vec![1, 2, 3]);
        let s = InvokeBody::Json(serde_json::json!("data:image/png;base64,AQID"));
        assert_eq!(body_bytes(&s).unwrap(), vec![1, 2, 3]);
        assert!(body_bytes(&InvokeBody::Json(serde_json::Value::Null)).unwrap().is_empty());
        assert!(body_bytes(&InvokeBody::default()).unwrap().is_empty());
        assert!(body_bytes(&InvokeBody::Json(serde_json::json!(42))).is_err());
    }

    #[test]
    fn header_args_survive_any_language_and_default_when_absent() {
        // encodeURIComponent('{"id":"a","note":"café — 東京 100%"}')
        let header = "%7B%22id%22%3A%22a%22%2C%22note%22%3A%22caf%C3%A9%20%E2%80%94%20%E6%9D%B1%E4%BA%AC%20100%25%22%7D";
        let args: Args = header_args(Some(header)).unwrap();
        assert_eq!(args, Args { id: "a".into(), note: "café — 東京 100%".into() });
        let none: serde_json::Value = header_args(None).unwrap();
        assert_eq!(none, serde_json::json!({}));
        assert!(header_args::<Args>(Some("%7Bnope")).is_err());
    }
}
