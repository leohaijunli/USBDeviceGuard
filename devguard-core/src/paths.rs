//! Filesystem layout.  `Paths::system` is what the daemon uses; tests and a
//! developer build can point [`Paths::rooted`] at a temporary tree.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// `/etc/devguard`
    pub etc: PathBuf,
    /// `/run/devguard`
    pub run: PathBuf,
    /// `/var/lib/devguard`
    pub state: PathBuf,
    /// `/dev/devguard`
    pub dev: PathBuf,
    /// `/etc/udev/rules.d`
    pub udev_rules: PathBuf,
}

impl Paths {
    pub fn system() -> Self {
        Self {
            etc: PathBuf::from("/etc/devguard"),
            run: PathBuf::from("/run/devguard"),
            state: PathBuf::from("/var/lib/devguard"),
            dev: PathBuf::from("/dev/devguard"),
            udev_rules: PathBuf::from("/etc/udev/rules.d"),
        }
    }

    /// A complete tree under `root`, for tests and `devguardd --root`.
    pub fn rooted(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref();
        Self {
            etc: root.join("etc/devguard"),
            run: root.join("run/devguard"),
            state: root.join("var/lib/devguard"),
            dev: root.join("dev/devguard"),
            udev_rules: root.join("etc/udev/rules.d"),
        }
    }

    pub fn devices_dir(&self) -> PathBuf {
        self.etc.join("devices.d")
    }

    pub fn device_file(&self, name: &str) -> PathBuf {
        self.devices_dir().join(format!("{name}.toml"))
    }

    pub fn backups_dir(&self) -> PathBuf {
        self.state.join("backups")
    }

    pub fn journal_file(&self) -> PathBuf {
        self.state.join("journal.jsonl")
    }

    pub fn policy_file(&self) -> PathBuf {
        self.etc.join("policy.toml")
    }

    pub fn socket(&self) -> PathBuf {
        self.run.join("ctl.sock")
    }

    pub fn runtime_env(&self, name: &str) -> PathBuf {
        self.run.join(format!("{name}.env"))
    }

    pub fn last_online(&self) -> PathBuf {
        self.state.join("last-online.json")
    }

    pub fn state_file(&self) -> PathBuf {
        self.state.join("state.json")
    }

    pub fn rules_file(&self) -> PathBuf {
        self.udev_rules.join("90-devguard.rules")
    }

    pub fn create_dirs(&self) -> std::io::Result<()> {
        for dir in [
            self.etc.clone(),
            self.devices_dir(),
            self.run.clone(),
            self.state.clone(),
            self.backups_dir(),
        ] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }
}
