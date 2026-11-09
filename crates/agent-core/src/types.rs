//! Plain data types shared with Kotlin through UniFFI.
//!
//! Everything here crosses the FFI boundary, so it is kept flat and simple:
//! records (structs), enums and strings. Secrets arrive in [`Credentials`] as
//! plain strings (UniFFI has no secret type) and are converted into
//! `secrecy::SecretString` immediately by [`crate::client`].

/// How the app talks to Claude. Returned fresh by `PhoneBridge::credentials()`
/// for every command; never stored by the Rust side.
#[derive(Clone, uniffi::Enum)]
pub enum Credentials {
    /// Through the user's own relay.
    Relay {
        /// Relay base URL, e.g. `https://pocket-agent-relay.you.workers.dev`.
        base_url: String,
        /// Device token from pairing (`pa_dt_…`).
        device_token: String,
    },
    /// Directly to `api.anthropic.com` with the user's key.
    Direct {
        /// Anthropic API key (`sk-ant-…`).
        api_key: String,
    },
    /// Not set up yet (onboarding incomplete).
    None,
}

// Never print secrets.
impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Relay { base_url, .. } => write!(f, "Relay({base_url}, <token>)"),
            Self::Direct { .. } => write!(f, "Direct(<key>)"),
            Self::None => write!(f, "None"),
        }
    }
}

/// An installed, launchable app.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AppInfo {
    /// Package name, e.g. `com.spotify.music`.
    pub package: String,
    /// User-visible label, e.g. `Spotify`.
    pub label: String,
}

/// Travel mode for navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum TravelMode {
    /// Car.
    Driving,
    /// On foot.
    Walking,
    /// Bicycle.
    Bicycling,
    /// Public transport.
    Transit,
}

/// A validated Android intent request. Kotlin's `IntentExecutor` maps each
/// variant to a concrete `Intent` and checks `resolveActivity` first.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum IntentSpec {
    /// `MEDIA_PLAY_FROM_SEARCH`, optionally targeted at one package.
    PlayMusic {
        /// What to play.
        query: String,
        /// Target app; `None` lets Android choose.
        package: Option<String>,
    },
    /// Turn-by-turn navigation (`google.navigation:` for Google Maps, `geo:` otherwise).
    Navigate {
        /// Free-text destination.
        destination: String,
        /// Travel mode.
        mode: TravelMode,
        /// Target maps app; `None` lets Android choose.
        package: Option<String>,
    },
    /// `AlarmClock.ACTION_SET_ALARM`.
    SetAlarm {
        /// 0–23.
        hour: u8,
        /// 0–59.
        minute: u8,
        /// Optional label.
        label: Option<String>,
    },
    /// `AlarmClock.ACTION_SET_TIMER`.
    SetTimer {
        /// 1–86,400.
        seconds: u32,
        /// Optional label.
        label: Option<String>,
    },
    /// Phone call. Kotlin resolves a contact name to a number itself.
    Call {
        /// Phone number or contact name.
        target: String,
        /// Whether `target` is a phone number.
        is_number: bool,
    },
    /// Open a compose screen with the text pre-filled (sending is a separate,
    /// confirmed step).
    ComposeMessage {
        /// Messaging app package; `None` = default SMS app.
        package: Option<String>,
        /// Human-readable app name for confirmations.
        app_label: String,
        /// Contact name or number.
        contact: String,
        /// Message body.
        text: String,
    },
    /// Launch an installed app.
    OpenApp {
        /// Package resolved from `installed_apps()`.
        package: String,
        /// Its label.
        label: String,
    },
    /// `ACTION_WEB_SEARCH`.
    WebSearch {
        /// Search query.
        query: String,
    },
}

/// Scroll direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ScrollDirection {
    /// Content moves so you see what is further down.
    Down,
    /// Back towards the top.
    Up,
    /// Towards the right.
    Right,
    /// Towards the left.
    Left,
}

/// One UI action the screen agent asks the AccessibilityService to perform.
/// Node ids refer to the most recent [`ScreenSnapshot`].
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum UiAction {
    /// Tap a node.
    Click {
        /// Node id.
        id: u32,
    },
    /// Long-press a node.
    LongClick {
        /// Node id.
        id: u32,
    },
    /// Replace the text of an editable node.
    SetText {
        /// Node id.
        id: u32,
        /// New text.
        text: String,
    },
    /// Scroll a scrollable node (or the main scrollable area when `None`).
    Scroll {
        /// Node id.
        id: Option<u32>,
        /// Direction.
        direction: ScrollDirection,
    },
    /// System Back.
    Back,
    /// System Home.
    Home,
    /// Recent apps.
    Recents,
    /// Pull down the notification shade.
    OpenNotifications,
    /// Wait for the UI to settle.
    Wait {
        /// Milliseconds (clamped to 5,000).
        ms: u32,
    },
}

/// Result of a platform action.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ActionResult {
    /// Whether it worked.
    pub ok: bool,
    /// Short explanation on failure (shown to the model, never spoken raw).
    pub message: Option<String>,
}

impl ActionResult {
    /// Success.
    pub fn ok() -> Self {
        Self {
            ok: true,
            message: None,
        }
    }
    /// Failure with a reason.
    pub fn failed(msg: impl Into<String>) -> Self {
        Self {
            ok: false,
            message: Some(msg.into()),
        }
    }
}

/// Node bounds in screen pixels.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, uniffi::Record)]
pub struct Rect {
    /// Left edge.
    pub left: i32,
    /// Top edge.
    pub top: i32,
    /// Right edge (exclusive).
    pub right: i32,
    /// Bottom edge (exclusive).
    pub bottom: i32,
}

impl Rect {
    /// Width (0 if inverted).
    pub fn width(&self) -> i32 {
        (self.right - self.left).max(0)
    }
    /// Height (0 if inverted).
    pub fn height(&self) -> i32 {
        (self.bottom - self.top).max(0)
    }
    /// Centre point.
    pub fn center(&self) -> (i32, i32) {
        ((self.left + self.right) / 2, (self.top + self.bottom) / 2)
    }
}

/// One accessibility node, flattened. Parent links let the pruner collapse
/// single-child containers.
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct ScreenNode {
    /// Id unique within one snapshot.
    pub id: u32,
    /// Parent id (`None` for roots).
    pub parent: Option<u32>,
    /// View class, e.g. `android.widget.Button`.
    pub class_name: String,
    /// Visible text.
    pub text: Option<String>,
    /// Content description (accessibility label).
    pub content_desc: Option<String>,
    /// Resource id, e.g. `com.whatsapp:id/send`.
    pub resource_id: Option<String>,
    /// Bounds on screen.
    pub bounds: Rect,
    /// Tappable.
    pub clickable: bool,
    /// Long-pressable.
    pub long_clickable: bool,
    /// Accepts text input.
    pub editable: bool,
    /// Scrollable container.
    pub scrollable: bool,
    /// Checkable (switch, checkbox).
    pub checkable: bool,
    /// Current checked state.
    pub checked: bool,
    /// Password field: text is never sent and typing is refused.
    pub password: bool,
    /// Has input focus.
    pub focused: bool,
    /// Enabled.
    pub enabled: bool,
    /// Visible to the user.
    pub visible: bool,
}

/// The current screen, as captured by the AccessibilityService.
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct ScreenSnapshot {
    /// Foreground package.
    pub package: String,
    /// Foreground activity class, if known.
    pub activity: Option<String>,
    /// Screen width in pixels.
    pub width: i32,
    /// Screen height in pixels.
    pub height: i32,
    /// All nodes, depth-first.
    pub nodes: Vec<ScreenNode>,
}

impl ScreenSnapshot {
    /// Look up a node by id.
    pub fn node(&self, id: u32) -> Option<&ScreenNode> {
        self.nodes.iter().find(|n| n.id == id)
    }
}

/// What happened to a command. Spoken feedback has already been given through
/// `PhoneBridge::speak`; this is for the UI and for tests.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum Outcome {
    /// An action was performed.
    Done {
        /// What was said to the user.
        spoken: String,
    },
    /// Claude answered or asked something without acting.
    Replied {
        /// What was said to the user.
        spoken: String,
    },
    /// The user declined a confirmation or pressed STOP.
    Cancelled {
        /// What was said to the user.
        spoken: String,
    },
    /// Claude declined the request.
    Refused {
        /// What was said to the user.
        spoken: String,
    },
    /// Something went wrong.
    Failed {
        /// Machine-readable reason.
        kind: FailureKind,
        /// What was said to the user.
        spoken: String,
    },
}

impl Outcome {
    /// The text that was spoken.
    pub fn spoken(&self) -> &str {
        match self {
            Self::Done { spoken }
            | Self::Replied { spoken }
            | Self::Cancelled { spoken }
            | Self::Refused { spoken }
            | Self::Failed { spoken, .. } => spoken,
        }
    }
}

/// Machine-readable failure reasons, so the UI can offer the right fix
/// (re-pair, replace key, check network…).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FailureKind {
    /// No credentials configured.
    NotConfigured,
    /// 401/403: device token revoked / API key invalid.
    Unauthorized,
    /// Relay spending cap reached.
    RelayCap,
    /// Rate limited or overloaded after retries.
    Busy,
    /// No network.
    Offline,
    /// Request timed out.
    Timeout,
    /// Server error after retries.
    Server,
    /// Relay rejected the configured model.
    ModelNotAllowed,
    /// The phone could not perform the action.
    ActionFailed,
    /// The target app is on the user's blocklist.
    Blocked,
    /// Feature not available yet.
    Unsupported,
    /// The local daily or monthly spending limit was reached.
    SpendingLimit,
    /// Anything else (bug, unexpected response).
    Internal,
}
