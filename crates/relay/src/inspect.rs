//! Minimal inspection of Messages API bodies.
//!
//! To stay inside the 10 ms CPU budget the relay never builds a full JSON tree
//! of a request or response on the hot path. Targeted `serde` structs read only
//! the fields we need; serde_json skips everything else without allocating.
//! The original request bytes are forwarded unchanged unless `max_tokens` has
//! to be clamped (a rare path, where the body is re-serialised).

use std::borrow::Cow;

use serde::Deserialize;

/// Fields of a Messages request the relay cares about.
#[derive(Debug, PartialEq, Eq)]
pub struct RequestHead {
    /// Requested model id.
    pub model: String,
    /// Requested output cap.
    pub max_tokens: u64,
    /// Whether SSE streaming was requested (not supported by the relay).
    pub stream: bool,
}

// serde rejects duplicate keys for derived structs, so a body cannot carry one
// `model` for the allowlist check and a different one for the upstream.
#[derive(Deserialize)]
struct RequestWire<'a> {
    #[serde(borrow)]
    model: Cow<'a, str>,
    max_tokens: u64,
    #[serde(default)]
    stream: bool,
}

/// Why a request body was rejected.
#[derive(Debug, PartialEq, Eq)]
pub enum InspectError {
    /// Not JSON, or `model` / `max_tokens` missing or of the wrong type.
    Malformed,
}

/// Read `model`, `max_tokens` and `stream` from a request body.
pub fn request_head(body: &[u8]) -> Result<RequestHead, InspectError> {
    let w: RequestWire<'_> = serde_json::from_slice(body).map_err(|_| InspectError::Malformed)?;
    Ok(RequestHead {
        model: w.model.into_owned(),
        max_tokens: w.max_tokens,
        stream: w.stream,
    })
}

/// Return a copy of `body` with `max_tokens` set to `cap`.
pub fn clamp_max_tokens(body: &[u8], cap: u64) -> Result<Vec<u8>, InspectError> {
    let mut v: serde_json::Value =
        serde_json::from_slice(body).map_err(|_| InspectError::Malformed)?;
    let obj = v.as_object_mut().ok_or(InspectError::Malformed)?;
    obj.insert("max_tokens".to_owned(), cap.into());
    serde_json::to_vec(&v).map_err(|_| InspectError::Malformed)
}

/// Token usage reported by a successful Messages response.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct Usage {
    /// Uncached input tokens.
    #[serde(default)]
    pub input_tokens: u64,
    /// Output tokens.
    #[serde(default)]
    pub output_tokens: u64,
    /// Tokens read from cache.
    #[serde(default, deserialize_with = "null_as_zero")]
    pub cache_read_input_tokens: u64,
    /// Tokens written to cache.
    #[serde(default, deserialize_with = "null_as_zero")]
    pub cache_creation_input_tokens: u64,
}

fn null_as_zero<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    Ok(Option::<u64>::deserialize(d)?.unwrap_or(0))
}

#[derive(Deserialize)]
struct ResponseWire<'a> {
    #[serde(borrow, default)]
    model: Option<Cow<'a, str>>,
    #[serde(default)]
    usage: Option<Usage>,
}

/// Extract `(model, usage)` from a response body. `model` may differ from the
/// requested one when a server-side refusal fallback ran.
pub fn response_usage(body: &[u8]) -> Option<(Option<String>, Usage)> {
    let w: ResponseWire<'_> = serde_json::from_slice(body).ok()?;
    Some((w.model.map(Cow::into_owned), w.usage?))
}

/// Validate an `anthropic-beta` header against the allowlist. Returns the
/// first disallowed value, if any.
pub fn disallowed_beta<'a>(header: &'a str, allow: &[String]) -> Option<&'a str> {
    header
        .split(',')
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .find(|b| !allow.iter().any(|a| a == b))
}

/// Whether an `anthropic-version` header value looks like `YYYY-MM-DD`.
pub fn valid_api_version(v: &str) -> bool {
    let b = v.as_bytes();
    b.len() == 10
        && b.iter().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                *c == b'-'
            } else {
                c.is_ascii_digit()
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_head_and_ignores_rest() {
        let body = br#"{"messages":[{"role":"user","content":"hi \"model\": x"}],"model":"claude-haiku-5-5","max_tokens":512,"tools":[]}"#;
        assert_eq!(
            request_head(body).unwrap(),
            RequestHead {
                model: "claude-haiku-5-5".into(),
                max_tokens: 512,
                stream: false
            }
        );
    }

    #[test]
    fn rejects_duplicate_model_keys() {
        let body = br#"{"model":"claude-haiku-5-5","model":"claude-fable-5-1","max_tokens":1}"#;
        assert_eq!(request_head(body).unwrap_err(), InspectError::Malformed);
    }

    #[test]
    fn rejects_missing_or_bad_fields() {
        assert!(request_head(b"{}").is_err());
        assert!(request_head(br#"{"model":"m","max_tokens":-1}"#).is_err());
        assert!(request_head(br#"{"model":1,"max_tokens":1}"#).is_err());
        assert!(request_head(b"not json").is_err());
    }

    #[test]
    fn escaped_model_string() {
        let h = request_head(br#"{"model":"claude-haiku-5-5","max_tokens":1}"#).unwrap();
        assert_eq!(h.model, "claude-haiku-5-5");
    }

    #[test]
    fn stream_flag() {
        assert!(
            request_head(br#"{"model":"m","max_tokens":1,"stream":true}"#)
                .unwrap()
                .stream
        );
    }

    #[test]
    fn clamp_rewrites_only_max_tokens() {
        let body =
            br#"{"model":"m","max_tokens":99999,"messages":[{"role":"user","content":"x"}]}"#;
        let out = clamp_max_tokens(body, 100).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["max_tokens"], 100);
        assert_eq!(v["messages"][0]["content"], "x");
        assert_eq!(v["model"], "m");
    }

    #[test]
    fn usage_extraction() {
        let body = br#"{"id":"msg_1","model":"claude-haiku-5-5","content":[{"type":"text","text":"usage"}],
            "usage":{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":null,"cache_creation_input_tokens":3,"service_tier":"standard"}}"#;
        let (model, u) = response_usage(body).unwrap();
        assert_eq!(model.as_deref(), Some("claude-haiku-5-5"));
        assert_eq!(
            u,
            Usage {
                input_tokens: 10,
                output_tokens: 5,
                cache_read_input_tokens: 0,
                cache_creation_input_tokens: 3
            }
        );
        assert!(response_usage(br#"{"type":"error"}"#).is_none());
    }

    #[test]
    fn beta_allowlist() {
        let allow = vec!["a-1".to_owned(), "b-2".to_owned()];
        assert_eq!(disallowed_beta("a-1, b-2", &allow), None);
        assert_eq!(disallowed_beta("a-1,evil", &allow), Some("evil"));
        assert_eq!(disallowed_beta("", &allow), None);
    }

    #[test]
    fn api_version_format() {
        assert!(valid_api_version("2026-10-21"));
        assert!(!valid_api_version("2026-10-2"));
        assert!(!valid_api_version("2026/10/21"));
        assert!(!valid_api_version("2026-10-21\r\nX: y"));
    }
}
