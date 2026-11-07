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
}
