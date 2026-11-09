//! `open_app(name)` → launch an installed app (resolved from `installed_apps()`).

use serde::Deserialize;
use serde_json::Value;

use super::{Tool, ToolCtx, ToolError, ToolPlan, apps, parse, say, schema, text};
use crate::types::IntentSpec;

/// Open an app.
pub struct OpenApp;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    name: String,
    say: String,
}

impl Tool for OpenApp {
    fn name(&self) -> &'static str {
        "open_app"
    }

    fn description(&self) -> &'static str {
        "Open an installed app by name (\"open Instagram\"). Only for just opening an app; \
         if the user also wants something done inside it, use screen_task instead."
    }

    fn input_schema(&self) -> Value {
        schema::object(&[
            ("name", schema::string("App name as spoken.")),
            ("say", schema::say()),
        ])
    }

    fn plan(&self, input: &Value, ctx: &ToolCtx<'_>) -> Result<ToolPlan, ToolError> {
        let i: Input = parse(input)?;
        let name = text("name", &i.name, 60)?;
        let app = apps::find(ctx.apps(), &name).ok_or_else(|| {
            let near = apps::suggestions(ctx.apps(), &name, 5);
            ToolError::NotFound(if near.is_empty() {
                format!("no installed app matches \"{name}\"")
            } else {
                format!("no installed app matches \"{name}\"; similar: {near}")
            })
        })?;
        Ok(ToolPlan::Intent {
            spec: IntentSpec::OpenApp {
                package: app.package.clone(),
                label: app.label.clone(),
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
    use crate::tools::apps::sample_apps;
    use serde_json::json;

    #[test]
    fn resolves_or_suggests() {
        let s = Settings::default();
        let ctx = ToolCtx::with_apps(&s, sample_apps());
        let p = OpenApp
            .plan(&json!({"name":"swiggy","say":"Opening Swiggy."}), &ctx)
            .unwrap();
        assert!(
            matches!(p, ToolPlan::Intent { spec: IntentSpec::OpenApp { ref package, .. }, .. } if package == "in.swiggy.android")
        );
        let e = OpenApp
            .plan(&json!({"name":"Mapsy Pro","say":""}), &ctx)
            .unwrap_err();
        assert!(e.to_string().contains("similar: Maps, Maps Go"), "{e}");
    }
}
