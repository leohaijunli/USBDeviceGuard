//! Snapshot differences, not PnP broadcasts, determine USB arrival notices.

use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

use nsh_core::time::Instant;

use super::types::UsbSerialDevice;

const COOLDOWN: Duration = Duration::from_secs(5 * 60);

#[derive(Default)]
pub(super) struct Arrivals {
    previous: Option<HashSet<String>>,
    announced: HashMap<String, Instant>,
}

impl Arrivals {
    pub(super) fn observe(
        &mut self,
        devices: &[UsbSerialDevice],
        at: Instant,
    ) -> Vec<UsbSerialDevice> {
        self.announced
            .retain(|_, time| at.saturating_duration_since(*time) < COOLDOWN);
        let mut incoming = HashSet::new();
        let mut added = Vec::new();
        for device in devices.iter().filter(|device| is_usb_serial(device)) {
            // Windows instance IDs are case-insensitive. Port renumbering,
            // metadata updates and repeated broadcasts are not new devices.
            let id = device.instance_id.to_ascii_uppercase();
            if incoming.insert(id.clone())
                && self
                    .previous
                    .as_ref()
                    .is_some_and(|previous| !previous.contains(&id))
                && !self.announced.contains_key(&id)
            {
                self.announced.insert(id, at);
                added.push(device.clone());
            }
        }
        // The first successful snapshot is a baseline, even when empty.
        self.previous = Some(incoming);
        added
    }
}

fn is_usb_serial(device: &UsbSerialDevice) -> bool {
    let hex_id = |id: &str| id.len() == 4 && id.bytes().all(|byte| byte.is_ascii_hexdigit());
    let port = device.port.to_ascii_uppercase();
    let instance = device.instance_id.to_ascii_uppercase();
    hex_id(&device.vid)
        && hex_id(&device.pid)
        && port.strip_prefix("COM").is_some_and(|number| {
            !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
        })
        && instance.starts_with(&format!(
            "USB\\VID_{}&PID_{}",
            device.vid.to_ascii_uppercase(),
            device.pid.to_ascii_uppercase()
        ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::test_support::device;

    #[test]
    fn first_snapshot_is_a_silent_baseline() {
        let mut arrivals = Arrivals::default();
        let at = Instant::now();
        assert!(arrivals.observe(&[device("alpha", "COM41")], at).is_empty());
        assert!(arrivals.observe(&[device("alpha", "COM41")], at).is_empty());
    }

    #[test]
    fn empty_baseline_still_announces_the_next_device_once() {
        let mut arrivals = Arrivals::default();
        let at = Instant::now();
        assert!(arrivals.observe(&[], at).is_empty());
        let probe = device("alpha", "COM41");
        assert_eq!(
            arrivals.observe(std::slice::from_ref(&probe), at),
            vec![probe.clone()]
        );
        assert!(arrivals.observe(&[probe], at).is_empty());
    }

    #[test]
    fn cooldown_expires_at_five_minutes_not_on_every_repeat() {
        let mut arrivals = Arrivals::default();
        let at = Instant::now();
        arrivals.observe(&[], at);
        assert_eq!(arrivals.observe(&[device("alpha", "COM41")], at).len(), 1);
        arrivals.observe(&[], at + Duration::from_secs(20));
        assert!(arrivals
            .observe(&[device("alpha", "COM41")], at + Duration::from_secs(299))
            .is_empty());
        arrivals.observe(&[], at + Duration::from_secs(299));
        assert_eq!(
            arrivals
                .observe(&[device("alpha", "COM41")], at + COOLDOWN)
                .len(),
            1
        );
    }

    #[test]
    fn continuously_present_devices_are_not_reannounced_after_cooldown() {
        let mut arrivals = Arrivals::default();
        let at = Instant::now();
        arrivals.observe(&[], at);
        arrivals.observe(&[device("alpha", "COM41")], at);
        assert!(arrivals
            .observe(&[device("alpha", "COM41")], at + COOLDOWN)
            .is_empty());
        assert!(
            arrivals.announced.is_empty(),
            "expired cooldown entries were retained"
        );
    }

    #[test]
    fn same_model_with_distinct_instances_is_announced_separately() {
        let mut arrivals = Arrivals::default();
        let at = Instant::now();
        arrivals.observe(&[device("alpha", "COM41")], at);
        let beta = device("beta", "COM42");
        assert_eq!(
            arrivals.observe(&[device("alpha", "COM41"), beta.clone()], at),
            vec![beta]
        );
    }

    #[test]
    fn duplicate_rows_case_changes_and_port_renumbering_are_not_arrivals() {
        let mut arrivals = Arrivals::default();
        let at = Instant::now();
        arrivals.observe(&[], at);
        let probe = device("alpha", "COM41");
        assert_eq!(arrivals.observe(&[probe.clone(), probe], at).len(), 1);
        let mut renamed = device("alpha", "COM99");
        renamed.instance_id = renamed.instance_id.to_ascii_lowercase();
        renamed.name = "Updated device description".into();
        assert!(arrivals.observe(&[renamed], at).is_empty());
    }

    #[test]
    fn bluetooth_and_non_serial_devices_never_trigger_notices() {
        let mut arrivals = Arrivals::default();
        let at = Instant::now();
        arrivals.observe(&[], at);
        let mut bluetooth = device("radio", "COM5");
        bluetooth.instance_id = "BTHENUM\\VIRTUAL_PORT".into();
        let mut keyboard = device("keyboard", "COM");
        keyboard.port = "HID".into();
        assert!(arrivals.observe(&[bluetooth, keyboard], at).is_empty());
    }
}
