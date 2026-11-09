//! `relay-setup`, `relay-deploy`, `relay-rotate-code`.

use std::fs;
use std::thread::sleep;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use pocket_agent_protocol::{Health, RELAY_NAME};
use zeroize::Zeroizing;

use crate::util::{self, relay_dir};
use crate::{check, qr, wrangler};

/// Interactive first-time setup. Safe to re-run: reuses an existing D1
/// database and overwrites secrets.
pub fn relay_setup(default_name: &str) -> Result<()> {
    let dir = relay_dir();
    println!("Pocket Agent relay setup");
    println!("========================");
    println!("This deploys YOUR OWN relay to YOUR Cloudflare account (Workers Free plan).");
    println!("Your Anthropic API key is stored only as a Cloudflare secret.\n");

    step(1, "Checking wrangler (Cloudflare CLI) via npx");
    let v = wrangler::output(&dir, &["--version"])?;
    println!("  wrangler {}", v.trim());

    step(2, "Checking Cloudflare login");
    let who = wrangler::output(&dir, &["whoami"]).unwrap_or_default();
    if who.contains("not authenticated") || !who.to_ascii_lowercase().contains("logged in") {
        println!("  Not logged in. Opening the Cloudflare login page in your browser…");
        wrangler::run(&dir, &["login"])?;
    } else {
        println!("  Logged in.");
    }

    step(3, "Choosing a Worker name");
    let name = loop {
        let n = util::prompt("  Worker name", default_name)?;
        if valid_worker_name(&n) {
            break n;
        }
        println!(
            "  Use 1-54 lowercase letters, digits and dashes (not starting or ending with a dash)."
        );
    };
    let db_name = format!("{name}-db");

    step(4, "Creating (or reusing) the D1 database");
    let db_id = d1_find_or_create(&db_name)?;
    println!("  {db_name} = {db_id}");

    step(5, "Writing crates/relay/wrangler.toml (git-ignored)");
    write_wrangler_toml(&name, &db_name, &db_id, "wrangler.toml")?;

    step(6, "Applying database migrations");
    wrangler::run(&dir, &["d1", "migrations", "apply", "DB", "--remote"])?;

    step(
        7,
        "Building and deploying the Worker (first build takes a few minutes)",
    );
    let deploy_out = wrangler::run_tee(&dir, &["deploy"])?;
    let url = match wrangler::find_workers_dev_url(&deploy_out) {
        Some(u) => u,
        None => util::prompt(
            "  Could not find the workers.dev URL in the output. Paste it here",
            "",
        )?,
    };
    let url =
        pocket_agent_protocol::pairing::normalize_relay_url(&url).context("invalid relay URL")?;

    step(8, "Storing your Anthropic API key as a Cloudflare secret");
    println!("  Create a key at https://console.anthropic.com/settings/keys");
    println!("  Tip: set a monthly spend limit in the Console as a hard backstop.");
    let key = Zeroizing::new(util::prompt_secret("  ANTHROPIC_API_KEY (input hidden): ")?);
    if !key.starts_with("sk-ant-") {
        println!("  Warning: Anthropic API keys normally start with `sk-ant-`.");
        if !util::confirm("  Use it anyway?", false)? {
            bail!("aborted; re-run `cargo xtask relay-setup` when you have your key");
        }
    }
    wrangler::secret_put(&dir, "ANTHROPIC_API_KEY", &key)?;
    drop(key);

    step(9, "Generating and storing a pairing code");
    let code = Zeroizing::new(util::generate_pairing_code()?);
    wrangler::secret_put(&dir, "PAIRING_CODE", &code)?;

    step(10, "Waiting for the relay to come online");
    wait_healthy(&url)?;

    if util::confirm(
        "\nSend one tiny test request to Claude through the relay (costs a fraction of a cent)?",
        true,
    )? {
        check::relay_check(&url, &code)?;
    }

    qr::print_pairing(&url, &code)?;
    println!("Done. Your relay: {url}");
    Ok(())
}

/// Redeploy using the existing `wrangler.toml`.
pub fn relay_deploy() -> Result<()> {
    let dir = relay_dir();
    require_wrangler_toml()?;
    wrangler::run(&dir, &["d1", "migrations", "apply", "DB", "--remote"])?;
    let out = wrangler::run_tee(&dir, &["deploy"])?;
    if let Some(url) = wrangler::find_workers_dev_url(&out) {
        wait_healthy(&url)?;
        println!("Deployed: {url}");
    }
    Ok(())
}

/// New pairing code → secret → QR.
pub fn rotate_code(url: Option<String>) -> Result<()> {
    let dir = relay_dir();
    require_wrangler_toml()?;
    let url = match url {
        Some(u) => u,
        None => util::prompt("Relay URL (https://….workers.dev)", "")?,
    };
    let code = Zeroizing::new(util::generate_pairing_code()?);
    wrangler::secret_put(&dir, "PAIRING_CODE", &code)?;
    println!("New pairing code stored. Pairing is unlocked; existing devices keep working.");
    qr::print_pairing(&url, &code)
}

fn step(n: u32, text: &str) {
    println!("\n[{n}/10] {text}");
}

fn require_wrangler_toml() -> Result<()> {
    if !relay_dir().join("wrangler.toml").exists() {
        bail!("crates/relay/wrangler.toml not found; run `cargo xtask relay-setup` first");
    }
    Ok(())
}

fn valid_worker_name(n: &str) -> bool {
    (1..=54).contains(&n.len())
        && n.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !n.starts_with('-')
        && !n.ends_with('-')
}

fn d1_find_or_create(db_name: &str) -> Result<String> {
    let dir = relay_dir();
    if let Some(id) = d1_lookup(db_name)? {
        println!("  Reusing existing database.");
        return Ok(id);
    }
    wrangler::run(&dir, &["d1", "create", db_name])?;
    d1_lookup(db_name)?.context("database was created but could not be found in `wrangler d1 list`")
}

fn d1_lookup(db_name: &str) -> Result<Option<String>> {
    let out = wrangler::output(&relay_dir(), &["d1", "list", "--json"])?;
    // wrangler may print banner lines before the JSON array.
    let start = out.find('[').unwrap_or(0);
    let list: serde_json::Value = serde_json::from_str(&out[start..])
        .context("unexpected `wrangler d1 list --json` output")?;
    Ok(list.as_array().and_then(|dbs| {
        dbs.iter()
            .find(|d| d["name"] == db_name)
            .and_then(|d| d["uuid"].as_str().map(str::to_owned))
    }))
}

/// Render `wrangler.toml.example` with concrete values into `file_name`.
pub fn write_wrangler_toml(name: &str, db_name: &str, db_id: &str, file_name: &str) -> Result<()> {
    let dir = relay_dir();
    let template = fs::read_to_string(dir.join("wrangler.toml.example"))
        .context("crates/relay/wrangler.toml.example is missing")?;
    let rendered = template
        .replace("__WORKER_NAME__", name)
        .replace("__DATABASE_NAME__", db_name)
        .replace("__DATABASE_ID__", db_id);
    fs::write(dir.join(file_name), rendered)?;
    Ok(())
}

fn wait_healthy(url: &str) -> Result<()> {
    let http = util::http();
    for _ in 0..30 {
        if let Ok(mut r) = http.get(format!("{url}/healthz")).call()
            && r.status() == 200
            && let Ok(h) = r.body_mut().read_json::<Health>()
            && h.name == RELAY_NAME
        {
            println!(
                "  Relay is up: version {}, protocol {}",
                h.version, h.protocol
            );
            return Ok(());
        }
        sleep(Duration::from_secs(2));
    }
    bail!("relay did not answer {url}/healthz within 60 s; check `wrangler tail` for errors")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_names() {
        assert!(valid_worker_name("pocket-agent-relay"));
        assert!(!valid_worker_name("Pocket"));
        assert!(!valid_worker_name("-x"));
        assert!(!valid_worker_name(""));
        assert!(!valid_worker_name(&"a".repeat(55)));
    }
}
