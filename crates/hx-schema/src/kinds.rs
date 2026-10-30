//! Core observation kinds: the only payloads the compiler understands.
//! Connectors may emit other kinds; a normalizer maps them to these.

use crate::{Completeness, Interval};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const FOCUS_SPAN: &str = "focus.span";
pub const PRESENCE_SPAN: &str = "presence.span";
pub const INPUT_BUCKET: &str = "input.bucket";
pub const UI_CLICK: &str = "ui.click";
pub const TEXT_SNAPSHOT: &str = "text.snapshot";
pub const WEB_VISIT: &str = "web.visit";
pub const SHELL_COMMAND: &str = "shell.command";
pub const CODE_COMMIT: &str = "code.commit";
pub const AGENT_MESSAGE: &str = "agent.message";
pub const CALENDAR_EVENT: &str = "calendar.event";
pub const MESSAGE_EVENT: &str = "message.event";
pub const MEDIA_CHUNK: &str = "media.chunk";
pub const MEDIA_STILL: &str = "media.still";
pub const USAGE_AGGREGATE: &str = "usage.aggregate";
pub const HEALTH_SAMPLE: &str = "health.sample";
pub const LOCATION_VISIT: &str = "location.visit";
pub const COLLECTOR_HEALTH: &str = "collector.health";
pub const CLOCK_CALIBRATION: &str = "clock.calibration";
pub const CORRECTION: &str = "correction.note";

/// Foreground app/window exposure. Time is in the envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FocusSpan {
    pub app_bundle_id: String,
    pub app_name: String,
    #[serde(default)]
    pub window_title: Option<String>,
    #[serde(default)]
    pub pid: Option<i32>,
    #[serde(default)]
    pub display: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeviceState {
    /// Unlocked, awake, input seen recently.
    Active,
    /// Unlocked and awake, no input for the idle threshold.
    Idle,
    Locked,
    DeviceAsleep,
    DisplayOff,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PresenceSpan {
    pub state: DeviceState,
    /// Idle threshold used to derive `Idle`, so the rule is reproducible.
    #[serde(default)]
    pub idle_threshold_s: Option<u32>,
}

/// Input intensity for one bucket (normally 1 s). Never contains key identities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct InputBucket {
    pub bucket_ms: u32,
    pub keys: u32,
    pub clicks: u32,
    pub scroll_events: u32,
    pub mouse_px: u32,
}

/// Accessibility context sampled after a click, at its own sample time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UiClick {
    pub app_bundle_id: String,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    /// Milliseconds between the click and the hit-test sample.
    pub sample_lag_ms: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TextSource {
    Accessibility,
    Ocr,
    Page,
    Editor,
    Terminal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TextSnapshot {
    /// What surface the text came from, e.g. app bundle + window title, or a URL.
    pub surface: String,
    pub source: TextSource,
    pub text: String,
    /// SHA-256 of `text`, for dedup across a span.
    pub text_sha256: String,
    pub completeness: Completeness,
    /// Loaded is not seen: true only when the text was in the visible viewport.
    #[serde(default)]
    pub visible: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WebVisit {
    pub url: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub tab_id: Option<i64>,
    #[serde(default)]
    pub window_id: Option<i64>,
    /// Page visible and browser window focused at observation time.
    #[serde(default)]
    pub visible: Option<bool>,
    #[serde(default)]
    pub browser: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ShellCommand {
    /// argv after redaction.
    pub command: String,
    pub cwd: String,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub shell: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CodeCommit {
    pub repo: String,
    pub sha: String,
    pub message: String,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub files_changed: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentRole {
    /// A human request.
    User,
    /// Assistant output.
    Assistant,
    /// Tool call or tool result.
    Tool,
    /// Lifecycle record (thread created, turn started/finished).
    Lifecycle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentMessage {
    /// `t3code`, `codex`, `claude-code`, ...
    pub provider: String,
    pub thread_id: String,
    #[serde(default)]
    pub thread_title: Option<String>,
    #[serde(default)]
    pub turn_id: Option<String>,
    pub role: AgentRole,
    pub text: String,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CalendarEvent {
    pub title: String,
    pub planned: Interval,
    #[serde(default)]
    pub attendees: Vec<String>,
    #[serde(default)]
    pub location: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MessageDirection {
    Sent,
    /// Delivered, not proof of being read.
    Received,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MessageEvent {
    pub service: String,
    pub conversation: String,
    pub direction: MessageDirection,
    #[serde(default)]
    pub counterpart: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MediaChunk {
    pub display: String,
    pub codec: String,
    pub width: u32,
    pub height: u32,
    pub frames: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StillTrigger {
    FocusChange,
    ContentChange,
    Periodic,
    ServerRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MediaStill {
    pub display: String,
    pub trigger: StillTrigger,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub app_bundle_id: Option<String>,
    #[serde(default)]
    pub window_title: Option<String>,
}

/// Aggregate usage that cannot be placed into specific seconds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UsageAggregate {
    pub device_label: String,
    pub app: String,
    /// Civil day of the source, `YYYY-MM-DD`.
    pub day: String,
    pub seconds_estimated: u64,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SampleShape {
    Point,
    Interval,
    Aggregate,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HealthSample {
    pub metric: String,
    pub unit: String,
    pub value: f64,
    pub shape: SampleShape,
    #[serde(default)]
    pub sampling_interval_s: Option<u32>,
    #[serde(default)]
    pub algorithm_version: Option<String>,
    #[serde(default)]
    pub source_day: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LocationVisit {
    pub latitude: f64,
    pub longitude: f64,
    #[serde(default)]
    pub accuracy_m: Option<f64>,
    #[serde(default)]
    pub place_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CollectorStatus {
    Running,
    /// Paused on purpose (user pause, policy). Metadata keeps flowing.
    Paused,
    /// Missing OS permission.
    PermissionDenied,
    Failed,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CollectorHealth {
    pub collector: String,
    pub status: CollectorStatus,
    #[serde(default)]
    pub last_sequence: Option<u64>,
    #[serde(default)]
    pub last_observed_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub queue_age_ms: Option<u64>,
    pub dropped: u64,
    #[serde(default)]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClockCalibration {
    /// Estimated offset of the device wall clock from true UTC, ms.
    pub offset_ms: i64,
    /// Error bound of the estimate, ms.
    pub error_ms: u64,
    pub method: String,
}

/// Every core payload, tagged by kind. Used for the JSON Schema export and for
/// typed decoding in normalizers and the compiler.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "payload")]
pub enum CorePayload {
    #[serde(rename = "focus.span")]
    FocusSpan(FocusSpan),
    #[serde(rename = "presence.span")]
    PresenceSpan(PresenceSpan),
    #[serde(rename = "input.bucket")]
    InputBucket(InputBucket),
    #[serde(rename = "ui.click")]
    UiClick(UiClick),
    #[serde(rename = "text.snapshot")]
    TextSnapshot(TextSnapshot),
    #[serde(rename = "web.visit")]
    WebVisit(WebVisit),
    #[serde(rename = "shell.command")]
    ShellCommand(ShellCommand),
    #[serde(rename = "code.commit")]
    CodeCommit(CodeCommit),
    #[serde(rename = "agent.message")]
    AgentMessage(AgentMessage),
    #[serde(rename = "calendar.event")]
    CalendarEvent(CalendarEvent),
    #[serde(rename = "message.event")]
    MessageEvent(MessageEvent),
    #[serde(rename = "media.chunk")]
    MediaChunk(MediaChunk),
    #[serde(rename = "media.still")]
    MediaStill(MediaStill),
    #[serde(rename = "usage.aggregate")]
    UsageAggregate(UsageAggregate),
    #[serde(rename = "health.sample")]
    HealthSample(HealthSample),
    #[serde(rename = "location.visit")]
    LocationVisit(LocationVisit),
    #[serde(rename = "collector.health")]
    CollectorHealth(CollectorHealth),
    #[serde(rename = "clock.calibration")]
    ClockCalibration(ClockCalibration),
}

impl CorePayload {
    /// Decode a core payload from an envelope's kind and payload. `None` for non-core kinds.
    pub fn decode(
        kind: &str,
        payload: &serde_json::Value,
    ) -> Option<Result<Self, serde_json::Error>> {
        if !is_core(kind) {
            return None;
        }
        Some(serde_json::from_value(
            serde_json::json!({ "kind": kind, "payload": payload }),
        ))
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::FocusSpan(_) => FOCUS_SPAN,
            Self::PresenceSpan(_) => PRESENCE_SPAN,
            Self::InputBucket(_) => INPUT_BUCKET,
            Self::UiClick(_) => UI_CLICK,
            Self::TextSnapshot(_) => TEXT_SNAPSHOT,
            Self::WebVisit(_) => WEB_VISIT,
            Self::ShellCommand(_) => SHELL_COMMAND,
            Self::CodeCommit(_) => CODE_COMMIT,
            Self::AgentMessage(_) => AGENT_MESSAGE,
            Self::CalendarEvent(_) => CALENDAR_EVENT,
            Self::MessageEvent(_) => MESSAGE_EVENT,
            Self::MediaChunk(_) => MEDIA_CHUNK,
            Self::MediaStill(_) => MEDIA_STILL,
            Self::UsageAggregate(_) => USAGE_AGGREGATE,
            Self::HealthSample(_) => HEALTH_SAMPLE,
            Self::LocationVisit(_) => LOCATION_VISIT,
            Self::CollectorHealth(_) => COLLECTOR_HEALTH,
            Self::ClockCalibration(_) => CLOCK_CALIBRATION,
        }
    }

    pub fn payload_value(&self) -> serde_json::Value {
        let mut v = serde_json::to_value(self).expect("core payload serializes");
        v.get_mut("payload")
            .map(serde_json::Value::take)
            .unwrap_or_default()
    }
}

pub const CORE_KINDS: &[&str] = &[
    FOCUS_SPAN,
    PRESENCE_SPAN,
    INPUT_BUCKET,
    UI_CLICK,
    TEXT_SNAPSHOT,
    WEB_VISIT,
    SHELL_COMMAND,
    CODE_COMMIT,
    AGENT_MESSAGE,
    CALENDAR_EVENT,
    MESSAGE_EVENT,
    MEDIA_CHUNK,
    MEDIA_STILL,
    USAGE_AGGREGATE,
    HEALTH_SAMPLE,
    LOCATION_VISIT,
    COLLECTOR_HEALTH,
    CLOCK_CALIBRATION,
];

pub fn is_core(kind: &str) -> bool {
    CORE_KINDS.contains(&kind)
}
