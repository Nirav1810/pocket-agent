# Pocket Agent

An open-source AI phone agent for Android. Hold the power button, say what you
want, and it gets done: by firing the right Android intent when one exists, or
by operating the screen itself when it doesn't. Powered by Claude, with **your
own** API key.

> **Status: early development (v0.1.0 in progress).** The relay is complete
> (Milestone 1). The on-device agent and Android app are being built. See the
> [roadmap](#roadmap).

<!-- Demo GIF goes here once the app runs end to end. -->

## What it does

- *"Play Arijit Singh on YouTube Music"* → music starts.
- *"Take me to the airport"* → navigation starts in your maps app.
- *"Wake me up at 6:30"* → alarm set.
- *"Message Rahul on WhatsApp that I'm running late"* → drafted, **you confirm**, sent.
- *"Open Swiggy and reorder my last order"* → no intent exists, so the agent
  reads the screen, taps, types and scrolls until done, **asking before payment**.

It answers briefly by voice, in your language.

## Privacy stance

**Your key, your relay, no telemetry.**

- The project runs no servers. No accounts, analytics, crash reporting or telemetry.
- What leaves your phone: your command text and a pruned, password-redacted text
  version of the current screen. It goes to your own relay or straight to
  `api.anthropic.com`, and nowhere else. See [PRIVACY.md](docs/PRIVACY.md).
- No Google Play Services. FOSS dependencies only (F-Droid compatible).

## Safety

- Anything that sends a message, makes a call, spends money, deletes data or
  changes security settings needs your explicit **Approve** (or a spoken "yes").
  This is enforced in code, not just in the prompt.
- A visible "agent is acting" pill with **STOP** whenever it controls the screen.
- Hard cap on actions per task (25 by default). A blocklist of apps it never touches.
- On-screen text is treated as untrusted data (prompt-injection defence).

## Quick start

1. Get an Anthropic API key and set a spend limit: <https://console.anthropic.com>.
2. Choose a connection mode:
   - **Relay (recommended):** deploy your own free Cloudflare Worker with
     `cargo xtask relay-setup`, then scan the QR code it prints. Your key stays in
     Cloudflare; the phone holds only a revocable token.
   - **Direct (quickest):** paste your key into the app; it is encrypted with the
     Android Keystore.
3. Install the APK from GitHub Releases, run the setup wizard, and set Pocket
   Agent as your default assistant.

Full guide: [docs/SETUP.md](docs/SETUP.md).

## Cost

You pay only for your own Claude usage. Rough estimates (2026-11 prices; to be replaced by measurements):

| Command type | Model | Typical cost |
|---|---|---|
| Intent ("set an alarm") | Claude Haiku 5.5 | ~$0.0005 |
| Screen task, ~10 steps | Claude Sonnet 5.5 | ~$0.05–0.25 |

The relay runs on the Cloudflare Workers Free plan ($0). Measured numbers will be
in [PERF.md](docs/PERF.md).

## Supported devices

Android 10 (API 29) and newer, arm64. Some manufacturers (Xiaomi, Oppo, Vivo,
Samsung, …) kill background services aggressively; the wizard links to fixes in
[DEVICE_QUIRKS.md](docs/DEVICE_QUIRKS.md).

Distributed via GitHub Releases (F-Droid metadata included). Not on Google Play,
whose AccessibilityService policy doesn't allow this kind of app.

## Repository layout

```
crates/protocol    shared wire types
crates/agent-core  on-device agent (Rust → Android .so via UniFFI)
crates/relay       your self-hosted Cloudflare Worker (Rust → WASM)
xtask              `cargo xtask …` setup/build/release commands
android            Kotlin app (thin platform layer)
docs               architecture, decisions, setup, security, privacy, perf
```

See [ARCHITECTURE.md](docs/ARCHITECTURE.md) and [DECISIONS.md](docs/DECISIONS.md).

## FAQ

**Why do I need my own API key?** So nobody (including this project) sits between
you and Claude, sees your data, or pays for your usage.

**Is the relay required?** No. Direct mode works without it. The relay keeps the
key off your phone and adds spending caps and multi-device revocation.

**Does it run an LLM on the phone?** No. All reasoning happens in Claude. The
on-device part is a small Rust library plus the Android platform glue.

**Does it keep running in the background?** No. There is no foreground service,
wakelock or polling. Idle, only the accessibility service is bound, doing nothing.

## Roadmap

1. ✅ Repo and self-hosted relay (pairing, auth, passthrough, usage ledger, caps)
2. ☐ agent-core: Claude client, tools, router, safety policy, desktop REPL
3. ☐ Android shell: onboarding, intents, speech, Quick Settings tile
4. ☐ Default assistant (power-button long-press)
5. ☐ Screen agent (accessibility, confirmations, STOP)
6. ☐ Hardening (retries, offline, refusals, localisation)
7. ☐ Performance budgets, docs, signed v0.1.0 release

## Contributing

Contributions are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md) and the
[Code of Conduct](CODE_OF_CONDUCT.md). Report security issues privately: see
[SECURITY.md](docs/SECURITY.md).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT), at your option. Unless you explicitly state
otherwise, any contribution you intentionally submit for inclusion in this work,
as defined in the Apache-2.0 license, is dual licensed as above, without any
additional terms or conditions.
