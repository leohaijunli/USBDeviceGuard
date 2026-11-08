use std::fs;
use std::os::unix::fs::MetadataExt;
use std::process::Command;

use devguard_core::protocol::AppliedSetting;
use devguard_core::runtime_env::RuntimeEnv;
use devguard_core::{CheckItem, CheckReport, DeviceConfig, DoctorItem, DoctorStatus, Paths, UdevFacts};

use crate::io::{ApplyOutcome, DeviceIo};
use crate::rules::{self, RULES_HEADER};
use crate::linux::termios;

use super::{accounts, inventory::Inventory};

pub struct LinuxIo {
    paths: Paths,
    sandbox: bool,
    inventory: Inventory,
}

impl LinuxIo {
    pub fn new(paths: Paths, sandbox: bool) -> Self {
        Self { paths, sandbox, inventory: Inventory::default() }
    }
}

impl DeviceIo for LinuxIo {
    fn enumerate(&self) -> Result<Vec<UdevFacts>, String> {
        self.inventory.enumerate()
    }

    fn facts_for_devnode(&self, devnode: &str) -> Result<Option<UdevFacts>, String> {
        self.inventory.facts_for_devnode(devnode)
    }

    fn apply(&mut self, config: &DeviceConfig, facts: &UdevFacts) -> Result<ApplyOutcome, String> {
        // Phase-3 scope: termios + runtime env file.  Symlink, group/mode and
        // the ModemManager flag stay udev-rule driven (staged in phase 2),
        // and autosuspend awaits its own sysfs acceptance gate.
        // `termios::apply` fails when the read-back differs from the config.
        let actual = termios::apply(&facts.devnode, &config.serial)?;
        let mut applied = vec![AppliedSetting {
            key: "termios.device".into(),
            value: facts.devnode.clone(),
        }];
        if let Some(rate) = actual.baud {
            applied.push(AppliedSetting { key: "termios.baud".into(), value: rate.to_string() });
        }
        applied.push(AppliedSetting { key: "termios.data_bits".into(), value: actual.data_bits.to_string() });
        applied.push(AppliedSetting { key: "termios.parity".into(), value: actual.parity.serial_key().to_owned() });
        applied.push(AppliedSetting { key: "termios.stop_bits".into(), value: actual.stop_bits.to_string() });
        applied.push(AppliedSetting { key: "termios.flow".into(), value: actual.flow.to_string() });
        applied.push(AppliedSetting { key: "termios.raw".into(), value: u8::from(actual.raw).to_string() });
        let mut warnings = vec![
            "autosuspend (power/control) is not applied: its Linux sysfs acceptance is a later phase gate".into(),
        ];
        if self.sandbox {
            warnings.push("sandbox: the stable symlink needs rule activation on the target system".into());
        }
        let env = RuntimeEnv::new(&facts.devnode, config.serial.clone());
        devguard_core::store::atomic_write(&self.paths.runtime_env(&config.name), env.render().as_bytes(), 0o644)
            .map_err(|error| format!("cannot write the runtime env file: {error}"))?;
        Ok(ApplyOutcome { devnode: facts.devnode.clone(), applied, warnings })
    }

    fn check(&self, config: &DeviceConfig, facts: &UdevFacts) -> CheckReport {
        let mut items = Vec::new();
        let link = self.paths.dev.join(&config.name);
        if fs::canonicalize(&link).ok().as_ref() == Some(&std::path::PathBuf::from(&facts.devnode)) {
            items.push(CheckItem::ok(format!("{}.symlink", config.name), "stable symlink"));
        } else {
            items.push(CheckItem::warning(format!("{}.symlink", config.name), "stable symlink missing or different", link.display().to_string(), "activate the reviewed udev rules and reconnect the device"));
        }
        match fs::metadata(&facts.devnode) {
            Ok(metadata) => {
                let expected_mode = devguard_core::util::parse_mode(&config.access.mode).unwrap_or(0o660);
                let group = if config.access.group == "auto" { accounts::serial_group() } else { Ok(config.access.group.clone()) };
                let expected_gid = group.and_then(|group| accounts::group_id(&group).map_err(|error| error.to_string())?.ok_or_else(|| format!("group {group} does not exist")));
                let correct = metadata.mode() & 0o777 == expected_mode && expected_gid.as_ref().ok() == Some(&metadata.gid());
                items.push(if correct {
                    CheckItem::ok(format!("{}.access", config.name), "device permissions and group")
                } else {
                    CheckItem::warning(format!("{}.access", config.name), "device permissions or group differ", format!("mode {:04o}, gid {}", metadata.mode() & 0o777, metadata.gid()), "review the configured access group and udev rules")
                });
            }
            Err(error) => items.push(CheckItem::error(format!("{}.access", config.name), "cannot inspect device permissions", error.to_string(), "reconnect the device and retry")),
        }
        items.push(if facts.properties.get("ID_MM_DEVICE_IGNORE").map(String::as_str) == Some("1") {
            CheckItem::ok(format!("{}.modem_manager", config.name), "ModemManager ignore flag")
        } else {
            CheckItem::warning(format!("{}.modem_manager", config.name), "ModemManager ignore flag missing", "ID_MM_DEVICE_IGNORE is not 1", "activate the reviewed rules; no service is disabled automatically")
        });
        items.push(match termios::read(&facts.devnode) {
            Ok(actual) => {
                let drift = termios::drift(&config.serial, &actual);
                if drift.is_empty() {
                    CheckItem::ok(format!("{}.termios", config.name), "line discipline matches the configuration")
                } else {
                    let actual = drift
                        .iter()
                        .map(|(key, _, got)| format!("{key}={got}"))
                        .collect::<Vec<_>>()
                        .join(" ");
                    CheckItem::warning(
                        format!("{}.termios", config.name),
                        "line discipline differs from the configuration",
                        actual,
                        "run apply to set the configured line discipline",
                    )
                }
            }
            Err(error) => CheckItem::warning(
                format!("{}.termios", config.name),
                "termios cannot be inspected",
                error,
                "check that the daemon user may open the device; reading termios requires opening the tty",
            ),
        });
        items.push(CheckItem::warning(format!("{}.autosuspend", config.name), "autosuspend not yet verified", "power/control is not managed yet", "sysfs power writes await their phase gate; no service setting is changed"));
        CheckReport::new(Some(config.name.clone()), items.into_iter().map(|item| item.with_device(&config.name)).collect())
    }

    fn set_authorized(&mut self, _usb_path: &str, _authorize: bool) -> Result<(), String> {
        Err("USB authorization writes are disabled until composite-safety and phase-5 acceptance pass".into())
    }

    fn write_rules(&mut self, devices: &[DeviceConfig]) -> Result<(), String> {
        let group = accounts::serial_group()?;
        for device in devices {
            let group = if device.access.group == "auto" { &group } else { &device.access.group };
            if accounts::group_id(group).map_err(|error| error.to_string())?.is_none() {
                return Err(format!("serial access group {group} does not exist"));
            }
        }
        let generated = rules::generate(devices, &group);
        if !generated.skipped.is_empty() {
            return Err(format!("rules were not changed: {}", generated.skipped.join("; ")));
        }
        let path = self.paths.rules_file();
        match fs::symlink_metadata(&path) {
            Ok(metadata) if !metadata.is_file() => return Err("refusing to replace a non-regular rules file".into()),
            Ok(_) => {
                if !fs::read_to_string(&path).map_err(|error| error.to_string())?.starts_with(RULES_HEADER) {
                    return Err("the rules file is not generated by devguardd; preserve it and review manually".into());
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
        devguard_core::store::atomic_write(&path, generated.text.as_bytes(), 0o644).map_err(|error| error.to_string())?;
        if !self.sandbox {
            let status = Command::new("/usr/bin/udevadm").args(["control", "--reload-rules"]).status().map_err(|error| error.to_string())?;
            if !status.success() {
                return Err("the rules file was saved, but udev reload failed; retry after starting systemd-udevd".into());
            }
        }
        Ok(())
    }

    fn doctor_probe(&self) -> Vec<DoctorItem> {
        let mut items = vec![DoctorItem::new("phase3", "termios apply is enabled; enforcement writes are not", DoctorStatus::Warning, "apply sets the line discipline and writes the runtime env file; pause, policy changes and USB authorization await their phase gates")];
        if self.sandbox {
            items.push(DoctorItem::new("sandbox", "Private development sandbox", DoctorStatus::Warning, "configuration and rules stay under --root; host udev rules are never reloaded"));
        }
        for group in ["devguard", "dialout", "uucp"] {
            let present = accounts::group_id(group).ok().flatten().is_some();
            items.push(DoctorItem::new(format!("group.{group}"), format!("Group {group}"), if present { DoctorStatus::Ok } else { DoctorStatus::Warning }, if present { "found in the local account database" } else { "not present; group configuration is an administrator action" }));
        }
        for service in ["ModemManager.service", "brltty.service"] {
            match Command::new("/usr/bin/systemctl").args(["is-active", service]).output() {
                Ok(output) => {
                    let state = String::from_utf8_lossy(&output.stdout).trim().to_owned();
                    items.push(DoctorItem::new(format!("service.{service}"), service, if output.status.success() { DoctorStatus::Warning } else { DoctorStatus::Ok }, format!("state: {state}; no service settings are changed")));
                }
                Err(error) => items.push(DoctorItem::new(format!("service.{service}"), service, DoctorStatus::Warning, error.to_string())),
            }
        }
        items
    }
}
