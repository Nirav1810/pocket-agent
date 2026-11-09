//! `set_timer(seconds, label?)` → `AlarmClock.ACTION_SET_TIMER`.

use serde::Deserialize;
use serde_json::Value;

use super::{Tool, ToolCtx, ToolError, ToolPlan, opt_text, parse, say, schema};
use crate::types::IntentSpec;

/// Start a countdown timer.
pub struct SetTimer;

/// Longest timer accepted (24 h).
const MAX_SECONDS: u32 = 86_400;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    seconds: i64,
    label: Option<String>,
    say: String,
}

impl Tool for SetTimer {
    fn name(&self) -> &'static str {
        "set_timer"
    }

    fn description(&self) -> &'static str {
        "Start a countdown timer for a duration (\"timer for 10 minutes\", \"remind me in 20 minutes\"). \
         Convert the duration to seconds (max 24 hours)."
    }

    fn input_schema(&self) -> Value {
        schema::object(&[
            ("seconds", schema::integer("Duration in seconds, 1-86400.")),
            (
                "label",
                schema::nullable_string("Label if given, else null."),
            ),
            ("say", schema::say()),
        ])
    }

    fn plan(&self, input: &Value, _ctx: &ToolCtx<'_>) -> Result<ToolPlan, ToolError> {
        let i: Input = parse(input)?;
        let seconds = u32::try_from(i.seconds)
            .ok()
            .filter(|s| (1..=MAX_SECONDS).contains(s))
            .ok_or_else(|| ToolError::Invalid("seconds must be 1-86400".into()))?;
        let label = opt_text("label", i.label.as_deref(), 60)?;
        Ok(ToolPlan::Intent {
            spec: IntentSpec::SetTimer { seconds, label },
            say: say(&i.say),
            follow_up: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;
    use serde_json::json;

    #[test]
    fn validates_duration() {
        let s = Settings::default();
        let ctx = ToolCtx::with_apps(&s, vec![]);
        assert!(
            SetTimer
                .plan(&json!({"seconds":600,"label":"tea","say":"ok"}), &ctx)
                .is_ok()
        );
        for bad in [0, -5, 86_401] {
            assert!(
                SetTimer
                    .plan(&json!({"seconds":bad,"label":null,"say":""}), &ctx)
                    .is_err()
            );
        }
    }
}
