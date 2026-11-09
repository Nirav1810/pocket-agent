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

## agent-core (Milestone 2, 2026-11-09)

- Release library (`cargo build --release -p pocket-agent-core`, x86-64
  Windows, opt-level z, LTO, includes rustls+ring, ureq, UniFFI scaffolding):
  **1.7 MB**. The arm64 Android `.so` is measured in Milestone 3.
- Router request: tools + system prompt are byte-identical across commands
  (tested), so after the first command only the user message is uncached.
  Live `cache_read_input_tokens` numbers are pending a live REPL run.
- `relay-it` re-run on 2026-11-09 under heavier machine load: median 63.6 ms
  (vs 21.7 ms earlier). Local wall time is noisy; it is not a CPU figure.

### Live router check (direct mode, 2026-11-09)

`examples/repl.rs` against `api.anthropic.com`, Claude Haiku 5.5, effort low,
from a Windows desktop in India (wall time per command, includes TLS + network):

| Command | Wall time | Input | Output | Cache read | Cache write |
|---|---|---|---|---|---|
| Wake me up at 6:30 (first) | 11.0 s | 107 | 125 | 0 | 3,234 |
| Play Arijit Singh on YouTube Music | 2.55 s | 113 | 115 | 3,234 | 0 |
| Message Rahul on WhatsApp… (declined at confirm) | 3.00 s | 115 | 136 | 3,234 | 0 |
| Set a timer for 10 minutes | 1.86 s | 109 | 101 | 3,234 | 0 |

- Prompt caching works: after the first command the whole 3,234-token prefix
  (tools + system) is read from cache; only ~110 tokens per command are uncached.
- Estimated cost: **$0.00079 for all four** (≈ $0.0002 per intent command).
- The first request is slow (cache write plus first-use compilation of the
  strict tool schemas). Warm commands take 1.9–3.0 s, so router latency alone is
  at or above the 2 s speech-to-action budget. Revisit in Milestone 7: keep a
  warm TLS connection while listening, measure on-phone networks, and compare
  thinking-off behaviour at low effort.
