//! Isolated notification fixtures. Never compiled into the diagnostic app.

use super::types::{UsbSerialDevice, UsbSnapshot};

pub(crate) fn device(serial: &str, port: &str) -> UsbSerialDevice {
    let instance = format!("USB\\VID_1209&PID_0001\\TEST-{serial}");
    UsbSerialDevice {
        instance_id: instance.clone(),
        usb_instance_id: instance,
        port: port.into(),
        name: format!("Test probe {serial}"),
        status: "OK".into(),
        vid: "1209".into(),
        pid: "0001".into(),
        serial_candidate: Some(format!("TEST-{serial}")),
        driver_service: Some("usbser".into()),
        driver_version: None,
        problem_code: Some(0),
        composite_parent: false,
        location_paths: Vec::new(),
    }
}

pub(crate) fn snapshot(devices: Vec<UsbSerialDevice>) -> UsbSnapshot {
    UsbSnapshot {
        captured_at: "2026-11-07T00:00:00Z".into(),
        computer: "ISOLATED-TEST".into(),
        automatic_updates: true,
        devices,
        ignored_non_usb_ports: Vec::new(),
    }
}
