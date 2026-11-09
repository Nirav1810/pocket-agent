//! Test doubles: a scripted `PhoneBridge` and fixture loading.

#![allow(dead_code, clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use pocket_agent_core::{
    ActionResult, Agent, AppInfo, Credentials, IntentSpec, PhoneBridge, ScreenSnapshot, UiAction,
};

/// Records everything the agent asked the phone to do.
#[derive(Default)]
pub struct FakeBridge {
    pub creds: Mutex<Option<Credentials>>,
    pub confirm_answer: Mutex<bool>,
    pub launch_ok: Mutex<bool>,
    pub locale: Mutex<String>,
    pub launched: Mutex<Vec<IntentSpec>>,
    pub spoken: Mutex<Vec<String>>,
    pub confirms: Mutex<Vec<String>>,
}

impl FakeBridge {
    pub fn new(base_url: &str) -> Arc<Self> {
        Arc::new(Self {
            creds: Mutex::new(Some(Credentials::Relay {
                base_url: base_url.to_owned(),
                device_token: "pa_dt_testtoken".into(),
            })),
            confirm_answer: Mutex::new(true),
            launch_ok: Mutex::new(true),
            locale: Mutex::new("en-IN".into()),
            ..Default::default()
        })
    }
    pub fn launched(&self) -> Vec<IntentSpec> {
        self.launched.lock().unwrap().clone()
    }
    pub fn spoken(&self) -> Vec<String> {
        self.spoken.lock().unwrap().clone()
    }
    pub fn confirms(&self) -> Vec<String> {
        self.confirms.lock().unwrap().clone()
    }
}

impl PhoneBridge for FakeBridge {
    fn credentials(&self) -> Credentials {
        self.creds
            .lock()
            .unwrap()
            .clone()
            .unwrap_or(Credentials::None)
    }
    fn launch_intent(&self, spec: IntentSpec) -> ActionResult {
        self.launched.lock().unwrap().push(spec);
        if *self.launch_ok.lock().unwrap() {
            ActionResult::ok()
        } else {
            ActionResult::failed("no activity found")
        }
    }
    fn installed_apps(&self) -> Vec<AppInfo> {
        [
            ("com.google.android.apps.youtube.music", "YT Music"),
            ("com.spotify.music", "Spotify"),
            ("com.whatsapp", "WhatsApp"),
            ("com.x8bit.bitwarden", "Bitwarden"),
            ("in.swiggy.android", "Swiggy"),
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
    fn perform(&self, _action: UiAction) -> ActionResult {
        ActionResult::failed("not in this test")
    }
    fn confirm(&self, summary: String) -> bool {
        self.confirms.lock().unwrap().push(summary);
        *self.confirm_answer.lock().unwrap()
    }
    fn speak(&self, text: String) {
        self.spoken.lock().unwrap().push(text);
    }
    fn status(&self, _text: String) {}
    fn is_stopped(&self) -> bool {
        false
    }
    fn locale(&self) -> String {
        self.locale.lock().unwrap().clone()
    }
}

/// Load `tests/fixtures/<name>.json`.
pub fn fixture(name: &str) -> String {
    let path = format!("{}/tests/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// Agent with a fresh settings dir.
pub fn agent(bridge: Arc<FakeBridge>) -> (Arc<Agent>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let a = Agent::new(bridge, dir.path().to_string_lossy().into_owned()).unwrap();
    (a, dir)
}
