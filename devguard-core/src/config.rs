//! The configuration schema, strict validation and TOML round-tripping.
//!
//! Every struct uses `deny_unknown_fields`: a typo in a hand-written file is
//! an error, never silently ignored.  The same validation runs in the daemon
//! before writing and in the GUI before presenting a diff.

use std::collections::{HashMap, HashSet};
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::check::Severity;
use crate::util::{normalize_hex4, parse_mode, validate_group, validate_name};

/// Termios baud rates that Linux can express with `Bxxx` constants.
pub const ALLOWED_BAUDS: &[u32] = &[
    300, 600, 1200, 1800, 2400, 4800, 9600, 19200, 38400, 57600, 115200, 230400, 460800, 500000,
    576000, 921600, 1000000, 1152000, 1500000, 2000000, 2500000, 3000000, 3500000, 4000000,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeviceKind {
    Px4,
    Radio,
    SerialAdapter,
}

impl DeviceKind {
    pub fn label(self) -> &'static str {
        match self {
            DeviceKind::Px4 => "PX4 flight controller",
            DeviceKind::Radio => "USB telemetry radio",
            DeviceKind::SerialAdapter => "serial adapter",
        }
    }

    /// Sensible defaults for the registration wizard.
    pub fn default_serial(self) -> SerialConfig {
        match self {
            DeviceKind::Px4 => SerialConfig {
                baud: BaudRate::NotApplicable,
                data_bits: 8,
                parity: Parity::N,
                stop_bits: 1,
                flow: FlowControl::None,
                raw: true,
            },
            DeviceKind::Radio => SerialConfig {
                baud: BaudRate::Rate(57600),
                data_bits: 8,
                parity: Parity::N,
                stop_bits: 1,
                flow: FlowControl::None,
                raw: true,
            },
            DeviceKind::SerialAdapter => SerialConfig {
                baud: BaudRate::Rate(115200),
                data_bits: 8,
                parity: Parity::N,
                stop_bits: 1,
                flow: FlowControl::None,
                raw: true,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Parity {
    #[default]
    #[serde(alias = "N")]
    N,
    #[serde(alias = "E")]
    E,
    #[serde(alias = "O")]
    O,
}

impl Parity {
    pub fn as_char(self) -> &'static str {
        match self {
            Parity::N => "N",
            Parity::E => "E",
            Parity::O => "O",
        }
    }
}

impl fmt::Display for Parity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_char())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FlowControl {
    #[default]
    None,
    Hardware,
    Software,
}

impl fmt::Display for FlowControl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FlowControl::None => f.write_str("none"),
            FlowControl::Hardware => f.write_str("hardware"),
            FlowControl::Software => f.write_str("software"),
        }
    }
}

/// `baud = 57600` or, for CDC-ACM (PX4), `baud = "n/a"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BaudRate {
    Rate(u32),
    NotApplicable,
}

impl BaudRate {
    pub fn rate(self) -> Option<u32> {
        match self {
            BaudRate::Rate(v) => Some(v),
            BaudRate::NotApplicable => None,
        }
    }

    pub fn is_supported(self) -> bool {
        match self {
            BaudRate::Rate(v) => ALLOWED_BAUDS.contains(&v),
            BaudRate::NotApplicable => true,
        }
    }
}

impl fmt::Display for BaudRate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BaudRate::Rate(v) => write!(f, "{v}"),
            BaudRate::NotApplicable => f.write_str("n/a"),
        }
    }
}

impl Serialize for BaudRate {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            BaudRate::Rate(v) => serializer.serialize_u32(*v),
            BaudRate::NotApplicable => serializer.serialize_str("n/a"),
        }
    }
}

impl<'de> Deserialize<'de> for BaudRate {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Rate(u32),
            Text(String),
        }
        match Repr::deserialize(deserializer)? {
            Repr::Rate(v) => Ok(BaudRate::Rate(v)),
            Repr::Text(s) if s.trim() == "n/a" => Ok(BaudRate::NotApplicable),
            Repr::Text(s) => Err(serde::de::Error::custom(format!(
                "invalid baud rate `{s}`: use a number (e.g. 57600) or \"n/a\""
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SerialConfig {
    pub baud: BaudRate,
    pub data_bits: u8,
    pub parity: Parity,
    pub stop_bits: u8,
    pub flow: FlowControl,
    pub raw: bool,
}

impl Default for SerialConfig {
    fn default() -> Self {
        Self {
            baud: BaudRate::Rate(115200),
            data_bits: 8,
            parity: Parity::N,
            stop_bits: 1,
            flow: FlowControl::None,
            raw: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum IdRole {
    #[default]
    App,
    Bootloader,
}

impl IdRole {
    pub fn label(self) -> &'static str {
        match self {
            IdRole::App => "runtime",
            IdRole::Bootloader => "bootloader",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceId {
    pub vid: String,
    pub pid: String,
    /// `ID_SERIAL_SHORT`.  Strong binding: survives moving ports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
    /// `ID_PATH`.  Weak binding: the device is tied to this physical port.
    /// Required when `serial` is absent, and shown as "bound to port" in the GUI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default)]
    pub role: IdRole,
}

impl DeviceId {
    pub fn key(&self) -> String {
        let vid = self.vid.trim().to_ascii_lowercase();
        let pid = self.pid.trim().to_ascii_lowercase();
        match (&self.serial, &self.path) {
            (Some(serial), _) => format!("{vid}:{pid}:serial:{serial}"),
            (None, Some(path)) => format!("{vid}:{pid}:path:{path}"),
            (None, None) => format!("{vid}:{pid}:none"),
        }
    }

    pub fn binding_label(&self) -> &'static str {
        if self.serial.is_some() {
            "bound by serial"
        } else {
            "bound to port"
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccessConfig {
    /// `auto` probes the distribution's serial group (dialout / uucp).
    #[serde(default = "default_group")]
    pub group: String,
    #[serde(default = "default_mode")]
    pub mode: String,
}

fn default_group() -> String {
    "auto".to_owned()
}

fn default_mode() -> String {
    "0660".to_owned()
}

impl Default for AccessConfig {
    fn default() -> Self {
        Self {
            group: default_group(),
            mode: default_mode(),
        }
    }
}

impl AccessConfig {
    pub fn mode_bits(&self) -> u32 {
        parse_mode(&self.mode).unwrap_or(0o660)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceConfig {
    pub name: String,
    pub kind: DeviceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub ids: Vec<DeviceId>,
    pub serial: SerialConfig,
    #[serde(default)]
    pub access: AccessConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationIssue {
    pub field: String,
    pub message: String,
    pub next_step: String,
    pub severity: Severity,
}

impl ValidationIssue {
    pub fn error(
        field: impl Into<String>,
        message: impl Into<String>,
        next: impl Into<String>,
    ) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
            next_step: next.into(),
            severity: Severity::Error,
        }
    }

    pub fn warning(
        field: impl Into<String>,
        message: impl Into<String>,
        next: impl Into<String>,
    ) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
            next_step: next.into(),
            severity: Severity::Warning,
        }
    }
}

fn check_description(description: Option<&str>, issues: &mut Vec<ValidationIssue>) {
    if let Some(description) = description {
        if description.chars().count() > 200 {
            issues.push(ValidationIssue::error(
                "description",
                "the description is limited to 200 characters",
                "shorten the description or drop it",
            ));
        }
        if description.chars().any(char::is_control) {
            issues.push(ValidationIssue::error(
                "description",
                "the description contains control characters",
                "remove line breaks and invisible characters",
            ));
        }
    }
}

impl DeviceConfig {
    pub fn validate(&self) -> Vec<ValidationIssue> {
        let mut issues = Vec::new();

        if let Err(error) = validate_name(&self.name) {
            issues.push(ValidationIssue::error(
                "name",
                error,
                "use a name like px4-main or radio-1",
            ));
        }
        check_description(self.description.as_deref(), &mut issues);

        if self.ids.is_empty() {
            issues.push(ValidationIssue::error(
                "ids",
                "at least one [[ids]] entry is required",
                "run the registration wizard again or add an [[ids]] entry",
            ));
        }
        if self.ids.len() > 8 {
            issues.push(ValidationIssue::error(
                "ids",
                "one device may register at most 8 ids",
                "remove ids that are no longer used",
            ));
        }
        if self.kind == DeviceKind::Px4 && !self.ids.iter().any(|id| id.role == IdRole::App) {
            issues.push(ValidationIssue::warning(
                "ids",
                "the PX4 configuration has no id with role = \"app\"",
                "register runtime and bootloader ids, or mark the runtime one as app",
            ));
        }
        if self.kind == DeviceKind::Px4 && self.ids.len() == 1 && self.ids[0].role == IdRole::App {
            issues.push(ValidationIssue::warning(
                "ids",
                "only one PX4 id is registered; the bootloader re-enumerates with a different PID",
                "enter the bootloader and register it too, with role = bootloader",
            ));
        }

        let mut seen = HashSet::new();
        for (index, id) in self.ids.iter().enumerate() {
            let field = format!("ids[{index}]");
            match normalize_hex4(&id.vid) {
                Ok(vid) if vid != id.vid => issues.push(ValidationIssue::warning(
                    format!("{field}.vid"),
                    "the VID is normalized to lower case",
                    "write lower-case hexadecimal directly",
                )),
                Ok(_) => {}
                Err(error) => issues.push(ValidationIssue::error(
                    format!("{field}.vid"),
                    error,
                    "fill in the USB vendor id, e.g. 26ac",
                )),
            }
            match normalize_hex4(&id.pid) {
                Ok(pid) if pid != id.pid => issues.push(ValidationIssue::warning(
                    format!("{field}.pid"),
                    "the PID is normalized to lower case",
                    "write lower-case hexadecimal directly",
                )),
                Ok(_) => {}
                Err(error) => issues.push(ValidationIssue::error(
                    format!("{field}.pid"),
                    error,
                    "fill in the USB product id, e.g. 0032",
                )),
            }

            match (id.serial.as_deref(), id.path.as_deref()) {
                (None, None) => issues.push(ValidationIssue::error(
                    field.to_string(),
                    "neither serial nor path is set, so the device cannot be bound stably",
                    "fill in serial when available; otherwise use the ID_PATH shown at registration (bound to port)",
                )),
                (Some(""), _) => issues.push(ValidationIssue::error(
                    format!("{field}.serial"),
                    "serial must not be an empty string",
                    "fill in ID_SERIAL_SHORT, or drop serial and use path",
                )),
                (Some(serial), _) => {
                    if serial.len() > 128 {
                        issues.push(ValidationIssue::error(
                            format!("{field}.serial"),
                            "serial is limited to 128 characters",
                            "use ID_SERIAL_SHORT only, not a concatenation of fields",
                        ));
                    }
                    if serial.chars().any(char::is_control) {
                        issues.push(ValidationIssue::error(
                            format!("{field}.serial"),
                            "serial contains control characters",
                            "keep only the visible characters of ID_SERIAL_SHORT",
                        ));
                    }
                }
                (None, Some("")) => issues.push(ValidationIssue::error(
                    format!("{field}.path"),
                    "path must not be an empty string",
                    "fill in the udev ID_PATH, or use serial",
                )),
                (None, Some(path)) => {
                    if path.len() > 256
                        || !path
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
                    {
                        issues.push(ValidationIssue::error(
                            format!("{field}.path"),
                            "the ID_PATH format is invalid",
                            "copy ID_PATH from `udevadm info` exactly",
                        ));
                    }
                }
            }
            if id.serial.is_some() && id.path.is_some() {
                issues.push(ValidationIssue::warning(
                    field.to_string(),
                    "both serial and path are set; matching prefers serial",
                    "usually keep only serial; use path only when there is no serial",
                ));
            }

            let key = id.key();
            if !seen.insert(key) {
                issues.push(ValidationIssue::error(
                    field.to_string(),
                    "duplicate VID:PID + serial/path inside one device",
                    "remove the duplicate [[ids]] entry",
                ));
            }
        }

        match self.serial.baud {
            BaudRate::Rate(rate) if !self.serial.baud.is_supported() => {
                issues.push(ValidationIssue::error(
                    "serial.baud",
                    format!("Linux termios does not support baud rate {rate}"),
                    format!(
                        "use one of: {}, or \"n/a\" for CDC-ACM",
                        ALLOWED_BAUDS
                            .iter()
                            .map(u32::to_string)
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                ))
            }
            BaudRate::NotApplicable if self.kind != DeviceKind::Px4 => {
                issues.push(ValidationIssue::error(
                    "serial.baud",
                    "baud = \"n/a\" only applies to CDC-ACM PX4 flight controllers",
                    "write the actual baud rate for radios or adapters, e.g. 57600",
                ))
            }
            _ => {}
        }
        if !(5..=8).contains(&self.serial.data_bits) {
            issues.push(ValidationIssue::error(
                "serial.data_bits",
                "data_bits must be 5, 6, 7 or 8",
                "8 is the usual value",
            ));
        }
        if !matches!(self.serial.stop_bits, 1 | 2) {
            issues.push(ValidationIssue::error(
                "serial.stop_bits",
                "stop_bits must be 1 or 2",
                "1 is the usual value",
            ));
        }

        if let Err(error) = validate_group(&self.access.group) {
            issues.push(ValidationIssue::error(
                "access.group",
                error,
                "use auto, or the distribution's serial group (dialout / uucp)",
            ));
        }
        if let Err(error) = parse_mode(&self.access.mode) {
            issues.push(ValidationIssue::error(
                "access.mode",
                error,
                "use 0660 (recommended) or 0600",
            ));
        }

        issues
    }

    pub fn has_errors(&self) -> bool {
        self.validate()
            .iter()
            .any(|issue| issue.severity == Severity::Error)
    }

    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }

    pub fn from_toml(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }
}

/// Cross-file rules: names and identity keys are unique across all devices.
pub fn validate_set(devices: &[DeviceConfig]) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    let mut names: HashMap<&str, usize> = HashMap::new();
    let mut ids: HashMap<String, String> = HashMap::new();

    for device in devices {
        issues.extend(device.validate());
        let position = names.len();
        if names.insert(device.name.as_str(), position).is_some() {
            issues.push(ValidationIssue::error(
                "name",
                format!("the device name `{}` is duplicated", device.name),
                "delete or rename one of the devices",
            ));
        }
        for id in &device.ids {
            if let Some(other) = ids.insert(id.key(), device.name.clone()) {
                if other != device.name {
                    issues.push(ValidationIssue::error(
                        "ids",
                        format!(
                            "the id {}:{} already belongs to `{other}` and `{}`",
                            id.vid, id.pid, device.name
                        ),
                        "one USB identity can only be registered to one device; edit one of the configurations",
                    ));
                }
            }
        }
    }
    issues
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> DeviceConfig {
        DeviceConfig {
            name: "px4-main".into(),
            kind: DeviceKind::Px4,
            description: Some("Pixhawk on bench".into()),
            ids: vec![
                DeviceId {
                    vid: "26ac".into(),
                    pid: "0032".into(),
                    serial: Some("0A1B2C3D".into()),
                    path: None,
                    role: IdRole::App,
                },
                DeviceId {
                    vid: "26ac".into(),
                    pid: "0011".into(),
                    serial: Some("0A1B2C3D".into()),
                    path: None,
                    role: IdRole::Bootloader,
                },
            ],
            serial: DeviceKind::Px4.default_serial(),
            access: AccessConfig::default(),
        }
    }

    #[test]
    fn accepts_schema_example() {
        let device = base();
        assert!(device
            .validate()
            .iter()
            .all(|i| i.severity != Severity::Error));
        let toml = device.to_toml().unwrap();
        assert!(toml.contains("baud = \"n/a\""), "{toml}");
        let parsed = DeviceConfig::from_toml(&toml).unwrap();
        assert_eq!(parsed, device);
    }

    #[test]
    fn rejects_unknown_key() {
        let text = r#"
name = "px4-main"
kind = "px4"
description = "x"
[[ids]]
vid = "26ac"
pid = "0032"
serial = "abc"
role = "app"
[serial]
baud = "n/a"
data_bits = 8
parity = "n"
stop_bits = 1
flow = "none"
raw = true
surprise = 1
[access]
group = "auto"
mode = "0660"
"#;
        let err = DeviceConfig::from_toml(text).unwrap_err().to_string();
        assert!(
            err.contains("unknown field") || err.contains("surprise"),
            "{err}"
        );
    }

    #[test]
    fn rejects_duplicate_ids() {
        let mut device = base();
        device.ids[1] = device.ids[0].clone();
        assert!(device.has_errors());
    }

    #[test]
    fn rejects_cross_device_duplicate_ids() {
        let one = base();
        let mut two = base();
        two.name = "px4-second".into();
        assert!(validate_set(&[one, two])
            .iter()
            .any(|i| i.message.contains("already belongs")));
    }

    #[test]
    fn rejects_bad_baud() {
        let mut device = base();
        device.kind = DeviceKind::Radio;
        device.serial.baud = BaudRate::Rate(12345);
        let issues = device.validate();
        assert!(issues.iter().any(|i| i.field == "serial.baud"));
    }

    #[test]
    fn na_only_for_px4() {
        let mut device = base();
        device.kind = DeviceKind::SerialAdapter;
        assert!(device.validate().iter().any(|i| i.field == "serial.baud"));
    }

    #[test]
    fn serial_number_is_required_without_path() {
        let mut device = base();
        device.ids[0].serial = None;
        assert!(device
            .validate()
            .iter()
            .any(|i| i.field.starts_with("ids[0]")));
        device.ids[0].path = Some("pci-0000:00:14.0-usb-0:2:1.0".into());
        assert!(!device.has_errors());
    }

    #[test]
    fn baud_roundtrips_both_shapes() {
        #[derive(Serialize, Deserialize, PartialEq)]
        struct Wrapper {
            baud: BaudRate,
        }
        assert_eq!(
            toml::to_string(&Wrapper { baud: BaudRate::Rate(57600) })
                .unwrap()
                .trim(),
            "baud = 57600"
        );
        assert_eq!(
            toml::to_string(&Wrapper {
                baud: BaudRate::NotApplicable
            })
            .unwrap()
            .trim(),
            "baud = \"n/a\""
        );
        let parsed: Wrapper = toml::from_str("baud = 57600").unwrap();
        assert_eq!(parsed.baud, BaudRate::Rate(57600));
        let parsed: Wrapper = toml::from_str("baud = \"n/a\"").unwrap();
        assert_eq!(parsed.baud, BaudRate::NotApplicable);
    }
}
