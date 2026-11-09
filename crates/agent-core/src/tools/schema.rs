//! Helpers for writing strict tool schemas.
//!
//! Strict tool use supports a JSON-Schema subset: no `minimum`/`maximum`,
//! no `minLength`/`maxLength`; every object needs `additionalProperties:
//! false`. We list every property in `required` and express optional values
//! as `anyOf: [<type>, null]`, which is unambiguous. Ranges and lengths are
//! validated in Rust in each tool's `plan()`.

use serde_json::{Map, Value, json};

/// A string property.
pub fn string(description: &str) -> Value {
    json!({"type": "string", "description": description})
}

/// A string-or-null property.
pub fn nullable_string(description: &str) -> Value {
    json!({"anyOf": [{"type": "string"}, {"type": "null"}], "description": description})
}

/// An integer property.
pub fn integer(description: &str) -> Value {
    json!({"type": "integer", "description": description})
}

/// One of a fixed set of strings, or null.
pub fn nullable_enum(values: &[&str], description: &str) -> Value {
    json!({"anyOf": [{"type": "string", "enum": values}, {"type": "null"}], "description": description})
}

/// The `say` property every router tool has.
pub fn say() -> Value {
    string(
        "Very short spoken confirmation (max ~8 words) in the reply language from <context>, \
         e.g. \"Playing Arijit Singh on YouTube Music.\" Plain text, no markdown.",
    )
}

/// An object schema with all properties required and no extras.
pub fn object(props: &[(&str, Value)]) -> Value {
    let mut map = Map::new();
    for (k, v) in props {
        map.insert((*k).to_owned(), v.clone());
    }
    json!({
        "type": "object",
        "properties": map,
        "required": props.iter().map(|(k, _)| *k).collect::<Vec<_>>(),
        "additionalProperties": false,
    })
}

/// Test helper: assert a schema follows the strict-mode rules above.
#[cfg(test)]
pub fn assert_strict(schema: &Value, tool: &str) {
    fn walk(v: &Value, tool: &str) {
        match v {
            Value::Object(o) => {
                for banned in [
                    "minimum",
                    "maximum",
                    "minLength",
                    "maxLength",
                    "multipleOf",
                    "pattern",
                ] {
                    assert!(
                        !o.contains_key(banned),
                        "{tool}: `{banned}` is not supported in strict mode"
                    );
                }
                if o.get("type") == Some(&Value::String("object".into())) {
                    assert_eq!(
                        o.get("additionalProperties"),
                        Some(&Value::Bool(false)),
                        "{tool}: objects need additionalProperties:false"
                    );
                    let props: Vec<&String> = o["properties"]
                        .as_object()
                        .map(|p| p.keys().collect())
                        .unwrap_or_default();
                    let req: Vec<&str> = o["required"]
                        .as_array()
                        .map(|r| r.iter().filter_map(Value::as_str).collect())
                        .unwrap_or_default();
                    for p in props {
                        assert!(
                            req.contains(&p.as_str()),
                            "{tool}: property `{p}` must be in required"
                        );
                    }
                }
                o.values().for_each(|c| walk(c, tool));
            }
            Value::Array(a) => a.iter().for_each(|c| walk(c, tool)),
            _ => {}
        }
    }
    walk(schema, tool);
}
