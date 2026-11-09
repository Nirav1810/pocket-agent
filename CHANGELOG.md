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
- Shared `protocol` crate (relay API types, pairing QR payload).
- CI: fmt, clippy (native + wasm32), tests, cargo-deny, relay integration test
  under `wrangler dev`, gitleaks.
