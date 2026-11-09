//! End-to-end router tests: real `Agent`, real HTTP client, wiremock in place
//! of the relay / Claude, a fake phone.

#![allow(clippy::unwrap_used, clippy::panic)]

mod common;

use std::sync::Arc;

use common::{FakeBridge, agent, fixture};
use pocket_agent_core::{Credentials, FailureKind, IntentSpec, Outcome};
use serde_json::Value;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn server_with(responses: &[&str]) -> MockServer {
    let server = MockServer::start().await;
    for (i, name) in responses.iter().enumerate() {
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_string(fixture(name)))
            .up_to_n_times(1)
            .with_priority(u8::try_from(i + 1).unwrap())
            .mount(&server)
            .await;
    }
    server
}

async fn run(bridge: Arc<FakeBridge>, command: &str) -> Outcome {
    let command = command.to_owned();
    tokio::task::spawn_blocking(move || {
        let (a, _dir) = agent(bridge);
        a.handle_command(command)
    })
    .await
    .unwrap()
}

async fn bodies(server: &MockServer) -> Vec<Value> {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect()
}

#[tokio::test]
async fn alarm_end_to_end_and_request_shape() {
    let server = server_with(&["router_set_alarm"]).await;
    let bridge = FakeBridge::new(&server.uri());
    let out = run(bridge.clone(), "Wake me up at 6:30").await;

    assert_eq!(
        out,
        Outcome::Done {
            spoken: "Alarm set for 6:30 AM.".into()
        }
    );
    assert_eq!(
        bridge.launched(),
        vec![IntentSpec::SetAlarm {
            hour: 6,
            minute: 30,
            label: None
        }]
    );
    assert!(bridge.confirms().is_empty(), "alarms need no confirmation");

    let reqs = server.received_requests().await.unwrap();
    assert_eq!(reqs.len(), 1);
    let h = &reqs[0].headers;
    assert_eq!(h.get("authorization").unwrap(), "Bearer pa_dt_testtoken");
    assert_eq!(h.get("anthropic-version").unwrap(), "2026-10-21");
    assert!(h.get("x-api-key").is_none());
    assert!(
        h.get("anthropic-beta").is_none(),
        "Haiku has no server-side fallback"
    );

    let b: Value = serde_json::from_slice(&reqs[0].body).unwrap();
    assert_eq!(b["model"], "claude-haiku-5-5");
    assert_eq!(b["output_config"]["effort"], "low");
    assert_eq!(b["tool_choice"]["type"], "auto");
    assert_eq!(b["system"][0]["cache_control"]["type"], "ephemeral");
    assert!(
        b["tools"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["strict"] == true)
    );
    for banned in [
        "temperature",
        "top_p",
        "top_k",
        "thinking",
        "fallbacks",
        "stream",
    ] {
        assert!(b.get(banned).is_none(), "{banned} sent");
    }
    // Volatile data is in the user message, after the cache breakpoint.
    let first = &b["messages"][0]["content"];
    assert!(
        first[0]["text"]
            .as_str()
            .unwrap()
            .contains("reply_language: en-IN")
    );
    assert_eq!(first[1]["text"], "<command>Wake me up at 6:30</command>");
}

#[tokio::test]
async fn system_and_tools_are_byte_identical_across_commands() {
    let server = server_with(&["router_set_alarm", "router_play_music"]).await;
    let bridge = FakeBridge::new(&server.uri());
    run(bridge.clone(), "alarm 6:30").await;
    run(bridge, "play something").await;
    let reqs = server.received_requests().await.unwrap();
    let prefix = |i: usize| {
        let v: Value = serde_json::from_slice(&reqs[i].body).unwrap();
        serde_json::to_string(&(v["tools"].clone(), v["system"].clone())).unwrap()
    };
    assert_eq!(
        prefix(0),
        prefix(1),
        "cache prefix must not change between commands"
    );
}

#[tokio::test]
async fn music_resolves_named_app() {
    let server = server_with(&["router_play_music"]).await;
    let bridge = FakeBridge::new(&server.uri());
    let out = run(bridge.clone(), "Play Arijit Singh on YouTube Music").await;
    assert_eq!(out.spoken(), "Playing Arijit Singh on YouTube Music.");
    assert_eq!(
        bridge.launched(),
        vec![IntentSpec::PlayMusic {
            query: "Arijit Singh".into(),
            package: Some("com.google.android.apps.youtube.music".into())
        }]
    );
}

#[tokio::test]
async fn message_requires_confirmation_and_cancel_launches_nothing() {
    let server = server_with(&["router_send_message"]).await;
    let bridge = FakeBridge::new(&server.uri());
    *bridge.confirm_answer.lock().unwrap() = false;
    let out = run(
        bridge.clone(),
        "Message Rahul on WhatsApp that I'm running late",
    )
    .await;
    assert_eq!(
        bridge.confirms(),
        vec!["Send to Rahul on WhatsApp: \"I'm running late\"?".to_owned()]
    );
    assert!(matches!(out, Outcome::Cancelled { .. }));
    assert!(bridge.launched().is_empty());
}

#[tokio::test]
async fn approved_message_opens_compose() {
    let server = server_with(&["router_send_message"]).await;
    let bridge = FakeBridge::new(&server.uri());
    let out = run(
        bridge.clone(),
        "Message Rahul on WhatsApp that I'm running late",
    )
    .await;
    assert!(matches!(out, Outcome::Done { .. }));
    assert!(matches!(
        &bridge.launched()[..],
        [IntentSpec::ComposeMessage { package: Some(p), contact, .. }] if p == "com.whatsapp" && contact == "Rahul"
    ));
}

#[tokio::test]
async fn tool_error_goes_back_to_claude_with_history_appended_verbatim() {
    let server = server_with(&["router_open_unknown_app", "router_text_after_error"]).await;
    let bridge = FakeBridge::new(&server.uri());
    let out = run(bridge.clone(), "Open Zomato").await;
    assert_eq!(
        out,
        Outcome::Replied {
            spoken: "Zomato isn't installed on your phone.".into()
        }
    );
    assert!(bridge.launched().is_empty());

    let b = bodies(&server).await;
    assert_eq!(b.len(), 2);
    // Turn 2 = turn 1 + assistant content verbatim + tool_result error.
    assert_eq!(b[1]["messages"][0], b[0]["messages"][0]);
    let first_resp: Value = serde_json::from_str(&fixture("router_open_unknown_app")).unwrap();
    assert_eq!(b[1]["messages"][1]["role"], "assistant");
    assert_eq!(
        b[1]["messages"][1]["content"], first_resp["content"],
        "thinking block must be replayed unchanged"
    );
    let result = &b[1]["messages"][2]["content"][0];
    assert_eq!(result["type"], "tool_result");
    assert_eq!(result["tool_use_id"], "toolu_01Open");
    assert_eq!(result["is_error"], true);
    assert!(
        result["content"]
            .as_str()
            .unwrap()
            .contains("no installed app matches \"Zomato\"")
    );
    // Static prefix unchanged on the follow-up turn.
    assert_eq!(b[0]["system"], b[1]["system"]);
    assert_eq!(b[0]["tools"], b[1]["tools"]);
}

#[tokio::test]
async fn launch_failure_is_reported_to_claude() {
    let server = server_with(&["router_open_swiggy", "router_text_after_error"]).await;
    let bridge = FakeBridge::new(&server.uri());
    *bridge.launch_ok.lock().unwrap() = false;
    let out = run(bridge, "open swiggy").await;
    assert!(matches!(out, Outcome::Replied { .. }));
    let b = bodies(&server).await;
    let result = &b[1]["messages"][2]["content"][0];
    assert_eq!(result["is_error"], true);
    assert!(
        result["content"]
            .as_str()
            .unwrap()
            .contains("could not perform this action")
    );
}

#[tokio::test]
async fn plain_answers_are_spoken() {
    let server = server_with(&["router_text_answer"]).await;
    let bridge = FakeBridge::new(&server.uri());
    let out = run(bridge.clone(), "How far is the moon?").await;
    assert_eq!(
        out,
        Outcome::Replied {
            spoken: "It is about 384,400 kilometres away.".into()
        }
    );
    assert_eq!(
        bridge.spoken(),
        vec!["It is about 384,400 kilometres away.".to_owned()]
    );
}

#[tokio::test]
async fn refusal_is_handled_before_reading_content() {
    let server = server_with(&["router_refusal"]).await;
    let bridge = FakeBridge::new(&server.uri());
    *bridge.locale.lock().unwrap() = "hi-IN".into();
    let out = run(bridge, "something bad").await;
    assert_eq!(
        out,
        Outcome::Refused {
            spoken: "माफ़ कीजिए, मैं इसमें मदद नहीं कर सकता।".into()
        }
    );
}

#[tokio::test]
async fn blocklisted_app_is_never_launched() {
    let server = server_with(&["router_open_blocked"]).await;
    let bridge = FakeBridge::new(&server.uri());
    let out = run(bridge.clone(), "open bitwarden").await;
    assert!(
        matches!(
            out,
            Outcome::Failed {
                kind: FailureKind::Blocked,
                ..
            }
        ),
        "{out:?}"
    );
    assert!(bridge.launched().is_empty());
}

#[tokio::test]
async fn not_configured_makes_no_request() {
    let server = MockServer::start().await;
    let bridge = FakeBridge::new(&server.uri());
    *bridge.creds.lock().unwrap() = Some(Credentials::None);
    let out = run(bridge, "anything").await;
    assert!(matches!(
        out,
        Outcome::Failed {
            kind: FailureKind::NotConfigured,
            ..
        }
    ));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn revoked_device_gets_repair_message() {
    let server = MockServer::start().await;
    Mock::given(header("authorization", "Bearer pa_dt_testtoken"))
        .respond_with(
            ResponseTemplate::new(401)
                .insert_header("x-pocket-relay-error", "authentication_error")
                .set_body_string(r#"{"type":"error","error":{"type":"authentication_error","message":"revoked"}}"#),
        )
        .mount(&server)
        .await;
    let bridge = FakeBridge::new(&server.uri());
    let out = run(bridge, "anything").await;
    assert_eq!(
        out,
        Outcome::Failed {
            kind: FailureKind::Unauthorized,
            spoken: "This phone is no longer paired with your relay. Please pair it again.".into()
        }
    );
}

#[tokio::test]
async fn sonnet_router_requests_server_fallback() {
    let server = server_with(&["router_set_alarm"]).await;
    let bridge = FakeBridge::new(&server.uri());
    let b2 = bridge.clone();
    tokio::task::spawn_blocking(move || {
        let (a, _dir) = agent(b2);
        let mut s = a.settings();
        s.models.router_model = "claude-sonnet-5-5".into();
        a.update_settings(s).unwrap();
        a.handle_command("alarm".into())
    })
    .await
    .unwrap();
    let reqs = server.received_requests().await.unwrap();
    assert_eq!(
        reqs[0].headers.get("anthropic-beta").unwrap(),
        "server-side-fallback-2026-07-01"
    );
    let b: Value = serde_json::from_slice(&reqs[0].body).unwrap();
    assert_eq!(b["fallbacks"], "default");
}

#[tokio::test]
async fn usage_is_accumulated() {
    let server = server_with(&["router_set_alarm"]).await;
    let bridge = FakeBridge::new(&server.uri());
    let u = tokio::task::spawn_blocking(move || {
        let (a, _dir) = agent(bridge);
        a.handle_command("alarm".into());
        a.session_usage()
    })
    .await
    .unwrap();
    assert_eq!((u.requests, u.cache_read_input_tokens), (1, 2214));
}

#[tokio::test]
async fn local_spending_limit_blocks_further_requests() {
    let server = server_with(&["router_set_alarm", "router_set_alarm"]).await;
    let bridge = FakeBridge::new(&server.uri());
    let (first, second, status) = tokio::task::spawn_blocking(move || {
        let (a, _dir) = agent(bridge);
        let mut s = a.settings();
        s.daily_limit_usd_micros = 1; // one micro-dollar: the first request crosses it
        a.update_settings(s).unwrap();
        (
            a.handle_command("alarm".into()),
            a.handle_command("alarm".into()),
            a.budget_status(),
        )
    })
    .await
    .unwrap();
    assert!(matches!(first, Outcome::Done { .. }));
    assert_eq!(
        second,
        Outcome::Failed {
            kind: FailureKind::SpendingLimit,
            spoken: "You've reached today's spending limit. You can raise it in settings.".into()
        }
    );
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        1,
        "no request after the limit"
    );
    assert!(status.today_usd_micros > 0);
}
