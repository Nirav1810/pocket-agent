//! Small helpers shared by xtask commands.

use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};

/// Workspace root (parent of the `xtask` crate).
pub fn root() -> PathBuf {
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    here.parent().map(PathBuf::from).unwrap_or(here)
}

/// Relay crate directory.
pub fn relay_dir() -> PathBuf {
    root().join("crates").join("relay")
}

/// Ask a question on the terminal; empty answer returns `default`.
pub fn prompt(question: &str, default: &str) -> Result<String> {
    if default.is_empty() {
        print!("{question}: ");
    } else {
        print!("{question} [{default}]: ");
    }
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    let line = line.trim();
    Ok(if line.is_empty() {
        default.to_owned()
    } else {
        line.to_owned()
    })
}

/// Yes/no question.
pub fn confirm(question: &str, default_yes: bool) -> Result<bool> {
    let ans = prompt(question, if default_yes { "Y/n" } else { "y/N" })?;
    Ok(match ans.to_ascii_lowercase().as_str() {
        "y" | "yes" => true,
        "n" | "no" => false,
        _ => default_yes,
    })
}

/// Read a secret without echo.
pub fn prompt_secret(question: &str) -> Result<String> {
    let s = rpassword::prompt_password(question).context("could not read from terminal")?;
    let s = s.trim().to_owned();
    if s.is_empty() {
        bail!("no value entered");
    }
    Ok(s)
}

/// Generate a pairing code: 20 Crockford base32 characters in groups of five
/// (100 bits of entropy), e.g. `7KQ2M-9XAPL-3RW8D-V0T5H`.
pub fn generate_pairing_code() -> Result<String> {
    const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let mut bytes = [0u8; 20];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("no OS randomness: {e}"))?;
    let mut out = String::with_capacity(23);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && i % 5 == 0 {
            out.push('-');
        }
        out.push(char::from(ALPHABET[usize::from(b & 31)]));
    }
    Ok(out)
}

/// HTTP agent that returns non-2xx responses instead of erroring.
pub fn http() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(60)))
        .build()
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_code_shape() {
        let c = generate_pairing_code().unwrap();
        assert_eq!(c.len(), 23);
        assert_eq!(c.matches('-').count(), 3);
        assert!(
            c.chars()
                .all(|ch| ch == '-' || ch.is_ascii_digit() || ch.is_ascii_uppercase())
        );
        assert_ne!(c, generate_pairing_code().unwrap());
    }
}
