//! `dial(number_or_contact)` → phone call (always confirmed by the safety policy).

use serde::Deserialize;
use serde_json::Value;

use super::{Tool, ToolCtx, ToolError, ToolPlan, parse, say, schema, text};
use crate::types::IntentSpec;

/// Call a number or contact.
pub struct Dial;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    number_or_contact: String,
    say: String,
}

/// Whether `s` looks like a dialable number (digits plus `+ - ( ) space * # .`).
pub fn is_phone_number(s: &str) -> bool {
    let digits = s.chars().filter(char::is_ascii_digit).count();
    digits >= 3
        && s.chars()
            .all(|c| c.is_ascii_digit() || " +-()*#.".contains(c))
}

impl Tool for Dial {
    fn name(&self) -> &'static str {
        "dial"
    }

    fn description(&self) -> &'static str {
        "Make a phone call to a contact name or phone number (\"call Mom\", \"dial 112\"). \
         Use exactly the name or number the user said; never invent a number. The user is asked to confirm."
    }

    fn input_schema(&self) -> Value {
        schema::object(&[
            (
                "number_or_contact",
                schema::string("Contact name as spoken, or the phone number."),
            ),
            ("say", schema::say()),
        ])
    }

    fn plan(&self, input: &Value, _ctx: &ToolCtx<'_>) -> Result<ToolPlan, ToolError> {
        let i: Input = parse(input)?;
        let target = text("number_or_contact", &i.number_or_contact, 80)?;
        let is_number = is_phone_number(&target);
        Ok(ToolPlan::Intent {
            spec: IntentSpec::Call { target, is_number },
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
    fn number_detection() {
        assert!(is_phone_number("+91 98765-43210"));
        assert!(is_phone_number("112"));
        assert!(!is_phone_number("Mom"));
        assert!(!is_phone_number("12"));
        assert!(!is_phone_number("call 112"));
    }

    #[test]
    fn plans_call() {
        let s = Settings::default();
        let ctx = ToolCtx::with_apps(&s, vec![]);
        let p = Dial
            .plan(
                &json!({"number_or_contact":"Mom","say":"Calling Mom."}),
                &ctx,
            )
            .unwrap();
        assert!(matches!(
            p,
            ToolPlan::Intent {
                spec: IntentSpec::Call {
                    is_number: false,
                    ..
                },
                ..
            }
        ));
    }
}
