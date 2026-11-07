#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::PathBuf;

use devguardd::linux::inventory::{parse_properties, Inventory};

struct UsbTree {
    _dir: tempfile::TempDir,
    sysfs: PathBuf,
}

impl UsbTree {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let sysfs = dir.path().join("sys");
        let bus = sysfs.join("bus/usb");
        let usb = sysfs.join("devices/pci0/usb1/1-2");
        let interface = usb.join("1-2:1.0");
        fs::create_dir_all(interface.join("tty/ttyACM0")).unwrap();
        fs::create_dir_all(bus.join("drivers/cdc_acm")).unwrap();
        fs::write(usb.join("idVendor"), "26ac\n").unwrap();
        fs::write(usb.join("idProduct"), "0032\n").unwrap();
        fs::write(usb.join("uevent"), "DEVTYPE=usb_device\n").unwrap();
        symlink(&bus, usb.join("subsystem")).unwrap();
        symlink(bus.join("drivers/cdc_acm"), interface.join("driver")).unwrap();
        Self { _dir: dir, sysfs }
    }

    fn inventory(&self) -> Inventory { Inventory::new(&self.sysfs, "/unused/udevadm") }

    fn properties(&self) -> BTreeMap<String, String> {
        parse_properties(concat!(
            "DEVNAME=/dev/ttyACM0\nSUBSYSTEM=tty\nID_BUS=usb\n",
            "ID_VENDOR_ID=26ac\nID_MODEL_ID=0032\nID_SERIAL_SHORT=SN-1\n",
            "DEVPATH=/devices/pci0/usb1/1-2/1-2:1.0/tty/ttyACM0\n",
        )).unwrap()
    }
}

#[test]
fn inventory_refuses_devnodes_outside_the_exact_usb_serial_namespace() {
    let tree = UsbTree::new();
    for name in ["/dev/disk/by-id/ttyACM0", "/dev/ttyACM", "/dev/ttyUSBfake", "/dev/ttyS0", "/elsewhere/ttyUSB0", "/dev/ttyACM0/../ttyUSB0"] {
        let mut properties = tree.properties();
        properties.insert("DEVNAME".into(), name.into());
        assert!(tree.inventory().from_properties(properties).unwrap().is_none(), "{name}");
    }
}

#[test]
fn inventory_resolves_the_physical_usb_parent_not_the_tty_devpath() {
    let tree = UsbTree::new();
    let facts = tree.inventory().from_properties(tree.properties()).unwrap().unwrap();
    assert_eq!(facts.usb_path.as_deref(), Some("/devices/pci0/usb1/1-2"));
    assert_eq!(facts.driver.as_deref(), Some("cdc_acm"));
    assert!(facts.is_cdc_acm);
    assert_eq!(facts.properties.get("DEVPATH").map(String::as_str), Some("/devices/pci0/usb1/1-2/1-2:1.0/tty/ttyACM0"));
}

#[test]
fn mismatched_usb_ids_and_escaping_devpaths_are_refused() {
    let tree = UsbTree::new();
    let mut wrong_id = tree.properties();
    wrong_id.insert("ID_MODEL_ID".into(), "ffff".into());
    assert!(tree.inventory().from_properties(wrong_id).is_err());
    for path in ["/devices/../../outside", "/dev/ttyACM0"] {
        let mut properties = tree.properties();
        properties.insert("DEVPATH".into(), path.into());
        assert!(tree.inventory().from_properties(properties).is_err());
    }
    let outside = tree._dir.path().join("outside");
    fs::create_dir(&outside).unwrap();
    symlink(outside, tree.sysfs.join("devices/escape")).unwrap();
    let mut properties = tree.properties();
    properties.insert("DEVPATH".into(), "/devices/escape".into());
    assert!(tree.inventory().from_properties(properties).is_err());
}

#[test]
fn duplicate_udev_properties_are_not_silently_overwritten() {
    assert!(parse_properties("DEVNAME=/dev/ttyUSB0\nDEVNAME=/dev/ttyACM0\n").is_err());
    assert_eq!(parse_properties("ID_MODEL=A=B\n").unwrap().get("ID_MODEL").map(String::as_str), Some("A=B"));
}
