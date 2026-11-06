//! Policy mode and the in-memory pause state.
//!
//! Pause is intentionally never persisted: a daemon restart or a machine
//! reboot always returns to `Running` (section 4.2).

use std::fs;
use std::path::PathBuf;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::identity::OnlineDevice;
use crate::paths::Paths;
use crate::store::atomic_write;

pub const MAX_PAUSE_MINUTES: u32 = 60;
pub const PAUSE_PRESETS: [u32; 4] = [5, 15, 30, 60];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PolicyMode {
    /// Record only.  The default.
    #[default]
    Audit,
    /// Block unregistered USB serial devices with `authorized=0`.
    Enforce,
}

impl PolicyMode {
    pub fn label(self) -> &'static str {
        match self {
            PolicyMode::Audit => "audit（只记录）",
            PolicyMode::Enforce => "enforce（拦截未登记）",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PauseKind {
    /// Unregistered devices are allowed, registered ones are still maintained.
    Enforcement,
    /// The daemon stays alive but writes no sysfs/termios and blocks nothing.
    Service,
}

impl PauseKind {
    pub fn label(self) -> &'static str {
        match self {
            PauseKind::Enforcement => "仅暂停拦截",
            PauseKind::Service => "整个服务休眠",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PauseState {
    pub kind: PauseKind,
    pub minutes: u32,
    pub started_at: DateTime<Utc>,
    pub until: DateTime<Utc>,
}

impl PauseState {
    pub fn new(kind: PauseKind, minutes: u32) -> Result<Self, String> {
        if minutes == 0 {
            return Err("暂停时长必须大于 0 分钟".into());
        }
        if minutes > MAX_PAUSE_MINUTES {
            return Err(format!("暂停最多 {MAX_PAUSE_MINUTES} 分钟；不允许永久暂停"));
        }
        let started_at = Utc::now();
        Ok(Self {
            kind,
            minutes,
            started_at,
            until: started_at + Duration::minutes(i64::from(minutes)),
        })
    }

    pub fn remaining_secs(&self) -> i64 {
        (self.until - Utc::now()).num_seconds().max(0)
    }

    pub fn expired(&self) -> bool {
        Utc::now() >= self.until
    }
}

/// What the GUI's top status strip shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ServiceState {
    Running {
        mode: PolicyMode,
    },
    Paused {
        mode: PolicyMode,
        kind: PauseKind,
        minutes: u32,
        started_at: DateTime<Utc>,
        until: DateTime<Utc>,
    },
    Stopped {
        autostart: bool,
    },
}

impl ServiceState {
    pub fn running(mode: PolicyMode) -> Self {
        ServiceState::Running { mode }
    }

    pub fn mode(&self) -> Option<PolicyMode> {
        match self {
            ServiceState::Running { mode } | ServiceState::Paused { mode, .. } => Some(*mode),
            ServiceState::Stopped { .. } => None,
        }
    }

    pub fn is_paused(&self) -> bool {
        matches!(self, ServiceState::Paused { .. })
    }

    pub fn is_running(&self) -> bool {
        matches!(self, ServiceState::Running { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartupReconcile {
    /// Devices that appeared while the daemon was stopped.
    pub added: Vec<OnlineDevice>,
    /// Registered names whose devices disappeared while stopped.
    pub removed: Vec<String>,
    /// Pause is memory-only, so a start always ends it.
    pub pause_cleared: bool,
    pub note: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyFile {
    #[serde(default)]
    pub mode: PolicyMode,
}

#[derive(Debug, Clone)]
pub struct PolicyStore {
    pub path: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum PolicyError {
    #[error("读取策略失败：{0}")]
    Io(#[from] std::io::Error),
    #[error("解析策略失败：{0}")]
    Parse(#[from] toml::de::Error),
    #[error("写入策略失败：{0}")]
    Serialize(#[from] toml::ser::Error),
}

impl PolicyStore {
    pub fn new(paths: &Paths) -> Self {
        Self {
            path: paths.policy_file(),
        }
    }

    pub fn load(&self) -> Result<PolicyFile, PolicyError> {
        match fs::read_to_string(&self.path) {
            Ok(text) => Ok(toml::from_str(&text)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(PolicyFile::default()),
            Err(error) => Err(error.into()),
        }
    }

    pub fn set(&self, mode: PolicyMode) -> Result<PolicyFile, PolicyError> {
        let file = PolicyFile { mode };
        let text = toml::to_string_pretty(&file)?;
        let parent = self
            .path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));
        fs::create_dir_all(parent)?;
        atomic_write(&self.path, text.as_bytes(), 0o644)?;
        Ok(file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pause_is_bounded() {
        assert!(PauseState::new(PauseKind::Enforcement, 0).is_err());
        assert!(PauseState::new(PauseKind::Enforcement, 61).is_err());
        for minutes in PAUSE_PRESETS {
            let pause = PauseState::new(PauseKind::Service, minutes).unwrap();
            assert!(!pause.expired());
            assert!(pause.remaining_secs() > 0);
        }
    }

    #[test]
    fn service_state_serializes_tagged() {
        let json = serde_json::to_string(&ServiceState::Running {
            mode: PolicyMode::Enforce,
        })
        .unwrap();
        assert!(json.contains("\"state\":\"running\""));
        let paused = ServiceState::Paused {
            mode: PolicyMode::Audit,
            kind: PauseKind::Enforcement,
            minutes: 5,
            started_at: Utc::now(),
            until: Utc::now() + Duration::minutes(5),
        };
        let json = serde_json::to_string(&paused).unwrap();
        assert!(json.contains("\"state\":\"paused\""));
        assert!(json.contains("\"kind\":\"enforcement\""));
    }

    #[test]
    fn policy_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::rooted(dir.path());
        paths.create_dirs().unwrap();
        let store = PolicyStore::new(&paths);
        assert_eq!(store.load().unwrap().mode, PolicyMode::Audit);
        store.set(PolicyMode::Enforce).unwrap();
        assert_eq!(store.load().unwrap().mode, PolicyMode::Enforce);
    }
}
