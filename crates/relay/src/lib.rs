//! # Pocket Agent relay
//!
//! A deliberately small Cloudflare Worker (Rust → WASM via `workers-rs`) that
//! each user deploys to their own account on the Workers Free plan. It:
//!
//! 1. authenticates the phone with a revocable device token,
//! 2. enforces a model allowlist, a `max_tokens` clamp and soft USD caps,
//! 3. forwards the request bytes to `api.anthropic.com` with the user's API key
//!    (a Worker secret the phone never sees),
//! 4. records token usage in D1.
//!
//! All agent logic lives on the phone. The relay never logs request or
//! response content, headers, or secrets.
//!
//! Module map:
//! - [`config`]: `wrangler.toml` vars → [`config::Config`]
//! - [`auth`]: device tokens and pairing-code checks
//! - [`inspect`]: targeted parsing of Messages API bodies (CPU budget)
//! - [`usage`]: cost estimates, aggregation and caps
//! - [`time`]: UTC day/month helpers
//! - `worker` (wasm32 only): the HTTP handlers and D1 queries
//!
//! Everything except the `worker` module is plain Rust and unit-tested natively.

pub mod auth;
pub mod config;
pub mod inspect;
pub mod time;
pub mod usage;

#[cfg(target_arch = "wasm32")]
mod worker_glue;

use pocket_agent_protocol::{ErrorEnvelope, RelayErrorKind};

/// Relay build version (workspace SemVer), reported by `/healthz`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The routes the relay serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// `GET /healthz`
    Health,
    /// `POST /v1/pair`
    Pair,
    /// `POST /v1/messages`
    Messages,
    /// `GET /v1/usage`
    Usage,
    /// `GET /v1/prefs`
    PrefsGet,
    /// `PUT /v1/prefs`
    PrefsPut,
    /// `GET /v1/devices`
    Devices,
    /// `POST /v1/devices/revoke`
    Revoke,
    /// Known path, wrong method.
    MethodNotAllowed,
    /// Anything else.
    NotFound,
}

impl Route {
    /// Match a method and path. Trailing slashes are not accepted, to keep
    /// one canonical URL per route.
    pub fn parse(method: &str, path: &str) -> Self {
        let m = method.to_ascii_uppercase();
        let known = |get: Option<Route>, post: Option<Route>, put: Option<Route>| {
            match m.as_str() {
                "GET" => get,
                "POST" => post,
                "PUT" => put,
                _ => None,
            }
            .unwrap_or(Route::MethodNotAllowed)
        };
        match path {
            "/healthz" => known(Some(Route::Health), None, None),
            "/v1/pair" => known(None, Some(Route::Pair), None),
            "/v1/messages" => known(None, Some(Route::Messages), None),
            "/v1/usage" => known(Some(Route::Usage), None, None),
            "/v1/prefs" => known(Some(Route::PrefsGet), None, Some(Route::PrefsPut)),
            "/v1/devices" => known(Some(Route::Devices), None, None),
            "/v1/devices/revoke" => known(None, Some(Route::Revoke), None),
            _ => Route::NotFound,
        }
    }

    /// Short name for logs (never includes user data).
    pub fn name(self) -> &'static str {
        match self {
            Route::Health => "healthz",
            Route::Pair => "pair",
            Route::Messages => "messages",
            Route::Usage => "usage",
            Route::PrefsGet => "prefs_get",
            Route::PrefsPut => "prefs_put",
            Route::Devices => "devices",
            Route::Revoke => "revoke",
            Route::MethodNotAllowed => "method_not_allowed",
            Route::NotFound => "not_found",
        }
    }
}

/// An error the relay itself returns to the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayError {
    /// Category; determines the HTTP status.
    pub kind: RelayErrorKind,
    /// Message safe to show to the user. Never contains secrets.
    pub message: String,
    /// Override status (only used for 405).
    pub status_override: Option<u16>,
}

impl RelayError {
    /// New error.
    pub fn new(kind: RelayErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            status_override: None,
        }
    }

    /// snake_case error type, as in the body (`relay_cap_exceeded`, …).
    pub fn kind_str(&self) -> &'static str {
        match self.kind {
            RelayErrorKind::InvalidRequestError => "invalid_request_error",
            RelayErrorKind::AuthenticationError => "authentication_error",
            RelayErrorKind::PairingLocked => "pairing_locked",
            RelayErrorKind::ModelNotAllowed => "model_not_allowed",
            RelayErrorKind::RelayCapExceeded => "relay_cap_exceeded",
            RelayErrorKind::NotFoundError => "not_found_error",
            RelayErrorKind::UpstreamUnavailable => "upstream_unavailable",
            RelayErrorKind::RelayError => "relay_error",
        }
    }

    /// HTTP status code.
    pub fn status(&self) -> u16 {
        self.status_override.unwrap_or_else(|| self.kind.status())
    }

    /// JSON body in Anthropic's error envelope shape.
    pub fn body(&self) -> Vec<u8> {
        serde_json::to_vec(&ErrorEnvelope::relay(self.kind, self.message.clone())).unwrap_or_else(
            |_| br#"{"type":"error","error":{"type":"relay_error","message":"error"}}"#.to_vec(),
        )
    }

    /// 405 for a known path with the wrong method.
    pub fn method_not_allowed() -> Self {
        Self {
            status_override: Some(405),
            ..Self::new(RelayErrorKind::InvalidRequestError, "method not allowed")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routing_table() {
        assert_eq!(Route::parse("GET", "/healthz"), Route::Health);
        assert_eq!(Route::parse("post", "/v1/messages"), Route::Messages);
        assert_eq!(Route::parse("GET", "/v1/messages"), Route::MethodNotAllowed);
        assert_eq!(Route::parse("PUT", "/v1/prefs"), Route::PrefsPut);
        assert_eq!(Route::parse("GET", "/v1/prefs"), Route::PrefsGet);
        assert_eq!(Route::parse("POST", "/v1/devices/revoke"), Route::Revoke);
        assert_eq!(Route::parse("GET", "/v1/messages/"), Route::NotFound);
        assert_eq!(Route::parse("GET", "/"), Route::NotFound);
        assert_eq!(
            Route::parse("DELETE", "/v1/devices"),
            Route::MethodNotAllowed
        );
    }

    #[test]
    fn error_body_shape() {
        let e = RelayError::new(RelayErrorKind::ModelNotAllowed, "model not allowed");
        assert_eq!(e.status(), 403);
        let v: serde_json::Value = serde_json::from_slice(&e.body()).unwrap();
        assert_eq!(v["error"]["type"], "model_not_allowed");
        assert_eq!(RelayError::method_not_allowed().status(), 405);
    }

    #[test]
    fn kind_str_matches_body_type() {
        use RelayErrorKind::*;
        for k in [
            InvalidRequestError,
            AuthenticationError,
            PairingLocked,
            ModelNotAllowed,
            RelayCapExceeded,
            NotFoundError,
            UpstreamUnavailable,
            RelayError,
        ] {
            let e = super::RelayError::new(k, "m");
            let v: serde_json::Value = serde_json::from_slice(&e.body()).unwrap();
            assert_eq!(v["error"]["type"], e.kind_str());
        }
    }
}
