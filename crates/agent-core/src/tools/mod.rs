//! Tool registry: what Claude can ask the phone to do from the router turn.
//!
//! Each tool is one module implementing [`Tool`]:
//!
//! * a stable `name` and a `description` written for the model,
//! * a **strict** JSON schema (`additionalProperties: false`, every property
//!   listed in `required`, optional values expressed as nullable),
//! * `plan()`, which validates the input in Rust (strict schemas cannot
//!   express ranges or lengths) and turns it into a [`ToolPlan`].
//!
//! The Kotlin side maps each [`IntentSpec`] variant in `IntentExecutor`.
//! See `docs/ADDING_TOOLS.md` for a walkthrough.

pub mod apps;
mod dial;
mod navigate;
mod open_app;
mod play_music;
pub mod schema;
mod screen_task;
mod send_message;
mod set_alarm;
mod set_timer;
mod web_search;

use std::cell::OnceCell;

use serde::de::DeserializeOwned;
use serde_json::Value;
use thiserror::Error;

use crate::bridge::PhoneBridge;
use crate::settings::Settings;
use crate::types::{AppInfo, IntentSpec};

/// Longest accepted spoken confirmation.
pub const MAX_SAY_CHARS: usize = 200;

/// What the agent should do after a tool call validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolPlan {
    /// Fire an intent, then speak `say`.
    Intent {
        /// The intent.
        spec: IntentSpec,
        /// Short confirmation in the reply language (written by the model).
        say: String,
        /// Optional screen step to finish the job (e.g. press Send).
        follow_up: Option<String>,
    },
    /// Hand over to the screen agent.
    Screen {
        /// Self-contained goal.
        goal: String,
        /// Short acknowledgement to speak before starting.
        say: String,
    },
}

/// Why a tool call could not be planned. The message goes back to Claude as
/// an `is_error` tool result, so it is written for the model.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ToolError {
    /// Arguments were syntactically valid but semantically wrong.
    #[error("invalid input: {0}")]
    Invalid(String),
    /// Something the call refers to does not exist on the phone.
    #[error("not found: {0}")]
    NotFound(String),
}

/// Context available while planning a tool call.
pub struct ToolCtx<'a> {
    /// Current settings.
    pub settings: &'a Settings,
    bridge: Option<&'a dyn PhoneBridge>,
    apps: OnceCell<Vec<AppInfo>>,
}

impl<'a> ToolCtx<'a> {
    /// New context. `installed_apps()` is fetched lazily, only by tools that need it.
    pub fn new(settings: &'a Settings, bridge: &'a dyn PhoneBridge) -> Self {
        Self {
            settings,
            bridge: Some(bridge),
            apps: OnceCell::new(),
        }
    }

    /// Context with a fixed app list (tests).
    pub fn with_apps(settings: &'a Settings, apps: Vec<AppInfo>) -> Self {
        let cell = OnceCell::new();
        let _ = cell.set(apps);
        Self {
            settings,
            bridge: None,
            apps: cell,
        }
    }

    /// Installed launchable apps (fetched once per command).
    pub fn apps(&self) -> &[AppInfo] {
        self.apps
            .get_or_init(|| self.bridge.map(|b| b.installed_apps()).unwrap_or_default())
    }
}

/// One tool Claude can call.
pub trait Tool: Send + Sync {
    /// Tool name (snake_case, stable: it is part of the cached prompt).
    fn name(&self) -> &'static str;
    /// Description for the model: when to use it, and when not to.
    fn description(&self) -> &'static str;
    /// Strict JSON schema of the input.
    fn input_schema(&self) -> Value;
    /// Validate input and produce a plan.
    fn plan(&self, input: &Value, ctx: &ToolCtx<'_>) -> Result<ToolPlan, ToolError>;
}

/// All router tools, in a fixed order (order is part of the prompt cache key).
pub fn router_tools() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(play_music::PlayMusic),
        Box::new(navigate::Navigate),
        Box::new(set_alarm::SetAlarm),
        Box::new(set_timer::SetTimer),
        Box::new(dial::Dial),
        Box::new(send_message::SendMessage),
        Box::new(open_app::OpenApp),
        Box::new(web_search::WebSearch),
        Box::new(screen_task::ScreenTask),
    ]
}

/// API tool definitions for a tool list.
pub fn definitions(tools: &[Box<dyn Tool>]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            serde_json::json!({
                "name": t.name(),
                "description": t.description(),
                "input_schema": t.input_schema(),
                "strict": true,
            })
        })
        .collect()
}

/// Deserialize tool input into a typed struct.
pub(crate) fn parse<T: DeserializeOwned>(input: &Value) -> Result<T, ToolError> {
    serde_json::from_value(input.clone()).map_err(|e| ToolError::Invalid(e.to_string()))
}

/// Trim, require non-empty, cap length.
pub(crate) fn text(field: &str, value: &str, max_chars: usize) -> Result<String, ToolError> {
    let v = value.trim();
    if v.is_empty() {
        return Err(ToolError::Invalid(format!("`{field}` must not be empty")));
    }
    if v.chars().count() > max_chars {
        return Err(ToolError::Invalid(format!(
            "`{field}` is longer than {max_chars} characters"
        )));
    }
    if v.chars().any(|c| c.is_control() && c != '\n') {
        return Err(ToolError::Invalid(format!(
            "`{field}` contains control characters"
        )));
    }
    Ok(v.to_owned())
}

/// Optional text: `None`/blank → `None`.
pub(crate) fn opt_text(
    field: &str,
    value: Option<&str>,
    max_chars: usize,
) -> Result<Option<String>, ToolError> {
    match value.map(str::trim) {
        None | Some("") => Ok(None),
        Some(v) => text(field, v, max_chars).map(Some),
    }
}

/// The spoken confirmation; falls back to empty (the agent supplies a default).
pub(crate) fn say(value: &str) -> String {
    value.trim().chars().take(MAX_SAY_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_has_a_valid_strict_schema() {
        let tools = router_tools();
        let mut names = std::collections::HashSet::new();
        for t in &tools {
            assert!(names.insert(t.name()), "duplicate tool {}", t.name());
            assert!(t.name().chars().all(|c| c.is_ascii_lowercase() || c == '_'));
            assert!(
                t.description().len() > 40,
                "{} needs a real description",
                t.name()
            );
            schema::assert_strict(&t.input_schema(), t.name());
            let props = t.input_schema()["properties"].as_object().cloned().unwrap();
            assert!(props.contains_key("say"), "{} must have `say`", t.name());
        }
    }

    #[test]
    fn definitions_are_deterministic() {
        let a = serde_json::to_string(&definitions(&router_tools())).unwrap();
        let b = serde_json::to_string(&definitions(&router_tools())).unwrap();
        assert_eq!(a, b);
        assert!(a.contains(r#""strict":true"#));
    }

    #[test]
    fn text_validation() {
        assert!(text("f", "  ", 10).is_err());
        assert!(text("f", "abcdefghijk", 10).is_err());
        assert!(text("f", "a\u{0007}", 10).is_err());
        assert_eq!(text("f", " ok ", 10).unwrap(), "ok");
        assert_eq!(opt_text("f", Some(" "), 10).unwrap(), None);
    }
}
