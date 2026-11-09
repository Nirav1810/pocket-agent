//! `send_message(app, contact, text)` → open a pre-filled compose screen; the
//! safety policy asks the user to confirm, then the screen agent presses Send.

use serde::Deserialize;
use serde_json::Value;

use super::{Tool, ToolCtx, ToolError, ToolPlan, apps, parse, say, schema, text};
use crate::types::IntentSpec;

/// Send a text message.
pub struct SendMessage;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    app: Option<String>,
    contact: String,
    text: String,
    say: String,
}

/// Spoken names that mean "the default SMS app" (normalised).
const SMS_NAMES: &[&str] = &["sms", "text", "textmessage", "message", "messages"];

impl Tool for SendMessage {
    fn name(&self) -> &'static str {
        "send_message"
    }

    fn description(&self) -> &'static str {
        "Send a text message to a contact via a messaging app (\"message Rahul on WhatsApp that I'm running late\"). \
         `text` is the message body written as the user would send it, in the language they used. \
         The user always confirms before sending."
    }

    fn input_schema(&self) -> Value {
        schema::object(&[
            (
                "app",
                schema::nullable_string(
                    "Messaging app the user named (e.g. \"WhatsApp\", \"SMS\", \"Telegram\"), else null.",
                ),
            ),
            (
                "contact",
                schema::string("Recipient name or number as spoken."),
            ),
            ("text", schema::string("Message body to send.")),
            ("say", schema::say()),
        ])
    }

    fn plan(&self, input: &Value, ctx: &ToolCtx<'_>) -> Result<ToolPlan, ToolError> {
        let i: Input = parse(input)?;
        let contact = text("contact", &i.contact, 80)?;
        let body = text("text", &i.text, 1000)?;
        let named = i.app.as_deref().map(str::trim).filter(|a| !a.is_empty());
        let (package, app_label) = match named {
            Some(name) if SMS_NAMES.contains(&apps::normalize(name).as_str()) => {
                (None, "SMS".to_owned())
            }
            Some(name) => {
                let a = apps::find(ctx.apps(), name).ok_or_else(|| {
                    ToolError::NotFound(format!("no installed messaging app matches \"{name}\""))
                })?;
                (Some(a.package.clone()), a.label.clone())
            }
            None => match &ctx.settings.messaging_app {
                Some(pkg) => {
                    let label = ctx
                        .apps()
                        .iter()
                        .find(|a| &a.package == pkg)
                        .map_or_else(|| pkg.clone(), |a| a.label.clone());
                    (Some(pkg.clone()), label)
                }
                None => (None, "SMS".to_owned()),
            },
        };
        Ok(ToolPlan::Intent {
            spec: IntentSpec::ComposeMessage { package, app_label, contact, text: body },
            say: say(&i.say),
            follow_up: Some(
                "The message is already drafted in the open chat. Press the Send button once, then finish.".into(),
            ),
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
    fn whatsapp_and_sms() {
        let s = Settings::default();
        let ctx = ToolCtx::with_apps(&s, sample_apps());
        let p = SendMessage
            .plan(&json!({"app":"WhatsApp","contact":"Rahul","text":"I'm running late","say":"Sending."}), &ctx)
            .unwrap();
        match p {
            ToolPlan::Intent {
                spec:
                    IntentSpec::ComposeMessage {
                        package,
                        app_label,
                        contact,
                        text,
                    },
                follow_up,
                ..
            } => {
                assert_eq!(package.as_deref(), Some("com.whatsapp"));
                assert_eq!(
                    (app_label.as_str(), contact.as_str(), text.as_str()),
                    ("WhatsApp", "Rahul", "I'm running late")
                );
                assert!(follow_up.is_some());
            }
            other => panic!("{other:?}"),
        }
        let p = SendMessage
            .plan(
                &json!({"app":"text message","contact":"5551234","text":"hi","say":""}),
                &ctx,
            )
            .unwrap();
        assert!(matches!(
            p,
            ToolPlan::Intent {
                spec: IntentSpec::ComposeMessage { package: None, .. },
                ..
            }
        ));
    }

    #[test]
    fn unknown_app_is_an_error() {
        let s = Settings::default();
        let ctx = ToolCtx::with_apps(&s, sample_apps());
        assert!(matches!(
            SendMessage.plan(
                &json!({"app":"Signal","contact":"x","text":"y","say":""}),
                &ctx
            ),
            Err(ToolError::NotFound(_))
        ));
    }
}
