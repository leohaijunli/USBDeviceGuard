//! JSON-lines protocol shared by the daemon, the CLI and the GUI.
//!
//! Requests:  `{"v":1,"id":7,"op":"list"}`
//! Responses: `{"v":1,"id":7,"ok":true,"data":{...}}`
//! Events:    `{"v":1,"seq":12,"ts":"...","event":"device_added",...}`
//!
//! Every error carries a reason and a next step.  The version field is
//! mandatory; a mismatch is rejected instead of guessed at.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::check::{CheckItem, CheckReport};
use crate::config::{DeviceConfig, DeviceKind, ValidationIssue};
use crate::doctor::DoctorReport;
use crate::identity::{OnlineDevice, UdevFacts};
use crate::journal::{JournalQuery, LogFilter};
use crate::policy::{PauseKind, PolicyMode, ServiceState, StartupReconcile};
use crate::util::now_rfc3339;

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Operator {
    pub uid: u32,
    pub user: String,
}

impl Operator {
    pub fn new(uid: u32, user: impl Into<String>) -> Self {
        Self {
            uid,
            user: crate::util::clean_user(&user.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtocolError {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_step: Option<String>,
}

impl ProtocolError {
    pub fn new(
        code: impl Into<String>,
        message: impl Into<String>,
        next: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            next_step: Some(next.into()),
        }
    }

    pub fn unauthorized(message: impl Into<String>, next: impl Into<String>) -> Self {
        Self::new("unauthorized", message, next)
    }

    pub fn invalid(message: impl Into<String>, next: impl Into<String>) -> Self {
        Self::new("invalid_request", message, next)
    }

    pub fn not_found(message: impl Into<String>, next: impl Into<String>) -> Self {
        Self::new("not_found", message, next)
    }

    pub fn conflict(message: impl Into<String>, next: impl Into<String>) -> Self {
        Self::new("conflict", message, next)
    }

    pub fn busy(message: impl Into<String>, next: impl Into<String>) -> Self {
        Self::new("busy", message, next)
    }

    pub fn unsupported(message: impl Into<String>, next: impl Into<String>) -> Self {
        Self::new("unsupported", message, next)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(
            "internal_error",
            message,
            "查看 `journalctl -u devguardd` 或 /var/lib/devguard/journal.jsonl",
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    List,
    Show {
        name: String,
    },
    LearnCandidate {
        devnode: String,
    },
    SaveDevice {
        config: DeviceConfig,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    DeleteDevice {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    Apply {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        devnode: Option<String>,
    },
    Check {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    Pause {
        kind: PauseKind,
        minutes: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    Resume {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    PolicyGet,
    PolicySet {
        mode: PolicyMode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    LogQuery {
        #[serde(default)]
        filter: LogFilter,
    },
    ExportLog {
        #[serde(default)]
        filter: LogFilter,
    },
    Doctor,
    SubscribeEvents,
    Shutdown {
        release_blocked: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    ServiceState,
    /// Block one unregistered device after a pause ends (the user chose
    /// "拦截" instead of registering it).
    BlockDevice {
        devnode: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// Keep an unregistered device usable until it is unplugged.
    AllowDevice {
        devnode: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
}

impl Request {
    pub fn name(&self) -> &'static str {
        match self {
            Request::List => "list",
            Request::Show { .. } => "show",
            Request::LearnCandidate { .. } => "learn_candidate",
            Request::SaveDevice { .. } => "save_device",
            Request::DeleteDevice { .. } => "delete_device",
            Request::Apply { .. } => "apply",
            Request::Check { .. } => "check",
            Request::Pause { .. } => "pause",
            Request::Resume { .. } => "resume",
            Request::PolicyGet => "policy_get",
            Request::PolicySet { .. } => "policy_set",
            Request::LogQuery { .. } => "log_query",
            Request::ExportLog { .. } => "export_log",
            Request::Doctor => "doctor",
            Request::SubscribeEvents => "subscribe_events",
            Request::Shutdown { .. } => "shutdown",
            Request::ServiceState => "service_state",
            Request::BlockDevice { .. } => "block_device",
            Request::AllowDevice { .. } => "allow_device",
        }
    }

    /// Requests that only read: available to every member of the `devguard`
    /// group.  Everything else goes through polkit.
    pub fn is_read_only(&self) -> bool {
        matches!(
            self,
            Request::List
                | Request::Show { .. }
                | Request::LearnCandidate { .. }
                | Request::Check { .. }
                | Request::PolicyGet
                | Request::LogQuery { .. }
                | Request::ExportLog { .. }
                | Request::Doctor
                | Request::SubscribeEvents
                | Request::ServiceState
        )
    }

    pub fn needs_polkit_action(&self) -> Option<&'static str> {
        match self {
            Request::Pause { .. }
            | Request::Resume { .. }
            | Request::BlockDevice { .. }
            | Request::AllowDevice { .. } => Some("dev.devguard.pause"),
            Request::Shutdown { .. } => Some("dev.devguard.service-stop"),
            Request::SaveDevice { .. } | Request::DeleteDevice { .. } | Request::Apply { .. } => {
                Some("dev.devguard.modify")
            }
            Request::PolicySet { .. } => Some("dev.devguard.modify"),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestEnvelope {
    pub v: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    #[serde(flatten)]
    pub request: Request,
}

impl RequestEnvelope {
    pub fn new(id: u64, request: Request) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            id: Some(id),
            request,
        }
    }

    pub fn parse(line: &str) -> Result<Self, ProtocolError> {
        let envelope: RequestEnvelope = serde_json::from_str(line).map_err(|error| {
            ProtocolError::invalid(
                format!("无法解析请求：{error}"),
                "发送一行 JSON，包含 v 和 op",
            )
        })?;
        if envelope.v != PROTOCOL_VERSION {
            return Err(ProtocolError::unsupported(
                format!(
                    "协议版本 {} 不受支持（本机支持 v{}）",
                    envelope.v, PROTOCOL_VERSION
                ),
                "升级 devguardd 与客户端到同一版本",
            ));
        }
        Ok(envelope)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResponseEnvelope {
    pub v: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ProtocolError>,
}

impl ResponseEnvelope {
    pub fn ok(id: Option<u64>, data: impl Serialize) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            id,
            ok: true,
            data: serde_json::to_value(data).ok(),
            error: None,
        }
    }

    pub fn error(id: Option<u64>, error: ProtocolError) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            id,
            ok: false,
            data: None,
            error: Some(error),
        }
    }

    pub fn parse(line: &str) -> Result<Self, ProtocolError> {
        let envelope: ResponseEnvelope = serde_json::from_str(line).map_err(|error| {
            ProtocolError::invalid(format!("无法解析响应：{error}"), "检查守护进程版本")
        })?;
        if envelope.v != PROTOCOL_VERSION {
            return Err(ProtocolError::unsupported(
                format!("响应协议版本 {} 不受支持", envelope.v),
                "升级 devguardd 与客户端到同一版本",
            ));
        }
        Ok(envelope)
    }

    pub fn into_data<T: for<'de> Deserialize<'de>>(self) -> Result<T, ProtocolError> {
        if let Some(error) = self.error {
            return Err(error);
        }
        let value = self.data.unwrap_or(Value::Null);
        serde_json::from_value(value).map_err(|error| {
            ProtocolError::internal(format!("响应字段与客户端类型不匹配：{error}"))
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    DeviceAdded {
        device: OnlineDevice,
    },
    DeviceRemoved {
        name: String,
        devnode: String,
    },
    DeviceUpdated {
        device: OnlineDevice,
    },
    DriftDetected {
        name: String,
        items: Vec<CheckItem>,
    },
    Blocked {
        device: OnlineDevice,
        reason: String,
    },
    PauseStarted {
        kind: PauseKind,
        minutes: u32,
        until: chrono::DateTime<chrono::Utc>,
        note: Option<String>,
        operator: Operator,
    },
    PauseEnded {
        kind: PauseKind,
        reason: String,
        allowed: Vec<OnlineDevice>,
    },
    AllowedDuringPause {
        device: OnlineDevice,
        kind: PauseKind,
    },
    ConfigChanged {
        name: String,
        before: Option<DeviceConfig>,
        after: Option<DeviceConfig>,
        note: Option<String>,
        operator: Operator,
    },
    PolicyChanged {
        before: PolicyMode,
        after: PolicyMode,
        note: Option<String>,
        operator: Operator,
    },
    ServiceStopping {
        release_blocked: bool,
        note: Option<String>,
        operator: Operator,
    },
    ServiceStarted {
        pid: u32,
        reconcile: StartupReconcile,
    },
    StartupReconcile {
        reconcile: StartupReconcile,
    },
}

impl Event {
    pub fn name(&self) -> &'static str {
        match self {
            Event::DeviceAdded { .. } => "device_added",
            Event::DeviceRemoved { .. } => "device_removed",
            Event::DeviceUpdated { .. } => "device_updated",
            Event::DriftDetected { .. } => "drift_detected",
            Event::Blocked { .. } => "blocked",
            Event::PauseStarted { .. } => "pause_started",
            Event::PauseEnded { .. } => "pause_ended",
            Event::AllowedDuringPause { .. } => "allowed_during_pause",
            Event::ConfigChanged { .. } => "config_changed",
            Event::PolicyChanged { .. } => "policy_changed",
            Event::ServiceStopping { .. } => "service_stopping",
            Event::ServiceStarted { .. } => "service_started",
            Event::StartupReconcile { .. } => "startup_reconcile",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventMessage {
    pub v: u32,
    pub seq: u64,
    pub ts: String,
    #[serde(flatten)]
    pub event: Event,
}

impl EventMessage {
    pub fn new(seq: u64, event: Event) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            seq,
            ts: now_rfc3339(),
            event,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Problem {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    pub message: String,
    pub severity: crate::check::Severity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_step: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListResponse {
    pub service: ServiceState,
    pub policy: PolicyMode,
    pub devices: Vec<DeviceConfig>,
    pub online: Vec<OnlineDevice>,
    #[serde(default)]
    pub problems: Vec<Problem>,
    /// Unregistered devices that were allowed during the current pause.
    #[serde(default)]
    pub paused_allowed: Vec<OnlineDevice>,
    /// Snapshot metadata for the GUI: the daemon always sends fresh data.
    pub generated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShowResponse {
    pub device: DeviceConfig,
    pub online: Vec<OnlineDevice>,
    pub check: CheckReport,
    pub symlink: String,
    pub runtime_env: String,
    pub toml: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LearnResponse {
    pub facts: UdevFacts,
    pub draft: DeviceConfig,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SaveResponse {
    pub name: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup: Option<String>,
    pub warnings: Vec<ValidationIssue>,
    pub toml: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeleteResponse {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppliedSetting {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApplyResponse {
    pub name: String,
    pub devnode: String,
    pub applied: Vec<AppliedSetting>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckResponse {
    pub overall: CheckReport,
    pub reports: Vec<CheckReport>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PauseResponse {
    pub service: ServiceState,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResumeResponse {
    pub service: ServiceState,
    pub allowed: Vec<OnlineDevice>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyState {
    pub mode: PolicyMode,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PolicyResponse {
    pub policy: PolicyState,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogResponse {
    pub query: JournalQuery,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub export_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DoctorResponse {
    pub report: DoctorReport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShutdownResponse {
    pub accepted: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServiceStateResponse {
    pub service: ServiceState,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceActionResponse {
    pub devnode: String,
    pub action: String,
    pub ok: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceKindInfo {
    pub kind: DeviceKind,
    pub label: String,
    pub baud: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_roundtrip() {
        let request = Request::Pause {
            kind: PauseKind::Enforcement,
            minutes: 5,
            note: Some("bench test".into()),
        };
        let line = serde_json::to_string(&RequestEnvelope::new(7, request.clone())).unwrap();
        assert!(line.contains("\"v\":1"), "{line}");
        assert!(line.contains("\"op\":\"pause\""), "{line}");
        let parsed = RequestEnvelope::parse(&line).unwrap();
        assert_eq!(parsed.request, request);
        assert_eq!(parsed.id, Some(7));
    }

    #[test]
    fn shutdown_roundtrip() {
        let request = Request::Shutdown {
            release_blocked: true,
            note: Some("stop for maintenance".into()),
        };
        let line = serde_json::to_string(&RequestEnvelope::new(1, request.clone())).unwrap();
        let parsed = RequestEnvelope::parse(&line).unwrap();
        assert_eq!(parsed.request, request);
    }

    #[test]
    fn wrong_version_is_rejected() {
        let error = RequestEnvelope::parse(r#"{"v":99,"op":"list"}"#).unwrap_err();
        assert_eq!(error.code, "unsupported");
    }

    #[test]
    fn unknown_op_is_rejected() {
        let error = RequestEnvelope::parse(r#"{"v":1,"op":"take_over_world"}"#).unwrap_err();
        assert_eq!(error.code, "invalid_request");
    }

    #[test]
    fn response_roundtrip() {
        let response = ResponseEnvelope::ok(
            Some(3),
            ListResponse {
                service: ServiceState::running(PolicyMode::Audit),
                policy: PolicyMode::Audit,
                devices: Vec::new(),
                online: Vec::new(),
                problems: Vec::new(),
                paused_allowed: Vec::new(),
                generated_at: now_rfc3339(),
            },
        );
        let line = serde_json::to_string(&response).unwrap();
        let parsed = ResponseEnvelope::parse(&line).unwrap();
        assert!(parsed.ok);
        let data: ListResponse = parsed.into_data().unwrap();
        assert_eq!(data.policy, PolicyMode::Audit);
    }

    #[test]
    fn event_roundtrip() {
        let event = Event::PauseStarted {
            kind: PauseKind::Service,
            minutes: 15,
            until: chrono::Utc::now() + chrono::Duration::minutes(15),
            note: Some("testing".into()),
            operator: Operator::new(1000, "alice"),
        };
        let message = EventMessage::new(12, event);
        let line = serde_json::to_string(&message).unwrap();
        assert!(line.contains("\"event\":\"pause_started\""), "{line}");
        let parsed: EventMessage = serde_json::from_str(&line).unwrap();
        assert_eq!(parsed.seq, 12);
    }

    #[test]
    fn read_only_classification() {
        assert!(Request::List.is_read_only());
        assert!(Request::Doctor.is_read_only());
        assert!(!Request::Pause {
            kind: PauseKind::Enforcement,
            minutes: 5,
            note: None
        }
        .is_read_only());
        assert_eq!(
            Request::Shutdown {
                release_blocked: true,
                note: None
            }
            .needs_polkit_action(),
            Some("dev.devguard.service-stop")
        );
    }
}
