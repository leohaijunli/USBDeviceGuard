//! Append-only JSONL journal (`/var/lib/devguard/journal.jsonl`).
//!
//! Every line is one record with the fields required by section 8:
//! `ts, event, name, devnode, vid, pid, serial, details, uid, user`.
//! Configuration changes also carry `before` and `after`.

use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::util::now_rfc3339;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JournalEntry {
    pub ts: String,
    pub event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub devnode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
    #[serde(default)]
    pub details: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<Value>,
}

impl JournalEntry {
    pub fn new(event: impl Into<String>) -> Self {
        Self {
            ts: now_rfc3339(),
            event: event.into(),
            name: None,
            devnode: None,
            vid: None,
            pid: None,
            serial: None,
            details: Value::Null,
            uid: None,
            user: None,
            before: None,
            after: None,
        }
    }

    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn device(mut self, device: &crate::identity::OnlineDevice) -> Self {
        self.devnode = Some(device.devnode.clone());
        self.vid = Some(device.vid.clone());
        self.pid = Some(device.pid.clone());
        self.serial = device.serial.clone();
        self
    }

    pub fn devnode(mut self, devnode: impl Into<String>) -> Self {
        self.devnode = Some(devnode.into());
        self
    }

    pub fn operator(mut self, uid: u32, user: impl Into<String>) -> Self {
        self.uid = Some(uid);
        self.user = Some(crate::util::clean_user(&user.into()));
        self
    }

    pub fn details(mut self, details: impl Serialize) -> Self {
        self.details = serde_json::to_value(details).unwrap_or(Value::Null);
        self
    }

    pub fn change(mut self, before: impl Serialize, after: impl Serialize) -> Self {
        self.before = serde_json::to_value(before).ok();
        self.after = serde_json::to_value(after).ok();
        self
    }

    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.details = serde_json::json!({ "note": note.into() });
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogFilter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Exact event name, or a prefix ending in `*` (e.g. `pause_*`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
    /// Free-text search over the serialized record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

impl LogFilter {
    pub fn matches(&self, entry: &JournalEntry, raw: &str) -> bool {
        if let Some(name) = &self.name {
            if entry.name.as_deref() != Some(name.as_str()) {
                return false;
            }
        }
        if let Some(event) = &self.event {
            let matches = if let Some(prefix) = event.strip_suffix('*') {
                entry.event.starts_with(prefix)
            } else {
                entry.event == *event
            };
            if !matches {
                return false;
            }
        }
        if let Some(since) = &self.since {
            if let (Ok(since), Ok(ts)) = (
                chrono::DateTime::parse_from_rfc3339(since),
                chrono::DateTime::parse_from_rfc3339(&entry.ts),
            ) {
                if ts < since {
                    return false;
                }
            }
        }
        if let Some(until) = &self.until {
            if let (Ok(until), Ok(ts)) = (
                chrono::DateTime::parse_from_rfc3339(until),
                chrono::DateTime::parse_from_rfc3339(&entry.ts),
            ) {
                if ts > until {
                    return false;
                }
            }
        }
        if let Some(text) = &self.text {
            if !raw.to_lowercase().contains(&text.to_lowercase()) {
                return false;
            }
        }
        true
    }

    pub fn limit(&self) -> usize {
        self.limit.unwrap_or(500).clamp(1, 5000)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct JournalQuery {
    pub entries: Vec<JournalEntry>,
    pub skipped: usize,
    pub total_seen: usize,
}

#[derive(Debug, Clone)]
pub struct Journal {
    pub path: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum JournalError {
    #[error("读取日志失败：{0}")]
    Io(#[from] std::io::Error),
    #[error("序列化日志失败：{0}")]
    Json(#[from] serde_json::Error),
}

impl Journal {
    pub fn new(paths: &crate::paths::Paths) -> Self {
        Self {
            path: paths.journal_file(),
        }
    }

    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Append one record and make it durable before returning.  The audit
    /// trail must not lose the last operation when the daemon is stopped.
    pub fn append(&self, entry: &JournalEntry) -> Result<(), JournalError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let line = serde_json::to_string(entry)?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = file.set_permissions(fs::Permissions::from_mode(0o640));
        }
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
        file.sync_data()?;
        Ok(())
    }

    pub fn query(&self, filter: &LogFilter) -> Result<JournalQuery, JournalError> {
        let file = match fs::File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(JournalQuery::default())
            }
            Err(error) => return Err(error.into()),
        };
        let reader = BufReader::new(file);
        let mut entries = Vec::new();
        let mut skipped = 0;
        let mut total_seen = 0;
        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            total_seen += 1;
            match serde_json::from_str::<JournalEntry>(&line) {
                Ok(entry) if filter.matches(&entry, &line) => entries.push(entry),
                Ok(_) => {}
                Err(_) => skipped += 1,
            }
        }
        let limit = filter.limit();
        if entries.len() > limit {
            entries.drain(..entries.len() - limit);
        }
        Ok(JournalQuery {
            entries,
            skipped,
            total_seen,
        })
    }

    pub fn export(&self, filter: &LogFilter, destination: &Path) -> Result<usize, JournalError> {
        let query = self.query(filter)?;
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(destination)?;
        for entry in &query.entries {
            serde_json::to_writer(&mut file, entry)?;
            file.write_all(b"\n")?;
        }
        file.sync_all()?;
        Ok(query.entries.len())
    }
}
