# Security

## Reporting a vulnerability

Please **do not** open a public issue. Use GitHub's private reporting:
repository → **Security** → **Report a vulnerability** (GitHub Security
Advisories). We aim to acknowledge within 7 days and to fix or mitigate
critical issues within 30 days, then publish an advisory crediting you (unless
you prefer otherwise).

In scope: the Android app, `agent-core`, the relay, `xtask`, and CI/release
workflows. Out of scope: Cloudflare, Anthropic and Android platform bugs (report
those upstream).

## Secret scanning

- CI runs `gitleaks` over the full history with `.gitleaks.toml`, which adds
  rules for Anthropic keys (`sk-ant-…`) and relay device tokens (`pa_dt_…`).
- Maintainers: enable **Secret scanning** and **Push protection** under
  repository Settings → Code security.
- If you ever commit a secret: revoke it first (Anthropic Console / `cargo xtask
  relay-rotate-code` / revoke the device), then remove it from history.

## Threat model (relay — Milestone 1)

Assets: the user's Anthropic API key; device tokens; the pairing code; usage
data; command and screen text in transit.

| Threat | Mitigation |
|---|---|
| Someone finds the relay URL and spends the user's credit | Every route except `/healthz` and `/v1/pair` needs a device token. Tokens are 256-bit random; only SHA-256 hashes are stored. |
| Brute-forcing the pairing code | ~100-bit random code, constant-time compare, global lock after `PAIRING_MAX_FAILURES` (default 10) wrong attempts until the code is rotated. |
| Attacker locks pairing (DoS) | Only blocks *new* pairings; existing phones keep working. Rotating the code unlocks. |
| Stolen / lost phone | Revoke its token (Settings → Devices or `/v1/devices/revoke`). The API key was never on the phone. |
| Runaway costs (bug, loop, stolen token) | Soft daily/monthly USD caps (HTTP 402), `max_tokens` clamp, model allowlist, plus the user's Console spend limit. |
| Smuggling a different model past the allowlist | Duplicate JSON keys rejected; the relay inspects the same bytes it forwards. |
| API key exfiltration through misconfiguration | Upstream fixed to `https://api.anthropic.com` (only loopback overrides allowed, for local tests). The key is only ever in a Worker secret and the outbound header. |
| Logs leaking content | Relay logs route, status and duration only. |
| Device token sent to Anthropic | The relay builds a fresh header set; `authorization` is never forwarded (integration-tested). |
| Secrets on the developer's disk | `relay-setup` pipes secrets to `wrangler secret put` via stdin; never argv or files. `wrangler.toml`, `.dev.vars` and `.wrangler/` are git-ignored. |

The on-device threat model (prompt injection from screen content, accessibility
abuse, key storage) is added in Milestones 2–5.
