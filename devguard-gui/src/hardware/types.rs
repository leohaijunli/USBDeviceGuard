use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UsbSerialDevice {
    pub instance_id: String,
    pub usb_instance_id: String,
    pub port: String,
    pub name: String,
    pub status: String,
    pub vid: String,
    pub pid: String,
    pub serial_candidate: Option<String>,
    pub driver_service: Option<String>,
    pub driver_version: Option<String>,
    pub problem_code: Option<u32>,
    pub composite_parent: bool,
    pub location_paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UsbSnapshot {
    pub captured_at: String,
    pub computer: String,
    pub automatic_updates: bool,
    pub devices: Vec<UsbSerialDevice>,
    pub ignored_non_usb_ports: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum HardwareEvent {
    Snapshot { snapshot: UsbSnapshot },
    WatchWarning { message: String },
    Error { message: String },
}

impl UsbSerialDevice {
    pub(crate) fn healthy(&self) -> bool {
        self.status.eq_ignore_ascii_case("ok") && self.problem_code.is_none_or(|code| code == 0)
    }

    pub(crate) fn matches(&self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        query.is_empty()
            || [
                &self.name,
                &self.port,
                &self.vid,
                &self.pid,
                &self.instance_id,
            ]
            .iter()
            .any(|field| field.to_lowercase().contains(&query))
            || format!("{}:{}", self.vid, self.pid).contains(&query)
            || self
                .serial_candidate
                .as_ref()
                .is_some_and(|serial| serial.to_lowercase().contains(&query))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_the_observed_windows_cdc_device() {
        let json = r#"{
            "event":"snapshot",
            "snapshot":{
                "captured_at":"2026-11-06T11:00:00+00:00", "computer":"CFAR083",
                "automatic_updates":true, "ignored_non_usb_ports":["COM1","COM3"],
                "devices":[{
                    "instance_id":"USB\\VID_1209&PID_0001&MI_00\\6&275C0824&0&0000",
                    "usb_instance_id":"USB\\VID_1209&PID_0001\\V6X-RUST-001",
                    "port":"COM26", "name":"V6X Rust CDC", "status":"OK",
                    "vid":"1209", "pid":"0001", "serial_candidate":"V6X-RUST-001",
                    "driver_service":"usbser", "driver_version":"10.0.19041.3636",
                    "problem_code":0, "composite_parent":true,
                    "location_paths":["PCIROOT(0)#PCI(1400)#USBROOT(0)#USB(1)"]
                }]
            }
        }"#;
        let HardwareEvent::Snapshot { snapshot } = serde_json::from_str(json).unwrap() else {
            panic!("expected a snapshot");
        };
        let device = &snapshot.devices[0];
        assert_eq!(device.port, "COM26");
        assert_eq!(device.serial_candidate.as_deref(), Some("V6X-RUST-001"));
        assert!(device.healthy());
        assert!(device.matches("1209:0001"));
        assert!(device.matches("v6x-rust"));
        assert!(device.matches("com26"));
        assert!(!device.matches("COM3"));
    }

    #[test]
    fn watcher_errors_are_explicit_not_silent_empty_lists() {
        let event: HardwareEvent = serde_json::from_str(
            r#"{"event":"error","message":"Access denied. Retry as the desktop user."}"#,
        )
        .unwrap();
        assert!(
            matches!(event, HardwareEvent::Error { message } if message.contains("Access denied"))
        );
    }
}
