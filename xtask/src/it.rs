//! `relay-it`: local integration test of the relay.
//!
//! Starts a mock Anthropic upstream on 127.0.0.1, runs the real Worker under
//! `wrangler dev --local` (workerd + local D1) pointed at it, and exercises
//! every route. Needs Node.js but no Cloudflare account and no API key.

use std::fs;
use std::net::TcpListener;
use std::path::Path;
use std::process::{Child, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::{self, sleep};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use pocket_agent_protocol::{DeviceList, Health, PairResponse, Prefs, UsageSummary};
use serde_json::{Value, json};

use crate::setup::write_wrangler_toml;
use crate::util::{self, relay_dir, root};
use crate::wrangler;

const UPSTREAM_KEY: &str = "it-upstream-key-not-a-real-key";
const PAIRING_CODE: &str = "IT000-PAIR0-CODE0-12345";
const CONFIG_FILE: &str = "wrangler.it.toml";
/// Each mock response costs 1000 in + 200 out on Haiku 5.5 = 200 micro-USD.
const MOCK_OK: &str = r#"{"id":"msg_it","type":"message","role":"assistant","model":"claude-haiku-5-5","content":[{"type":"text","text":"OK"}],"stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":1000,"output_tokens":200,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}"#;
const COST_PER_MOCK: u64 = 200;
/// Daily cap: 0.02 USD = 100 mock requests.
const DAILY_CAP: &str = "0.02";

#[derive(Debug, Clone)]
struct Seen {
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Seen {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

type SeenLog = Arc<Mutex<Vec<Seen>>>;

fn start_mock() -> Result<(u16, SeenLog)> {
    let server =
        tiny_http::Server::http("127.0.0.1:0").map_err(|e| anyhow::anyhow!("mock server: {e}"))?;
    let port = server
        .server_addr()
        .to_ip()
        .map(|a| a.port())
        .context("mock server has no IP address")?;
    let seen: SeenLog = Arc::default();
    let log = Arc::clone(&seen);
    thread::spawn(move || {
        for mut req in server.incoming_requests() {
            let mut body = Vec::new();
            let _ = req.as_reader().read_to_end(&mut body);
            let headers: Vec<(String, String)> = req
                .headers()
                .iter()
                .map(|h| {
                    (
                        h.field.as_str().as_str().to_ascii_lowercase(),
                        h.value.as_str().to_owned(),
                    )
                })
                .collect();
            let s = Seen { headers, body };
            let (status, resp, retry_after) = if s.header("x-api-key") != Some(UPSTREAM_KEY) {
                (
                    401,
                    r#"{"type":"error","error":{"type":"authentication_error","message":"bad key"}}"#,
                    None,
                )
            } else if String::from_utf8_lossy(&s.body).contains("trigger-429") {
                (
                    429,
                    r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#,
                    Some("7"),
                )
            } else {
                (200, MOCK_OK, None)
            };
            if let Ok(mut l) = log.lock() {
                l.push(s);
            }
            let mut r = tiny_http::Response::from_string(resp).with_status_code(status);
            for (k, v) in [
                ("content-type", "application/json"),
                ("request-id", "req_it_1"),
            ]
            .into_iter()
            .chain(retry_after.map(|v| ("retry-after", v)))
            {
                if let Ok(h) = tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()) {
                    r = r.with_header(h);
                }
            }
            let _ = req.respond(r);
        }
    });
    Ok((port, seen))
}

fn free_port() -> Result<u16> {
    Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

/// Run the integration test.
pub fn run() -> Result<()> {
    let dir = relay_dir();
    let state = root().join("target").join("relay-it-state");
    let log_path = root().join("target").join("relay-it-wrangler.log");
    let _ = fs::remove_dir_all(&state);
    fs::create_dir_all(&state)?;
    write_wrangler_toml(
        "pocket-agent-relay-it",
        "pocket-agent-relay-it-db",
        "00000000-0000-4000-8000-000000000000",
        CONFIG_FILE,
    )?;
    let state_s = state.to_string_lossy().into_owned();

    println!("[relay-it] applying migrations to a fresh local D1");
    let status = wrangler::command(
        &dir,
        &[
            "d1",
            "migrations",
            "apply",
            "DB",
            "--local",
            "--config",
            CONFIG_FILE,
            "--persist-to",
            &state_s,
        ],
    )
    .env("CI", "true")
    .stdout(Stdio::null())
    .status()?;
    if !status.success() {
        bail!("migrations failed");
    }

    let (mock_port, seen) = start_mock()?;
    let dev = |code: &str| start_dev(&state_s, &log_path, mock_port, code);

    // Phase 1: every route, ending with pairing locked.
    let (mut child, base) = dev(PAIRING_CODE)?;
    let phase1 = wait_up(&base).and_then(|()| checks(&base, &seen));
    wrangler::kill_tree(&mut child);

    // Phase 2: restart with a rotated PAIRING_CODE on the same database.
    let result = phase1.and_then(|token| {
        let (mut child, base) = dev(ROTATED_CODE)?;
        let r = wait_up(&base).and_then(|()| rotation_checks(&base, &token));
        wrangler::kill_tree(&mut child);
        r
    });

    let _ = fs::remove_file(dir.join(CONFIG_FILE));
    match result {
        Ok(()) => {
            println!("[relay-it] ALL CHECKS PASSED");
            Ok(())
        }
        Err(e) => {
            println!("[relay-it] FAILED: {e:#}\n  see {}", log_path.display());
            Err(e)
        }
    }
}

const ROTATED_CODE: &str = "IT000-ROTAT-ED000-67890";

fn start_dev(
    state: &str,
    log_path: &Path,
    mock_port: u16,
    pairing_code: &str,
) -> Result<(Child, String)> {
    let port = free_port()?;
    let port_s = port.to_string();
    let upstream = format!("UPSTREAM_URL:http://127.0.0.1:{mock_port}");
    let key = format!("ANTHROPIC_API_KEY:{UPSTREAM_KEY}");
    let code = format!("PAIRING_CODE:{pairing_code}");
    let cap = format!("DAILY_USD_CAP:{DAILY_CAP}");
    println!(
        "[relay-it] starting wrangler dev on 127.0.0.1:{port} (log: {})",
        log_path.display()
    );
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;
    let child = wrangler::command(
        &relay_dir(),
        &[
            "dev",
            "--config",
            CONFIG_FILE,
            "--local",
            "--ip",
            "127.0.0.1",
            "--port",
            &port_s,
            "--persist-to",
            state,
            "--var",
            &upstream,
            "--var",
            &key,
            "--var",
            &code,
            "--var",
            &cap,
            "--var",
            "PAIRING_MAX_FAILURES:3",
            "--var",
            "MAX_TOKENS_CAP:1000",
        ],
    )
    .env("CI", "true")
    .stdin(Stdio::null())
    .stdout(log.try_clone()?)
    .stderr(log)
    .spawn()?;
    Ok((child, format!("http://127.0.0.1:{port}")))
}

fn rotation_checks(base: &str, old_token: &str) -> Result<()> {
    let r = call(base, "GET", "/v1/devices", Some(old_token), None, &[])?;
    expect(
        r.status == 200,
        "after rotation: existing device token still valid",
    )?;
    let pair = |code: &str| {
        call(
            base,
            "POST",
            "/v1/pair",
            None,
            Some(format!(r#"{{"pairing_code":"{code}"}}"#).as_bytes()),
            &[],
        )
    };
    expect(
        pair(PAIRING_CODE)?.status == 401,
        "after rotation: old pairing code rejected",
    )?;
    expect(
        pair(ROTATED_CODE)?.status == 200,
        "after rotation: pairing unlocked with new code",
    )?;
    Ok(())
}

fn wait_up(base: &str) -> Result<()> {
    let http = util::http();
    let deadline = Instant::now() + Duration::from_secs(600);
    while Instant::now() < deadline {
        if let Ok(r) = http.get(format!("{base}/healthz")).call()
            && r.status() == 200
        {
            return Ok(());
        }
        sleep(Duration::from_millis(500));
    }
    bail!("wrangler dev did not come up within 10 minutes")
}

struct Resp {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Resp {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
    fn parse<T: serde::de::DeserializeOwned>(&self) -> Result<T> {
        serde_json::from_slice(&self.body)
            .with_context(|| format!("unexpected body: {}", String::from_utf8_lossy(&self.body)))
    }
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
    fn error_type(&self) -> String {
        self.json()["error"]["type"]
            .as_str()
            .unwrap_or("")
            .to_owned()
    }
}

fn call(
    base: &str,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<&[u8]>,
    extra: &[(&str, &str)],
) -> Result<Resp> {
    let http = util::http();
    let url = format!("{base}{path}");
    let mut b = ureq::http::Request::builder().method(method).uri(&url);
    if let Some(t) = token {
        b = b.header("authorization", format!("Bearer {t}"));
    }
    for (k, v) in extra {
        b = b.header(*k, *v);
    }
    let mut r = match body {
        Some(body) => http.run(
            b.header("content-type", "application/json")
                .body(body.to_vec())?,
        )?,
        None => http.run(b.body(())?)?,
    };
    let headers = r
        .headers()
        .iter()
        .map(|(k, v)| (k.as_str().to_owned(), v.to_str().unwrap_or("").to_owned()))
        .collect();
    Ok(Resp {
        status: r.status().as_u16(),
        headers,
        body: r.body_mut().read_to_vec()?,
    })
}

fn expect(cond: bool, what: &str) -> Result<()> {
    if cond {
        println!("  ok   {what}");
        Ok(())
    } else {
        bail!("check failed: {what}")
    }
}

fn msg_body(model: &str, max_tokens: u64, text: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "model": model,
        "max_tokens": max_tokens,
        "messages": [{"role": "user", "content": text}]
    }))
    .unwrap_or_default()
}

fn last_seen(seen: &SeenLog) -> Option<Seen> {
    seen.lock().ok().and_then(|l| l.last().cloned())
}

fn checks(base: &str, seen: &SeenLog) -> Result<String> {
    let mut successes: u64 = 0;

    let h: Health = call(base, "GET", "/healthz", None, None, &[])?.parse()?;
    expect(
        h.name == "pocket-agent-relay" && h.protocol == 1,
        "healthz reports name and protocol",
    )?;

    let r = call(
        base,
        "POST",
        "/v1/messages",
        None,
        Some(&msg_body("claude-haiku-5-5", 10, "hi")),
        &[],
    )?;
    expect(
        r.status == 401 && r.error_type() == "authentication_error",
        "messages without token → 401",
    )?;

    let r = call(base, "GET", "/v1/messages", None, None, &[])?;
    expect(r.status == 405, "wrong method → 405")?;
    let r = call(base, "GET", "/nope", None, None, &[])?;
    expect(r.status == 404, "unknown path → 404")?;

    let r = call(
        base,
        "POST",
        "/v1/pair",
        None,
        Some(br#"{"pairing_code":"WRONG"}"#),
        &[],
    )?;
    expect(r.status == 401, "pair with wrong code → 401 (failure 1/3)")?;

    let r = call(
        base,
        "POST",
        "/v1/pair",
        None,
        Some(format!(r#"{{"pairing_code":"{PAIRING_CODE}","device_name":"it phone"}}"#).as_bytes()),
        &[],
    )?;
    expect(r.status == 200, "pair with right code → 200")?;
    expect(
        r.header("cache-control") == Some("no-store"),
        "pair response is no-store",
    )?;
    let pair: PairResponse = r.parse()?;
    let token = pair.device_token.clone();
    expect(
        token.starts_with("pa_dt_") && token.len() == 49,
        "device token format",
    )?;

    // Passthrough: exact bytes in, exact bytes out, key injected, bearer not leaked.
    let body = msg_body("claude-haiku-5-5", 100, "hello");
    let r = call(
        base,
        "POST",
        "/v1/messages",
        Some(&token),
        Some(&body),
        &[("anthropic-beta", "server-side-fallback-2026-07-01")],
    )?;
    expect(
        r.status == 200 && r.body == MOCK_OK.as_bytes(),
        "messages → 200 with upstream body unchanged",
    )?;
    expect(
        r.header("request-id") == Some("req_it_1"),
        "upstream request-id passed through",
    )?;
    successes += 1;
    let s = last_seen(seen).context("mock saw nothing")?;
    expect(
        s.body == body,
        "upstream received the original request bytes",
    )?;
    expect(
        s.header("x-api-key") == Some(UPSTREAM_KEY),
        "relay injected x-api-key",
    )?;
    expect(
        s.header("anthropic-version") == Some("2026-10-21"),
        "default anthropic-version sent",
    )?;
    expect(
        s.header("anthropic-beta") == Some("server-side-fallback-2026-07-01"),
        "allowlisted beta forwarded",
    )?;
    expect(
        s.header("authorization").is_none(),
        "device token NOT forwarded upstream",
    )?;

    let n_before = seen.lock().map(|l| l.len()).unwrap_or(0);
    let r = call(
        base,
        "POST",
        "/v1/messages",
        Some(&token),
        Some(&msg_body("claude-fable-5-1", 10, "x")),
        &[],
    )?;
    expect(
        r.status == 403 && r.error_type() == "model_not_allowed",
        "non-allowlisted model → 403",
    )?;
    let r = call(
        base,
        "POST",
        "/v1/messages",
        Some(&token),
        Some(br#"{"model":"claude-haiku-5-5","max_tokens":10,"stream":true,"messages":[]}"#),
        &[],
    )?;
    expect(r.status == 400, "stream=true → 400")?;
    let r = call(
        base,
        "POST",
        "/v1/messages",
        Some(&token),
        Some(&body),
        &[("anthropic-beta", "some-other-beta")],
    )?;
    expect(r.status == 400, "non-allowlisted beta → 400")?;
    let r = call(
        base,
        "POST",
        "/v1/messages",
        Some(&token),
        Some(b"not json"),
        &[],
    )?;
    expect(r.status == 400, "malformed body → 400")?;
    let r = call(
        base,
        "POST",
        "/v1/messages",
        Some(&token),
        Some(br#"{"model":"claude-haiku-5-5","model":"claude-fable-5-1","max_tokens":1}"#),
        &[],
    )?;
    expect(r.status == 400, "duplicate model keys → 400")?;
    expect(
        seen.lock().map(|l| l.len()).unwrap_or(0) == n_before,
        "rejected requests never reached upstream",
    )?;

    let r = call(
        base,
        "POST",
        "/v1/messages",
        Some(&token),
        Some(&msg_body("claude-sonnet-5-5", 64_000, "clamp me")),
        &[("anthropic-version", "2026-10-21")],
    )?;
    expect(r.status == 200, "over-cap max_tokens still forwarded")?;
    successes += 1;
    let s = last_seen(seen).context("mock saw nothing")?;
    let fwd: Value = serde_json::from_slice(&s.body)?;
    expect(
        fwd["max_tokens"] == 1000 && fwd["messages"][0]["content"] == "clamp me",
        "max_tokens clamped to MAX_TOKENS_CAP",
    )?;

    let r = call(
        base,
        "POST",
        "/v1/messages",
        Some(&token),
        Some(&msg_body("claude-haiku-5-5", 10, "trigger-429")),
        &[],
    )?;
    expect(
        r.status == 429 && r.header("retry-after") == Some("7"),
        "upstream 429 + retry-after passed through",
    )?;
    expect(
        r.header("x-pocket-relay-error").is_none(),
        "forwarded upstream errors carry no relay-error header",
    )?;

    // Usage is written in waitUntil; give it a moment.
    let mut usage: Option<UsageSummary> = None;
    for _ in 0..20 {
        let u: UsageSummary = call(base, "GET", "/v1/usage", Some(&token), None, &[])?.parse()?;
        if u.today.requests >= successes {
            usage = Some(u);
            break;
        }
        sleep(Duration::from_millis(100));
    }
    let u = usage.context("usage never recorded")?;
    expect(
        u.device_today.requests == successes && u.today.input_tokens == 1000 * successes,
        "usage ledger recorded tokens",
    )?;
    expect(
        u.today.usd_micros == COST_PER_MOCK * successes,
        "usage cost computed from price table",
    )?;
    expect(u.caps.daily_usd_micros == 20_000, "caps reported")?;

    let r = call(
        base,
        "PUT",
        "/v1/prefs",
        Some(&token),
        Some(br#"{"prefs":{"music_app":"spotify","lang":"hi-IN"}}"#),
        &[],
    )?;
    expect(r.status == 200, "prefs PUT")?;
    let p: Prefs = call(base, "GET", "/v1/prefs", Some(&token), None, &[])?.parse()?;
    expect(
        p.prefs["music_app"] == "spotify" && p.updated_at.is_some(),
        "prefs GET returns stored object",
    )?;
    let r = call(
        base,
        "PUT",
        "/v1/prefs",
        Some(&token),
        Some(br#"{"prefs":[1,2]}"#),
        &[],
    )?;
    expect(r.status == 400, "prefs must be an object")?;

    // Second device, then revoke it.
    let p2: PairResponse = call(
        base,
        "POST",
        "/v1/pair",
        None,
        Some(format!(r#"{{"pairing_code":"{PAIRING_CODE}"}}"#).as_bytes()),
        &[],
    )?
    .parse()?;
    let list: DeviceList = call(base, "GET", "/v1/devices", Some(&token), None, &[])?.parse()?;
    expect(
        list.devices.len() == 2 && list.devices.iter().filter(|d| d.current).count() == 1,
        "devices lists both, marks current",
    )?;
    expect(
        list.devices.iter().any(|d| d.name == "it phone")
            && list.devices.iter().any(|d| d.name == "Android device"),
        "device names stored (default applied)",
    )?;
    let r = call(
        base,
        "POST",
        "/v1/devices/revoke",
        Some(&token),
        Some(format!(r#"{{"device_id":"{}"}}"#, p2.device_id).as_bytes()),
        &[],
    )?;
    expect(r.status == 200, "revoke second device")?;
    let r = call(base, "GET", "/v1/usage", Some(&p2.device_token), None, &[])?;
    expect(r.status == 401, "revoked token → 401")?;
    let r = call(
        base,
        "POST",
        "/v1/devices/revoke",
        Some(&token),
        Some(br#"{"device_id":"dev_nope"}"#),
        &[],
    )?;
    expect(r.status == 404, "revoke unknown device → 404")?;

    // Relay overhead with an instant local upstream (wall time, includes D1 auth lookup).
    let mut times = Vec::new();
    for _ in 0..20 {
        let t = Instant::now();
        let r = call(base, "POST", "/v1/messages", Some(&token), Some(&body), &[])?;
        times.push(t.elapsed().as_secs_f64() * 1000.0);
        if r.status != 200 {
            bail!("timing request failed: {}", r.status);
        }
        successes += 1;
    }
    times.sort_by(f64::total_cmp);
    println!(
        "  info relay round-trip with instant upstream (local workerd): median {:.1} ms, p90 {:.1} ms",
        times[10], times[18]
    );

    // Spend cap → 402 once crossed.
    let mut hit = None;
    for i in 0..150 {
        let r = call(base, "POST", "/v1/messages", Some(&token), Some(&body), &[])?;
        if r.status == 402 {
            hit = Some((i, r.error_type()));
            expect(
                r.header("x-pocket-relay-error") == Some("relay_cap_exceeded"),
                "relay-generated errors carry x-pocket-relay-error",
            )?;
            break;
        }
        successes += 1;
        sleep(Duration::from_millis(20));
    }
    let (_, kind) = hit.context("daily cap never triggered")?;
    expect(
        kind == "relay_cap_exceeded",
        "daily cap → 402 relay_cap_exceeded",
    )?;
    expect(
        successes * COST_PER_MOCK >= 20_000
            && successes * COST_PER_MOCK <= 20_000 + 10 * COST_PER_MOCK,
        "cap triggered close to the configured amount",
    )?;

    // Pairing lock: the earlier successful pairing reset the counter; max is 3.
    for _ in 0..3 {
        call(
            base,
            "POST",
            "/v1/pair",
            None,
            Some(br#"{"pairing_code":"WRONG"}"#),
            &[],
        )?;
    }
    let r = call(
        base,
        "POST",
        "/v1/pair",
        None,
        Some(format!(r#"{{"pairing_code":"{PAIRING_CODE}"}}"#).as_bytes()),
        &[],
    )?;
    expect(
        r.status == 423 && r.error_type() == "pairing_locked",
        "pairing locks after PAIRING_MAX_FAILURES",
    )?;
    let r = call(base, "GET", "/v1/devices", Some(&token), None, &[])?;
    expect(
        r.status == 200,
        "existing devices keep working while pairing is locked",
    )?;
    Ok(token)
}
