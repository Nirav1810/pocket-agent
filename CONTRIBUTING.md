# Contributing to Pocket Agent

Thanks for helping! This guide gets you from clone to a green test run.

## Ground rules

- Be kind; see the [Code of Conduct](CODE_OF_CONDUCT.md).
- Nothing secret or user-specific in the repo: no API keys, tokens, account
  ids or personal URLs. CI runs `gitleaks` on every push.
- No telemetry, analytics or network calls beyond the user's relay and
  `api.anthropic.com`.
- Language split: Rust for everything except Android-platform glue (Kotlin).
  No Java, Python, or hand-written JS/TS.

## Dev setup

### All platforms

```sh
rustup toolchain install stable
rustup target add wasm32-unknown-unknown
```

Node.js 20+ is needed only for relay work (it runs `wrangler` via `npx`).

### Linux / macOS

Nothing extra for the Rust crates. Android work (Milestone 3+): Android Studio
or the command-line SDK, NDK r27+, and `cargo install cargo-ndk`.

### Windows

Works natively (PowerShell or Git Bash). Use the MSVC Rust toolchain.
`cargo xtask` commands replace shell scripts, so there is nothing to translate.

## Running checks

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p pocket-agent-relay --target wasm32-unknown-unknown -- -D warnings
cargo test --workspace
cargo deny check                 # cargo install cargo-deny
cargo xtask relay-it             # relay under `wrangler dev` vs a mock upstream
```

`relay-it` needs Node.js but no Cloudflare account or API key.

## Code standards

- No `unwrap()`/`expect()`/`panic!` outside tests (enforced by clippy lints).
- Errors with `thiserror` in libraries; `anyhow` only in `xtask`.
- Never log secrets or message content at info level or above.
- Secrets in `secrecy`/`zeroize` types.
- Public items get doc comments; every module gets a module-level doc.
- Every tool, relay route and safety rule needs tests.
- Prefer clear code over clever code; explain any trick in a comment.

## Commits and PRs

- [Conventional Commits](https://www.conventionalcommits.org/):
  `feat(relay): …`, `fix(agent-core): …`, `docs: …`, `test: …`, `chore: …`.
- One logical change per PR; fill in the PR template checklist.
- Record notable design choices in `docs/DECISIONS.md`.

## Adding a tool

See [docs/ADDING_TOOLS.md](docs/ADDING_TOOLS.md) (Milestone 2): one Rust module
implementing `Tool`, a Kotlin mapping in `IntentExecutor`, and tests.

## Translations

User-facing strings live in `android/app/src/main/res/values/strings.xml`
(Milestone 3). Add `values-<lang>/strings.xml`; safety keyword lists for new
languages live in `crates/agent-core/safety/keywords.toml`.
