# Decisions

Lightweight decision log. Each entry: the decision, why, and what would change it.
Facts marked *verified* were checked against official docs on the date shown.

## D-01 License — MIT OR Apache-2.0 *(confirmed by maintainer 2026-11-09)*
Rust ecosystem norm; Apache-2.0 adds a patent grant, MIT keeps it simple for reuse.

## D-02 Claude API version `2026-10-21` *(verified 2026-11-09)*
Current `anthropic-version`. On this version, `temperature`/`top_p`/`top_k` return
400 on every model, so the client has no sampling parameters at all.

## D-03 Default models
Router `claude-haiku-5-5` (effort `low`), screen loop `claude-sonnet-5-5` (effort
`medium`), escalation `claude-opus-5-5`. All configurable. Haiku 5.5 and Opus 5.5
default to effort `medium`, so effort is always sent explicitly.

## D-04 No forced tool choice; strict tools
`tool_choice: {type: "auto"}` only (forced `any`/`tool` returns 400 on Sonnet 5.5 /
Opus 5.5). Each tool sets `strict: true`. The prompt instructs "always respond with
exactly one tool call"; a plain-text reply is treated as a spoken answer.

## D-05 Append-only history
Assistant `content` (including `thinking` blocks) is appended unchanged and never
edited within a task (preserved-thinking check rejects edited history). Screen
snapshots are therefore never "replaced" in history; context growth is bounded by
the action cap and pruning instead.

## D-06 Non-streaming requests
Outputs are short tool calls. Non-streaming keeps the relay a byte passthrough with
one small `usage` parse and avoids SSE parsing on device.

## D-07 Blocking FFI instead of UniFFI async
`Agent::handle_command` is synchronous; Kotlin calls it on `Dispatchers.IO`.
Reasons: `ureq` is blocking and small; no async runtime in the `.so` (the spec
forbids multi-thread tokio and a single-thread executor would add size for no
gain); every `PhoneBridge` callback is naturally blocking (confirm waits for the
user). Cancellation is cooperative via `is_stopped()`. Revisit if we need
concurrent requests on device.

## D-08 HTTP client: `ureq` 3 + rustls + `rustls-platform-verifier`
Smaller than `reqwest` (no hyper/tokio). Platform verifier uses the Android trust
store, so user-installed CAs behave like the rest of the OS.

## D-09 Android UI: Views, not Compose
Compose adds several MB and runtime memory; the UI is a wizard, a settings list and
two overlays. Views meet the < 10 MB APK and < 25 MB idle budgets more easily.

## D-10 Local store: atomic versioned JSON
`serde_json` is already a dependency; `redb` would add code size for a few KB of
settings. Write temp → fsync → rename; `schema_version` + migration functions.

## D-11 Relay error for spending caps: HTTP 402
`{"type":"error","error":{"type":"relay_cap_exceeded","message":...}}`. Distinct
from Anthropic's 429 so the app does not back off and retry a cap.

## D-12 Pairing payload
QR encodes `pocketagent://pair?v=1&url=<relay-url>&code=<pairing-code>`. The app
exchanges it for a device token immediately; the pairing code is never stored on
the phone.

## D-13 Cloudflare Workers Free limits *(verified 2026-11-09)*
100,000 requests/day (reset 00:00 UTC), 10 ms CPU per HTTP request, 128 MB memory,
50 subrequests per invocation, 64 env vars/secrets of ≤ 5 KB each. D1 Free: 10
databases, 500 MB per DB, 50 queries per invocation. Each relay request uses ≤ 1
Anthropic subrequest plus ≤ 4 D1 queries.

## D-14 Refusal fallback
On the Claude API, `claude-sonnet-5-5` and `claude-opus-5-5` support server-side
refusal fallbacks (`fallbacks: "default"` with beta `server-side-fallback-2026-07-01`).
The screen loop enables it by default (setting to disable). The relay forwards an
allowlisted set of `anthropic-beta` values. Haiku 5.5 has no server-side fallback;
router refusals are spoken as a refusal message.

## D-15 No "Deploy to Cloudflare" button *(verified 2026-11-09)*
The button requires the Worker to live in a fully isolated directory (no
workspace path dependencies), drives builds through `package.json` scripts, and
its Workers Builds environment does not document Rust support. The relay shares
`crates/protocol` with the app. Self-deploy is `cargo xtask relay-setup`, with
`deploy-relay.yml` for forks. Revisit if Workers Builds documents Rust.

## D-16 Relay does not support `stream: true`
Requests with `stream: true` get HTTP 400. Streaming would need SSE parsing on
the relay to extract `usage` (more CPU, more code) and the app does not stream
(D-06).

## D-17 `wrangler` pinned and run via `npx`
`wrangler@4.152.0` (released 2026-10-22) runs through `npx --yes`, so users do
not install it globally and everyone gets the same version. Bump deliberately.
`WRANGLER_SEND_METRICS=false` is set by xtask.

## D-18 Pairing lockout is global and resets on success or rotation
One failure counter per relay (not per IP: IPs are cheap to rotate and the relay
has a single owner). A successful pairing resets it; rotating `PAIRING_CODE`
resets it (D1 stores a domain-separated SHA-256 fingerprint of the current code
to detect rotation). A lock never affects already-paired devices.

## D-19 Project name and application id
Display name "Pocket Agent"; crates `pocket-agent-*`; Android `applicationId`
`io.github.pocketagent.app` (permanent once released).

## D-20 Relay WASM is not stripped by rustc
`strip = true` removes the `target_features` section wasm-bindgen needs;
`worker-build` runs `wasm-opt` afterwards anyway. Per-package profile override.

## D-21 Default soft caps $2/day, $20/month
Safe defaults for a new relay; users edit `[vars]`. The Anthropic Console spend
limit remains the hard backstop and SETUP.md asks users to set it first.

## D-22 Relay forwards a fixed set of response headers
`content-type`, `request-id`, `retry-after`, `x-should-retry`, and
`anthropic-ratelimit-*`. The app needs `retry-after` for backoff (429/529) and
`request-id` for bug reports; nothing else is useful to it.

## D-23 Prefs are relay-global
`/v1/prefs` stores one object per relay (single-owner relay), so re-pairing or
adding a phone keeps preferences.

## D-24 Strict tool schemas: every property required, optionals nullable
Strict tool use rejects unsupported keywords (`minimum`, `maxLength`, …), so
schemas use `anyOf: [<type>, null]` for optional values and list every property
in `required`; ranges and lengths are validated in Rust (`Tool::plan`). A test
walks every schema and enforces this.

## D-25 Spoken confirmations are written by the model (`say`)
Every router tool has a `say` field: a short confirmation in the reply
language. This gives localised replies for any language Claude speaks, with no
extra request. Rust only generates text for errors and confirmation prompts
(`i18n.rs`, English + Hindi + Spanish so far, English fallback).

## D-26 Safety keywords in JSON, not TOML
`crates/agent-core/safety/keywords.json` is embedded with `include_str!` and
parsed with `serde_json`, which is already a dependency (a TOML parser would add
code size). A test checks it parses and covers the shipped languages.

## D-27 Relay marks its own errors with `x-pocket-relay-error`
A 401 can mean "this phone's token was revoked" (relay) or "the relay's API key
is invalid" (Anthropic, forwarded unchanged). The relay adds
`x-pocket-relay-error: <type>` to errors it generates, so the app can tell
the user the right fix.

## D-28 TLS: rustls + ring + platform verifier
`ureq` with `rustls-no-provider` + `platform-verifier`, `rustls` with the `ring`
provider. Uses Android's trust store (and the OS store on desktop). On Android
the verifier must be initialised from Kotlin with the app `Context`
(`rustls-platform-verifier` Android component); wired up in Milestone 3.
Redirects are disabled, so the client only contacts its configured host.

## D-29 Router: up to 3 turns, one tool call each
`disable_parallel_tool_use: true`. Tool validation errors and failed intent
launches go back to Claude as `is_error` tool results, so it can pick another
tool (often `screen_task`) or explain. Three turns bound cost and latency.

## D-30 Messages: confirm, then compose, then press Send
`send_message` confirms the exact text first, opens the app's compose screen
pre-filled, then (Milestone 5) the screen agent presses Send. Until then the
user taps Send. There is no Android intent that sends silently in third-party
apps, and none should be used.

## D-31 Calls are confirmed in-app
`dial` always asks for confirmation; Kotlin then uses `ACTION_CALL` when the
user granted `CALL_PHONE`, otherwise `ACTION_DIAL` (the dialer then needs one
more tap).
