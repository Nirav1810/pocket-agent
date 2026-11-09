//! Device tokens and pairing-code checks.
//!
//! Device tokens are 256 random bits, so a single fast SHA-256 is an adequate
//! storage hash (there is nothing to brute-force) and fits the Workers Free
//! 10 ms CPU budget. Slow password hashes are only needed for low-entropy
//! secrets.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// Prefix of every device token; lets secret scanners recognise leaked tokens.
pub const TOKEN_PREFIX: &str = "pa_dt_";
/// Length of an encoded token: prefix + base64url(32 bytes).
pub const TOKEN_LEN: usize = TOKEN_PREFIX.len() + 43;

/// Encode 32 random bytes as a device token.
pub fn token_from_bytes(bytes: &[u8; 32]) -> String {
    format!("{TOKEN_PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes))
}

/// Encode random bytes as a short device id (`dev_` + base64url).
pub fn device_id_from_bytes(bytes: &[u8; 9]) -> String {
    format!("dev_{}", URL_SAFE_NO_PAD.encode(bytes))
}

/// Hex SHA-256 of a token: the only form stored in D1.
pub fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// Extract a well-formed device token from an `Authorization` header value.
pub fn parse_bearer(header: Option<&str>) -> Option<&str> {
    let header = header?.trim();
    let (scheme, token) = header.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    let well_formed = token.len() == TOKEN_LEN
        && token.starts_with(TOKEN_PREFIX)
        && token[TOKEN_PREFIX.len()..]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    well_formed.then_some(token)
}

/// Constant-time pairing-code comparison. Both sides are hashed first so the
/// comparison is fixed-length and leaks nothing about the code's length.
pub fn pairing_code_matches(given: &str, expected: &str) -> bool {
    let a = Sha256::digest(given.trim().as_bytes());
    let b = Sha256::digest(expected.trim().as_bytes());
    a.ct_eq(&b).into()
}

/// Fingerprint of the current `PAIRING_CODE`, stored in D1 to detect that the
/// secret was rotated (which clears a pairing lock). Domain-separated so it is
/// never equal to any other hash we store.
pub fn pairing_fingerprint(code: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"pocket-agent/pairing-fingerprint/v1\0");
    h.update(code.trim().as_bytes());
    hex::encode(h.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_format() {
        let t = token_from_bytes(&[7u8; 32]);
        assert_eq!(t.len(), TOKEN_LEN);
        assert!(t.starts_with("pa_dt_"));
        assert_eq!(parse_bearer(Some(&format!("Bearer {t}"))), Some(t.as_str()));
        assert_eq!(
            parse_bearer(Some(&format!("bearer   {t}  "))),
            Some(t.as_str())
        );
    }

    #[test]
    fn bearer_rejects_malformed() {
        let t = token_from_bytes(&[1u8; 32]);
        assert_eq!(parse_bearer(None), None);
        assert_eq!(parse_bearer(Some(&t)), None);
        assert_eq!(parse_bearer(Some(&format!("Basic {t}"))), None);
        assert_eq!(parse_bearer(Some("Bearer pa_dt_short")), None);
        let bad = format!("Bearer {}!", &t[..t.len() - 1]);
        assert_eq!(parse_bearer(Some(&bad)), None);
        // An Anthropic API key must never be accepted as a device token.
        assert_eq!(
            parse_bearer(Some(
                "Bearer sk-ant-api03-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
            )),
            None
        );
    }

    #[test]
    fn hash_is_stable_hex() {
        let h = hash_token("pa_dt_abc");
        assert_eq!(h.len(), 64);
        assert_eq!(h, hash_token("pa_dt_abc"));
        assert_ne!(h, hash_token("pa_dt_abd"));
    }

    #[test]
    fn pairing_compare() {
        assert!(pairing_code_matches("ABCD-EFGH", "ABCD-EFGH"));
        assert!(pairing_code_matches(" ABCD-EFGH\n", "ABCD-EFGH"));
        assert!(!pairing_code_matches("ABCD-EFGX", "ABCD-EFGH"));
        assert!(!pairing_code_matches("", "ABCD-EFGH"));
    }

    #[test]
    fn fingerprint_is_domain_separated() {
        assert_ne!(pairing_fingerprint("x"), hash_token("x"));
        assert_eq!(pairing_fingerprint("x"), pairing_fingerprint(" x "));
    }

    #[test]
    fn device_id_format() {
        assert_eq!(device_id_from_bytes(&[0u8; 9]), "dev_AAAAAAAAAAAA");
    }
}
