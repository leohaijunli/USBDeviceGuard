//! The device I/O seam.  The daemon's decision logic (`core`) only talks to
//! this trait, so the state machine runs — and is tested — without Linux.
//! The phase-2 implementation is `linux::LinuxIo`; it reads udev/sysfs and
//! stages rules. Hardware writes remain unavailable until their gates pass.

use devguard_core::{CheckReport, DeviceConfig, DoctorItem, UdevFacts};

pub struct ApplyOutcome {
    pub devnode: String,
    pub applied: Vec<devguard_core::protocol::AppliedSetting>,
    pub warnings: Vec<String>,
}

pub trait DeviceIo {
    /// All USB serial devices currently visible, with udev facts. Failure is
    /// not an empty bus: callers must surface enumeration errors.
    fn enumerate(&self) -> Result<Vec<UdevFacts>, String>;

    /// Facts for one devnode, if it is present.
    fn facts_for_devnode(&self, devnode: &str) -> Result<Option<UdevFacts>, String>;

    /// Apply one configuration to the device the facts describe: symlink
    /// presence, group/mode, ModemManager ignore, autosuspend, termios and
    /// the runtime env file.  Read-back verification belongs here.
    fn apply(&mut self, config: &DeviceConfig, facts: &UdevFacts) -> Result<ApplyOutcome, String>;

    /// Compare the live device against its configuration.
    fn check(&self, config: &DeviceConfig, facts: &UdevFacts) -> CheckReport;

    /// Set or clear `authorized` on the USB device behind `usb_path`.
    /// Implementations must refuse to deauthorize devices whose other
    /// interfaces include non-serial functions (composite safety).
    fn set_authorized(&mut self, usb_path: &str, authorize: bool) -> Result<(), String>;

    /// Write the udev rules file and ask udev to reload it.
    fn write_rules(&mut self, devices: &[DeviceConfig]) -> Result<(), String>;

    /// System-level doctor probes (groups, ModemManager, brltty, ...).
    fn doctor_probe(&self) -> Vec<DoctorItem>;
}
