# Fixtures

Messages API responses used by `tests/router.rs`, written from the official
response shape for API version `2026-10-21`. They include empty `thinking`
blocks with signatures (thinking display defaults to `omitted` on current
models), which the agent must replay verbatim.

When behaviour against the live API differs from a fixture, update the fixture
from a real response (run `examples/repl.rs` and capture the body with a local
proxy). Commit only responses; never request bodies or headers, and check them
for personal content first.
