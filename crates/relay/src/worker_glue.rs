//! Cloudflare Workers glue: HTTP handlers and D1 queries.
//!
//! Only compiled for `wasm32`. All decisions (auth checks, clamping, cost,
//! caps) are delegated to the natively-tested modules; this file is I/O.
//!
//! Logging rule: log the route name, status and duration only. Never log
//! bodies, headers, tokens, pairing codes or API keys.

use std::collections::HashMap;
use std::fmt::Display;

use pocket_agent_protocol::{
    DeviceInfo, DeviceList, Health, MAX_PREFS_BYTES, PROTOCOL_VERSION, PairRequest, PairResponse,
    Prefs, RELAY_NAME, RelayErrorKind, RevokeRequest, UsageTotals,
};
use serde::{Deserialize, Serialize};
use worker::wasm_bindgen::JsValue;
use worker::{
    Context, D1Database, Date, Env, Fetch, Headers, Method, Request, RequestInit, Response,
    console_error, console_log, event,
};

use crate::config::{Config, DEFAULT_ANTHROPIC_VERSION};
use crate::usage::{CapHit, UsageRow, check_caps, cost_micros, summarize};
use crate::{RelayError, Route, auth, inspect, time};

type HResult = Result<Response, RelayError>;

/// Response headers from Anthropic that are useful to the client.
const PASSTHROUGH_HEADERS: [&str; 4] = [
    "content-type",
    "request-id",
    "retry-after",
    "x-should-retry",
];
const MAX_PAIR_BODY: usize = 4 * 1024;
const MAX_SMALL_BODY: usize = 4 * 1024;
const MAX_DEVICE_NAME: usize = 64;

#[event(fetch)]
async fn fetch(mut req: Request, env: Env, ctx: Context) -> worker::Result<Response> {
    let started = Date::now().as_millis();
    let route = Route::parse(req.method().as_ref(), &req.path());
    let resp = match handle(route, &mut req, &env, &ctx).await {
        Ok(r) => r,
        Err(e) => error_response(&e)?,
    };
    console_log!(
        "{} {} {}ms",
        route.name(),
        resp.status_code(),
        Date::now().as_millis().saturating_sub(started)
    );
    Ok(resp)
}

async fn handle(route: Route, req: &mut Request, env: &Env, ctx: &Context) -> HResult {
    match route {
        Route::Health => return json(200, &health()),
        Route::NotFound => return Err(RelayError::new(RelayErrorKind::NotFoundError, "not found")),
        Route::MethodNotAllowed => return Err(RelayError::method_not_allowed()),
        _ => {}
    }
    let config = Config::from_lookup(|name| env.var(name).ok().map(|v| v.to_string()))
        .map_err(|e| RelayError::new(RelayErrorKind::RelayError, e.to_string()))?;
    let db = env.d1("DB").map_err(|_| {
        RelayError::new(
            RelayErrorKind::RelayError,
            "D1 binding `DB` is missing; check wrangler.toml",
        )
    })?;

    match route {
        Route::Pair => pair(req, env, &db, &config).await,
        Route::Messages => messages(req, env, ctx, &db, &config).await,
        Route::Usage => {
            let device = authenticate(req, &db).await?;
            usage(&db, &device, &config).await
        }
        Route::PrefsGet => {
            authenticate(req, &db).await?;
            prefs_get(&db).await
        }
        Route::PrefsPut => {
            authenticate(req, &db).await?;
            prefs_put(req, &db).await
        }
        Route::Devices => {
            let device = authenticate(req, &db).await?;
            json(200, &device_list(&db, &device).await?)
        }
        Route::Revoke => {
            let device = authenticate(req, &db).await?;
            revoke(req, &db, &device).await
        }
        Route::Health | Route::NotFound | Route::MethodNotAllowed => {
            Err(RelayError::new(RelayErrorKind::NotFoundError, "not found"))
        }
    }
}

// ---------------------------------------------------------------- helpers ---

fn health() -> Health {
    Health {
        name: RELAY_NAME.to_owned(),
        version: crate::VERSION.to_owned(),
        protocol: PROTOCOL_VERSION,
    }
}

/// Map an infrastructure error to a generic 500. The detail goes to the Worker
/// log (it comes from D1/runtime, never from secrets), not to the client.
fn internal(e: impl Display) -> RelayError {
    console_error!("internal error: {e}");
    RelayError::new(RelayErrorKind::RelayError, "relay internal error")
}

fn invalid(msg: impl Into<String>) -> RelayError {
    RelayError::new(RelayErrorKind::InvalidRequestError, msg)
}

fn unauthorized(msg: &str) -> RelayError {
    RelayError::new(RelayErrorKind::AuthenticationError, msg)
}

fn base_headers() -> Headers {
    let h = Headers::new();
    // Setting static, valid header names cannot fail.
    let _ = h.set("content-type", "application/json");
    let _ = h.set("cache-control", "no-store");
    h
}

fn bytes_response(status: u16, body: Vec<u8>, headers: Headers) -> HResult {
    Ok(Response::from_bytes(body)
        .map_err(internal)?
        .with_status(status)
        .with_headers(headers))
}

fn json<T: Serialize>(status: u16, v: &T) -> HResult {
    bytes_response(
        status,
        serde_json::to_vec(v).map_err(internal)?,
        base_headers(),
    )
}

/// Relay-generated errors carry `x-pocket-relay-error: <kind>` so the app can
/// tell them from Anthropic errors forwarded unchanged (e.g. a 401 meaning
/// "re-pair this phone" vs. "the relay's API key is invalid").
fn error_response(e: &RelayError) -> worker::Result<Response> {
    let headers = base_headers();
    headers.set("x-pocket-relay-error", e.kind_str())?;
    Ok(Response::from_bytes(e.body())?
        .with_status(e.status())
        .with_headers(headers))
}

async fn read_body(req: &mut Request, limit: usize) -> Result<Vec<u8>, RelayError> {
    let declared = req
        .headers()
        .get("content-length")
        .ok()
        .flatten()
        .and_then(|v| v.parse::<usize>().ok());
    if declared.is_some_and(|n| n > limit) {
        return Err(invalid(format!("request body larger than {limit} bytes")));
    }
    let body = req
        .bytes()
        .await
        .map_err(|_| invalid("could not read request body"))?;
    if body.len() > limit {
        return Err(invalid(format!("request body larger than {limit} bytes")));
    }
    Ok(body)
}

fn num(n: u64) -> JsValue {
    // D1 numbers are JS doubles; our counters stay far below 2^53.
    #[allow(clippy::cast_precision_loss)]
    JsValue::from_f64(n as f64)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn to_u64(f: f64) -> u64 {
    if f.is_finite() && f > 0.0 {
        f as u64
    } else {
        0
    }
}

#[allow(clippy::cast_possible_truncation)]
fn to_i64(f: f64) -> i64 {
    if f.is_finite() { f as i64 } else { 0 }
}

fn now_ms() -> u64 {
    Date::now().as_millis()
}

fn now_secs() -> u64 {
    now_ms() / 1000
}

fn random<const N: usize>() -> Result<[u8; N], RelayError> {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).map_err(internal)?;
    Ok(b)
}

fn secret(env: &Env, name: &str) -> Result<String, RelayError> {
    env.secret(name)
        .map(|s| s.to_string())
        .ok()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            RelayError::new(
                RelayErrorKind::RelayError,
                format!("{name} secret is not set on the relay; run `wrangler secret put {name}`"),
            )
        })
}

// ------------------------------------------------------------------- auth ---

#[derive(Deserialize)]
struct DeviceAuthRow {
    id: String,
    revoked: f64,
}

/// Validate the bearer token; returns the device id.
async fn authenticate(req: &Request, db: &D1Database) -> Result<String, RelayError> {
    let header = req.headers().get("authorization").ok().flatten();
    let token = auth::parse_bearer(header.as_deref()).ok_or_else(|| {
        unauthorized("missing or malformed device token; pair this device with the relay")
    })?;
    let row: Option<DeviceAuthRow> = db
        .prepare("SELECT id, revoked FROM devices WHERE token_hash = ?1")
        .bind(&[auth::hash_token(token).into()])
        .map_err(internal)?
        .first(None)
        .await
        .map_err(internal)?;
    match row {
        None => Err(unauthorized("unknown device token; pair this device again")),
        Some(r) if r.revoked != 0.0 => Err(unauthorized("this device was revoked; pair it again")),
        Some(r) => Ok(r.id),
    }
}

// ---------------------------------------------------------------- pairing ---

#[derive(Deserialize)]
struct MetaRow {
    key: String,
    value: String,
}

async fn meta_get(db: &D1Database) -> Result<HashMap<String, String>, RelayError> {
    let rows: Vec<MetaRow> = db
        .prepare("SELECT key, value FROM meta WHERE key IN ('pair_fp', 'pair_failures')")
        .all()
        .await
        .map_err(internal)?
        .results()
        .map_err(internal)?;
    Ok(rows.into_iter().map(|r| (r.key, r.value)).collect())
}

async fn meta_set(db: &D1Database, pairs: &[(&str, String)]) -> Result<(), RelayError> {
    let mut stmts = Vec::with_capacity(pairs.len());
    for (k, v) in pairs {
        stmts.push(
            db.prepare("INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value")
                .bind(&[(*k).into(), v.as_str().into()])
                .map_err(internal)?,
        );
    }
    db.batch(stmts).await.map_err(internal)?;
    Ok(())
}

fn clean_device_name(name: Option<String>) -> String {
    let name: String = name
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_DEVICE_NAME)
        .collect();
    let name = name.trim();
    if name.is_empty() {
        "Android device".to_owned()
    } else {
        name.to_owned()
    }
}

async fn pair(req: &mut Request, env: &Env, db: &D1Database, config: &Config) -> HResult {
    let body = read_body(req, MAX_PAIR_BODY).await?;
    let pr: PairRequest = serde_json::from_slice(&body)
        .map_err(|_| invalid(r#"body must be {"pairing_code": "..."}"#))?;
    let expected = secret(env, "PAIRING_CODE")?;
    let fp = auth::pairing_fingerprint(&expected);

    // Failure counter is tied to the current PAIRING_CODE: rotating the
    // secret changes the fingerprint and therefore resets the lock.
    let meta = meta_get(db).await?;
    let failures: u32 = if meta.get("pair_fp") == Some(&fp) {
        meta.get("pair_failures")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    } else {
        meta_set(
            db,
            &[("pair_fp", fp.clone()), ("pair_failures", "0".into())],
        )
        .await?;
        0
    };
    if failures >= config.pairing_max_failures {
        return Err(RelayError::new(
            RelayErrorKind::PairingLocked,
            "pairing is locked after too many wrong codes; rotate the code with `cargo xtask relay-rotate-code`",
        ));
    }
    if !auth::pairing_code_matches(&pr.pairing_code, &expected) {
        meta_set(db, &[("pair_failures", (failures + 1).to_string())]).await?;
        return Err(unauthorized("wrong pairing code"));
    }
    if failures > 0 {
        meta_set(db, &[("pair_failures", "0".into())]).await?;
    }

    let token = auth::token_from_bytes(&random::<32>()?);
    let device_id = auth::device_id_from_bytes(&random::<9>()?);
    db.prepare("INSERT INTO devices (id, token_hash, name, created_at) VALUES (?1, ?2, ?3, ?4)")
        .bind(&[
            device_id.as_str().into(),
            auth::hash_token(&token).into(),
            clean_device_name(pr.device_name).into(),
            num(now_secs()),
        ])
        .map_err(internal)?
        .run()
        .await
        .map_err(internal)?;

    json(
        200,
        &PairResponse {
            device_id,
            device_token: token,
            relay: health(),
        },
    )
}

// --------------------------------------------------------------- messages ---

async fn messages(
    req: &mut Request,
    env: &Env,
    ctx: &Context,
    db: &D1Database,
    config: &Config,
) -> HResult {
    let device_id = authenticate(req, db).await?;
    let body = read_body(req, config.max_body_bytes).await?;

    let head = inspect::request_head(&body).map_err(|_| {
        invalid("body must be JSON with a string `model` and an integer `max_tokens`")
    })?;
    if head.stream {
        return Err(invalid(
            "streaming is not supported by the relay; send \"stream\": false",
        ));
    }
    if !config.model_allowed(&head.model) {
        let shown: String = head.model.chars().take(64).collect();
        return Err(RelayError::new(
            RelayErrorKind::ModelNotAllowed,
            format!("model `{shown}` is not in this relay's MODEL_ALLOWLIST"),
        ));
    }
    let body = if head.max_tokens > config.max_tokens_cap {
        inspect::clamp_max_tokens(&body, config.max_tokens_cap)
            .map_err(|_| invalid("body must be a JSON object"))?
    } else {
        body
    };

    let beta = req.headers().get("anthropic-beta").ok().flatten();
    if let Some(b) = beta.as_deref()
        && let Some(bad) = inspect::disallowed_beta(b, &config.beta_allowlist)
    {
        let shown: String = bad.chars().take(64).collect();
        return Err(invalid(format!(
            "anthropic-beta `{shown}` is not in this relay's BETA_ALLOWLIST"
        )));
    }
    let version = req
        .headers()
        .get("anthropic-version")
        .ok()
        .flatten()
        .filter(|v| inspect::valid_api_version(v))
        .unwrap_or_else(|| DEFAULT_ANTHROPIC_VERSION.to_owned());

    let today = time::utc_day(now_ms());
    if config.caps.daily_usd_micros > 0 || config.caps.monthly_usd_micros > 0 {
        let rows = month_rows(db, &today).await?;
        let s = summarize(&rows, &today, &device_id, config.caps);
        if let Some(hit) = check_caps(config.caps, s.today.usd_micros, s.month.usd_micros) {
            let which = match hit {
                CapHit::Daily => "daily",
                CapHit::Monthly => "monthly",
            };
            return Err(RelayError::new(
                RelayErrorKind::RelayCapExceeded,
                format!("the relay's {which} spending cap has been reached"),
            ));
        }
    }

    let api_key = secret(env, "ANTHROPIC_API_KEY")?;
    let headers = Headers::new();
    for (k, v) in [
        ("content-type", "application/json"),
        ("x-api-key", api_key.as_str()),
        ("anthropic-version", version.as_str()),
    ] {
        headers.set(k, v).map_err(internal)?;
    }
    if let Some(b) = beta.as_deref() {
        headers.set("anthropic-beta", b).map_err(internal)?;
    }
    let mut init = RequestInit::new();
    init.with_method(Method::Post)
        .with_headers(headers)
        .with_body(Some(
            worker::js_sys::Uint8Array::from(body.as_slice()).into(),
        ));
    let upstream = Request::new_with_init(&format!("{}/v1/messages", config.upstream), &init)
        .map_err(internal)?;

    let unavailable = || {
        RelayError::new(
            RelayErrorKind::UpstreamUnavailable,
            "the relay could not reach the Claude API",
        )
    };
    let mut up = Fetch::Request(upstream)
        .send()
        .await
        .map_err(|_| unavailable())?;
    let status = up.status_code();
    let resp_body = up.bytes().await.map_err(|_| unavailable())?;

    let out = Headers::new();
    let _ = out.set("cache-control", "no-store");
    for name in PASSTHROUGH_HEADERS {
        if let Ok(Some(v)) = up.headers().get(name) {
            let _ = out.set(name, &v);
        }
    }
    for (k, v) in up.headers().entries() {
        if k.starts_with("anthropic-ratelimit-") {
            let _ = out.set(&k, &v);
        }
    }

    if status == 200
        && let Some((model, u)) = inspect::response_usage(&resp_body)
    {
        let model = model.unwrap_or(head.model);
        let usd = cost_micros(&u, &config.price_for(&model));
        let env = env.clone();
        ctx.wait_until(async move {
            if let Err(e) = record_usage(&env, &today, &device_id, &model, &u, usd).await {
                console_error!("usage write failed: {e}");
            }
        });
    }

    bytes_response(status, resp_body, out)
}

async fn record_usage(
    env: &Env,
    day: &str,
    device_id: &str,
    model: &str,
    u: &inspect::Usage,
    usd_micros: u64,
) -> worker::Result<()> {
    let db = env.d1("DB")?;
    let upsert = db
        .prepare(
            "INSERT INTO usage (day, device_id, model, input_tokens, output_tokens, cache_read, cache_write, usd_micros, requests)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1)
             ON CONFLICT(day, device_id, model) DO UPDATE SET
               input_tokens = input_tokens + excluded.input_tokens,
               output_tokens = output_tokens + excluded.output_tokens,
               cache_read = cache_read + excluded.cache_read,
               cache_write = cache_write + excluded.cache_write,
               usd_micros = usd_micros + excluded.usd_micros,
               requests = requests + 1",
        )
        .bind(&[
            day.into(),
            device_id.into(),
            model.into(),
            num(u.input_tokens),
            num(u.output_tokens),
            num(u.cache_read_input_tokens),
            num(u.cache_creation_input_tokens),
            num(usd_micros),
        ])?;
    let seen = db
        .prepare("UPDATE devices SET last_seen = ?1 WHERE id = ?2")
        .bind(&[num(now_secs()), device_id.into()])?;
    db.batch(vec![upsert, seen]).await?;
    Ok(())
}

// ------------------------------------------------------------------ usage ---

#[derive(Deserialize)]
struct UsageDbRow {
    day: String,
    device_id: String,
    input_tokens: f64,
    output_tokens: f64,
    cache_read: f64,
    cache_write: f64,
    usd_micros: f64,
    requests: f64,
}

async fn month_rows(db: &D1Database, today: &str) -> Result<Vec<UsageRow>, RelayError> {
    let rows: Vec<UsageDbRow> = db
        .prepare(
            "SELECT day, device_id, SUM(input_tokens) AS input_tokens, SUM(output_tokens) AS output_tokens,
                    SUM(cache_read) AS cache_read, SUM(cache_write) AS cache_write,
                    SUM(usd_micros) AS usd_micros, SUM(requests) AS requests
             FROM usage WHERE day >= ?1 GROUP BY day, device_id",
        )
        .bind(&[time::month_start(today).into()])
        .map_err(internal)?
        .all()
        .await
        .map_err(internal)?
        .results()
        .map_err(internal)?;
    Ok(rows
        .into_iter()
        .map(|r| UsageRow {
            day: r.day,
            device_id: r.device_id,
            totals: UsageTotals {
                input_tokens: to_u64(r.input_tokens),
                output_tokens: to_u64(r.output_tokens),
                cache_read_input_tokens: to_u64(r.cache_read),
                cache_creation_input_tokens: to_u64(r.cache_write),
                usd_micros: to_u64(r.usd_micros),
                requests: to_u64(r.requests),
            },
        })
        .collect())
}

async fn usage(db: &D1Database, device_id: &str, config: &Config) -> HResult {
    let today = time::utc_day(now_ms());
    let rows = month_rows(db, &today).await?;
    json(200, &summarize(&rows, &today, device_id, config.caps))
}

// ------------------------------------------------------------------ prefs ---

#[derive(Deserialize)]
struct PrefsRow {
    json: String,
    updated_at: f64,
}

#[derive(Deserialize)]
struct PrefsPutBody {
    prefs: serde_json::Value,
}

async fn prefs_get(db: &D1Database) -> HResult {
    let row: Option<PrefsRow> = db
        .prepare("SELECT json, updated_at FROM prefs WHERE scope = 'global'")
        .first(None)
        .await
        .map_err(internal)?;
    let prefs = match row {
        Some(r) => Prefs {
            prefs: serde_json::from_str(&r.json).unwrap_or_else(|_| serde_json::json!({})),
            updated_at: Some(to_i64(r.updated_at)),
        },
        None => Prefs {
            prefs: serde_json::json!({}),
            updated_at: None,
        },
    };
    json(200, &prefs)
}

async fn prefs_put(req: &mut Request, db: &D1Database) -> HResult {
    let body = read_body(req, MAX_PREFS_BYTES + 64).await?;
    let put: PrefsPutBody =
        serde_json::from_slice(&body).map_err(|_| invalid(r#"body must be {"prefs": {...}}"#))?;
    if !put.prefs.is_object() {
        return Err(invalid("`prefs` must be a JSON object"));
    }
    let stored = serde_json::to_string(&put.prefs).map_err(internal)?;
    if stored.len() > MAX_PREFS_BYTES {
        return Err(invalid(format!(
            "prefs larger than {MAX_PREFS_BYTES} bytes"
        )));
    }
    let now = now_secs();
    db.prepare(
        "INSERT INTO prefs (scope, json, updated_at) VALUES ('global', ?1, ?2)
         ON CONFLICT(scope) DO UPDATE SET json = excluded.json, updated_at = excluded.updated_at",
    )
    .bind(&[stored.as_str().into(), num(now)])
    .map_err(internal)?
    .run()
    .await
    .map_err(internal)?;
    json(
        200,
        &Prefs {
            prefs: put.prefs,
            updated_at: i64::try_from(now).ok(),
        },
    )
}

// ---------------------------------------------------------------- devices ---

#[derive(Deserialize)]
struct DeviceDbRow {
    id: String,
    name: String,
    created_at: f64,
    last_seen: Option<f64>,
    revoked: f64,
}

async fn device_list(db: &D1Database, current: &str) -> Result<DeviceList, RelayError> {
    let rows: Vec<DeviceDbRow> = db
        .prepare(
            "SELECT id, name, created_at, last_seen, revoked FROM devices ORDER BY created_at DESC",
        )
        .all()
        .await
        .map_err(internal)?
        .results()
        .map_err(internal)?;
    Ok(DeviceList {
        devices: rows
            .into_iter()
            .map(|r| DeviceInfo {
                current: r.id == current,
                id: r.id,
                name: r.name,
                created_at: to_i64(r.created_at),
                last_seen: r.last_seen.map(to_i64),
                revoked: r.revoked != 0.0,
            })
            .collect(),
    })
}

async fn revoke(req: &mut Request, db: &D1Database, current: &str) -> HResult {
    let body = read_body(req, MAX_SMALL_BODY).await?;
    let r: RevokeRequest = serde_json::from_slice(&body)
        .map_err(|_| invalid(r#"body must be {"device_id": "..."}"#))?;
    let result = db
        .prepare("UPDATE devices SET revoked = 1 WHERE id = ?1")
        .bind(&[r.device_id.as_str().into()])
        .map_err(internal)?
        .run()
        .await
        .map_err(internal)?;
    let changed = result
        .meta()
        .ok()
        .flatten()
        .and_then(|m| m.changes)
        .unwrap_or(0);
    if changed == 0 {
        return Err(RelayError::new(
            RelayErrorKind::NotFoundError,
            "no such device",
        ));
    }
    json(200, &device_list(db, current).await?)
}
