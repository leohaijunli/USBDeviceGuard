//! Read-only udev enumeration with a verified physical USB parent.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use devguard_core::{util::normalize_hex4, UdevFacts};

pub struct Inventory {
    sysfs: PathBuf,
    udevadm: PathBuf,
}

impl Default for Inventory {
    fn default() -> Self {
        Self::new("/sys", "/usr/bin/udevadm")
    }
}

impl Inventory {
    /// Alternative roots are for fixture tests, not a different write target.
    pub fn new(sysfs: impl Into<PathBuf>, udevadm: impl Into<PathBuf>) -> Self {
        Self { sysfs: sysfs.into(), udevadm: udevadm.into() }
    }

    pub fn enumerate(&self) -> Result<Vec<UdevFacts>, String> {
        let entries = fs::read_dir(self.sysfs.join("class/tty"))
            .map_err(|error| format!("cannot enumerate sysfs tty devices: {error}"))?;
        let mut names = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| format!("cannot read a tty entry: {error}"))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if serial_kernel_name(&name) {
                names.push(name);
            }
        }
        names.sort();
        let mut devices = Vec::new();
        for name in names {
            if let Some(facts) = self.facts_for_devnode(&format!("/dev/{name}"))? {
                devices.push(facts);
            }
        }
        Ok(devices)
    }

    pub fn facts_for_devnode(&self, devnode: &str) -> Result<Option<UdevFacts>, String> {
        let Some(name) = devnode.strip_prefix("/dev/").filter(|name| serial_kernel_name(name)) else {
            return Ok(None);
        };
        let path = match fs::canonicalize(self.sysfs.join("class/tty").join(name)) {
            Ok(path) => path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("cannot resolve {devnode}: {error}")),
        };
        let output = Command::new(&self.udevadm)
            .args(["info", "--query=property", "--path"])
            .arg(&path)
            .output()
            .map_err(|error| format!("cannot run udevadm info: {error}"))?;
        if !output.status.success() {
            if !path.exists() {
                return Ok(None); // Unplugged during the snapshot.
            }
            return Err(format!("udevadm info failed for {devnode}: {}", String::from_utf8_lossy(&output.stderr).trim()));
        }
        let text = String::from_utf8(output.stdout).map_err(|_| "udevadm returned non-UTF-8 properties")?;
        let properties = parse_properties(&text)?;
        let facts = self.from_properties(properties)?;
        if let Some(facts) = &facts {
            if facts.devnode != devnode || self.device_path(&facts.properties)? != path {
                return Err(format!("udev identity changed while reading {devnode}; retry the snapshot"));
            }
        }
        Ok(facts)
    }

    pub fn from_properties(&self, properties: BTreeMap<String, String>) -> Result<Option<UdevFacts>, String> {
        let Some(mut facts) = UdevFacts::from_properties(properties) else {
            return Ok(None);
        };
        if !facts.is_managed_tty() {
            return Ok(None);
        }
        let device = self.device_path(&facts.properties)?;
        let devices_root = fs::canonicalize(self.sysfs.join("devices")).map_err(|error| error.to_string())?;
        let usb_bus = fs::canonicalize(self.sysfs.join("bus/usb")).map_err(|error| error.to_string())?;
        let mut driver = None;
        for parent in device.ancestors().take_while(|parent| *parent != devices_root) {
            if driver.is_none() {
                driver = fs::read_link(parent.join("driver")).ok()
                    .and_then(|path| path.file_name().map(|name| name.to_string_lossy().into_owned()));
            }
            let is_usb_device = fs::canonicalize(parent.join("subsystem")).ok().as_ref() == Some(&usb_bus)
                && fs::read_to_string(parent.join("uevent")).ok()
                    .is_some_and(|text| text.lines().any(|line| line == "DEVTYPE=usb_device"));
            if !is_usb_device {
                continue;
            }
            let vid = read_usb_id(parent, "idVendor")?;
            let pid = read_usb_id(parent, "idProduct")?;
            if vid != facts.vid || pid != facts.pid {
                return Err("udev VID/PID disagree with the physical USB parent; no device was selected".into());
            }
            facts.usb_path = Some(format!("/devices/{}", parent.strip_prefix(&devices_root)
                .map_err(|error| error.to_string())?.display()));
            facts.driver = driver.or(facts.driver);
            facts.is_cdc_acm = facts.driver.as_deref() == Some("cdc_acm");
            return Ok(Some(facts));
        }
        Err(format!("{} has no verified physical USB parent", facts.devnode))
    }

    fn device_path(&self, properties: &BTreeMap<String, String>) -> Result<PathBuf, String> {
        let raw = properties.get("DEVPATH").ok_or("udev did not provide DEVPATH")?;
        if !raw.starts_with("/devices/") || Path::new(raw).components().any(|part| matches!(part, Component::ParentDir | Component::CurDir)) {
            return Err("DEVPATH must stay inside /sys/devices".into());
        }
        let path = fs::canonicalize(self.sysfs.join(raw.trim_start_matches('/'))).map_err(|error| error.to_string())?;
        let root = fs::canonicalize(self.sysfs.join("devices")).map_err(|error| error.to_string())?;
        if !path.starts_with(&root) || path == root {
            return Err("DEVPATH escaped the sysfs device tree".into());
        }
        Ok(path)
    }
}

pub fn serial_kernel_name(name: &str) -> bool {
    name.strip_prefix("ttyACM").or_else(|| name.strip_prefix("ttyUSB"))
        .is_some_and(|suffix| !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit()))
}

pub fn parse_properties(text: &str) -> Result<BTreeMap<String, String>, String> {
    if text.len() > 256 * 1024 {
        return Err("udev properties exceed the size limit".into());
    }
    let mut properties = BTreeMap::new();
    for line in text.lines().filter(|line| !line.is_empty()) {
        let Some((key, value)) = line.split_once('=') else {
            return Err("udev properties must use KEY=VALUE lines".into());
        };
        if key.is_empty() || key.len() > 128 || !key.bytes().all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_') {
            return Err(format!("invalid udev property key: {key}"));
        }
        if properties.insert(key.to_owned(), value.to_owned()).is_some() {
            return Err(format!("duplicate udev property: {key}"));
        }
    }
    Ok(properties)
}

fn read_usb_id(parent: &Path, name: &str) -> Result<String, String> {
    normalize_hex4(&fs::read_to_string(parent.join(name)).map_err(|error| error.to_string())?)
}
