//! `web_search(query)` → `ACTION_WEB_SEARCH` in the user's browser.

use serde::Deserialize;
use serde_json::Value;

use super::{Tool, ToolCtx, ToolError, ToolPlan, parse, say, schema, text};
use crate::types::IntentSpec;

/// Search the web.
pub struct WebSearch;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    query: String,
    say: String,
}

impl Tool for WebSearch {
    fn name(&self) -> &'static str {
        "web_search"
    }

    fn description(&self) -> &'static str {
        "Open a web search for the user (\"search for vegan restaurants nearby\", \"look up the IPL score\") \
         when they want to see results, or for factual questions about current events you cannot answer."
    }

    fn input_schema(&self) -> Value {
        schema::object(&[
            ("query", schema::string("Search query.")),
            ("say", schema::say()),
        ])
    }

    fn plan(&self, input: &Value, _ctx: &ToolCtx<'_>) -> Result<ToolPlan, ToolError> {
        let i: Input = parse(input)?;
        Ok(ToolPlan::Intent {
            spec: IntentSpec::WebSearch {
                query: text("query", &i.query, 300)?,
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
    fn plans_search() {
        let s = Settings::default();
        let ctx = ToolCtx::with_apps(&s, vec![]);
        assert!(
            WebSearch
                .plan(&json!({"query":"weather","say":""}), &ctx)
                .is_ok()
        );
        assert!(WebSearch.plan(&json!({"query":"","say":""}), &ctx).is_err());
    }
}
