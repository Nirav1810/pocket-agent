//! HTTP client for the Claude Messages API, in relay or direct mode.
//!
//! The two modes differ only in base URL and auth header:
//!
//! | Mode | Base URL | Auth |
//! |---|---|---|
//! | Relay | user's relay (`https://….workers.dev`) | `Authorization: Bearer pa_dt_…` |
//! | Direct | `https://api.anthropic.com` | `x-api-key: sk-ant-…` |
//!
//! Network policy: the client only ever contacts its configured base URL
//! (redirects are disabled, so the host allowlist is exactly that one host).
//! TLS uses rustls with the platform verifier (Android's trust store).
//! Requests are blocking; the caller runs them off the main thread.

use std::sync::Once;
use std::thread::sleep;
use std::time::Duration;

use pocket_agent_protocol::pairing::normalize_relay_url;
use pocket_agent_protocol::{ErrorEnvelope, Health, PairRequest, PairResponse, RELAY_NAME};
use secrecy::{ExposeSecret, SecretString};
use thiserror::Error;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};

use crate::api::{ANTHROPIC_VERSION, MessagesResponse};
use crate::types::Credentials;

/// The only direct-mode host.
pub const ANTHROPIC_BASE: &str = "https://api.anthropic.com";
/// Header the relay sets on errors it generated itself (vs. forwarded from Anthropic).
pub const RELAY_ERROR_HEADER: &str = "x-pocket-relay-error";

const MAX_RETRIES: u32 = 2;
const MAX_RESPONSE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_RETRY_AFTER_SECS: f64 = 30.0;

/// Errors talking to Claude (directly or through the relay).
#[derive(Debug, Error, Clone, PartialEq)]
pub enum ApiError {
    /// No credentials configured.
    #[error("not configured")]
    NotConfigured,
    /// Credentials malformed (bad relay URL etc.).
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    /// 401/403 from the relay: device token unknown or revoked → re-pair.
    #[error("device not authorised by relay")]
    DeviceUnauthorized,
    /// 401/403 from Anthropic: API key invalid (direct: replace key; relay:
    /// the relay's `ANTHROPIC_API_KEY` secret is wrong).
    #[error("API key rejected (via relay: {via_relay})")]
    KeyRejected {
        /// Whether the request went through the relay.
        via_relay: bool,
    },
    /// Relay daily/monthly cap reached (HTTP 402).
    #[error("relay spending cap reached")]
    RelayCap,
    /// Relay pairing locked.
    #[error("relay pairing locked")]
    PairingLocked,
    /// Relay does not allow this model.
    #[error("model not allowed by relay: {0}")]
    ModelNotAllowed(String),
    /// 429.
    #[error("rate limited")]
    RateLimited {
        /// Seconds to wait, from `retry-after`.
        retry_after: Option<f64>,
    },
    /// 529.
    #[error("overloaded")]
    Overloaded {
        /// Seconds to wait, from `retry-after`.
        retry_after: Option<f64>,
    },
    /// 5xx (or relay could not reach Anthropic).
    #[error("server error {0}")]
    Server(u16),
    /// 400/404/413 etc.: our request is wrong. Not retried.
    #[error("bad request ({status}): {message}")]
    BadRequest {
        /// HTTP status.
        status: u16,
        /// Error message from the server.
        message: String,
    },
    /// Timed out.
    #[error("timed out")]
    Timeout,
    /// No connection (DNS, refused, no network).
    #[error("offline: {0}")]
    Offline(String),
    /// Response could not be parsed.
    #[error("unexpected response: {0}")]
    Decode(String),
    /// URL is not a Pocket Agent relay.
    #[error("not a Pocket Agent relay")]
    NotARelay,
    /// User pressed STOP while waiting.
    #[error("stopped")]
    Stopped,
}

impl ApiError {
    /// Worth retrying automatically.
    pub fn retryable(&self) -> bool {
        matches!(
            self,
            Self::RateLimited { .. }
                | Self::Overloaded { .. }
                | Self::Server(_)
                | Self::Timeout
                | Self::Offline(_)
        )
    }

    fn retry_after(&self) -> Option<f64> {
        match self {
            Self::RateLimited { retry_after } | Self::Overloaded { retry_after } => *retry_after,
            _ => None,
        }
    }
}

enum Auth {
    Bearer(SecretString),
    ApiKey(SecretString),
}

/// Messages API client for one command. Created from fresh credentials and
/// dropped afterwards, so secrets live only for the duration of a command.
pub struct ApiClient {
    http: ureq::Agent,
    base: String,
    auth: Auth,
}

impl std::fmt::Debug for ApiClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiClient")
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

static CRYPTO_INIT: Once = Once::new();

fn http_agent(timeout: Duration) -> ureq::Agent {
    CRYPTO_INIT.call_once(|| {
        // Ignore the error if another component already installed a provider.
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
    ureq::Agent::config_builder()
        .tls_config(
            TlsConfig::builder()
                .provider(TlsProvider::Rustls)
                .root_certs(RootCerts::PlatformVerifier)
                .build(),
        )
        // Never follow redirects: the configured host is the only host we talk to.
        .max_redirects(0)
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_global(Some(timeout))
        .user_agent(concat!("pocket-agent/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

impl ApiClient {
    /// Build a client from the phone's current credentials.
    pub fn from_credentials(creds: Credentials) -> Result<Self, ApiError> {
        match creds {
            Credentials::None => Err(ApiError::NotConfigured),
            Credentials::Relay {
                base_url,
                device_token,
            } => {
                let base = normalize_relay_url(&base_url)
                    .map_err(|e| ApiError::InvalidConfig(e.to_string()))?;
                if device_token.trim().is_empty() {
                    return Err(ApiError::NotConfigured);
                }
                Ok(Self::new(
                    base,
                    Auth::Bearer(SecretString::from(device_token)),
                ))
            }
            Credentials::Direct { api_key } => {
                if api_key.trim().is_empty() {
                    return Err(ApiError::NotConfigured);
                }
                Ok(Self::new(
                    ANTHROPIC_BASE.to_owned(),
                    Auth::ApiKey(SecretString::from(api_key)),
                ))
            }
        }
    }

    fn new(base: String, auth: Auth) -> Self {
        Self {
            http: http_agent(Duration::from_secs(90)),
            base,
            auth,
        }
    }

    /// Direct-mode client against an arbitrary base URL (tests only).
    #[cfg(test)]
    pub(crate) fn direct_for_tests(base: &str, key: &str) -> Self {
        Self::new(
            base.to_owned(),
            Auth::ApiKey(SecretString::from(key.to_owned())),
        )
    }

    /// Whether requests go through a relay.
    pub fn via_relay(&self) -> bool {
        matches!(self.auth, Auth::Bearer(_))
    }

    /// Host this client talks to (for diagnostics).
    pub fn host(&self) -> &str {
        pocket_agent_protocol::pairing::relay_host(&self.base).unwrap_or(&self.base)
    }

    /// `POST /v1/messages` with retries for transient errors (429, 529, 5xx,
    /// timeouts, connection failures), honouring `retry-after`.
    pub fn messages(
        &self,
        body: &[u8],
        betas: &[&str],
        is_stopped: &dyn Fn() -> bool,
    ) -> Result<MessagesResponse, ApiError> {
        let mut attempt = 0;
        loop {
            if is_stopped() {
                return Err(ApiError::Stopped);
            }
            match self.post_once(body, betas) {
                Ok(r) => return Ok(r),
                Err(e) if e.retryable() && attempt < MAX_RETRIES => {
                    let wait = backoff(attempt, e.retry_after());
                    match wait {
                        Some(wait) => {
                            tracing::debug!(error = %e, attempt, wait_ms = wait.as_millis() as u64, "retrying");
                            sleep_unless_stopped(wait, is_stopped)?;
                            attempt += 1;
                        }
                        None => return Err(e),
                    }
                }
                Err(e) => return Err(e),
            }
        }
    }

    fn post_once(&self, body: &[u8], betas: &[&str]) -> Result<MessagesResponse, ApiError> {
        let url = format!("{}/v1/messages", self.base);
        let mut req = ureq::http::Request::builder()
            .method("POST")
            .uri(&url)
            .header("content-type", "application/json")
            .header("anthropic-version", ANTHROPIC_VERSION);
        req = match &self.auth {
            Auth::Bearer(t) => req.header("authorization", format!("Bearer {}", t.expose_secret())),
            Auth::ApiKey(k) => req.header("x-api-key", k.expose_secret()),
        };
        if !betas.is_empty() {
            req = req.header("anthropic-beta", betas.join(","));
        }
        let req = req
            .body(body.to_vec())
            .map_err(|e| ApiError::InvalidConfig(e.to_string()))?;
        let mut resp = self.http.run(req).map_err(map_transport)?;
        let status = resp.status().as_u16();
        let header = |name: &str| {
            resp.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        let relay_err = header(RELAY_ERROR_HEADER);
        let retry_after = header("retry-after").and_then(|v| v.trim().parse::<f64>().ok());
        let should_retry = header("x-should-retry");
        let bytes = resp
            .body_mut()
            .with_config()
            .limit(MAX_RESPONSE_BYTES)
            .read_to_vec()
            .map_err(map_transport)?;
        if status == 200 {
            return serde_json::from_slice(&bytes).map_err(|e| ApiError::Decode(e.to_string()));
        }
        let err = classify_error(
            status,
            &bytes,
            relay_err.as_deref(),
            retry_after,
            self.via_relay(),
        );
        if should_retry.as_deref() == Some("false") && err.retryable() {
            // Keep the category (so the user hears "busy", not "bug") but stop retrying.
            return Err(match err {
                ApiError::RateLimited { .. } => ApiError::RateLimited {
                    retry_after: Some(f64::INFINITY),
                },
                ApiError::Overloaded { .. } => ApiError::Overloaded {
                    retry_after: Some(f64::INFINITY),
                },
                other => ApiError::BadRequest {
                    status,
                    message: format!("{other}; server asked not to retry"),
                },
            });
        }
        Err(err)
    }
}

/// Map an error response to [`ApiError`].
pub fn classify_error(
    status: u16,
    body: &[u8],
    relay_error: Option<&str>,
    retry_after: Option<f64>,
    via_relay: bool,
) -> ApiError {
    let envelope: Option<ErrorEnvelope> = serde_json::from_slice(body).ok();
    let kind = envelope
        .as_ref()
        .map(|e| e.error.kind.as_str())
        .unwrap_or("");
    let message = envelope
        .as_ref()
        .map(|e| e.error.message.clone())
        .unwrap_or_default();

    if let Some(relay_kind) = relay_error {
        return match relay_kind {
            "authentication_error" => ApiError::DeviceUnauthorized,
            "relay_cap_exceeded" => ApiError::RelayCap,
            "pairing_locked" => ApiError::PairingLocked,
            "model_not_allowed" => ApiError::ModelNotAllowed(message),
            "upstream_unavailable" => ApiError::Server(status),
            "relay_error" => ApiError::Server(status),
            _ => ApiError::BadRequest { status, message },
        };
    }
    match status {
        401 | 403 => ApiError::KeyRejected { via_relay },
        429 => ApiError::RateLimited { retry_after },
        529 => ApiError::Overloaded { retry_after },
        500..=599 => ApiError::Server(status),
        _ if kind == "overloaded_error" => ApiError::Overloaded { retry_after },
        _ => ApiError::BadRequest { status, message },
    }
}

fn map_transport(e: ureq::Error) -> ApiError {
    match e {
        ureq::Error::Timeout(_) => ApiError::Timeout,
        ureq::Error::HostNotFound | ureq::Error::ConnectionFailed => {
            ApiError::Offline(e.to_string())
        }
        ureq::Error::Io(io) => match io.kind() {
            std::io::ErrorKind::TimedOut => ApiError::Timeout,
            _ => ApiError::Offline(io.to_string()),
        },
        ureq::Error::BodyExceedsLimit(_) => ApiError::Decode("response too large".into()),
        other => ApiError::Offline(other.to_string()),
    }
}

/// Delay before retry `attempt` (0-based). `None` = do not retry (server asked
/// for longer than we are willing to keep the user waiting).
fn backoff(attempt: u32, retry_after: Option<f64>) -> Option<Duration> {
    if let Some(secs) = retry_after {
        return (secs.is_finite() && (0.0..=MAX_RETRY_AFTER_SECS).contains(&secs))
            .then(|| Duration::from_secs_f64(secs));
    }
    let base_ms = 500u64 << attempt.min(4); // 0.5 s, 1 s, 2 s …
    let jitter = getrandom::u32().unwrap_or(0) % 1000; // 0..1 s spread
    Some(Duration::from_millis(
        base_ms.min(8_000) + u64::from(jitter) * base_ms / 2000,
    ))
}

fn sleep_unless_stopped(total: Duration, is_stopped: &dyn Fn() -> bool) -> Result<(), ApiError> {
    let step = Duration::from_millis(100);
    let mut left = total;
    while !left.is_zero() {
        if is_stopped() {
            return Err(ApiError::Stopped);
        }
        let s = left.min(step);
        sleep(s);
        left -= s;
    }
    Ok(())
}

// ------------------------------------------------------------- relay admin --

fn relay_agent() -> ureq::Agent {
    http_agent(Duration::from_secs(20))
}

/// `GET /healthz` on a relay.
pub fn relay_health(url: &str) -> Result<Health, ApiError> {
    let base = normalize_relay_url(url).map_err(|e| ApiError::InvalidConfig(e.to_string()))?;
    let mut r = relay_agent()
        .get(format!("{base}/healthz"))
        .call()
        .map_err(map_transport)?;
    if r.status().as_u16() != 200 {
        return Err(ApiError::NotARelay);
    }
    let bytes = r
        .body_mut()
        .with_config()
        .limit(64 * 1024)
        .read_to_vec()
        .map_err(map_transport)?;
    let h: Health = serde_json::from_slice(&bytes).map_err(|_| ApiError::NotARelay)?;
    if h.name != RELAY_NAME {
        return Err(ApiError::NotARelay);
    }
    Ok(h)
}

/// `POST /v1/pair`: exchange the pairing code for a device token.
pub fn relay_pair(url: &str, code: &str, device_name: &str) -> Result<PairResponse, ApiError> {
    let base = normalize_relay_url(url).map_err(|e| ApiError::InvalidConfig(e.to_string()))?;
    let body = serde_json::to_vec(&PairRequest {
        pairing_code: code.to_owned(),
        device_name: Some(device_name.to_owned()),
    })
    .map_err(|e| ApiError::Decode(e.to_string()))?;
    let req = ureq::http::Request::builder()
        .method("POST")
        .uri(format!("{base}/v1/pair"))
        .header("content-type", "application/json")
        .body(body)
        .map_err(|e| ApiError::InvalidConfig(e.to_string()))?;
    let mut r = relay_agent().run(req).map_err(map_transport)?;
    let status = r.status().as_u16();
    let relay_err = r
        .headers()
        .get(RELAY_ERROR_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let bytes = r.body_mut().read_to_vec().map_err(map_transport)?;
    if status == 200 {
        return serde_json::from_slice(&bytes).map_err(|_| ApiError::NotARelay);
    }
    Err(match (status, relay_err.as_deref()) {
        (401, _) => ApiError::KeyRejected { via_relay: true }, // wrong pairing code
        (_, Some(kind)) => classify_error(status, &bytes, Some(kind), None, true),
        (404, None) => ApiError::NotARelay,
        _ => ApiError::Server(status),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, header_exists, headers, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const OK: &str = r#"{"id":"msg_1","model":"claude-haiku-5-5","content":[{"type":"text","text":"hi"}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}"#;

    fn never() -> bool {
        false
    }

    async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        tokio::task::spawn_blocking(f).await.unwrap()
    }

    #[tokio::test]
    async fn relay_mode_sends_bearer_and_version() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .and(header("authorization", "Bearer pa_dt_test"))
            .and(header("anthropic-version", ANTHROPIC_VERSION))
            .and(headers("anthropic-beta", vec!["b1", "b2"]))
            .respond_with(ResponseTemplate::new(200).set_body_string(OK))
            .expect(1)
            .mount(&server)
            .await;
        let base = server.uri();
        let r = blocking(move || {
            let c = ApiClient::from_credentials(Credentials::Relay {
                base_url: base,
                device_token: "pa_dt_test".into(),
            })
            .unwrap();
            assert!(c.via_relay());
            c.messages(b"{}", &["b1", "b2"], &never)
        })
        .await
        .unwrap();
        assert_eq!(r.text(), "hi");
    }

    #[tokio::test]
    async fn direct_mode_sends_api_key_not_bearer() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(header("x-api-key", "sk-ant-test"))
            .respond_with(ResponseTemplate::new(200).set_body_string(OK))
            .expect(1)
            .mount(&server)
            .await;
        let base = server.uri();
        blocking(move || {
            ApiClient::direct_for_tests(&base, "sk-ant-test").messages(b"{}", &[], &never)
        })
        .await
        .unwrap();
        let reqs = server.received_requests().await.unwrap();
        assert!(reqs[0].headers.get("authorization").is_none());
        assert!(reqs[0].headers.get("anthropic-beta").is_none());
    }

    #[tokio::test]
    async fn retries_429_honouring_retry_after_then_succeeds() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(429)
                    .insert_header("retry-after", "0")
                    .set_body_string(
                        r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow"}}"#,
                    ),
            )
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string(OK))
            .mount(&server)
            .await;
        let base = server.uri();
        let r =
            blocking(move || ApiClient::direct_for_tests(&base, "k").messages(b"{}", &[], &never))
                .await;
        assert!(r.is_ok());
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn gives_up_after_max_retries_on_529() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(529)
                    .insert_header("retry-after", "0")
                    .set_body_string(
                        r#"{"type":"error","error":{"type":"overloaded_error","message":"busy"}}"#,
                    ),
            )
            .mount(&server)
            .await;
        let base = server.uri();
        let r =
            blocking(move || ApiClient::direct_for_tests(&base, "k").messages(b"{}", &[], &never))
                .await;
        assert!(matches!(r, Err(ApiError::Overloaded { .. })));
        assert_eq!(server.received_requests().await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn does_not_retry_bad_request_or_long_retry_after() {
        let server = MockServer::start().await;
        Mock::given(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(400).set_body_string(
                r#"{"type":"error","error":{"type":"invalid_request_error","message":"bad field"}}"#,
            ))
            .mount(&server)
            .await;
        let base = server.uri();
        let r =
            blocking(move || ApiClient::direct_for_tests(&base, "k").messages(b"{}", &[], &never))
                .await;
        assert_eq!(
            r.unwrap_err(),
            ApiError::BadRequest {
                status: 400,
                message: "bad field".into()
            }
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn relay_errors_are_distinguished_from_anthropic_errors() {
        let server = MockServer::start().await;
        Mock::given(header_exists("authorization"))
            .respond_with(
                ResponseTemplate::new(401)
                    .insert_header(RELAY_ERROR_HEADER, "authentication_error")
                    .set_body_string(r#"{"type":"error","error":{"type":"authentication_error","message":"revoked"}}"#),
            )
            .mount(&server)
            .await;
        let base = server.uri();
        let r = blocking(move || {
            ApiClient::from_credentials(Credentials::Relay {
                base_url: base,
                device_token: "pa_dt_x".into(),
            })
            .unwrap()
            .messages(b"{}", &[], &never)
        })
        .await;
        assert_eq!(r.unwrap_err(), ApiError::DeviceUnauthorized);
    }

    #[tokio::test]
    async fn stop_interrupts_backoff() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(529).insert_header("retry-after", "20"))
            .mount(&server)
            .await;
        let base = server.uri();
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let c2 = calls.clone();
        let started = std::time::Instant::now();
        let r = blocking(move || {
            ApiClient::direct_for_tests(&base, "k").messages(b"{}", &[], &move || {
                c2.fetch_add(1, std::sync::atomic::Ordering::SeqCst) > 2
            })
        })
        .await;
        assert_eq!(r.unwrap_err(), ApiError::Stopped);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn classification_table() {
        let e = |s, relay: Option<&str>| classify_error(s, b"{}", relay, None, true);
        assert_eq!(e(402, Some("relay_cap_exceeded")), ApiError::RelayCap);
        assert_eq!(e(423, Some("pairing_locked")), ApiError::PairingLocked);
        assert_eq!(e(401, None), ApiError::KeyRejected { via_relay: true });
        assert_eq!(e(502, Some("upstream_unavailable")), ApiError::Server(502));
        assert_eq!(e(500, None), ApiError::Server(500));
        assert!(matches!(
            e(403, Some("model_not_allowed")),
            ApiError::ModelNotAllowed(_)
        ));
    }

    #[test]
    fn backoff_rules() {
        assert_eq!(backoff(0, Some(2.0)), Some(Duration::from_secs(2)));
        assert_eq!(backoff(0, Some(120.0)), None);
        let d = backoff(1, None).unwrap();
        assert!(d >= Duration::from_millis(1000) && d < Duration::from_millis(1600));
    }

    #[test]
    fn credentials_validation() {
        assert_eq!(
            ApiClient::from_credentials(Credentials::None).unwrap_err(),
            ApiError::NotConfigured
        );
        assert!(matches!(
            ApiClient::from_credentials(Credentials::Relay {
                base_url: "http://evil.example".into(),
                device_token: "t".into()
            }),
            Err(ApiError::InvalidConfig(_))
        ));
        let c = ApiClient::from_credentials(Credentials::Direct {
            api_key: "sk-ant-x".into(),
        })
        .unwrap();
        assert_eq!(c.host(), "api.anthropic.com");
        assert!(!format!("{c:?}").contains("sk-ant"));
    }

    #[tokio::test]
    async fn pairing_flow() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/pair"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"device_id":"dev_1","device_token":"pa_dt_abc","relay":{"name":"pocket-agent-relay","version":"0.1.0","protocol":1}}"#,
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/healthz"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(
                    r#"{"name":"pocket-agent-relay","version":"0.1.0","protocol":1}"#,
                ),
            )
            .mount(&server)
            .await;
        let base = server.uri();
        let (h, p) =
            blocking(move || (relay_health(&base), relay_pair(&base, "CODE", "phone"))).await;
        assert_eq!(h.unwrap().protocol, 1);
        assert_eq!(p.unwrap().device_token, "pa_dt_abc");
    }
}
