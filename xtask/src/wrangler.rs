//! Running the pinned `wrangler` CLI through `npx`.
//!
//! Node/wrangler is a dev-time tool only; nothing it produces is committed.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};

use anyhow::{Context, Result, bail};

/// Pinned wrangler version (bump deliberately, after checking release notes).
pub const WRANGLER_VERSION: &str = "4.152.0";

fn npx() -> &'static str {
    if cfg!(windows) { "npx.cmd" } else { "npx" }
}

/// Base command: `npx --yes wrangler@<pinned> <args>` run in `dir`.
pub fn command(dir: &Path, args: &[&str]) -> Command {
    let mut c = Command::new(npx());
    c.arg("--yes")
        .arg(format!("wrangler@{WRANGLER_VERSION}"))
        .args(args)
        .current_dir(dir)
        // No anonymous usage metrics from our tooling.
        .env("WRANGLER_SEND_METRICS", "false");
    c
}

/// Run interactively (inherits the terminal). Errors on non-zero exit.
pub fn run(dir: &Path, args: &[&str]) -> Result<()> {
    let status = command(dir, args)
        .status()
        .with_context(|| "could not start `npx`; install Node.js 20+ from https://nodejs.org")?;
    if !status.success() {
        bail!("`wrangler {}` failed ({status})", args.join(" "));
    }
    Ok(())
}

/// Run and capture stdout (stderr still shown). Errors on non-zero exit.
pub fn output(dir: &Path, args: &[&str]) -> Result<String> {
    let out = command(dir, args)
        .stderr(Stdio::inherit())
        .output()
        .with_context(|| "could not start `npx`; install Node.js 20+ from https://nodejs.org")?;
    if !out.status.success() {
        bail!("`wrangler {}` failed ({})", args.join(" "), out.status);
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Run while both showing output live (so interactive prompts are visible)
/// and capturing it for parsing. stdin stays attached to the terminal.
pub fn run_tee(dir: &Path, args: &[&str]) -> Result<String> {
    let mut child = command(dir, args)
        .stdout(Stdio::piped())
        .spawn()
        .with_context(|| "could not start `npx`; install Node.js 20+ from https://nodejs.org")?;
    let mut captured = Vec::new();
    if let Some(mut out) = child.stdout.take() {
        let mut buf = [0u8; 4096];
        let mut stdout = std::io::stdout();
        loop {
            let n = out.read(&mut buf)?;
            if n == 0 {
                break;
            }
            stdout.write_all(&buf[..n])?;
            stdout.flush()?;
            captured.extend_from_slice(&buf[..n]);
        }
    }
    let status = child.wait()?;
    if !status.success() {
        bail!("`wrangler {}` failed ({status})", args.join(" "));
    }
    Ok(String::from_utf8_lossy(&captured).into_owned())
}

/// `wrangler secret put NAME`, with the value written to stdin (never argv,
/// never disk).
pub fn secret_put(dir: &Path, name: &str, value: &str) -> Result<()> {
    let mut child = command(dir, &["secret", "put", name])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .context("could not start wrangler")?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(value.as_bytes())?;
        // Dropping stdin closes it so wrangler sees EOF.
    }
    let status = child.wait()?;
    if !status.success() {
        bail!("`wrangler secret put {name}` failed ({status})");
    }
    Ok(())
}

/// Kill a spawned wrangler and its whole process tree (`npx` → `node` → `workerd`).
pub fn kill_tree(child: &mut Child) {
    if cfg!(windows) {
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &child.id().to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Extract the first `https://….workers.dev` URL from wrangler output.
pub fn find_workers_dev_url(text: &str) -> Option<String> {
    text.split(|c: char| c.is_whitespace() || c == '"' || c == '\'')
        .map(|w| w.trim_end_matches(['.', ',', ')']))
        .find(|w| w.starts_with("https://") && w.ends_with(".workers.dev"))
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_deploy_url() {
        let out = "Uploaded pocket-agent-relay (3.1 sec)\nDeployed pocket-agent-relay triggers (0.4 sec)\n  https://pocket-agent-relay.alice.workers.dev\nCurrent Version ID: x";
        assert_eq!(
            find_workers_dev_url(out).as_deref(),
            Some("https://pocket-agent-relay.alice.workers.dev")
        );
        assert_eq!(find_workers_dev_url("nothing here"), None);
    }
}
