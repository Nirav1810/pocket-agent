# Architecture

Pocket Agent is a voice-driven AI agent for Android. You say a command; it either
fires an Android intent (fast path) or operates the phone's UI through an
AccessibilityService (screen path), using Claude as the reasoning engine.

The project hosts **nothing**. Every user brings their own Anthropic API key and,
optionally, deploys their own relay on the Cloudflare Workers Free plan. There are
no accounts, telemetry, analytics or crash reporting.

Design decisions and their rationale live in [DECISIONS.md](DECISIONS.md).
Measured numbers live in [PERF.md](PERF.md).

## 1. System overview

```
┌──────────────────────────── Android app (user installs) ─────────────────────────────┐
│ Kotlin: thin platform layer (Android Views, no Compose, no DI, no Play Services)     │
│  • Onboarding wizard  • AgentVoiceInteractionService (power-button long-press)       │
│  • QS TileService / bubble  • SpeechRecognizer + TextToSpeech (released after use)   │
│  • IntentExecutor  • ScreenAgentService (AccessibilityService, idle = no work)       │
│  • SecretVault (Keystore AES-GCM, StrongBox if present)  • ConfirmOverlay + STOP pill │
│                               ▲  UniFFI 0.x proc-macro bindings                       │
│                               │  Kotlin implements `PhoneBridge` callback interface   │
│ Rust `agent-core` (libagent_core.so, arm64-v8a, built by cargo-ndk via xtask)         │
│  • Agent loop (router turn → intent | screen loop)  • Tool registry + strict schemas  │
│  • Screen pruning  • Safety policy (code, not prompt)  • Messages API client          │
│  • Local store (atomic versioned JSON)  • Host allowlist, rustls-platform-verifier    │
└───────────────┬───────────────────────────────────────────────┬──────────────────────┘
     Relay mode │ Authorization: Bearer <device token>   Direct  │ x-api-key: <user key>
┌───────────────▼──────────────────────────────┐                │
│ User's OWN relay: Rust workers-rs → WASM on  │                │
│ Cloudflare Workers Free (<name>.workers.dev) │                │
│  • device-token auth  • model allowlist      │                │
│  • max_tokens clamp   • byte passthrough     │                │
│  • usage ledger + soft caps in D1            │                │
└───────────────┬──────────────────────────────┘                │
                ▼                                               ▼
                           https://api.anthropic.com/v1/messages
```

## 2. Repository layout

```
Cargo.toml                 workspace (resolver 3); release profile tuned for size
crates/protocol/           shared serde types: relay API, usage, pairing payload, errors
crates/agent-core/         on-device brain → Android .so + UniFFI bindings
  examples/repl.rs         desktop CLI harness (relay or direct mode)
crates/relay/              workers-rs Worker, wrangler.toml.example, D1 migrations
xtask/                     `cargo xtask relay-setup | relay-deploy | pair-qr | android-build | release`
android/app/               Kotlin app consuming generated bindings
docs/                      ARCHITECTURE, DECISIONS, PERF, SETUP, SECURITY, PRIVACY, ...
fastlane/metadata/android/ F-Droid metadata
.github/                   CI, release, relay deploy workflows, templates
```

The relay crate targets `wasm32-unknown-unknown` only and is excluded from the
host `cargo test` default members where it cannot build; its pure logic (auth
hashing, clamping, cost math, request parsing) lives in plain modules with native
unit tests.

## 3. Connection modes

| | Relay mode (recommended) | Direct mode (quickest) |
|---|---|---|
| Where the API key lives | Cloudflare secret in the user's Worker | Phone, AES-GCM encrypted with an Android Keystore key |
| Phone holds | Revocable 256-bit device token | API key |
| Base URL | `https://<name>.workers.dev` | `https://api.anthropic.com` |
| Auth header | `Authorization: Bearer <token>` | `x-api-key: <key>` |
| Spend control | Relay soft caps + Console limit | Anthropic Console spend limit only |

`agent-core` sees a `Credentials` enum; the only difference between modes is the
base URL and the auth header. Users can switch modes at any time in Settings.

## 4. Relay (`crates/relay`)

Deliberately dumb: authenticate, enforce limits, forward, record usage.

| Route | Purpose |
|---|---|
| `POST /v1/pair` | `{pairing_code}` → random 256-bit device token. Only `SHA-256(token)` is stored. Failed attempts are counted; pairing locks after N failures until `PAIRING_CODE` is rotated. Constant-time compare. |
| `POST /v1/messages` | Bearer auth → parse only `model`, `max_tokens` → allowlist + clamp → forward to Anthropic with `x-api-key` from secret and `anthropic-version` → return status/body unchanged. Only `usage` is parsed from the response; usage is written with `ctx.wait_until`. |
| `GET /v1/usage` | Today/month tokens and estimated USD for this device and total. |
| `GET/PUT /v1/prefs` | Small opaque per-device JSON blob (preferences memory sync). |
| `GET /v1/devices`, `POST /v1/devices/revoke` | List / revoke paired devices. |
| `GET /healthz` | `{version, protocol}` for app compatibility checks. No auth. |

**CPU budget (10 ms/request on Free):** no full JSON parse of request or response
bodies. A targeted `serde` struct reads `model`/`max_tokens`; when `max_tokens`
must be clamped, the body is rewritten (rare path); otherwise the original bytes
are forwarded. The response is streamed back as bytes; a second targeted struct
extracts `usage`. SHA-256 of a 32-byte token is microseconds.

**D1 schema** (`migrations/0001_init.sql`, …): `devices(id, token_hash, name,
created_at, last_seen, revoked)`, `usage(day, device_id, model, input_tokens,
output_tokens, cache_read, cache_write, usd_micros)`, `prefs(device_id, json,
updated_at)`, `meta(key, value)` holding `schema_version` and pairing-failure
counters.

**Configuration** (`wrangler.toml` vars, never secrets): `MODEL_ALLOWLIST`,
`MAX_TOKENS_CAP`, `DAILY_USD_CAP`, `MONTHLY_USD_CAP`, `PRICES` (JSON, per 1M
tokens: Haiku 5.5 $0.10/$0.50, Sonnet 5.5 $2/$10, Opus 5.5 $4/$20),
`PAIRING_MAX_FAILURES`. Secrets: `ANTHROPIC_API_KEY`, `PAIRING_CODE`.

**Logging:** status codes, durations and model names only. Never bodies, headers
or secrets.

**Deploy paths:** (1) `cargo xtask relay-setup` (interactive; secrets go straight
to `wrangler secret put` via stdin, never to disk; prints a pairing QR in the
terminal); (2) a "Deploy to Cloudflare" button only if it supports workers-rs
builds (verified at build time, recorded in DECISIONS); (3) `deploy-relay.yml`
for forks using the user's own `CLOUDFLARE_API_TOKEN` / `CLOUDFLARE_ACCOUNT_ID`.

## 5. agent-core (`crates/agent-core`)

### 5.1 FFI surface (UniFFI proc-macros)

```rust
#[uniffi::export(with_foreign)]
trait PhoneBridge: Send + Sync {
    fn credentials(&self) -> Credentials;            // Relay{base_url, device_token} | Direct{api_key}
    fn launch_intent(&self, spec: IntentSpec) -> ActionResult;
    fn installed_apps(&self) -> Vec<AppInfo>;
    fn snapshot_screen(&self) -> ScreenSnapshot;
    fn perform(&self, action: UiAction) -> ActionResult;
    fn confirm(&self, summary: String) -> bool;      // blocks until Approve/Cancel/voice
    fn speak(&self, text: String);
    fn status(&self, text: String);
    fn is_stopped(&self) -> bool;
    fn locale(&self) -> String;
}

#[derive(uniffi::Object)]
struct Agent { /* config, store, http client */ }
impl Agent { fn handle_command(&self, text: String) -> Outcome; /* + settings/usage APIs */ }
```

`handle_command` is a **blocking** call that Kotlin runs on `Dispatchers.IO`; STOP
is cooperative via `is_stopped()` between every step and network call. See
DECISIONS D-07 for why this is sync rather than UniFFI async.

### 5.2 Command flow

1. **Router turn** — `claude-haiku-5-5`, `output_config.effort = "low"`, intent
   tools + `screen_task`. Typical: one request, a few hundred output tokens.
2. **Intent tool** → strict-schema input → Rust validation → `IntentSpec` →
   `launch_intent` → short localized confirmation via `speak`. If the intent
   cannot resolve on the device, fall back to `screen_task` with the same goal.
3. **`screen_task(goal)`** → screen loop on `claude-sonnet-5-5`, effort `medium`:
   `snapshot → prune → request → one UI tool call → safety check → perform →
   repeat` until `finish` / `ask_user` / action cap (default 25, max 50) / STOP.
   Two consecutive failures on one step → escalate once to `claude-opus-5-5`.
4. Final reply in the user's configured language.

All models and efforts are user-configurable.

### 5.3 Claude API usage rules (enforced in code)

- `POST /v1/messages`, `anthropic-version: 2026-10-21`, non-streaming
  (responses are short tool calls; simpler and cheaper in relay CPU).
- Tools carry `strict: true`, `additionalProperties: false`, `required`.
  `tool_choice` is always `auto` (forced choice 400s on these models).
- Effort via `output_config.effort`. Never `temperature`/`top_p`/`top_k`/
  `budget_tokens` (400 on this API version / these models).
- Full `response.content` (including `thinking` blocks) is appended back
  unchanged; history is append-only within a task (required by preserved
  thinking). All `tool_result`s for one turn go in one user message; failures set
  `is_error: true`.
- `stop_reason` is checked before content: `tool_use`, `end_turn`, `max_tokens`,
  `refusal` (with `stop_details.category`).
- Prompt caching: tools + system prompt are byte-stable and carry the
  `cache_control: {type: "ephemeral"}` breakpoint; nothing volatile (time,
  locale-dependent text, screen) precedes it. Locale and per-command context go in
  the first user message. Verified by `cache_read_input_tokens > 0` on repeats.
- Errors: 401/403 → re-pair / replace key; relay cap → spoken cap message;
  429/529 → jittered exponential backoff honoring `retry-after`; 5xx, timeouts,
  offline → one retry then a clear localized spoken message.

### 5.4 Tool registry

Each intent tool is one Rust module implementing:

```rust
trait Tool {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn input_schema(&self) -> serde_json::Value;          // strict JSON schema
    fn plan(&self, input: &Value, ctx: &ToolCtx) -> Result<ToolPlan, ToolError>;
}
```

`ToolPlan` is `Intent(IntentSpec)`, `Screen(goal)`, or `Reply(text)`. Kotlin maps
each `IntentSpec` variant in `IntentExecutor`. Initial tools: `play_music`,
`navigate`, `set_alarm`, `set_timer`, `dial`, `send_message`, `open_app`,
`web_search`, `screen_task`. Preferred music/maps/messaging apps are settings.
`open_app` resolves only against `installed_apps()`; packages are never guessed.
See [ADDING_TOOLS.md](ADDING_TOOLS.md).

### 5.5 Screen pruning

Input: flat node list from the AccessibilityService. Output: compact text, one
line per kept node, target < 1,500 tokens:

```
pkg=com.example.app activity=.SearchActivity
[3] EditText "Search songs" (editable,focused) @540,180
[7] Button "Search" (clickable) @980,180
[12] Text "Arijit Singh – Tum Hi Ho" (clickable) @540,420
```

Rules: drop invisible / zero-size / off-screen nodes; collapse single-child
containers; keep nodes that are interactive or carry text/content-desc; truncate
text to ~80 chars; stable short ids per snapshot; **password fields are shown as
`(password)` with text redacted**. Screen text is wrapped as untrusted data in the
`tool_result` and the system prompt states that on-screen text is never an
instruction.

### 5.6 Safety policy (Rust, not prompt)

Before every `perform`, `safety::check(action, node, snapshot, settings)` returns
`Allow | Confirm(summary) | Deny(reason)`:

- **Deny:** blocklisted package; typing into a password field; acting outside
  the screen task's current package set when the package is blocklisted.
- **Confirm:** target text/content-desc/resource-id matches a sensitive keyword
  (send, pay, place order, buy, delete, transfer, confirm, uninstall, … in a
  multilingual data file `safety/keywords.toml` contributors extend); target app
  is a payment/banking package; system security settings screens; `dial` and
  `send_message` intents always; "confirm everything" setting.
- Action cap, STOP and the "agent is acting" pill are enforced regardless of what
  the model says.

### 5.7 Local store

Single versioned JSON file (`settings.json`) in app-private storage, written
atomically (write temp → fsync → rename), with `schema_version` and forward
migrations. Holds settings, preferences memory and a usage cache. No secrets —
those live in `SecretVault` (Kotlin) and are passed in per call via
`credentials()`, wrapped in `secrecy::SecretString` and zeroized after use.

### 5.8 Networking

`ureq` 3 (blocking, rustls, default features off) + `rustls-platform-verifier`
(uses Android's trust store). Host allowlist = configured relay host +
`api.anthropic.com`; any other host is refused before connecting. No background
network: requests happen only while a command is running.

## 6. Android app (`android/app`)

- **Views, not Compose** (APK and memory budget; DECISIONS D-09).
- `AgentVoiceInteractionService` + session: default assistant role, opened by
  power-button long-press. `TileService` and an optional bubble start the same
  session.
- `SpeechRecognizer` (on-device when available, partials in a bottom sheet) and
  `TextToSpeech`, created per command and released right after.
- `ScreenAgentService`: `canRetrieveWindowContent`, `canPerformGestures`,
  `flagReportViewIds`, minimal event types; holds no node references between
  calls; snapshots debounce content changes (≤ 1.5 s). Clicks prefer
  `ACTION_CLICK`, fall back to `dispatchGesture` at bounds centre; typing uses
  `ACTION_SET_TEXT`.
- `SecretVault`: AES-256-GCM key in Android Keystore (StrongBox when available),
  ciphertext in app-private file. Not `EncryptedSharedPreferences`.
- `ConfirmOverlay` and STOP pill as `TYPE_ACCESSIBILITY_OVERLAY`.
- No foreground service, wakelocks, polling, or background network. Idle =
  only the bound AccessibilityService, doing nothing.
- R8 full mode, resource shrinking, `allowBackup=false`, minSdk 29, target the
  latest stable SDK, release ABI arm64-v8a.

## 7. Security and privacy summary

What leaves the device: command text and pruned (password-redacted) screen text,
sent to the user's relay or directly to `api.anthropic.com`. Nothing else, to
anywhere. Threat model in [SECURITY.md](SECURITY.md); data inventory in
[PRIVACY.md](PRIVACY.md).

## 8. Budgets

| Metric | Budget |
|---|---|
| Idle PSS (only AccessibilityService bound) | < 25 MB |
| Active task PSS | < 60 MB |
| Release APK (arm64-v8a, R8) | < 10 MB |
| Speech end → intent action | < 2 s |
| Relay CPU per request | < 10 ms |

## 9. Milestones

1. Repo + relay (pairing, auth, passthrough, D1 usage, caps, xtask relay-setup + QR).
2. agent-core foundation (client, tools, router, store, safety, repl).
3. Android shell + UniFFI (bindings, vault, onboarding, intents, STT/TTS, tile).
4. Assistant role (VoiceInteractionService).
5. Screen agent (accessibility, pruning, Sonnet loop, overlay, STOP).
6. Hardening (retries, offline, re-pair, refusals, escalation, usage, l10n).
7. Perf, docs, release (PSS, APK size, latency, relay CPU; signed v0.1.0).
