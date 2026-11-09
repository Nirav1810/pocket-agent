//! Native micro-benchmark of the relay's per-request CPU work (everything the
//! Worker does besides waiting on I/O). Run:
//!
//! ```sh
//! cargo run --release -p pocket-agent-relay --example hotpath_bench
//! ```
//!
//! WASM in workerd is typically 1.5–3× slower than native; the real figure is
//! the `cpuTime` shown by Workers Logs / `wrangler tail` (see docs/PERF.md).

#![allow(clippy::panic, clippy::unwrap_used)] // benchmark harness, not shipped code

use std::hint::black_box;
use std::time::Instant;

use pocket_agent_relay::{auth, config::Config, inspect, usage};

fn main() {
    // A realistic screen-loop request: tools + system + ~25 turns of history (~120 KB).
    let turn = r#"{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_01","content":"pkg=com.example activity=.Main\n[3] EditText \"Search\" (editable) @540,180\n[7] Button \"Go\" (clickable) @980,180\n[12] Text \"Result one with a fairly long title that gets truncated\" (clickable) @540,420"}]},{"role":"assistant","content":[{"type":"thinking","thinking":"","signature":"EqQBCkYIBxgCKkDxyz0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcdef"},{"type":"tool_use","id":"toolu_02","name":"click","input":{"id":7}}]}"#;
    let history = std::iter::repeat_n(turn, 50).collect::<Vec<_>>().join(",");
    let body = format!(
        r#"{{"model":"claude-sonnet-5-5","max_tokens":2048,"output_config":{{"effort":"medium"}},"system":[{{"type":"text","text":"{}","cache_control":{{"type":"ephemeral"}}}}],"tools":[],"messages":[{{"role":"user","content":"open swiggy"}},{history}]}}"#,
        "x".repeat(8000)
    );
    let resp = br#"{"id":"msg_1","type":"message","role":"assistant","model":"claude-sonnet-5-5","content":[{"type":"thinking","thinking":"","signature":"abc"},{"type":"tool_use","id":"toolu_9","name":"click","input":{"id":12}}],"stop_reason":"tool_use","stop_sequence":null,"usage":{"input_tokens":812,"output_tokens":95,"cache_read_input_tokens":30210,"cache_creation_input_tokens":1450}}"#;
    let config = Config::from_lookup(|_| None).unwrap_or_else(|e| panic!("{e}"));
    let token = auth::token_from_bytes(&[42; 32]);
    let header = format!("Bearer {token}");

    let n = 2000;
    let t = Instant::now();
    for _ in 0..n {
        let bearer = auth::parse_bearer(Some(black_box(&header))).unwrap_or_default();
        black_box(auth::hash_token(bearer));
        let head =
            inspect::request_head(black_box(body.as_bytes())).unwrap_or_else(|_| panic!("parse"));
        black_box(config.model_allowed(&head.model));
        let (model, u) =
            inspect::response_usage(black_box(resp)).unwrap_or_else(|| panic!("usage"));
        black_box(usage::cost_micros(
            &u,
            &config.price_for(model.as_deref().unwrap_or("")),
        ));
    }
    let per = t.elapsed().as_secs_f64() * 1000.0 / f64::from(n);
    println!("request body: {} KB", body.len() / 1024);
    println!(
        "hot path (auth hash + request inspect + usage parse + cost): {per:.3} ms/request native"
    );

    let t = Instant::now();
    for _ in 0..200 {
        black_box(inspect::clamp_max_tokens(black_box(body.as_bytes()), 1000).unwrap_or_default());
    }
    println!(
        "rare path (clamp = full re-serialise): {:.3} ms/request native",
        t.elapsed().as_secs_f64() * 1000.0 / 200.0
    );
}
