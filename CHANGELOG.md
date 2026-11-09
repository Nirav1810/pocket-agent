# Changelog

All notable changes are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/). Release notes are generated from
Conventional Commits.

## [Unreleased]

### Added
- Self-hosted relay (Rust, workers-rs → WASM) for the Cloudflare Workers Free
  plan: device pairing with lockout, bearer-token auth, model and beta
  allowlists, `max_tokens` clamp, byte passthrough to the Claude Messages API,
  D1 usage ledger, soft daily/monthly USD caps, preferences sync, device
  listing and revocation, `/healthz` with protocol version.
- `cargo xtask relay-setup | relay-deploy | relay-rotate-code | relay-check |
  pair-qr | relay-it`.
- `agent-core`: Claude Messages API client (relay and direct mode, rustls with
  the platform verifier, host allowlist, retries with `retry-after`, typed
  errors), nine router tools with strict schemas, router turn with
  append-only history and prompt caching, safety policy with multilingual
  keywords, atomic versioned settings store, spoken messages in en/hi/es, UniFFI
  bindings surface (`Agent`, `PhoneBridge`), desktop REPL.
- Relay marks its own errors with `x-pocket-relay-error`.
- Local daily/monthly spending limit enforced by agent-core (default $2/$20),
  shown in the REPL `/usage`; REPL reads keys and limits from a git-ignored `.env`.
- Shared `protocol` crate (relay API types, pairing QR payload).
- CI: fmt, clippy (native + wasm32), tests, cargo-deny, relay integration test
  under `wrangler dev`, gitleaks.
