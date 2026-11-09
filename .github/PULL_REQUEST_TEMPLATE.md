## What and why

<!-- One logical change. Link the issue it closes. -->

## Checklist

- [ ] Conventional Commit title (`feat(scope): …`, `fix(scope): …`, `docs: …`)
- [ ] `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` pass
- [ ] New tool / relay route / safety rule has tests
- [ ] No secrets, personal URLs, account ids or tokens added (gitleaks passes)
- [ ] No new network destinations, telemetry, or Play Services dependency
- [ ] No `unwrap()`/`expect()` outside tests; no secrets or message content logged
- [ ] User-facing strings in `strings.xml` (Android changes)
- [ ] Docs updated (`DECISIONS.md` for design choices, `PERF.md` if budgets are affected)
- [ ] Memory/APK budgets considered (Android changes)
