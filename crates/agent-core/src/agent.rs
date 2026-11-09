//! The [`Agent`] object exported to Kotlin, plus free functions for
//! onboarding (pairing, relay checks).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use pocket_agent_protocol::{PROTOCOL_VERSION, PairingPayload};
use serde_json::Value;

use crate::bridge::PhoneBridge;
use crate::budget::{self, Budget, BudgetStatus};
use crate::client::{self, ApiClient, ApiError};
use crate::i18n::{Msg, t};
use crate::router::{self, CommandCtx};
use crate::safety::Policy;
use crate::settings::{Settings, Store};
use crate::tools::{self, Tool};
use crate::types::{FailureKind, Outcome};
use crate::usage::{SessionUsage, UsageTotals};

/// Errors returned to Kotlin from non-command calls.
#[derive(Debug, thiserror::Error, uniffi::Error)]
#[uniffi(flat_error)]
pub enum AgentError {
    /// Settings could not be read or written.
    #[error("storage: {0}")]
    Storage(String),
    /// Input was invalid (bad QR code, bad JSON, bad URL).
    #[error("invalid input: {0}")]
    InvalidInput(String),
    /// Network problem.
    #[error("network: {0}")]
    Network(String),
    /// Wrong pairing code / rejected key.
    #[error("unauthorized")]
    Unauthorized,
    /// Relay locked pairing after too many wrong codes.
    #[error("pairing locked")]
    PairingLocked,
    /// URL is not a Pocket Agent relay.
    #[error("not a Pocket Agent relay")]
    NotARelay,
    /// The relay speaks an incompatible protocol version.
    #[error("relay protocol {relay} is not supported (app supports {app})")]
    IncompatibleRelay {
        /// Relay's version.
        relay: u32,
        /// App's version.
        app: u32,
    },
}

impl From<ApiError> for AgentError {
    fn from(e: ApiError) -> Self {
        match e {
            ApiError::KeyRejected { .. } | ApiError::DeviceUnauthorized => Self::Unauthorized,
            ApiError::PairingLocked => Self::PairingLocked,
            ApiError::NotARelay => Self::NotARelay,
            ApiError::InvalidConfig(m) => Self::InvalidInput(m),
            other => Self::Network(other.to_string()),
        }
    }
}

/// The on-device agent. One instance per app process; `handle_command` is
/// called on a background thread.
#[derive(uniffi::Object)]
pub struct Agent {
    bridge: Arc<dyn PhoneBridge>,
    store: Mutex<Store>,
    policy: Policy,
    tools: Vec<Box<dyn Tool>>,
    tool_defs: Vec<Value>,
    usage: SessionUsage,
    budget: Budget,
}

#[uniffi::export]
impl Agent {
    /// Create the agent. `data_dir` is app-private storage for `settings.json`.
    #[uniffi::constructor]
    pub fn new(bridge: Arc<dyn PhoneBridge>, data_dir: String) -> Result<Arc<Self>, AgentError> {
        let dir = PathBuf::from(data_dir);
        let budget = Budget::open(&dir);
        let store = Store::open(&dir).map_err(|e| AgentError::Storage(e.to_string()))?;
        let tools = tools::router_tools();
        let tool_defs = tools::definitions(&tools);
        Ok(Arc::new(Self {
            bridge,
            store: Mutex::new(store),
            policy: Policy::load(),
            tools,
            tool_defs,
            usage: SessionUsage::default(),
            budget,
        }))
    }

    /// Handle one spoken command end to end. Never throws: failures are
    /// spoken and returned as [`Outcome::Failed`].
    pub fn handle_command(&self, text: String) -> Outcome {
        let settings = self.settings();
        let locale = settings
            .language
            .clone()
            .unwrap_or_else(|| self.bridge.locale());
        if text.trim().is_empty() {
            return Outcome::Cancelled {
                spoken: String::new(),
            };
        }
        let client = match ApiClient::from_credentials(self.bridge.credentials()) {
            Ok(c) => c,
            Err(_) => {
                let spoken = t(&locale, Msg::NotConfigured, &[]);
                self.bridge.speak(spoken.clone());
                return Outcome::Failed {
                    kind: FailureKind::NotConfigured,
                    spoken,
                };
            }
        };
        let ctx = CommandCtx {
            bridge: self.bridge.as_ref(),
            settings: &settings,
            policy: &self.policy,
            tools: &self.tools,
            tool_defs: &self.tool_defs,
            locale,
            usage: &self.usage,
            budget: &self.budget,
            today: budget::today(),
        };
        router::run(&ctx, &client, &text)
    }

    /// Current settings.
    pub fn settings(&self) -> Settings {
        self.store
            .lock()
            .map(|s| s.get().clone())
            .unwrap_or_default()
    }

    /// Replace settings (values are clamped to valid ranges).
    pub fn update_settings(&self, settings: Settings) -> Result<(), AgentError> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| AgentError::Storage("lock poisoned".into()))?;
        store
            .set(settings)
            .map_err(|e| AgentError::Storage(e.to_string()))
    }

    /// Settings as JSON for export (never contains secrets).
    pub fn export_settings_json(&self) -> String {
        serde_json::to_string_pretty(&self.settings()).unwrap_or_default()
    }

    /// Import settings exported by [`Agent::export_settings_json`].
    pub fn import_settings_json(&self, json: String) -> Result<(), AgentError> {
        let s: Settings =
            serde_json::from_str(&json).map_err(|e| AgentError::InvalidInput(e.to_string()))?;
        self.update_settings(s)
    }

    /// Estimated local spend and limits (usage screen).
    pub fn budget_status(&self) -> BudgetStatus {
        let s = self.settings();
        self.budget.status(
            &budget::today(),
            s.daily_limit_usd_micros,
            s.monthly_limit_usd_micros,
        )
    }

    /// Token usage since the agent was created.
    pub fn session_usage(&self) -> UsageTotals {
        self.usage.totals()
    }
}

impl Agent {
    /// Usage of the most recent API response (REPL / tests).
    pub fn last_usage(&self) -> Option<crate::api::Usage> {
        self.usage.last()
    }
}

/// Relay URL + pairing code decoded from a QR code.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PairingInfo {
    /// Relay URL.
    pub url: String,
    /// Pairing code (exchange it immediately; never store it).
    pub code: String,
}

/// Result of pairing.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct PairResult {
    /// Relay URL (normalised).
    pub url: String,
    /// Device id.
    pub device_id: String,
    /// Device token: store it in the vault.
    pub device_token: String,
    /// Relay version.
    pub relay_version: String,
}

impl std::fmt::Debug for PairResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PairResult")
            .field("url", &self.url)
            .field("device_id", &self.device_id)
            .finish_non_exhaustive()
    }
}

/// Relay information from `/healthz`.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RelayInfo {
    /// Relay SemVer.
    pub version: String,
    /// Relay protocol version.
    pub protocol: u32,
    /// Whether this app supports that protocol.
    pub compatible: bool,
}

/// Decode a scanned `pocketagent://pair?...` QR code.
#[uniffi::export]
pub fn parse_pairing_uri(uri: String) -> Result<PairingInfo, AgentError> {
    let p = PairingPayload::parse(&uri).map_err(|e| AgentError::InvalidInput(e.to_string()))?;
    Ok(PairingInfo {
        url: p.url.clone(),
        code: p.code.clone(),
    })
}

/// Check a relay's `/healthz`.
#[uniffi::export]
pub fn check_relay(url: String) -> Result<RelayInfo, AgentError> {
    let h = client::relay_health(&url)?;
    Ok(RelayInfo {
        compatible: h.protocol == PROTOCOL_VERSION,
        version: h.version,
        protocol: h.protocol,
    })
}

/// Pair this device with a relay: health check, then exchange the code.
#[uniffi::export]
pub fn pair_relay(
    url: String,
    code: String,
    device_name: String,
) -> Result<PairResult, AgentError> {
    let payload =
        PairingPayload::new(&url, &code).map_err(|e| AgentError::InvalidInput(e.to_string()))?;
    let info = check_relay(payload.url.clone())?;
    if !info.compatible {
        return Err(AgentError::IncompatibleRelay {
            relay: info.protocol,
            app: PROTOCOL_VERSION,
        });
    }
    let r = client::relay_pair(&payload.url, &payload.code, &device_name)?;
    Ok(PairResult {
        url: payload.url,
        device_id: r.device_id,
        device_token: r.device_token,
        relay_version: r.relay.version,
    })
}
