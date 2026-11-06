//! `doctor` output.  The daemon performs the probes; the type is shared so the
//! CLI and GUI render the same report.

use serde::{Deserialize, Serialize};

use crate::util::now_rfc3339;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DoctorStatus {
    Ok,
    Warning,
    Error,
}

impl DoctorStatus {
    pub fn label(self) -> &'static str {
        match self {
            DoctorStatus::Ok => "正常",
            DoctorStatus::Warning => "提示",
            DoctorStatus::Error => "需要处理",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorItem {
    pub id: String,
    pub title: String,
    pub status: DoctorStatus,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_step: Option<String>,
}

impl DoctorItem {
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        status: DoctorStatus,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            status,
            detail: detail.into(),
            next_step: None,
        }
    }

    pub fn next(mut self, next_step: impl Into<String>) -> Self {
        self.next_step = Some(next_step.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorReport {
    pub checked_at: String,
    pub items: Vec<DoctorItem>,
}

impl DoctorReport {
    pub fn new(items: Vec<DoctorItem>) -> Self {
        Self {
            checked_at: now_rfc3339(),
            items,
        }
    }

    pub fn errors(&self) -> usize {
        self.items
            .iter()
            .filter(|item| item.status == DoctorStatus::Error)
            .count()
    }

    pub fn warnings(&self) -> usize {
        self.items
            .iter()
            .filter(|item| item.status == DoctorStatus::Warning)
            .count()
    }
}
