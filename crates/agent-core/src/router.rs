//! The router turn: one fast model call that maps a spoken command to a tool.
//!
//! ```text
//! command ─► Claude (Haiku, effort low, intent tools + screen_task)
//!              ├─ tool_use ─► validate (Tool::plan) ─► safety ─► launch_intent ─► speak `say`
//!              │                 └─ error ─► tool_result is_error ─► next turn (max 3)
//!              ├─ text      ─► speak it (answer or clarifying question)
//!              └─ refusal   ─► speak a refusal
//! ```
//!
//! History is append-only within a command: assistant content (including
//! thinking blocks) is appended verbatim and never edited.

use crate::api::{
    self, ANTHROPIC_VERSION, EPHEMERAL, FALLBACK_BETA, Message, MessagesRequest, MessagesResponse,
    OutputConfig, Role, StopReason, SystemBlock, ToolChoice,
};
use crate::bridge::PhoneBridge;
use crate::client::{ApiClient, ApiError};
use crate::i18n::{Msg, t};
use crate::prompts::ROUTER_SYSTEM;
use crate::safety::{Policy, Reason, Verdict};
use crate::settings::Settings;
use crate::tools::{Tool, ToolCtx, ToolPlan};
use crate::types::{FailureKind, IntentSpec, Outcome};
use crate::usage::SessionUsage;

/// Router turns per command (first try + up to two corrections).
pub const MAX_ROUTER_TURNS: usize = 3;
/// Output cap for router turns. Thinking counts towards it, so leave room.
pub const ROUTER_MAX_TOKENS: u32 = 2048;

const ROUTER_SYSTEM_BLOCKS: [SystemBlock; 1] = [SystemBlock {
    kind: "text",
    text: ROUTER_SYSTEM,
    cache_control: Some(EPHEMERAL),
}];

/// Everything one command needs.
pub struct CommandCtx<'a> {
    /// Phone services.
    pub bridge: &'a dyn PhoneBridge,
    /// Settings snapshot for this command.
    pub settings: &'a Settings,
    /// Safety policy.
    pub policy: &'a Policy,
    /// Router tools.
    pub tools: &'a [Box<dyn Tool>],
    /// Their API definitions (built once, byte-stable).
    pub tool_defs: &'a [serde_json::Value],
    /// Reply language.
    pub locale: String,
    /// Usage accumulator.
    pub usage: &'a SessionUsage,
}

/// Whether server-side refusal fallback may be requested for `model`.
/// Haiku 5.5 has none (a fallback list there is a 400).
pub fn server_fallback_supported(model: &str) -> bool {
    model.starts_with("claude-sonnet-5-5") || model.starts_with("claude-opus-5")
}

/// Build the first user message.
pub fn first_user_message(settings: &Settings, locale: &str, command: &str) -> Message {
    let mut ctx = format!("<context>\nreply_language: {locale}\n");
    if !settings.memory.is_empty() {
        ctx.push_str("remembered:\n");
        for m in &settings.memory {
            ctx.push_str("- ");
            ctx.push_str(m);
            ctx.push('\n');
        }
    }
    ctx.push_str("</context>");
    Message {
        role: Role::User,
        content: vec![
            api::text_block(&ctx),
            api::text_block(&format!("<command>{}</command>", command.trim())),
        ],
    }
}

/// Serialise a router request.
pub fn router_request_body(
    settings: &Settings,
    tool_defs: &[serde_json::Value],
    messages: &[Message],
) -> Vec<u8> {
    let model = settings.models.router_model.as_str();
    let req = MessagesRequest {
        model,
        max_tokens: ROUTER_MAX_TOKENS,
        system: &ROUTER_SYSTEM_BLOCKS,
        tools: tool_defs,
        tool_choice: ToolChoice {
            kind: "auto",
            disable_parallel_tool_use: true,
        },
        messages,
        output_config: OutputConfig {
            effort: settings.models.router_effort.clone(),
        },
        cache_control: None,
        fallbacks: (settings.models.server_fallbacks && server_fallback_supported(model))
            .then_some("default"),
    };
    serde_json::to_vec(&req).unwrap_or_default()
}

/// Run one command through the router.
pub fn run(ctx: &CommandCtx<'_>, client: &ApiClient, command: &str) -> Outcome {
    let speak = |text: String| {
        if !text.is_empty() {
            ctx.bridge.speak(text.clone());
        }
        text
    };
    let stop = || ctx.bridge.is_stopped();
    let model = ctx.settings.models.router_model.as_str();
    let betas: &[&str] = if ctx.settings.models.server_fallbacks && server_fallback_supported(model)
    {
        &[FALLBACK_BETA]
    } else {
        &[]
    };
    tracing::debug!(
        version = ANTHROPIC_VERSION,
        model,
        host = client.host(),
        "router start"
    );

    let mut messages = vec![first_user_message(ctx.settings, &ctx.locale, command)];
    let tool_ctx = ToolCtx::new(ctx.settings, ctx.bridge);

    for turn in 0..MAX_ROUTER_TURNS {
        let body = router_request_body(ctx.settings, ctx.tool_defs, &messages);
        let resp = match client.messages(&body, betas, &stop) {
            Ok(r) => r,
            Err(e) => return api_failure(ctx, &e),
        };
        ctx.usage.record(&resp.usage);
        tracing::debug!(turn, stop_reason = ?resp.stop_reason, model = %resp.model, "router response");

        match resp.stop_reason {
            Some(StopReason::Refusal) => {
                return Outcome::Refused {
                    spoken: speak(t(&ctx.locale, Msg::Refused, &[])),
                };
            }
            Some(StopReason::ToolUse) => {}
            Some(StopReason::MaxTokens) if resp.first_tool_use().is_none() => {
                return failed(ctx, FailureKind::Internal, Msg::Generic);
            }
            _ => {
                if resp.first_tool_use().is_none() {
                    let text = resp.text();
                    return if text.is_empty() {
                        failed(ctx, FailureKind::Internal, Msg::Generic)
                    } else {
                        Outcome::Replied {
                            spoken: speak(text),
                        }
                    };
                }
            }
        }

        // A tool call. Append the assistant turn verbatim before answering it.
        let calls = resp.tool_uses();
        let MessagesResponse { content, .. } = resp;
        messages.push(Message {
            role: Role::Assistant,
            content,
        });

        let mut results = Vec::with_capacity(calls.len());
        let mut outcome = None;
        for (i, (id, name, input)) in calls.iter().enumerate() {
            if i > 0 || outcome.is_some() {
                results.push(api::tool_result_block(
                    id,
                    "only one tool call is executed per turn",
                    true,
                ));
                continue;
            }
            let Some(tool) = ctx.tools.iter().find(|t| t.name() == name) else {
                results.push(api::tool_result_block(
                    id,
                    &format!("unknown tool `{name}`"),
                    true,
                ));
                continue;
            };
            match tool.plan(input, &tool_ctx) {
                Err(e) => {
                    tracing::debug!(tool = name, "tool input rejected");
                    results.push(api::tool_result_block(id, &e.to_string(), true));
                }
                Ok(ToolPlan::Screen { say, .. }) => {
                    // Screen control arrives in Milestone 5.
                    if !say.is_empty() {
                        tracing::debug!("screen_task requested");
                    }
                    outcome = Some(failed(
                        ctx,
                        FailureKind::Unsupported,
                        Msg::ScreenUnsupported,
                    ));
                }
                Ok(ToolPlan::Intent {
                    spec,
                    say,
                    follow_up,
                }) => match execute_intent(ctx, spec, say, follow_up) {
                    IntentResult::Finished(o) => outcome = Some(o),
                    IntentResult::Retry(why) => {
                        results.push(api::tool_result_block(id, &why, true))
                    }
                },
            }
        }
        if let Some(o) = outcome {
            return o;
        }
        messages.push(Message {
            role: Role::User,
            content: results,
        });
    }
    failed(ctx, FailureKind::Internal, Msg::Generic)
}

enum IntentResult {
    Finished(Outcome),
    /// Launch failed; tell Claude (it may try screen_task or explain).
    Retry(String),
}

fn execute_intent(
    ctx: &CommandCtx<'_>,
    spec: IntentSpec,
    say: String,
    follow_up: Option<String>,
) -> IntentResult {
    match ctx.policy.check_intent(&spec, ctx.settings) {
        Verdict::Deny(reason) => {
            let app = match &reason {
                Reason::Blocklisted { package } => package.clone(),
                _ => String::new(),
            };
            let spoken = t(
                &ctx.locale,
                Msg::Blocked,
                &[("app", &app_label(&spec).unwrap_or(app))],
            );
            ctx.bridge.speak(spoken.clone());
            return IntentResult::Finished(Outcome::Failed {
                kind: FailureKind::Blocked,
                spoken,
            });
        }
        Verdict::Confirm(reason) => {
            let summary = confirm_text(&ctx.locale, &reason, &say);
            if !ctx.bridge.confirm(summary) {
                let spoken = t(&ctx.locale, Msg::Cancelled, &[]);
                ctx.bridge.speak(spoken.clone());
                return IntentResult::Finished(Outcome::Cancelled { spoken });
            }
        }
        Verdict::Allow => {}
    }
    if ctx.bridge.is_stopped() {
        let spoken = t(&ctx.locale, Msg::Stopped, &[]);
        return IntentResult::Finished(Outcome::Cancelled { spoken });
    }
    let result = ctx.bridge.launch_intent(spec);
    if !result.ok {
        let why = result
            .message
            .unwrap_or_else(|| "no app on the phone can handle it".into());
        return IntentResult::Retry(format!(
            "The phone could not perform this action: {why}. Use screen_task if the goal can be done through an app's UI, otherwise tell the user."
        ));
    }
    let spoken = if follow_up.is_some() {
        // Pressing Send needs the screen agent (Milestone 5); until then the
        // user finishes it.
        t(&ctx.locale, Msg::MessageDrafted, &[])
    } else if say.is_empty() {
        t(&ctx.locale, Msg::Done, &[])
    } else {
        say
    };
    ctx.bridge.speak(spoken.clone());
    IntentResult::Finished(Outcome::Done { spoken })
}

fn app_label(spec: &IntentSpec) -> Option<String> {
    match spec {
        IntentSpec::OpenApp { label, .. } => Some(label.clone()),
        IntentSpec::ComposeMessage { app_label, .. } => Some(app_label.clone()),
        _ => None,
    }
}

/// Confirmation prompt text for a reason.
pub fn confirm_text(locale: &str, reason: &Reason, say: &str) -> String {
    match reason {
        Reason::Call { target } => t(locale, Msg::ConfirmCall, &[("target", target)]),
        Reason::Message { app, contact, text } => t(
            locale,
            Msg::ConfirmMessage,
            &[("app", app), ("contact", contact), ("text", text)],
        ),
        Reason::SensitiveWord { label, .. } => t(locale, Msg::ConfirmTap, &[("label", label)]),
        Reason::PaymentApp { label, .. } => t(locale, Msg::ConfirmPayment, &[("label", label)]),
        Reason::SecuritySettings { label } => t(locale, Msg::ConfirmSecurity, &[("label", label)]),
        Reason::ConfirmEverything { .. }
        | Reason::Blocklisted { .. }
        | Reason::PasswordField
        | Reason::UnknownNode(_) => {
            if say.is_empty() {
                t(locale, Msg::ConfirmGeneric, &[])
            } else {
                format!("{say} {}", t(locale, Msg::ConfirmGeneric, &[]))
            }
        }
    }
}

fn failed(ctx: &CommandCtx<'_>, kind: FailureKind, msg: Msg) -> Outcome {
    let spoken = t(&ctx.locale, msg, &[]);
    ctx.bridge.speak(spoken.clone());
    Outcome::Failed { kind, spoken }
}

/// Map an API error to a spoken message and outcome.
pub fn api_failure(ctx: &CommandCtx<'_>, e: &ApiError) -> Outcome {
    tracing::debug!(error = %e, "api error");
    if matches!(e, ApiError::Stopped) {
        return Outcome::Cancelled {
            spoken: t(&ctx.locale, Msg::Stopped, &[]),
        };
    }
    let (kind, msg) = classify(e);
    failed(ctx, kind, msg)
}

/// Failure category and spoken message for an API error.
pub fn classify(e: &ApiError) -> (FailureKind, Msg) {
    match e {
        ApiError::NotConfigured | ApiError::InvalidConfig(_) => {
            (FailureKind::NotConfigured, Msg::NotConfigured)
        }
        ApiError::DeviceUnauthorized => (FailureKind::Unauthorized, Msg::DeviceUnauthorized),
        ApiError::KeyRejected { via_relay: true } => {
            (FailureKind::Unauthorized, Msg::KeyRejectedRelay)
        }
        ApiError::KeyRejected { via_relay: false } => {
            (FailureKind::Unauthorized, Msg::KeyRejectedDirect)
        }
        ApiError::RelayCap => (FailureKind::RelayCap, Msg::RelayCap),
        ApiError::ModelNotAllowed(_) => (FailureKind::ModelNotAllowed, Msg::ModelNotAllowed),
        ApiError::RateLimited { .. } | ApiError::Overloaded { .. } => {
            (FailureKind::Busy, Msg::Busy)
        }
        ApiError::Server(_) => (FailureKind::Server, Msg::ServerError),
        ApiError::Timeout => (FailureKind::Timeout, Msg::Timeout),
        ApiError::Offline(_) => (FailureKind::Offline, Msg::Offline),
        ApiError::PairingLocked | ApiError::NotARelay => {
            (FailureKind::NotConfigured, Msg::NotConfigured)
        }
        ApiError::BadRequest { .. } | ApiError::Decode(_) | ApiError::Stopped => {
            (FailureKind::Internal, Msg::Generic)
        }
    }
}
