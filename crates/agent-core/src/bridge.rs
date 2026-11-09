//! The callback interface Kotlin implements: everything `agent-core` needs
//! from the phone.
//!
//! All methods are called from the agent's worker thread (Kotlin runs
//! `Agent::handle_command` on `Dispatchers.IO`), so blocking is fine and
//! expected: `confirm` blocks until the user answers.

use crate::types::{ActionResult, AppInfo, Credentials, IntentSpec, ScreenSnapshot, UiAction};

/// Platform services provided by the Android app.
#[uniffi::export(with_foreign)]
pub trait PhoneBridge: Send + Sync {
    /// Current credentials, read from the encrypted vault for this command.
    fn credentials(&self) -> Credentials;
    /// Launch an intent; `ok=false` if nothing on the phone can handle it.
    fn launch_intent(&self, spec: IntentSpec) -> ActionResult;
    /// Launchable apps (label + package).
    fn installed_apps(&self) -> Vec<AppInfo>;
    /// Snapshot the current screen via the AccessibilityService.
    fn snapshot_screen(&self) -> ScreenSnapshot;
    /// Perform one UI action.
    fn perform(&self, action: UiAction) -> ActionResult;
    /// Show Approve/Cancel (voice "yes"/"no" also accepted) and block until
    /// answered. `true` = approved.
    fn confirm(&self, summary: String) -> bool;
    /// Speak a short reply with TTS.
    fn speak(&self, text: String);
    /// Show a short status line (e.g. "Thinking…", "Opening Swiggy…").
    fn status(&self, text: String);
    /// Whether the user pressed STOP. Checked between every step.
    fn is_stopped(&self) -> bool;
    /// BCP-47 language for replies, e.g. `en-IN`, `hi-IN`.
    fn locale(&self) -> String;
}
