//! User settings and the on-disk store.
//!
//! One JSON file (`settings.json`) in app-private storage, written atomically
//! (temp file → fsync → rename) and versioned with forward migrations. It
//! holds **no secrets**: the API key / device token live in the Android
//! Keystore-backed vault and reach Rust only through `PhoneBridge::credentials`.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Current settings schema version.
pub const SCHEMA_VERSION: u32 = 1;
/// Upper bound for the per-task action cap.
pub const MAX_ACTION_CAP: u32 = 50;
/// Maximum number of remembered preferences.
pub const MAX_MEMORY_ITEMS: usize = 50;
/// Maximum length of one remembered preference.
pub const MAX_MEMORY_ITEM_CHARS: usize = 200;

/// Models and effort levels. All user-configurable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
#[serde(default)]
pub struct ModelSettings {
    /// Router turn (fast, cheap).
    pub router_model: String,
    /// Router effort.
    pub router_effort: String,
    /// Screen loop.
    pub screen_model: String,
    /// Screen-loop effort.
    pub screen_effort: String,
    /// Escalation after repeated failures on one step.
    pub escalation_model: String,
    /// Ask the API to retry refusals on a fallback model (Sonnet/Opus only).
    pub server_fallbacks: bool,
}

impl Default for ModelSettings {
    fn default() -> Self {
        Self {
            router_model: "claude-haiku-5-5".into(),
            router_effort: "low".into(),
            screen_model: "claude-sonnet-5-5".into(),
            screen_effort: "medium".into(),
            escalation_model: "claude-opus-5-5".into(),
            server_fallbacks: true,
        }
    }
}

/// Everything the user can configure (except secrets).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
#[serde(default)]
pub struct Settings {
    /// Schema version of this file.
    pub schema_version: u32,
    /// Models.
    pub models: ModelSettings,
    /// Reply language (BCP-47). `None` = phone locale.
    pub language: Option<String>,
    /// Preferred music app package.
    pub music_app: Option<String>,
    /// Preferred maps app package.
    pub maps_app: Option<String>,
    /// Preferred messaging app package (used when the user names none).
    pub messaging_app: Option<String>,
    /// Packages the agent must never operate or launch.
    pub blocklist: Vec<String>,
    /// Ask before every action, not just sensitive ones.
    pub confirm_everything: bool,
    /// Maximum UI actions per screen task (1..=50).
    pub action_cap: u32,
    /// Things the user asked the agent to remember ("my wife is Priya").
    pub memory: Vec<String>,
    /// Local spending limit per UTC day, micro-USD (0 = none). See `budget`.
    pub daily_limit_usd_micros: u64,
    /// Local spending limit per UTC month, micro-USD (0 = none).
    pub monthly_limit_usd_micros: u64,
}

/// Region-neutral defaults: password managers and authenticators. Payment
/// apps are not blocked but always require confirmation (see `safety`).
pub const DEFAULT_BLOCKLIST: &[&str] = &[
    "com.x8bit.bitwarden",
    "com.agilebits.onepassword",
    "com.lastpass.lpandroid",
    "com.dashlane",
    "keepass2android.keepass2android",
    "com.kunzisoft.keepass.libre",
    "com.google.android.apps.authenticator2",
    "com.azure.authenticator",
    "com.authy.authy",
    "org.fedorahosted.freeotp",
    "com.beemdevelopment.aegis",
];

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            models: ModelSettings::default(),
            language: None,
            music_app: None,
            maps_app: None,
            messaging_app: None,
            blocklist: DEFAULT_BLOCKLIST.iter().map(|s| (*s).to_owned()).collect(),
            confirm_everything: false,
            action_cap: 25,
            memory: Vec::new(),
            daily_limit_usd_micros: 2_000_000,
            monthly_limit_usd_micros: 20_000_000,
        }
    }
}

impl Settings {
    /// Clamp values into their allowed ranges (called after load and update).
    pub fn sanitize(&mut self) {
        self.action_cap = self.action_cap.clamp(1, MAX_ACTION_CAP);
        self.blocklist.retain(|p| !p.trim().is_empty());
        self.blocklist.sort();
        self.blocklist.dedup();
        self.memory.retain(|m| !m.trim().is_empty());
        for m in &mut self.memory {
            if m.chars().count() > MAX_MEMORY_ITEM_CHARS {
                *m = m.chars().take(MAX_MEMORY_ITEM_CHARS).collect();
            }
        }
        self.memory.truncate(MAX_MEMORY_ITEMS);
        for effort in [
            &mut self.models.router_effort,
            &mut self.models.screen_effort,
        ] {
            if !["low", "medium", "high", "xhigh", "max"].contains(&effort.as_str()) {
                *effort = "medium".into();
            }
        }
        let defaults = ModelSettings::default();
        for (m, d) in [
            (&mut self.models.router_model, defaults.router_model),
            (&mut self.models.screen_model, defaults.screen_model),
            (&mut self.models.escalation_model, defaults.escalation_model),
        ] {
            if m.trim().is_empty() {
                *m = d;
            }
        }
        self.schema_version = SCHEMA_VERSION;
    }

    /// Whether `package` is on the blocklist.
    pub fn is_blocked(&self, package: &str) -> bool {
        self.blocklist.iter().any(|b| b == package)
    }
}

/// Store errors.
#[derive(Debug, Error)]
pub enum StoreError {
    /// File system error.
    #[error("settings storage error: {0}")]
    Io(#[from] std::io::Error),
    /// File is not valid settings JSON.
    #[error("settings file is corrupt: {0}")]
    Corrupt(String),
    /// Written by a newer app version.
    #[error("settings were written by a newer version (schema {0})")]
    TooNew(u32),
}

/// The settings file.
#[derive(Debug)]
pub struct Store {
    path: PathBuf,
    settings: Settings,
}

impl Store {
    /// Open (or create with defaults) `<dir>/settings.json`.
    ///
    /// A corrupt file is moved aside to `settings.json.corrupt` and defaults
    /// are used, so a bad write can never brick the app.
    pub fn open(dir: &Path) -> Result<Self, StoreError> {
        fs::create_dir_all(dir)?;
        let path = dir.join("settings.json");
        let settings = match fs::read(&path) {
            Ok(bytes) => match load(&bytes) {
                Ok(s) => s,
                Err(StoreError::Corrupt(why)) => {
                    tracing::warn!("settings corrupt ({why}); resetting to defaults");
                    let _ = fs::rename(&path, dir.join("settings.json.corrupt"));
                    Settings::default()
                }
                Err(e) => return Err(e),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Settings::default(),
            Err(e) => return Err(e.into()),
        };
        let store = Self { path, settings };
        store.save()?;
        Ok(store)
    }

    /// Current settings.
    pub fn get(&self) -> &Settings {
        &self.settings
    }

    /// Replace settings (sanitised) and persist.
    pub fn set(&mut self, mut s: Settings) -> Result<(), StoreError> {
        s.sanitize();
        self.settings = s;
        self.save()
    }

    fn save(&self) -> Result<(), StoreError> {
        let json = serde_json::to_vec_pretty(&self.settings)
            .map_err(|e| StoreError::Corrupt(e.to_string()))?;
        atomic_write(&self.path, &json)?;
        Ok(())
    }
}

/// Parse and migrate a settings file.
fn load(bytes: &[u8]) -> Result<Settings, StoreError> {
    let mut v: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|e| StoreError::Corrupt(e.to_string()))?;
    let version = v
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let version =
        u32::try_from(version).map_err(|_| StoreError::Corrupt("bad schema_version".into()))?;
    if version > SCHEMA_VERSION {
        return Err(StoreError::TooNew(version));
    }
    migrate(&mut v, version);
    let mut s: Settings =
        serde_json::from_value(v).map_err(|e| StoreError::Corrupt(e.to_string()))?;
    s.sanitize();
    Ok(s)
}

/// Forward migrations, one step per version. Add `n => { … }` arms here when
/// bumping [`SCHEMA_VERSION`]; never edit old arms.
fn migrate(v: &mut serde_json::Value, from: u32) {
    let mut version = from;
    while version < SCHEMA_VERSION {
        if version == 0 {
            // v0 = pre-release files without a version field; fields are
            // compatible, defaults fill anything missing.
        }
        version += 1;
    }
    if let Some(obj) = v.as_object_mut() {
        obj.insert("schema_version".into(), SCHEMA_VERSION.into());
    }
}

/// Write `data` to `path` atomically: temp file in the same directory, fsync,
/// rename over the target.
pub fn atomic_write(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_defaults_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Store::open(dir.path()).unwrap();
        assert_eq!(s.get().action_cap, 25);
        assert!(s.get().is_blocked("com.x8bit.bitwarden"));
        let mut new = s.get().clone();
        new.music_app = Some("com.spotify.music".into());
        new.action_cap = 500;
        s.set(new).unwrap();
        let reopened = Store::open(dir.path()).unwrap();
        assert_eq!(
            reopened.get().music_app.as_deref(),
            Some("com.spotify.music")
        );
        assert_eq!(reopened.get().action_cap, MAX_ACTION_CAP);
        assert!(!dir.path().join("settings.tmp").exists());
    }

    #[test]
    fn missing_fields_get_defaults_and_unknown_fields_are_ignored() {
        let s = load(br#"{"schema_version":1,"language":"hi-IN","future_field":true}"#).unwrap();
        assert_eq!(s.language.as_deref(), Some("hi-IN"));
        assert_eq!(s.models, ModelSettings::default());
    }

    #[test]
    fn v0_files_migrate() {
        let s = load(br#"{"action_cap":10}"#).unwrap();
        assert_eq!(s.schema_version, SCHEMA_VERSION);
        assert_eq!(s.action_cap, 10);
    }

    #[test]
    fn newer_schema_is_refused() {
        assert!(matches!(
            load(br#"{"schema_version":99}"#),
            Err(StoreError::TooNew(99))
        ));
    }

    #[test]
    fn corrupt_file_is_moved_aside() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("settings.json"), b"{not json").unwrap();
        let s = Store::open(dir.path()).unwrap();
        assert_eq!(s.get(), &Settings::default());
        assert!(dir.path().join("settings.json.corrupt").exists());
    }

    #[test]
    fn sanitize_bounds() {
        let mut s = Settings {
            action_cap: 0,
            ..Default::default()
        };
        s.memory = vec!["x".repeat(500), " ".into()];
        s.models.router_effort = "turbo".into();
        s.models.screen_model = String::new();
        s.sanitize();
        assert_eq!(s.action_cap, 1);
        assert_eq!(s.memory.len(), 1);
        assert_eq!(s.memory[0].chars().count(), MAX_MEMORY_ITEM_CHARS);
        assert_eq!(s.models.router_effort, "medium");
        assert_eq!(s.models.screen_model, "claude-sonnet-5-5");
    }
}
