//! Desktop REPL for agent-core: type commands, see what the phone would do.
//!
//! ```sh
//! # Relay mode (pair once; prints a device token to export):
//! POCKET_AGENT_RELAY_URL=https://pocket-agent-relay.you.workers.dev \
//! POCKET_AGENT_PAIRING_CODE=XXXXX-XXXXX-XXXXX-XXXXX \
//!   cargo run -p pocket-agent-core --example repl
//! POCKET_AGENT_RELAY_URL=… POCKET_AGENT_DEVICE_TOKEN=pa_dt_… cargo run -p pocket-agent-core --example repl
//!
//! # Direct mode:
//! ANTHROPIC_API_KEY=sk-ant-… cargo run -p pocket-agent-core --example repl
//! ```
//!
//! Instead of exporting variables you can put them in the repo-root `.env`
//! (copy `.env.example`; it is git-ignored). Real env vars win over `.env`.
//!
//! Optional: `POCKET_AGENT_MODE=relay|direct` (when both are configured),
//! `POCKET_AGENT_LOCALE=hi-IN`, `POCKET_AGENT_AUTO_CONFIRM=1`.
//! Commands: `/usage`, `/settings`, `/quit`. Intents are printed, not executed.

#![allow(clippy::print_stdout)]

use std::io::{self, BufRead, Write};
use std::sync::Arc;

use pocket_agent_core::agent::pair_relay;
use pocket_agent_core::pricing_fmt;
use pocket_agent_core::{
    ActionResult, Agent, AppInfo, Credentials, IntentSpec, PhoneBridge, ScreenSnapshot, UiAction,
};

struct ConsoleBridge {
    creds: Credentials,
    locale: String,
    auto_confirm: bool,
}

impl PhoneBridge for ConsoleBridge {
    fn credentials(&self) -> Credentials {
        self.creds.clone()
    }
    fn launch_intent(&self, spec: IntentSpec) -> ActionResult {
        println!("  [intent] {spec:?}");
        ActionResult::ok()
    }
    fn installed_apps(&self) -> Vec<AppInfo> {
        [
            ("com.google.android.apps.youtube.music", "YT Music"),
            ("com.spotify.music", "Spotify"),
            ("com.google.android.apps.maps", "Maps"),
            ("net.osmand", "OsmAnd"),
            ("com.whatsapp", "WhatsApp"),
            ("org.telegram.messenger", "Telegram"),
            ("com.google.android.apps.messaging", "Messages"),
            ("in.swiggy.android", "Swiggy"),
            ("com.instagram.android", "Instagram"),
            ("com.android.chrome", "Chrome"),
            ("com.google.android.deskclock", "Clock"),
            ("com.x8bit.bitwarden", "Bitwarden"),
        ]
        .iter()
        .map(|(p, l)| AppInfo {
            package: (*p).into(),
            label: (*l).into(),
        })
        .collect()
    }
    fn snapshot_screen(&self) -> ScreenSnapshot {
        ScreenSnapshot::default()
    }
    fn perform(&self, action: UiAction) -> ActionResult {
        println!("  [ui] {action:?}");
        ActionResult::failed("no screen in the REPL")
    }
    fn confirm(&self, summary: String) -> bool {
        if self.auto_confirm {
            println!("  [confirm] {summary} -> auto-approved");
            return true;
        }
        print!("  [confirm] {summary} [y/N] ");
        let _ = io::stdout().flush();
        let mut line = String::new();
        let _ = io::stdin().lock().read_line(&mut line);
        matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
    }
    fn speak(&self, text: String) {
        println!("  [say] {text}");
    }
    fn status(&self, text: String) {
        println!("  [status] {text}");
    }
    fn is_stopped(&self) -> bool {
        false
    }
    fn locale(&self) -> String {
        self.locale.clone()
    }
}

/// Settings from the repo-root `.env` (git-ignored). Real environment
/// variables take precedence over it.
fn dotenv() -> &'static std::collections::HashMap<String, String> {
    static DOTENV: std::sync::OnceLock<std::collections::HashMap<String, String>> =
        std::sync::OnceLock::new();
    DOTENV.get_or_init(|| {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.env");
        std::fs::read_to_string(path)
            .map(|s| parse_dotenv(&s))
            .unwrap_or_default()
    })
}

/// Minimal `.env` parser: `KEY=value` lines, `#` comments, optional
/// `export ` prefix, optional matching single or double quotes.
fn parse_dotenv(text: &str) -> std::collections::HashMap<String, String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let l = l.strip_prefix("export ").unwrap_or(l);
            let (k, v) = l.split_once('=')?;
            let v = v.trim();
            let v = v
                .strip_prefix('"')
                .and_then(|x| x.strip_suffix('"'))
                .or_else(|| v.strip_prefix('\'').and_then(|x| x.strip_suffix('\'')))
                .unwrap_or(v);
            Some((k.trim().to_owned(), v.to_owned()))
        })
        .collect()
}

fn env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .or_else(|| dotenv().get(name).cloned())
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

fn credentials() -> Result<Credentials, String> {
    let mode = env("POCKET_AGENT_MODE");
    let relay = env("POCKET_AGENT_RELAY_URL");
    let key = env("ANTHROPIC_API_KEY");
    let want_relay = match mode.as_deref() {
        Some("direct") => false,
        Some("relay") => true,
        _ => relay.is_some(),
    };
    if want_relay {
        let url = relay.ok_or("set POCKET_AGENT_RELAY_URL")?;
        if let Some(token) = env("POCKET_AGENT_DEVICE_TOKEN") {
            return Ok(Credentials::Relay {
                base_url: url,
                device_token: token,
            });
        }
        let code = env("POCKET_AGENT_PAIRING_CODE")
            .ok_or("set POCKET_AGENT_DEVICE_TOKEN, or POCKET_AGENT_PAIRING_CODE to pair")?;
        let r = pair_relay(url, code, "desktop REPL".into())
            .map_err(|e| format!("pairing failed: {e}"))?;
        println!("Paired as {} (relay {}).", r.device_id, r.relay_version);
        println!(
            "Reuse this device without re-pairing:\n  POCKET_AGENT_DEVICE_TOKEN={}\n",
            r.device_token
        );
        return Ok(Credentials::Relay {
            base_url: r.url,
            device_token: r.device_token,
        });
    }
    let key = key.ok_or("set ANTHROPIC_API_KEY (direct) or POCKET_AGENT_RELAY_URL (relay)")?;
    Ok(Credentials::Direct { api_key: key })
}

fn main() {
    let creds = match credentials() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    println!("Mode: {creds:?}");
    let bridge = Arc::new(ConsoleBridge {
        creds,
        locale: env("POCKET_AGENT_LOCALE").unwrap_or_else(|| "en-US".into()),
        auto_confirm: env("POCKET_AGENT_AUTO_CONFIRM").is_some(),
    });
    let data_dir = env("POCKET_AGENT_DATA_DIR").unwrap_or_else(|| {
        std::env::temp_dir()
            .join("pocket-agent-repl")
            .to_string_lossy()
            .into_owned()
    });
    let agent = match Agent::new(bridge, data_dir) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("could not start agent: {e}");
            std::process::exit(1);
        }
    };
    apply_limits(&agent);
    println!("Type a command (or /usage, /settings, /quit).");
    let stdin = io::stdin();
    loop {
        print!("> ");
        let _ = io::stdout().flush();
        let mut line = String::new();
        if stdin.lock().read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let line = line.trim();
        match line {
            "" => continue,
            "/quit" | "/exit" => break,
            "/usage" => {
                println!("  {:?}", agent.session_usage());
                print_budget(&agent);
            }
            "/settings" => println!("{}", agent.export_settings_json()),
            cmd => {
                let started = std::time::Instant::now();
                let outcome = agent.handle_command(cmd.to_owned());
                let ms = started.elapsed().as_millis();
                println!("  [outcome] {outcome:?}");
                if let Some(u) = agent.last_usage() {
                    println!(
                        "  [usage] {ms} ms | in {} | out {} | cache read {} | cache write {}",
                        u.input_tokens,
                        u.output_tokens,
                        u.cache_read_input_tokens,
                        u.cache_creation_input_tokens
                    );
                }
            }
        }
    }
}

/// Apply `POCKET_AGENT_DAILY_LIMIT_USD` / `POCKET_AGENT_MONTHLY_LIMIT_USD`
/// (from the environment or `.env`) to the agent's settings.
fn apply_limits(agent: &Agent) {
    let mut s = agent.settings();
    for (var, field) in [
        (
            "POCKET_AGENT_DAILY_LIMIT_USD",
            &mut s.daily_limit_usd_micros,
        ),
        (
            "POCKET_AGENT_MONTHLY_LIMIT_USD",
            &mut s.monthly_limit_usd_micros,
        ),
    ] {
        if let Some(v) = env(var) {
            match pricing_fmt::usd_to_micros(&v) {
                Some(m) => *field = m,
                None => eprintln!("ignoring {var}: expected a dollar amount like 1 or 0.50"),
            }
        }
    }
    if let Err(e) = agent.update_settings(s) {
        eprintln!("could not save limits: {e}");
    }
    print_budget(agent);
}

fn print_budget(agent: &Agent) {
    let b = agent.budget_status();
    let limit = |m: u64| {
        if m == 0 {
            "no limit".to_owned()
        } else {
            pricing_fmt::format_usd(m)
        }
    };
    println!(
        "Spend (estimated): today {} of {}, this month {} of {}",
        pricing_fmt::format_usd(b.today_usd_micros),
        limit(b.daily_limit_usd_micros),
        pricing_fmt::format_usd(b.month_usd_micros),
        limit(b.monthly_limit_usd_micros),
    );
}
