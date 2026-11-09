//! `relay-check`: live end-to-end test of a deployed relay.

use std::time::Instant;

use anyhow::{Context, Result, bail};
use pocket_agent_protocol::{Health, PROTOCOL_VERSION, PairResponse, RELAY_NAME, UsageSummary};

use crate::util;

/// Pair a temporary device, make one minimal Claude request, revoke the device.
pub fn relay_check(url: &str, code: &str) -> Result<()> {
    let url = url.trim_end_matches('/');
    let http = util::http();
    println!("\nRelay check: {url}");

    let health: Health = http
        .get(format!("{url}/healthz"))
        .call()
        .context("relay unreachable")?
        .body_mut()
        .read_json()
        .context("/healthz did not return relay JSON")?;
    if health.name != RELAY_NAME {
        bail!("{url} is not a Pocket Agent relay");
    }
    if health.protocol != PROTOCOL_VERSION {
        println!(
            "  Warning: relay protocol {} differs from this tool's {PROTOCOL_VERSION}",
            health.protocol
        );
    }
    println!("  healthz        ok (version {})", health.version);

    let mut r = http.post(format!("{url}/v1/pair")).send_json(
        serde_json::json!({"pairing_code": code, "device_name": "xtask relay-check (temporary)"}),
    )?;
    if r.status() != 200 {
        bail!(
            "pairing failed: HTTP {} {}",
            r.status(),
            r.body_mut().read_to_string().unwrap_or_default()
        );
    }
    let pair: PairResponse = r.body_mut().read_json()?;
    println!("  pair           ok ({})", pair.device_id);
    let bearer = format!("Bearer {}", pair.device_token);

    let started = Instant::now();
    let mut r = http
        .post(format!("{url}/v1/messages"))
        .header("authorization", &bearer)
        .header("anthropic-version", "2026-10-21")
        .send_json(serde_json::json!({
            "model": "claude-haiku-5-5",
            "max_tokens": 64,
            "output_config": {"effort": "low"},
            "messages": [{"role": "user", "content": "Reply with exactly: OK"}]
        }))?;
    let elapsed = started.elapsed();
    let status = r.status();
    let body: serde_json::Value = r.body_mut().read_json().unwrap_or_default();
    let result = if status == 200 {
        let text: String = body["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|b| b["text"].as_str())
            .collect();
        println!(
            "  messages       ok in {} ms: {:?}",
            elapsed.as_millis(),
            text.trim()
        );
        println!("                 usage {}", body["usage"]);
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "Claude request through the relay failed: HTTP {status} {}",
            body["error"]
        ))
    };

    if let Ok(mut r) = http
        .get(format!("{url}/v1/usage"))
        .header("authorization", &bearer)
        .call()
        && let Ok(u) = r.body_mut().read_json::<UsageSummary>()
    {
        println!(
            "  usage          today: {} requests, ~${:.4}",
            u.today.requests,
            u.today.usd_micros as f64 / 1e6
        );
    }

    let revoked = http
        .post(format!("{url}/v1/devices/revoke"))
        .header("authorization", &bearer)
        .send_json(serde_json::json!({"device_id": pair.device_id}))
        .map(|r| r.status() == 200)
        .unwrap_or(false);
    println!(
        "  revoke temp    {}",
        if revoked {
            "ok"
        } else {
            "FAILED (revoke it in the app)"
        }
    );
    result
}
