//! devguard: configuration, validation and wire protocol types shared by
//! the daemon, the CLI and the GUI.

pub mod check;
pub mod config;
pub mod doctor;
pub mod identity;
pub mod journal;
pub mod paths;
pub mod policy;
pub mod protocol;
pub mod store;
pub mod util;

#[cfg(unix)]
pub mod client;

pub use check::{CheckItem, CheckReport, Severity};
pub use config::{
    AccessConfig, BaudRate, DeviceConfig, DeviceId, DeviceKind, FlowControl, IdRole, Parity,
    SerialConfig, ValidationIssue,
};
pub use doctor::{DoctorItem, DoctorReport, DoctorStatus};
pub use identity::{match_quality, Binding, DeviceStatus, OnlineDevice, UdevFacts};
pub use journal::{Journal, JournalEntry, JournalQuery, LogFilter};
pub use paths::Paths;
pub use policy::{PauseKind, PauseState, PolicyMode, ServiceState, StartupReconcile};
pub use protocol::{
    Event, EventMessage, Operator, PolicyState, ProtocolError, Request, RequestEnvelope,
    ResponseEnvelope, PROTOCOL_VERSION,
};
pub use store::{ConfigLoadError, DeviceStore, LoadedStore, SaveOutcome};
