# Setup

Two ways to connect the app to Claude. You choose in the app's first-run wizard
and can switch at any time.

| | **Relay mode** (recommended) | **Direct mode** (quickest) |
|---|---|---|
| Time | ~10 minutes, once | ~2 minutes |
| Where your API key lives | A secret in **your own** Cloudflare Worker | Encrypted on the phone (Android Keystore) |
| What the phone stores | A revocable device token | Your API key |
| Spending limits | Relay soft caps **and** Anthropic Console limit | Anthropic Console limit |
| Cost | Cloudflare Workers Free plan: $0 | $0 |

Either way you pay only for your own Claude API usage.

## 0. Get an Anthropic API key (both modes)

1. Sign in at <https://console.anthropic.com> and add billing.
2. **Settings → Limits**: set a monthly spend limit you are comfortable with.
3. **Settings → API keys → Create key**. Copy it; you will paste it once.

## 1. Direct mode

In the app: *Connection → Direct → paste key → Test connection*. Done.

The key is encrypted with an AES-256-GCM key held in the Android Keystore
(StrongBox when your phone has it) and is only sent to `api.anthropic.com`.

## 2. Relay mode

The relay is a tiny Rust program that runs on **your** Cloudflare account. The
project hosts nothing.

### 2.1 Prerequisites (on your computer)

| Tool | Why | Install |
|---|---|---|
| Rust (stable) | builds the relay | <https://rustup.rs> |
| `wasm32-unknown-unknown` target | relay compiles to WebAssembly | `rustup target add wasm32-unknown-unknown` |
| Node.js 20+ | runs Cloudflare's `wrangler` CLI (dev-time only) | <https://nodejs.org> |
| A Cloudflare account | free, no credit card needed | <https://dash.cloudflare.com/sign-up> |

Windows, macOS and Linux all work. You do not need to install `wrangler`
yourself; the setup command runs a pinned version via `npx`.

### 2.2 Deploy (one command)

```sh
git clone <this repository>
cd pocket-agent
cargo xtask relay-setup
```

It walks you through ten steps:

1. Checks `wrangler`.
2. Logs you in to Cloudflare (opens your browser) if needed.
3. Asks for a Worker name (default `pocket-agent-relay`).
4. Creates a D1 database `<name>-db` (or reuses it if it exists).
5. Writes `crates/relay/wrangler.toml` (git-ignored) from the template.
6. Applies the database migrations.
7. Builds and deploys the Worker. On a brand-new account `wrangler` asks you
   to pick your free `*.workers.dev` subdomain; answer the prompt.
8. Asks for your Anthropic API key (hidden input) and stores it with
   `wrangler secret put`. It is never written to disk.
9. Generates a random pairing code and stores it the same way.
10. Waits until the relay answers `/healthz`.

It then offers one tiny test request to Claude through the relay, and prints a
**pairing QR code** plus the URL and code for manual entry.

### 2.3 Pair your phone

In the app: *Connection → Relay → Scan QR*. The app exchanges the pairing code
for a device token and keeps only the token.

### 2.4 Day-to-day commands

| Task | Command |
|---|---|
| Lost the pairing code / pair another phone / unlock after too many wrong codes | `cargo xtask relay-rotate-code --url https://<name>.<you>.workers.dev` |
| Update the relay after `git pull` | `cargo xtask relay-deploy` |
| Check a relay end-to-end | `cargo xtask relay-check --url https://…` |
| Print a QR again (you know the code) | `cargo xtask pair-qr --url https://…` |
| Replace the API key | `cd crates/relay && npx wrangler secret put ANTHROPIC_API_KEY` |
| See live request log (no content is logged) | `cd crates/relay && npx wrangler tail` |
| Revoke a lost phone | App → Settings → Devices, or `POST /v1/devices/revoke` |

Rotating the pairing code does not log out already-paired phones.

### 2.5 Configuration

Edit `[vars]` in `crates/relay/wrangler.toml` and run `cargo xtask relay-deploy`.

| Variable | Default | Meaning |
|---|---|---|
| `MODEL_ALLOWLIST` | Haiku 5.5, Sonnet 5.5, Opus 5.5 | Models the relay forwards |
| `BETA_ALLOWLIST` | `server-side-fallback-2026-07-01` | `anthropic-beta` values forwarded |
| `MAX_TOKENS_CAP` | `16000` | Larger `max_tokens` is clamped |
| `DAILY_USD_CAP` / `MONTHLY_USD_CAP` | `2` / `20` | Soft caps (estimates); `0` disables. Over the cap the relay answers HTTP 402 |
| `PAIRING_MAX_FAILURES` | `10` | Wrong codes before pairing locks |
| `PRICES` | see template | USD per 1M tokens for the estimates |
| `MAX_BODY_BYTES` | `2097152` | Largest accepted request |

The caps are estimates computed from `PRICES`; the Anthropic Console limit is
the hard backstop.

### 2.6 Alternative: deploy from your fork with GitHub Actions

See the header of [`.github/workflows/deploy-relay.yml`](../.github/workflows/deploy-relay.yml).
You still set the two secrets once with `wrangler` (they persist across deploys).

### 2.7 Why there is no "Deploy to Cloudflare" button

Checked on 2026-11-09: the button needs the Worker in a self-contained
directory with `package.json` build scripts, and its build environment does not
document Rust support. The relay shares a Rust crate with the app, so the button
cannot build it. See DECISIONS D-15.

### 2.8 Free-plan limits (verified 2026-11-09)

100,000 requests/day, 10 ms CPU per request, 128 MB memory, 50 subrequests per
request; D1: 500 MB per database. A heavy personal user makes a few hundred
requests per day. The relay makes one upstream call and at most three D1 queries
per request.

## 3. Uninstall / delete everything

* Phone: Settings → *Delete all secrets*, then uninstall.
* Relay: `cd crates/relay && npx wrangler delete` and
  `npx wrangler d1 delete <name>-db`.
* Anthropic: delete the API key in the Console.
