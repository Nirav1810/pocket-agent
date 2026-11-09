# Adding a tool

A *tool* is something the router model can ask the phone to do with one
Android intent: "set a timer", "play music", "navigate". Anything that needs
tapping through an app's UI is handled by `screen_task` instead; you don't need
a tool for it.

Adding one takes three pieces: a Rust module, a Kotlin mapping, and tests.
This walkthrough adds a hypothetical `flashlight(on)` tool.

## 1. Decide the intent

Find a public, documented Android intent (or API) that does the job without
extra permissions, and works across manufacturers. Check `resolveActivity` on a
few phones. If no such intent exists, the feature belongs to `screen_task`.

## 2. Add an `IntentSpec` variant

`crates/agent-core/src/types.rs`:

```rust
pub enum IntentSpec {
    // …
    /// Turn the torch on or off.
    Flashlight {
        /// Desired state.
        on: bool,
    },
}
```

Keep fields validated and minimal: Kotlin should not have to re-check them.

## 3. Write the tool module

`crates/agent-core/src/tools/flashlight.rs`:

```rust
//! `flashlight(on)` → CameraManager.setTorchMode (via IntentExecutor).

use serde::Deserialize;
use serde_json::Value;

use super::{Tool, ToolCtx, ToolError, ToolPlan, parse, say, schema};
use crate::types::IntentSpec;

/// Toggle the flashlight.
pub struct Flashlight;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    on: bool,
    say: String,
}

impl Tool for Flashlight {
    fn name(&self) -> &'static str {
        "flashlight"
    }

    fn description(&self) -> &'static str {
        "Turn the phone's flashlight (torch) on or off (\"turn on the flashlight\")."
    }

    fn input_schema(&self) -> Value {
        schema::object(&[
            ("on", serde_json::json!({"type": "boolean", "description": "true = on, false = off."})),
            ("say", schema::say()),
        ])
    }

    fn plan(&self, input: &Value, _ctx: &ToolCtx<'_>) -> Result<ToolPlan, ToolError> {
        let i: Input = parse(input)?;
        Ok(ToolPlan::Intent { spec: IntentSpec::Flashlight { on: i.on }, say: say(&i.say), follow_up: None })
    }
}
```

Rules:

- **Strict schema.** Use the helpers in `tools/schema.rs`. Every property is
  listed in `required`; optional values are nullable (`schema::nullable_string`).
  Strict mode does not support `minimum`, `maxLength`, `pattern` and similar,
  so validate ranges and lengths in `plan()` (see `set_alarm.rs`).
- **Every tool has `say`**: a short confirmation the model writes in the
  user's language. Rust speaks it after the intent succeeds.
- **Never trust names.** If the tool refers to an app, resolve it with
  `apps::find(ctx.apps(), name)`; never let the model supply a package name.
- **Errors are for the model.** `ToolError` messages go back to Claude as an
  `is_error` tool result, so phrase them as facts it can act on
  ("no installed app matches \"X\"; similar: …").
- **Descriptions say when *not* to use the tool**, so the router chooses well.

## 4. Register it

In `tools/mod.rs`, add `mod flashlight;` and append `Box::new(flashlight::Flashlight)`
to `router_tools()`. **Append at the end**: the tool list is part of the cached
prompt prefix, and keeping existing tools in place keeps diffs small.

## 5. Safety

If the action sends, buys, deletes, calls, or changes security settings, add a
rule in `safety.rs` `check_intent` returning `Verdict::Confirm(..)`, a
matching `Reason`, and a confirmation template in `i18n.rs` (English plus any
languages you can provide). Add a test in `safety.rs`.

## 6. Kotlin mapping

In `android/app/src/main/java/.../IntentExecutor.kt` (Milestone 3), add a
branch for the new `IntentSpec` variant. Build the intent, check
`resolveActivity`, and return `ActionResult(ok = false, message = "…")` if
nothing can handle it, so the agent can fall back to `screen_task`.

## 7. Tests

- Unit tests in the tool module: valid input → expected `ToolPlan`; each
  invalid input → `ToolError`.
- The registry test (`tools::tests::every_tool_has_a_valid_strict_schema`)
  checks your schema automatically.
- For non-trivial flows, add a router test in `tests/router.rs` with a fixture
  response in `tests/fixtures/`.

Run:

```sh
cargo test -p pocket-agent-core
cargo clippy -p pocket-agent-core --all-targets -- -D warnings
```

Then try it for real:

```sh
ANTHROPIC_API_KEY=… cargo run -p pocket-agent-core --example repl
> turn on the flashlight
```
