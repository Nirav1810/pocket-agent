//! `screen_task(goal)` → hand over to the screen agent, which operates the UI.

use serde::Deserialize;
use serde_json::Value;

use super::{Tool, ToolCtx, ToolError, ToolPlan, parse, say, schema, text};

/// Operate apps through the UI when no specific tool fits.
pub struct ScreenTask;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    goal: String,
    say: String,
}

impl Tool for ScreenTask {
    fn name(&self) -> &'static str {
        "screen_task"
    }

    fn description(&self) -> &'static str {
        "Operate the phone's screen step by step (tap, type, scroll) to do something no other tool can, \
         e.g. \"open Swiggy and reorder my last order\", \"turn on dark mode in Instagram\". \
         Slower and costlier than the other tools, so use it only when they cannot do the job. \
         `goal` must be self-contained: include the app and every detail the user gave."
    }

    fn input_schema(&self) -> Value {
        schema::object(&[
            (
                "goal",
                schema::string(
                    "Complete description of what to achieve, including the app to use.",
                ),
            ),
            ("say", schema::say()),
        ])
    }

    fn plan(&self, input: &Value, _ctx: &ToolCtx<'_>) -> Result<ToolPlan, ToolError> {
        let i: Input = parse(input)?;
        Ok(ToolPlan::Screen {
            goal: text("goal", &i.goal, 1000)?,
            say: say(&i.say),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;
    use serde_json::json;

    #[test]
    fn plans_screen_task() {
        let s = Settings::default();
        let ctx = ToolCtx::with_apps(&s, vec![]);
        let p = ScreenTask
            .plan(
                &json!({"goal":"Open Swiggy and reorder the last order","say":"On it."}),
                &ctx,
            )
            .unwrap();
        assert_eq!(
            p,
            ToolPlan::Screen {
                goal: "Open Swiggy and reorder the last order".into(),
                say: "On it.".into()
            }
        );
    }
}
