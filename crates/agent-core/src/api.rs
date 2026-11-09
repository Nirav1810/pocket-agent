//! Claude Messages API wire types (`POST /v1/messages`, API version
//! `2026-10-21`), written from the official API reference.
//!
//! Content blocks Claude returns are kept as **raw JSON** ([`RawBlock`]) and
//! appended back to the conversation byte-for-byte: thinking blocks carry
//! signatures and history must be append-only (preserved thinking), so the
//! client must never re-shape them. [`BlockView`] is a read-only typed view
//! used to find text and tool calls.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_json::value::RawValue;

/// API version header value.
pub const ANTHROPIC_VERSION: &str = "2026-10-21";
/// Beta enabling `fallbacks: "default"` (server-side refusal fallback).
pub const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";

/// One content block, verbatim.
pub type RawBlock = Box<RawValue>;

/// Message role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// User turn (also carries tool results).
    User,
    /// Assistant turn.
    Assistant,
}

/// One conversation message.
#[derive(Debug, Clone, Serialize)]
pub struct Message {
    /// Role.
    pub role: Role,
    /// Content blocks, verbatim.
    pub content: Vec<RawBlock>,
}

/// `cache_control` marker.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct CacheControl {
    /// Always `ephemeral` (5-minute TTL).
    #[serde(rename = "type")]
    pub kind: &'static str,
}

/// The only cache marker we use.
pub const EPHEMERAL: CacheControl = CacheControl { kind: "ephemeral" };

/// A `system` text block.
#[derive(Debug, Clone, Serialize)]
pub struct SystemBlock {
    /// Always `text`.
    #[serde(rename = "type")]
    pub kind: &'static str,
    /// Prompt text.
    pub text: &'static str,
    /// Cache breakpoint (on the last static block).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

/// `tool_choice`. Always `auto`: forced choice returns 400 on Sonnet 5.5 /
/// Opus 5.5.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct ToolChoice {
    /// `auto`.
    #[serde(rename = "type")]
    pub kind: &'static str,
    /// At most one tool call per turn.
    pub disable_parallel_tool_use: bool,
}

/// `output_config`.
#[derive(Debug, Clone, Serialize)]
pub struct OutputConfig {
    /// `low` | `medium` | `high` | `xhigh` | `max`.
    pub effort: String,
}

/// Request body. Field order is fixed and nothing volatile precedes the cache
/// breakpoint (tools → system render first).
#[derive(Debug, Clone, Serialize)]
pub struct MessagesRequest<'a> {
    /// Model id.
    pub model: &'a str,
    /// Output cap (includes thinking).
    pub max_tokens: u32,
    /// Static system prompt; last block carries the cache marker.
    pub system: &'a [SystemBlock],
    /// Tool definitions (static, deterministic order).
    pub tools: &'a [Value],
    /// Always auto, single call.
    pub tool_choice: ToolChoice,
    /// Conversation so far.
    pub messages: &'a [Message],
    /// Effort.
    pub output_config: OutputConfig,
    /// Top-level automatic caching for the growing conversation tail.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
    /// `"default"` server-side refusal fallback (Sonnet/Opus only, needs beta).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallbacks: Option<&'static str>,
}

/// Why generation stopped.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// Natural end.
    EndTurn,
    /// Wants a tool call executed.
    ToolUse,
    /// Hit `max_tokens`.
    MaxTokens,
    /// Declined by safeguards or the model.
    Refusal,
    /// Stop sequence.
    StopSequence,
    /// Long server-tool turn paused.
    PauseTurn,
    /// Anything newer.
    #[serde(other)]
    Other,
}

/// `stop_details` (only populated on refusals; informational).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct StopDetails {
    /// e.g. `cyber`, `bio`, `general_harms`; may be null.
    #[serde(default)]
    pub category: Option<String>,
}

/// Token usage.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub struct Usage {
    /// Uncached input tokens.
    #[serde(default)]
    pub input_tokens: u64,
    /// Output tokens (including thinking).
    #[serde(default)]
    pub output_tokens: u64,
    /// Read from cache.
    #[serde(default, deserialize_with = "null_zero")]
    pub cache_read_input_tokens: u64,
    /// Written to cache.
    #[serde(default, deserialize_with = "null_zero")]
    pub cache_creation_input_tokens: u64,
}

fn null_zero<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    Ok(Option::<u64>::deserialize(d)?.unwrap_or(0))
}

/// Response body of a successful request.
#[derive(Debug, Deserialize)]
pub struct MessagesResponse {
    /// Message id.
    #[serde(default)]
    pub id: String,
    /// Model that actually answered (may differ after a fallback).
    #[serde(default)]
    pub model: String,
    /// Content blocks, verbatim.
    pub content: Vec<RawBlock>,
    /// Stop reason (`None` only in malformed responses).
    pub stop_reason: Option<StopReason>,
    /// Refusal details.
    #[serde(default)]
    pub stop_details: Option<StopDetails>,
    /// Usage.
    #[serde(default)]
    pub usage: Usage,
}

/// Typed, read-only view of a content block.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BlockView {
    /// Visible text.
    Text {
        /// The text.
        text: String,
    },
    /// A tool call.
    ToolUse {
        /// Call id to answer with a `tool_result`.
        id: String,
        /// Tool name.
        name: String,
        /// Arguments (schema-valid when `strict: true`).
        input: Value,
    },
    /// Thinking, redacted thinking, fallback markers, anything else.
    #[serde(other)]
    Other,
}

impl MessagesResponse {
    /// Typed views of all blocks (unparseable blocks become `Other`).
    pub fn views(&self) -> Vec<BlockView> {
        self.content
            .iter()
            .map(|b| serde_json::from_str(b.get()).unwrap_or(BlockView::Other))
            .collect()
    }

    /// Concatenated visible text.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for v in self.views() {
            if let BlockView::Text { text } = v {
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push_str(text.trim());
            }
        }
        out
    }

    /// The first tool call, if any.
    pub fn first_tool_use(&self) -> Option<(String, String, Value)> {
        self.views().into_iter().find_map(|v| match v {
            BlockView::ToolUse { id, name, input } => Some((id, name, input)),
            _ => None,
        })
    }

    /// All tool calls (we request one, but must answer every call made).
    pub fn tool_uses(&self) -> Vec<(String, String, Value)> {
        self.views()
            .into_iter()
            .filter_map(|v| match v {
                BlockView::ToolUse { id, name, input } => Some((id, name, input)),
                _ => None,
            })
            .collect()
    }
}

/// Build a raw block from any serialisable value.
pub fn raw(v: &impl Serialize) -> RawBlock {
    // Serialising our own simple structs/values to JSON cannot fail; fall back
    // to an empty text block rather than panicking.
    serde_json::value::to_raw_value(v).unwrap_or_else(|_| {
        serde_json::value::to_raw_value(&serde_json::json!({"type": "text", "text": ""}))
            .unwrap_or_else(|_| RawValue::NULL.to_owned())
    })
}

/// A user `text` block.
pub fn text_block(text: &str) -> RawBlock {
    raw(&serde_json::json!({"type": "text", "text": text}))
}

/// A `tool_result` block. `is_error` results tell Claude the call failed.
pub fn tool_result_block(tool_use_id: &str, content: &str, is_error: bool) -> RawBlock {
    if is_error {
        raw(
            &serde_json::json!({"type": "tool_result", "tool_use_id": tool_use_id, "content": content, "is_error": true}),
        )
    } else {
        raw(
            &serde_json::json!({"type": "tool_result", "tool_use_id": tool_use_id, "content": content}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RESP: &str = r#"{
      "id":"msg_01","type":"message","role":"assistant","model":"claude-haiku-5-5",
      "content":[
        {"type":"thinking","thinking":"","signature":"EqQBCkYIBxgCKkDsig"},
        {"type":"text","text":"Sure."},
        {"type":"tool_use","id":"toolu_01","name":"set_alarm","input":{"hour":6,"minute":30,"label":null,"say":"Alarm set for 6:30."}}
      ],
      "stop_reason":"tool_use","stop_sequence":null,
      "usage":{"input_tokens":12,"output_tokens":40,"cache_read_input_tokens":1800,"cache_creation_input_tokens":null}
    }"#;

    #[test]
    fn parses_response_and_keeps_blocks_verbatim() {
        let r: MessagesResponse = serde_json::from_str(RESP).unwrap();
        assert_eq!(r.stop_reason, Some(StopReason::ToolUse));
        assert_eq!(r.usage.cache_read_input_tokens, 1800);
        assert_eq!(r.usage.cache_creation_input_tokens, 0);
        // Thinking block replayed exactly, signature included.
        assert_eq!(
            r.content[0].get(),
            r#"{"type":"thinking","thinking":"","signature":"EqQBCkYIBxgCKkDsig"}"#
        );
        assert_eq!(r.text(), "Sure.");
        let (id, name, input) = r.first_tool_use().unwrap();
        assert_eq!((id.as_str(), name.as_str()), ("toolu_01", "set_alarm"));
        assert_eq!(input["hour"], 6);
    }

    #[test]
    fn unknown_stop_reason_and_blocks() {
        let r: MessagesResponse = serde_json::from_str(
            r#"{"content":[{"type":"brand_new","x":1}],"stop_reason":"something_new","usage":{}}"#,
        )
        .unwrap();
        assert_eq!(r.stop_reason, Some(StopReason::Other));
        assert_eq!(r.views(), vec![BlockView::Other]);
    }

    #[test]
    fn request_serialises_in_cache_friendly_shape() {
        let system = [SystemBlock {
            kind: "text",
            text: "sys",
            cache_control: Some(EPHEMERAL),
        }];
        let tools = [serde_json::json!({"name":"t"})];
        let msgs = [Message {
            role: Role::User,
            content: vec![text_block("hi")],
        }];
        let req = MessagesRequest {
            model: "claude-haiku-5-5",
            max_tokens: 100,
            system: &system,
            tools: &tools,
            tool_choice: ToolChoice {
                kind: "auto",
                disable_parallel_tool_use: true,
            },
            messages: &msgs,
            output_config: OutputConfig {
                effort: "low".into(),
            },
            cache_control: None,
            fallbacks: None,
        };
        let v: Value = serde_json::from_slice(&serde_json::to_vec(&req).unwrap()).unwrap();
        assert_eq!(v["system"][0]["cache_control"]["type"], "ephemeral");
        assert_eq!(
            v["tool_choice"],
            serde_json::json!({"type":"auto","disable_parallel_tool_use":true})
        );
        assert_eq!(v["output_config"]["effort"], "low");
        for banned in [
            "temperature",
            "top_p",
            "top_k",
            "thinking",
            "fallbacks",
            "cache_control",
            "stream",
        ] {
            assert!(v.get(banned).is_none(), "{banned} must not be sent");
        }
    }

    #[test]
    fn tool_result_shapes() {
        let ok: Value = serde_json::from_str(tool_result_block("t1", "done", false).get()).unwrap();
        assert!(ok.get("is_error").is_none());
        let err: Value = serde_json::from_str(tool_result_block("t1", "nope", true).get()).unwrap();
        assert_eq!(err["is_error"], true);
        assert_eq!(err["tool_use_id"], "t1");
    }
}
