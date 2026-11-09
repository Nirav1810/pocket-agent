# Performance

Budgets (from ARCHITECTURE §8) and measurements. Every number states how and
when it was measured. "Pending" means the milestone that produces it has not
landed yet.

| Metric | Budget | Measured | Status |
|---|---|---|---|
| Relay CPU per request | < 10 ms | ≈ 0.05 ms native hot path (see below); deployed `cpuTime` pending first live deploy | on track |
| Idle PSS | < 25 MB | pending (M7) | — |
| Active task PSS | < 60 MB | pending (M7) | — |
| Release APK (arm64-v8a) | < 10 MB | pending (M3/M7) | — |
| Speech end → intent action | < 2 s | pending (M3/M7) | — |

## Relay (Milestone 1, 2026-11-09)

**Build artefacts** (`worker-build --release`, worker 0.8.7, wasm-opt):
`index_bg.wasm` 481 KB, `index.js` shim 26 KB. Workers Free has no compressed
size limit that this approaches (64 MiB uncompressed limit, verified 2026-11-09).

**CPU work per request, native** (`cargo run --release -p pocket-agent-relay
--example hotpath_bench`, x86-64 Windows dev machine, 34 KB Sonnet screen-loop
request):

| Path | Native time |
|---|---|
| Bearer parse + SHA-256 + request inspect + allowlist + usage parse + cost | 0.046 ms |
| `max_tokens` clamp (rare: full JSON re-serialise) | 0.665 ms |

WASM under workerd is typically 1.5–3× slower than native, and the runtime
adds copies of the request and response bytes between JS and WASM. Even at 10×
the hot path stays below 1 ms, an order of magnitude under the 10 ms budget.
Body size is capped by `MAX_BODY_BYTES` (2 MiB) so a pathological request cannot
blow the budget.

**Local round-trip** (`cargo xtask relay-it`, `wrangler dev --local`, instant
mock upstream, includes local D1 auth lookup + loopback HTTP): median 21.7 ms,
p90 32 ms wall time. This is wall time dominated by local I/O, not CPU.

**How to measure the real CPU time on your deployment:**

```sh
cd crates/relay
npx wrangler tail --format json | grep -o '"cpuTime":[0-9]*'
```

or Cloudflare dashboard → Workers → your relay → Metrics → *CPU time*
(observability is enabled in the template). Record p50/p99 here.
