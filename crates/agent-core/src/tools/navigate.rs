//! `navigate(destination, mode?)` → turn-by-turn navigation in the preferred maps app.

use serde::Deserialize;
use serde_json::Value;

use super::{Tool, ToolCtx, ToolError, ToolPlan, parse, say, schema, text};
use crate::types::{IntentSpec, TravelMode};

/// Start navigation.
pub struct Navigate;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    destination: String,
    mode: Option<String>,
    say: String,
}

impl Tool for Navigate {
    fn name(&self) -> &'static str {
        "navigate"
    }

    fn description(&self) -> &'static str {
        "Start turn-by-turn navigation to a place (\"take me to the airport\", \"directions to Mom's office\"). \
         Use the destination as the user said it; the maps app resolves it."
    }

    fn input_schema(&self) -> Value {
        schema::object(&[
            (
                "destination",
                schema::string("Place name or address as spoken."),
            ),
            (
                "mode",
                schema::nullable_enum(
                    &["driving", "walking", "bicycling", "transit"],
                    "Travel mode if the user said one, else null (driving).",
                ),
            ),
            ("say", schema::say()),
        ])
    }

    fn plan(&self, input: &Value, ctx: &ToolCtx<'_>) -> Result<ToolPlan, ToolError> {
        let i: Input = parse(input)?;
        let destination = text("destination", &i.destination, 200)?;
        let mode = match i.mode.as_deref() {
            None | Some("driving") => TravelMode::Driving,
            Some("walking") => TravelMode::Walking,
            Some("bicycling") => TravelMode::Bicycling,
            Some("transit") => TravelMode::Transit,
            Some(other) => return Err(ToolError::Invalid(format!("unknown mode `{other}`"))),
        };
        Ok(ToolPlan::Intent {
            spec: IntentSpec::Navigate {
                destination,
                mode,
                package: ctx.settings.maps_app.clone(),
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
    fn plans_navigation() {
        let s = Settings {
            maps_app: Some("net.osmand".into()),
            ..Default::default()
        };
        let ctx = ToolCtx::with_apps(&s, vec![]);
        let p = Navigate
            .plan(
                &json!({"destination":"the airport","mode":"transit","say":"Navigating."}),
                &ctx,
            )
            .unwrap();
        assert!(matches!(
            p,
            ToolPlan::Intent { spec: IntentSpec::Navigate { mode: TravelMode::Transit, package: Some(ref p), .. }, .. }
                if p == "net.osmand"
        ));
        let p = Navigate
            .plan(&json!({"destination":"home","mode":null,"say":""}), &ctx)
            .unwrap();
        assert!(matches!(
            p,
            ToolPlan::Intent {
                spec: IntentSpec::Navigate {
                    mode: TravelMode::Driving,
                    ..
                },
                ..
            }
        ));
        assert!(
            Navigate
                .plan(&json!({"destination":"x","mode":"teleport","say":""}), &ctx)
                .is_err()
        );
    }
}
