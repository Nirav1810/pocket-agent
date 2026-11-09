//! Pairing payload carried by the QR code that `cargo xtask relay-setup` prints.
//!
//! Format (version 1):
//!
//! ```text
//! pocketagent://pair?v=1&url=<percent-encoded relay URL>&code=<percent-encoded pairing code>
//! ```
//!
//! The app scans it, calls `POST /v1/pair` immediately and keeps only the
//! returned device token. The pairing code is never stored on the phone.

use thiserror::Error;

const SCHEME_PREFIX: &str = "pocketagent://pair?";
const PAYLOAD_VERSION: &str = "1";
const MAX_CODE_LEN: usize = 128;

/// Relay URL + pairing code, as exchanged during pairing.
#[derive(Clone, PartialEq, Eq)]
pub struct PairingPayload {
    /// Relay base URL, e.g. `https://pocket-agent-relay.example.workers.dev`.
    /// No trailing slash.
    pub url: String,
    /// The relay's `PAIRING_CODE`.
    pub code: String,
}

// Never print the pairing code in debug output.
impl std::fmt::Debug for PairingPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PairingPayload")
            .field("url", &self.url)
            .field("code", &"<redacted>")
            .finish()
    }
}

/// Why a pairing payload or relay URL was rejected.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PairingError {
    /// Not a `pocketagent://pair?` URI.
    #[error("not a Pocket Agent pairing code")]
    WrongScheme,
    /// Payload version this build does not understand.
    #[error("unsupported pairing payload version {0:?}; update the app")]
    UnsupportedVersion(String),
    /// Required field missing.
    #[error("pairing payload is missing `{0}`")]
    Missing(&'static str),
    /// Percent-encoding was invalid.
    #[error("pairing payload is not correctly encoded")]
    BadEncoding,
    /// URL is not `https://` (plain `http://` is only allowed for localhost).
    #[error("relay URL must start with https://")]
    InsecureUrl,
    /// URL is syntactically unusable.
    #[error("relay URL is not valid")]
    BadUrl,
    /// Code is empty or too long.
    #[error("pairing code must be 1..={MAX_CODE_LEN} characters")]
    BadCode,
}

impl PairingPayload {
    /// Validate and build a payload. The URL is normalised (trailing `/` removed).
    pub fn new(url: &str, code: &str) -> Result<Self, PairingError> {
        let url = normalize_relay_url(url)?;
        let code = code.trim();
        if code.is_empty() || code.len() > MAX_CODE_LEN || code.chars().any(char::is_control) {
            return Err(PairingError::BadCode);
        }
        Ok(Self {
            url,
            code: code.to_owned(),
        })
    }

    /// Encode as a `pocketagent://pair?...` URI for a QR code.
    pub fn to_uri(&self) -> String {
        format!(
            "{SCHEME_PREFIX}v={PAYLOAD_VERSION}&url={}&code={}",
            percent_encode(&self.url),
            percent_encode(&self.code)
        )
    }

    /// Parse a scanned URI.
    pub fn parse(uri: &str) -> Result<Self, PairingError> {
        let query = uri
            .trim()
            .strip_prefix(SCHEME_PREFIX)
            .ok_or(PairingError::WrongScheme)?;
        let (mut version, mut url, mut code) = (None, None, None);
        for pair in query.split('&') {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            let v = percent_decode(v)?;
            match k {
                "v" => version = Some(v),
                "url" => url = Some(v),
                "code" => code = Some(v),
                _ => {} // ignore unknown keys for forward compatibility
            }
        }
        let version = version.ok_or(PairingError::Missing("v"))?;
        if version != PAYLOAD_VERSION {
            return Err(PairingError::UnsupportedVersion(version));
        }
        Self::new(
            &url.ok_or(PairingError::Missing("url"))?,
            &code.ok_or(PairingError::Missing("code"))?,
        )
    }
}

/// Validate a relay base URL: `https://host[:port][/path]`, or `http://` for
/// `localhost` / `127.0.0.1` (local development only). Returns it without a
/// trailing slash.
pub fn normalize_relay_url(url: &str) -> Result<String, PairingError> {
    let url = url.trim().trim_end_matches('/');
    let rest = if let Some(rest) = url.strip_prefix("https://") {
        rest
    } else if let Some(rest) = url.strip_prefix("http://") {
        let host = rest.split(['/', ':']).next().unwrap_or("");
        if host != "localhost" && host != "127.0.0.1" {
            return Err(PairingError::InsecureUrl);
        }
        rest
    } else {
        return Err(PairingError::InsecureUrl);
    };
    let host = rest.split(['/', ':', '?', '#']).next().unwrap_or("");
    let host_ok = !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    if !host_ok || rest.contains(['?', '#', '@', ' ']) {
        return Err(PairingError::BadUrl);
    }
    Ok(url.to_owned())
}

/// Host part of a validated relay URL (no port).
pub fn relay_host(url: &str) -> Option<&str> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    rest.split(['/', ':']).next().filter(|h| !h.is_empty())
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn percent_decode(s: &str) -> Result<String, PairingError> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = bytes.get(i + 1..i + 3).ok_or(PairingError::BadEncoding)?;
                let hex = std::str::from_utf8(hex).map_err(|_| PairingError::BadEncoding)?;
                out.push(u8::from_str_radix(hex, 16).map_err(|_| PairingError::BadEncoding)?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_| PairingError::BadEncoding)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let p = PairingPayload::new("https://my-relay.abc.workers.dev/", "K7Q2-M9XA-PL3R").unwrap();
        assert_eq!(p.url, "https://my-relay.abc.workers.dev");
        let uri = p.to_uri();
        assert!(uri.starts_with("pocketagent://pair?v=1&url=https%3A%2F%2Fmy-relay"));
        assert_eq!(PairingPayload::parse(&uri).unwrap(), p);
    }

    #[test]
    fn special_characters_survive() {
        let p = PairingPayload::new("https://r.example.com/base", "a&b=c%d +é").unwrap();
        assert_eq!(PairingPayload::parse(&p.to_uri()).unwrap(), p);
    }

    #[test]
    fn rejects_http_except_localhost() {
        assert_eq!(
            PairingPayload::new("http://evil.example.com", "x").unwrap_err(),
            PairingError::InsecureUrl
        );
        assert!(PairingPayload::new("http://127.0.0.1:8787", "x").is_ok());
        assert!(PairingPayload::new("http://localhost:8787", "x").is_ok());
        assert_eq!(
            PairingPayload::new("ftp://x.example", "x").unwrap_err(),
            PairingError::InsecureUrl
        );
    }

    #[test]
    fn rejects_userinfo_and_queries() {
        assert_eq!(
            normalize_relay_url("https://user@evil.example").unwrap_err(),
            PairingError::BadUrl
        );
        assert_eq!(
            normalize_relay_url("https://a.example?x=1").unwrap_err(),
            PairingError::BadUrl
        );
        assert!(normalize_relay_url("https://").is_err());
        assert_eq!(
            normalize_relay_url("https:///x").unwrap_err(),
            PairingError::BadUrl
        );
    }

    #[test]
    fn rejects_bad_codes_and_versions() {
        assert_eq!(
            PairingPayload::new("https://a.example", "  ").unwrap_err(),
            PairingError::BadCode
        );
        assert_eq!(
            PairingPayload::parse("pocketagent://pair?v=2&url=x&code=y").unwrap_err(),
            PairingError::UnsupportedVersion("2".into())
        );
        assert_eq!(
            PairingPayload::parse("https://example.com").unwrap_err(),
            PairingError::WrongScheme
        );
        assert_eq!(
            PairingPayload::parse("pocketagent://pair?v=1&url=https%3A%2F%2Fa.example")
                .unwrap_err(),
            PairingError::Missing("code")
        );
        assert_eq!(
            PairingPayload::parse("pocketagent://pair?v=1&url=%ZZ&code=y").unwrap_err(),
            PairingError::BadEncoding
        );
    }

    #[test]
    fn debug_redacts_code() {
        let p = PairingPayload::new("https://a.example", "supersecret").unwrap();
        assert!(!format!("{p:?}").contains("supersecret"));
    }

    #[test]
    fn host_extraction() {
        assert_eq!(
            relay_host("https://a.b.workers.dev"),
            Some("a.b.workers.dev")
        );
        assert_eq!(relay_host("http://127.0.0.1:8787/x"), Some("127.0.0.1"));
    }
}
