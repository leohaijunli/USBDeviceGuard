//! Pure presentation decisions. Received daemon records remain intact.

use devguard_core::{
    protocol::ListResponse, DeviceConfig, DeviceStatus, OnlineDevice, ServiceState,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Fault {
    pub code: String,
    pub message: String,
    pub next: String,
    pub retryable: bool,
}

impl Fault {
    pub(crate) fn new(code: &str, message: impl Into<String>, next: &str, retryable: bool) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            next: next.into(),
            retryable,
        }
    }

    pub(crate) fn display(&self) -> String {
        format!("{}\n{}", self.message, self.next)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Connection {
    Connecting,
    Live,
    Reconnecting { seconds: u64, fault: Fault },
    Unavailable(Fault),
    Stopped,
}

impl Connection {
    pub(crate) fn label(&self, service: Option<&ServiceState>) -> String {
        match self {
            Self::Connecting => "Connecting to daemon".into(),
            Self::Live => match service {
                Some(ServiceState::Running {
                    mode: devguard_core::PolicyMode::Audit,
                }) => "Running · audit · no blocking".into(),
                Some(ServiceState::Running { .. }) => "Running · enforce".into(),
                Some(ServiceState::Paused { .. }) => "Paused · reported by daemon".into(),
                Some(ServiceState::Stopped { .. }) => "Stopped · unprotected".into(),
                None => "Connected · awaiting snapshot".into(),
            },
            Self::Reconnecting { seconds, .. } => {
                format!("Connection interrupted · retry in {seconds}s")
            }
            Self::Unavailable(_) => "Connection unavailable · status unknown".into(),
            Self::Stopped => "Stopped · unprotected".into(),
        }
    }

    pub(crate) fn live(&self) -> bool {
        matches!(self, Self::Live)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeviceFilter {
    All,
    Attention,
    Registered,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeviceRow {
    pub key: String,
    pub title: String,
    pub config: Option<DeviceConfig>,
    pub online: Vec<OnlineDevice>,
}

impl DeviceRow {
    pub(crate) fn status(&self) -> &'static str {
        if self.online.is_empty() {
            return "Offline";
        }
        if self
            .online
            .iter()
            .any(|d| d.status == DeviceStatus::Conflict)
        {
            return "Conflict";
        }
        if self
            .online
            .iter()
            .any(|d| d.status == DeviceStatus::Blocked)
        {
            return "Blocked";
        }
        if self.online.iter().any(|d| d.status == DeviceStatus::Drift) {
            return "Drift";
        }
        if self.config.is_some() {
            "Registered"
        } else {
            "Unregistered"
        }
    }

    pub(crate) fn matches(&self, query: &str, filter: DeviceFilter) -> bool {
        if filter == DeviceFilter::Registered && self.config.is_none() {
            return false;
        }
        if filter == DeviceFilter::Attention && self.status() == "Registered" {
            return false;
        }
        let query = query.trim().to_lowercase();
        query.is_empty()
            || serde_json::to_string(&(&self.title, &self.config, &self.online))
                .is_ok_and(|text| text.to_lowercase().contains(&query))
    }

    pub(crate) fn ports(&self) -> String {
        if self.online.is_empty() {
            "Not connected".into()
        } else {
            self.online
                .iter()
                .map(|d| d.devnode.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        }
    }

    pub(crate) fn summary(&self) -> String {
        let identity = self
            .online
            .first()
            .map(|d| format!("{}:{}", d.vid, d.pid))
            .or_else(|| {
                self.config
                    .as_ref()
                    .and_then(|c| c.ids.first())
                    .map(|id| format!("{}:{}", id.vid, id.pid))
            })
            .unwrap_or_default();
        format!("{identity} · {}", self.ports())
    }
}

/// A serial identity follows the device across ports and devnode renumbering.
/// Without a serial, the physical ID_PATH is authoritative, never guessed.
pub(crate) fn online_key(device: &OnlineDevice) -> String {
    format!(
        "online:{}",
        serde_json::to_string(&(
            &device.vid,
            &device.pid,
            &device.serial,
            if device.serial.is_some() {
                None
            } else {
                device.id_path.as_ref()
            },
            if device.serial.is_none() && device.id_path.is_none() {
                Some(&device.devnode)
            } else {
                None
            },
        ))
        .expect("serializing strings cannot fail")
    )
}

pub(crate) fn rows(snapshot: &ListResponse) -> Vec<DeviceRow> {
    let mut rows: Vec<_> = snapshot
        .devices
        .iter()
        .map(|config| DeviceRow {
            key: format!("registered:{}", config.name),
            title: config.name.clone(),
            config: Some(config.clone()),
            online: snapshot
                .online
                .iter()
                .filter(|d| d.managed_by.as_deref() == Some(&config.name))
                .cloned()
                .collect(),
        })
        .collect();
    for device in &snapshot.online {
        if device
            .managed_by
            .as_ref()
            .is_some_and(|name| snapshot.devices.iter().any(|c| &c.name == name))
        {
            continue;
        }
        let key = online_key(device);
        if let Some(row) = rows.iter_mut().find(|row| row.key == key) {
            row.online.push(device.clone());
        } else {
            rows.push(DeviceRow {
                key,
                title: device.model.clone().unwrap_or_else(|| device.name.clone()),
                config: None,
                online: vec![device.clone()],
            });
        }
    }
    rows.sort_by(|a, b| a.title.cmp(&b.title).then_with(|| a.key.cmp(&b.key)));
    rows
}

/// Old buffered events are skipped only through the snapshot barrier. Any gap
/// after it invalidates the live view and requires a fresh subscription/snapshot.
#[cfg(any(target_os = "linux", test))]
#[derive(Debug, Default)]
pub(crate) struct Cursor {
    barrier: u64,
    last: u64,
}

#[cfg(any(target_os = "linux", test))]
impl Cursor {
    pub(crate) fn snapshot(sequence: Option<u64>) -> Result<Self, Fault> {
        let sequence = sequence.ok_or_else(|| {
            Fault::new(
                "snapshot_cursor_missing",
                "This daemon cannot synchronize snapshots with events.",
                "Update devguardd to this GUI's version and reconnect.",
                false,
            )
        })?;
        Ok(Self {
            barrier: sequence,
            last: sequence,
        })
    }

    pub(crate) fn accept(&mut self, sequence: u64) -> Result<bool, Fault> {
        if sequence <= self.barrier {
            return Ok(false);
        }
        if self.last.checked_add(1) != Some(sequence) {
            return Err(Fault::new(
                "event_sequence_invalid",
                "Device events were missed or duplicated.",
                "Reconnecting for a fresh snapshot; the previous data is stale.",
                true,
            ));
        }
        self.last = sequence;
        Ok(true)
    }
}

#[cfg(any(target_os = "linux", test))]
pub(crate) fn retry_seconds(attempt: u32) -> u64 {
    1_u64.checked_shl(attempt.min(5)).unwrap_or(30).min(30)
}
