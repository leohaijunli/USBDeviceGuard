//! Device identity and udev facts.
//!
//! The stable key is `VID:PID + ID_SERIAL_SHORT`.  Devices without a serial
//! number (many CH340 adapters) fall back to `VID:PID + ID_PATH`; the GUI
//! labels that "绑定物理口".  A PX4 flight controller may register several
//! ids (application and bootloader) under one name.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::config::{DeviceConfig, DeviceId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Binding {
    /// VID:PID + ID_SERIAL_SHORT: the device follows its serial number.
    Serial,
    /// VID:PID + ID_PATH: the device is bound to this physical port.
    PhysicalPort,
}

impl Binding {
    pub fn label(self) -> &'static str {
        match self {
            Binding::Serial => "序列号绑定",
            Binding::PhysicalPort => "绑定物理口",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceStatus {
    Managed,
    Unregistered,
    Conflict,
    Drift,
    Blocked,
}

impl DeviceStatus {
    pub fn label(self) -> &'static str {
        match self {
            DeviceStatus::Managed => "已管理",
            DeviceStatus::Unregistered => "未登记",
            DeviceStatus::Conflict => "冲突",
            DeviceStatus::Drift => "偏差",
            DeviceStatus::Blocked => "已拦截",
        }
    }
}

/// Facts collected from `udevadm info` or the udev monitor.  `properties`
/// keeps every `KEY=VALUE` so `learn_candidate` can show the raw evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct UdevFacts {
    pub devnode: String,
    pub subsystem: String,
    #[serde(default)]
    pub vid: String,
    #[serde(default)]
    pub pid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial_short: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usb_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driver: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    #[serde(default)]
    pub is_cdc_acm: bool,
    #[serde(default)]
    pub is_usb: bool,
    #[serde(default)]
    pub properties: BTreeMap<String, String>,
    /// True when udev did not report ID_USB_INTERFACES, or it was malformed:
    /// the device cannot be assumed safe to deauthorize.
    #[serde(default)]
    pub interface_unknown: bool,
    /// True when the USB parent also exposes a HID, hub, storage, video or
    /// audio interface: deauthorizing it would block more than the serial port.
    #[serde(default)]
    pub forbidden_interface_class: bool,
}

impl UdevFacts {
    pub fn from_properties(properties: BTreeMap<String, String>) -> Option<Self> {
        let devnode = properties.get("DEVNAME").map(|value| {
            if value.starts_with('/') {
                value.clone()
            } else {
                format!("/dev/{value}")
            }
        })?;
        let subsystem = properties
            .get("SUBSYSTEM")
            .cloned()
            .unwrap_or_else(|| "tty".to_owned());
        let vid = properties
            .get("ID_VENDOR_ID")
            .or_else(|| properties.get("ID_USB_VENDOR_ID"))
            .cloned()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let pid = properties
            .get("ID_MODEL_ID")
            .or_else(|| properties.get("ID_USB_MODEL_ID"))
            .cloned()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let driver = properties
            .get("ID_USB_DRIVER")
            .or_else(|| properties.get("DRIVER"))
            .cloned();
        let is_cdc_acm = driver.as_deref() == Some("cdc_acm")
            || properties
                .get("ID_USB_INTERFACES")
                .is_some_and(|value| value.contains(":0202:"));
        let is_usb = properties.get("ID_BUS").is_some_and(|bus| bus == "usb") || !vid.is_empty();
        // The interface classes come from the same properties; a malformed or
        // missing value must not silently pass a composite-safety gate.
        let interfaces = parse_usb_interfaces(properties.get("ID_USB_INTERFACES").map(String::as_str).unwrap_or(""));
        let (interface_unknown, forbidden_interface_class) = match interfaces {
            Ok(classes) => (false, classes.iter().any(|class| forbidden_usb_class(*class))),
            Err(_) => (true, false),
        };
        Some(Self {
            devnode,
            subsystem,
            vid,
            pid,
            serial_short: properties
                .get("ID_SERIAL_SHORT")
                .filter(|value| !value.trim().is_empty())
                .cloned(),
            id_path: properties.get("ID_PATH").cloned(),
            usb_path: properties.get("DEVPATH").cloned(),
            driver,
            model: properties.get("ID_MODEL").cloned(),
            vendor: properties.get("ID_VENDOR").cloned(),
            is_cdc_acm,
            is_usb,
            properties,
            interface_unknown,
            forbidden_interface_class,
        })
    }

    pub fn kernel_name(&self) -> &str {
        self.devnode
            .rsplit('/')
            .next()
            .unwrap_or(self.devnode.as_str())
    }

    /// Enforce must never touch keyboards, mice, hubs or storage: only tty
    /// devices named ttyUSB* / ttyACM* are candidates.
    pub fn is_managed_tty(&self) -> bool {
        if self.subsystem != "tty"
            || !self.is_usb
            || crate::util::normalize_hex4(&self.vid).is_err()
            || crate::util::normalize_hex4(&self.pid).is_err()
        {
            return false;
        }
        let Some(name) = self.devnode.strip_prefix("/dev/") else {
            return false;
        };
        name.strip_prefix("ttyUSB")
            .or_else(|| name.strip_prefix("ttyACM"))
            .is_some_and(|suffix| !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit()))
    }

    /// The interface classes a composite USB parent exposes, from the udev
    /// `ID_USB_INTERFACES` value (`:XXYYZZ:XXYYZZ:` where `XX` is the class,
    /// `YY` the subclass, `ZZ` the protocol). A missing or malformed value
    /// means "unknown" and is reported, not silently assumed safe.
    pub fn usb_interface_classes(&self) -> Result<Vec<u8>, String> {
        let Some(raw) = self.properties.get("ID_USB_INTERFACES") else {
            return Err("no ID_USB_INTERFACES from udev".into());
        };
        parse_usb_interfaces(raw)
    }

    /// Whether deauthorizing this USB parent would also disable HID, hub or
    /// mass-storage interfaces, which `enforce` must never do. The function
    /// classes come from the USB class code (the first byte of each
    /// `:XXYYZZ:` triplet); subclass and protocol are ignored here because the
    /// class already forbids the whole family (keyboards, mice, hubs, disks).
    pub fn affects_forbidden_interface_class(&self) -> Result<bool, String> {
        Ok(self
            .usb_interface_classes()?
            .iter()
            .any(|class| forbidden_usb_class(*class)))
    }

    pub fn to_online(&self) -> OnlineDevice {
        OnlineDevice {
            devnode: self.devnode.clone(),
            name: self.kernel_name().to_owned(),
            vid: self.vid.clone(),
            pid: self.pid.clone(),
            serial: self.serial_short.clone(),
            id_path: self.id_path.clone(),
            usb_path: self.usb_path.clone(),
            driver: self.driver.clone(),
            model: self.model.clone(),
            vendor: self.vendor.clone(),
            subsystem: self.subsystem.clone(),
            is_cdc_acm: self.is_cdc_acm,
            interface_unknown: self.interface_unknown,
            forbidden_interface_class: self.forbidden_interface_class,
            status: DeviceStatus::Unregistered,
            managed_by: None,
            binding: None,
            notes: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnlineDevice {
    pub devnode: String,
    pub name: String,
    pub vid: String,
    pub pid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usb_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driver: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    pub subsystem: String,
    pub is_cdc_acm: bool,
    #[serde(default)]
    pub interface_unknown: bool,
    #[serde(default)]
    pub forbidden_interface_class: bool,
    pub status: DeviceStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<Binding>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

impl OnlineDevice {
    pub fn display_name(&self) -> String {
        self.model
            .clone()
            .or_else(|| self.vendor.clone())
            .unwrap_or_else(|| format!("{}:{}", self.vid, self.pid))
    }

    pub fn id_label(&self) -> String {
        let mut label = format!("{}:{}", self.vid, self.pid);
        if let Some(serial) = &self.serial {
            label.push_str(&format!(" · {serial}"));
        } else if let Some(path) = &self.id_path {
            label.push_str(" · 绑定物理口");
            let _ = path;
        }
        label
    }
}

/// Returns the matching id and how it is bound.
pub fn match_quality<'a>(
    config: &'a DeviceConfig,
    facts: &UdevFacts,
) -> Option<(Binding, &'a DeviceId)> {
    if !facts.is_managed_tty() {
        return None;
    }
    let vid = facts.vid.trim().to_ascii_lowercase();
    let pid = facts.pid.trim().to_ascii_lowercase();
    for id in &config.ids {
        if id.vid.trim().to_ascii_lowercase() != vid || id.pid.trim().to_ascii_lowercase() != pid {
            continue;
        }
        if let Some(serial) = &id.serial {
            if facts.serial_short.as_deref().map(str::trim) == Some(serial.trim()) {
                return Some((Binding::Serial, id));
            }
            continue;
        }
        if let Some(path) = &id.path {
            if facts.id_path.as_deref() == Some(path.as_str()) {
                return Some((Binding::PhysicalPort, id));
            }
        }
    }
    None
}

/// The USB interface classes of an `ID_USB_INTERFACES` value: each
/// `:XXYYZZ:` triplet contributes `XX`, the interface class. Unknown or
/// malformed input is an error, so a caller reports it rather than assuming
/// the device is safe to deauthorize.
pub fn parse_usb_interfaces(value: &str) -> Result<Vec<u8>, String> {
    let value = value.trim();
    let inner = value
        .strip_prefix(':')
        .and_then(|value| value.strip_suffix(':'))
        .ok_or_else(|| {
            format!("ID_USB_INTERFACES must be a `:XXYYZZ:` list, got `{value}`")
        })?;
    let mut classes = Vec::new();
    if inner.is_empty() {
        return Ok(classes);
    }
    for triplet in inner.split(':') {
        if triplet.is_empty() || triplet.len() % 2 != 0 {
            return Err(format!("invalid interface triplet `{triplet}` in `{value}`"));
        }
        let class = u8::from_str_radix(&triplet[0..2], 16)
            .map_err(|_| format!("invalid interface class in `{value}`"))?;
        classes.push(class);
    }
    Ok(classes)
}

/// USB base class codes whose deauthorization would block HID, hubs or
/// storage. Enforce must never touch these families, even through a
/// composite parent that also carries a serial interface.
pub fn forbidden_usb_class(class: u8) -> bool {
    matches!(
        class,
        0x03 /* HID: keyboards, mice */ | 0x09 /* hub */
            | 0x08 /* mass storage: disks, card readers */ | 0x0e /* video */
            | 0x01 /* audio */
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AccessConfig, DeviceKind, IdRole, SerialConfig};

    fn config() -> DeviceConfig {
        DeviceConfig {
            name: "px4-main".into(),
            kind: DeviceKind::Px4,
            description: None,
            ids: vec![
                DeviceId {
                    vid: "26ac".into(),
                    pid: "0032".into(),
                    serial: Some("ABC".into()),
                    path: None,
                    role: IdRole::App,
                },
                DeviceId {
                    vid: "26ac".into(),
                    pid: "0011".into(),
                    serial: Some("ABC".into()),
                    path: None,
                    role: IdRole::Bootloader,
                },
            ],
            serial: SerialConfig::default(),
            access: AccessConfig::default(),
        }
    }

    fn facts(vid: &str, pid: &str, serial: Option<&str>, path: Option<&str>) -> UdevFacts {
        UdevFacts {
            devnode: "/dev/ttyACM0".into(),
            subsystem: "tty".into(),
            vid: vid.into(),
            pid: pid.into(),
            serial_short: serial.map(str::to_owned),
            id_path: path.map(str::to_owned),
            driver: Some("cdc_acm".into()),
            is_cdc_acm: true,
            is_usb: true,
            ..UdevFacts::default()
        }
    }

    #[test]
    fn matches_app_and_bootloader() {
        let mut facts = facts("26AC", "0011", Some("ABC"), None);
        assert!(matches!(
            match_quality(&config(), &facts),
            Some((Binding::Serial, id)) if id.role == IdRole::Bootloader
        ));
        facts.pid = "0032".into();
        assert!(match_quality(&config(), &facts).is_some());
    }

    #[test]
    fn serial_must_match() {
        let facts = facts("26ac", "0032", Some("OTHER"), None);
        assert!(match_quality(&config(), &facts).is_none());
    }

    #[test]
    fn a_serial_binding_never_falls_back_to_a_matching_port() {
        let mut config = config();
        config.ids[0].path = Some("pci-port-1".into());
        let facts = facts("26ac", "0032", Some("OTHER"), Some("pci-port-1"));
        assert!(match_quality(&config, &facts).is_none());
    }

    #[test]
    fn managed_tty_requires_usb_evidence_and_a_numeric_kernel_suffix() {
        let mut facts = facts("26ac", "0032", Some("ABC"), None);
        facts.is_usb = false;
        assert!(!facts.is_managed_tty());
        facts.is_usb = true;
        for name in ["ttyACM", "ttyUSBfake", "ttyUSB0/../hidraw0", "ttyS0"] {
            facts.devnode = format!("/dev/{name}");
            assert!(!facts.is_managed_tty(), "{name}");
        }
        for name in ["ttyACM0", "ttyUSB12"] {
            facts.devnode = format!("/dev/{name}");
            assert!(facts.is_managed_tty(), "{name}");
        }
        facts.vid.clear();
        assert!(!facts.is_managed_tty());
    }

    #[test]
    fn physical_port_fallback() {
        let mut config = config();
        config.ids[0].serial = None;
        config.ids[0].path = Some("pci-0000:00:14.0-usb-0:2:1.0".into());
        let facts = facts(
            "26ac",
            "0032",
            Some("CH340"),
            Some("pci-0000:00:14.0-usb-0:2:1.0"),
        );
        assert!(matches!(
            match_quality(&config, &facts),
            Some((Binding::PhysicalPort, _))
        ));
    }

    #[test]
    fn never_matches_non_tty() {
        let mut facts = facts("26ac", "0032", Some("ABC"), None);
        facts.subsystem = "usb".into();
        assert!(match_quality(&config(), &facts).is_none());
        facts.subsystem = "tty".into();
        facts.devnode = "/dev/ttyS0".into();
        assert!(match_quality(&config(), &facts).is_none());
    }

    #[test]
    fn parses_udev_properties() {
        let mut props = BTreeMap::new();
        props.insert("DEVNAME".into(), "/dev/ttyUSB0".into());
        props.insert("SUBSYSTEM".into(), "tty".into());
        props.insert("ID_VENDOR_ID".into(), "1A86".into());
        props.insert("ID_MODEL_ID".into(), "7523".into());
        props.insert("ID_SERIAL_SHORT".into(), "1420".into());
        props.insert("ID_PATH".into(), "pci-0000:00:14.0-usb-0:2:1.0".into());
        props.insert("ID_USB_DRIVER".into(), "ch341-uart".into());
        let facts = UdevFacts::from_properties(props).unwrap();
        assert_eq!(facts.vid, "1a86");
        assert_eq!(facts.kernel_name(), "ttyUSB0");
        assert!(facts.is_managed_tty());
    }

    #[test]
    fn interface_classes_parse_the_class_of_each_triplet() {
        assert_eq!(parse_usb_interfaces(":020200:0a0000:").unwrap(), vec![0x02, 0x0a]);
        assert_eq!(parse_usb_interfaces(":020200:").unwrap(), vec![0x02]);
        assert!(parse_usb_interfaces("020200").is_err());
        assert!(parse_usb_interfaces(":").is_err());
        assert!(parse_usb_interfaces("").is_err());
        assert!(parse_usb_interfaces(":GG0000:").is_err());
    }

    #[test]
    fn composite_parent_with_hid_or_hub_or_storage_is_forbidden() {
        let mut facts = facts("26ac", "0032", Some("ABC"), None);
        facts.properties.insert(
            "ID_USB_INTERFACES".into(),
            ":030101:020200:".into(), // HID keyboard + CDC-ACM serial
        );
        assert!(facts.affects_forbidden_interface_class().unwrap());
        facts.properties.insert(
            "ID_USB_INTERFACES".into(),
            ":080650:020200:".into(), // mass storage + CDC-ACM
        );
        assert!(facts.affects_forbidden_interface_class().unwrap());
        facts.properties.insert(
            "ID_USB_INTERFACES".into(),
            ":090000:020200:".into(), // hub + CDC-ACM
        );
        assert!(facts.affects_forbidden_interface_class().unwrap());
    }

    #[test]
    fn a_pure_serial_parent_is_not_forbidden() {
        let mut facts = facts("26ac", "0032", Some("ABC"), None);
        facts.properties.insert("ID_USB_INTERFACES".into(), ":020200:".into());
        assert!(!facts.affects_forbidden_interface_class().unwrap());
    }

    #[test]
    fn missing_interfaces_is_reported_not_assumed_safe() {
        let facts = facts("26ac", "0032", Some("ABC"), None);
        assert!(facts.affects_forbidden_interface_class().is_err());
    }
}
