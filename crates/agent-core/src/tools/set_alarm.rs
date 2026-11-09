//! `set_alarm(hour, minute, label?)` → `AlarmClock.ACTION_SET_ALARM`.

use serde::Deserialize;
use serde_json::Value;

use super::{Tool, ToolCtx, ToolError, ToolPlan, opt_text, parse, say, schema};
use crate::types::IntentSpec;

/// Set an alarm.
pub struct SetAlarm;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    hour: i64,
    minute: i64,
    label: Option<String>,
    say: String,
}

impl Tool for SetAlarm {
    fn name(&self) -> &'static str {
        "set_alarm"
    }

    fn description(&self) -> &'static str {
        "Set an alarm at a clock time (\"wake me up at 6:30\", \"alarm for 7 pm\"). \
         Use 24-hour time: 7 pm is hour 19. For a duration (\"in 20 minutes\") use set_timer."
    }

    fn input_schema(&self) -> Value {
        schema::object(&[
            ("hour", schema::integer("Hour, 0-23 (24-hour clock).")),
            ("minute", schema::integer("Minute, 0-59.")),
            (
                "label",
                schema::nullable_string("Alarm label if the user gave one, else null."),
            ),
            ("say", schema::say()),
        ])
    }

    fn plan(&self, input: &Value, _ctx: &ToolCtx<'_>) -> Result<ToolPlan, ToolError> {
        let i: Input = parse(input)?;
        let hour = u8::try_from(i.hour)
            .ok()
            .filter(|h| *h <= 23)
            .ok_or_else(|| ToolError::Invalid("hour must be 0-23".into()))?;
        let minute = u8::try_from(i.minute)
            .ok()
            .filter(|m| *m <= 59)
            .ok_or_else(|| ToolError::Invalid("minute must be 0-59".into()))?;
        let label = opt_text("label", i.label.as_deref(), 60)?;
        Ok(ToolPlan::Intent {
            spec: IntentSpec::SetAlarm {
                hour,
                minute,
                label,
            },
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
    fn validates_time() {
        let s = Settings::default();
        let ctx = ToolCtx::with_apps(&s, vec![]);
        let ok = SetAlarm
            .plan(
                &json!({"hour":6,"minute":30,"label":null,"say":"Alarm set."}),
                &ctx,
            )
            .unwrap();
        assert_eq!(
            ok,
            ToolPlan::Intent {
                spec: IntentSpec::SetAlarm {
                    hour: 6,
                    minute: 30,
                    label: None
                },
                say: "Alarm set.".into(),
                follow_up: None
            }
        );
        for (h, m) in [(24, 0), (-1, 0), (6, 60), (6, -5)] {
            assert!(
                SetAlarm
                    .plan(&json!({"hour":h,"minute":m,"label":null,"say":""}), &ctx)
                    .is_err(),
                "{h}:{m}"
            );
        }
    }
}
