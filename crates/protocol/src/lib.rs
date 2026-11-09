//! Wire types shared by the Pocket Agent relay, the on-device `agent-core` and
//! the `xtask` tooling.
//!
//! Everything here is plain `serde` data. The relay's `/v1/messages` route is a
//! byte passthrough of the Anthropic Messages API, so those request/response
//! shapes are *not* defined here; see `agent-core` for them.
//!
//! Versioning: [`PROTOCOL_VERSION`] is bumped on any breaking change to the
//! relay HTTP API. The app reads it from `GET /healthz` and warns the user when
//! its own supported range does not include the relay's value.

#![forbid(unsafe_code)]

pub mod pairing;

use serde::{Deserialize, Serialize};

pub use pairing::{PairingError, PairingPayload};

/// Version of the relay HTTP API described by this crate.
pub const PROTOCOL_VERSION: u32 = 1;

/// Name reported by `GET /healthz`, so the app can tell a Pocket Agent relay
/// from some other server at the same URL.
pub const RELAY_NAME: &str = "pocket-agent-relay";

/// Maximum size in bytes of the preferences blob stored by `PUT /v1/prefs`.
pub const MAX_PREFS_BYTES: usize = 16 * 1024;

/// Response of `GET /healthz`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health {
    /// Always [`RELAY_NAME`].
    pub name: String,
    /// SemVer of the relay build (the workspace version).
    pub version: String,
    /// [`PROTOCOL_VERSION`] the relay speaks.
    pub protocol: u32,
}

/// Body of `POST /v1/pair`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairRequest {
    /// The `PAIRING_CODE` secret configured on the relay.
    pub pairing_code: String,
    /// Optional human-readable device name shown in `GET /v1/devices`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_name: Option<String>,
}

/// Response of a successful `POST /v1/pair`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairResponse {
    /// Stable id of the new device record.
    pub device_id: String,
    /// Bearer token for all authenticated routes. Shown exactly once; the relay
    /// stores only its SHA-256 hash.
    pub device_token: String,
    /// Relay health info, so the app can check compatibility right away.
    pub relay: Health,
}

/// One paired device, as listed by `GET /v1/devices`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInfo {
    /// Device id.
    pub id: String,
    /// Name given at pairing time.
    pub name: String,
    /// Unix seconds.
    pub created_at: i64,
    /// Unix seconds of the last authenticated request, if any.
    pub last_seen: Option<i64>,
    /// Whether the token has been revoked.
    pub revoked: bool,
    /// Whether this is the device making the request.
    pub current: bool,
}

/// Response of `GET /v1/devices`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceList {
    /// All devices ever paired, newest first.
    pub devices: Vec<DeviceInfo>,
}

/// Body of `POST /v1/devices/revoke`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevokeRequest {
    /// Id of the device to revoke. May be the caller's own id.
    pub device_id: String,
}

/// Token and cost totals for a period.
///
/// Money is in integer micro-USD (1 USD = 1,000,000) to keep arithmetic exact.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageTotals {
    /// Uncached input tokens.
    pub input_tokens: u64,
    /// Output tokens (including thinking).
    pub output_tokens: u64,
    /// Input tokens read from the prompt cache.
    pub cache_read_input_tokens: u64,
    /// Input tokens written to the prompt cache.
    pub cache_creation_input_tokens: u64,
    /// Estimated cost in micro-USD, from the relay's configured price table.
    pub usd_micros: u64,
    /// Number of forwarded requests.
    pub requests: u64,
}

/// Configured soft spending caps in micro-USD. `0` means "no cap".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Caps {
    /// Daily cap (UTC day).
    pub daily_usd_micros: u64,
    /// Monthly cap (UTC calendar month).
    pub monthly_usd_micros: u64,
}

/// Response of `GET /v1/usage`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageSummary {
    /// UTC day the "today" figures refer to, `YYYY-MM-DD`.
    pub day: String,
    /// All devices, today.
    pub today: UsageTotals,
    /// All devices, this month.
    pub month: UsageTotals,
    /// Calling device, today.
    pub device_today: UsageTotals,
    /// Calling device, this month.
    pub device_month: UsageTotals,
    /// Configured caps.
    pub caps: Caps,
}

/// Response of `GET /v1/prefs`; also the body of `PUT /v1/prefs`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Prefs {
    /// Opaque JSON object owned by the app (preferences memory).
    pub prefs: serde_json::Value,
    /// Unix seconds of the last write, `None` if never written.
    #[serde(default)]
    pub updated_at: Option<i64>,
}

/// Error `type` values the relay itself produces (in addition to forwarding
/// Anthropic's own errors unchanged).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayErrorKind {
    /// Malformed request (bad JSON, missing fields, body too large).
    InvalidRequestError,
    /// Missing, malformed, unknown or revoked device token; or wrong pairing code.
    AuthenticationError,
    /// Too many failed pairing attempts; rotate `PAIRING_CODE` to unlock.
    PairingLocked,
    /// Requested model is not in the relay's `MODEL_ALLOWLIST`.
    ModelNotAllowed,
    /// A daily or monthly soft spending cap has been reached (HTTP 402).
    RelayCapExceeded,
    /// Unknown route.
    NotFoundError,
    /// The relay could not reach the upstream API.
    UpstreamUnavailable,
    /// Relay misconfiguration or internal failure.
    RelayError,
}

impl RelayErrorKind {
    /// HTTP status the relay uses for this error.
    pub fn status(self) -> u16 {
        match self {
            Self::InvalidRequestError => 400,
            Self::AuthenticationError => 401,
            Self::RelayCapExceeded => 402,
            Self::ModelNotAllowed => 403,
            Self::NotFoundError => 404,
            Self::PairingLocked => 423,
            Self::RelayError => 500,
            Self::UpstreamUnavailable => 502,
        }
    }
}

/// Relay error envelope. Same shape as Anthropic API errors
/// (`{"type":"error","error":{"type":..,"message":..}}`) so clients can parse
/// both with one type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorEnvelope {
    /// Always `"error"`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Error details.
    pub error: ErrorBody,
}

/// Inner part of [`ErrorEnvelope`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    /// Machine-readable error type: a [`RelayErrorKind`] in snake_case, or an
    /// Anthropic error type when forwarded.
    #[serde(rename = "type")]
    pub kind: String,
    /// Human-readable message. Never contains secrets or request content.
    pub message: String,
}

impl ErrorEnvelope {
    /// Build a relay-originated error.
    pub fn relay(kind: RelayErrorKind, message: impl Into<String>) -> Self {
        let kind_str = serde_json::to_value(kind)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_else(|| "relay_error".to_owned());
        Self {
            kind: "error".to_owned(),
            error: ErrorBody {
                kind: kind_str,
                message: message.into(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_envelope_matches_anthropic_shape() {
        let e = ErrorEnvelope::relay(RelayErrorKind::RelayCapExceeded, "daily cap reached");
        let json = serde_json::to_value(&e).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"type":"error","error":{"type":"relay_cap_exceeded","message":"daily cap reached"}})
        );
    }

    #[test]
    fn statuses_are_distinct_from_anthropic_retryable_codes() {
        // The app retries 429/529; a relay cap must never look retryable.
        assert_eq!(RelayErrorKind::RelayCapExceeded.status(), 402);
        for k in [
            RelayErrorKind::InvalidRequestError,
            RelayErrorKind::AuthenticationError,
            RelayErrorKind::PairingLocked,
            RelayErrorKind::ModelNotAllowed,
            RelayErrorKind::RelayCapExceeded,
            RelayErrorKind::NotFoundError,
        ] {
            assert!(![429, 529].contains(&k.status()));
        }
    }

    #[test]
    fn pair_request_device_name_is_optional() {
        let r: PairRequest = serde_json::from_str(r#"{"pairing_code":"abc"}"#).unwrap();
        assert_eq!(r.device_name, None);
    }
}
