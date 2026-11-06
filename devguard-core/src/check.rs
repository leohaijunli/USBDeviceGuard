//! The result of `check`: whether a registered device currently matches its
//! configuration.  `enforce` is only allowed when a check has no warnings and
//! no errors (section 3.8).

use serde::{Deserialize, Serialize};

use crate::util::now_rfc3339;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Ok,
    Warning,
    Error,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::Ok => "正常",
            Severity::Warning => "告警",
            Severity::Error => "错误",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckItem {
    /// Stable id, e.g. `px4-main.termios.baud`.
    pub id: String,
    /// Device this item belongs to, when it is device-specific.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    pub label: String,
    pub severity: Severity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_step: Option<String>,
}

impl CheckItem {
    pub fn ok(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            device: None,
            label: label.into(),
            severity: Severity::Ok,
            expected: None,
            actual: None,
            next_step: None,
        }
    }

    pub fn warning(
        id: impl Into<String>,
        label: impl Into<String>,
        actual: impl Into<String>,
        next_step: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            device: None,
            label: label.into(),
            severity: Severity::Warning,
            expected: None,
            actual: Some(actual.into()),
            next_step: Some(next_step.into()),
        }
    }

    pub fn error(
        id: impl Into<String>,
        label: impl Into<String>,
        actual: impl Into<String>,
        next_step: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            device: None,
            label: label.into(),
            severity: Severity::Error,
            expected: None,
            actual: Some(actual.into()),
            next_step: Some(next_step.into()),
        }
    }

    pub fn with_device(mut self, device: impl Into<String>) -> Self {
        self.device = Some(device.into());
        self
    }

    pub fn with_expected(mut self, expected: impl Into<String>) -> Self {
        self.expected = Some(expected.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckReport {
    pub checked_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    pub items: Vec<CheckItem>,
}

impl CheckReport {
    pub fn new(device: Option<String>, items: Vec<CheckItem>) -> Self {
        Self {
            checked_at: now_rfc3339(),
            device,
            items,
        }
    }

    pub fn warnings(&self) -> usize {
        self.items
            .iter()
            .filter(|item| item.severity == Severity::Warning)
            .count()
    }

    pub fn errors(&self) -> usize {
        self.items
            .iter()
            .filter(|item| item.severity == Severity::Error)
            .count()
    }

    /// Section 3.8: switching to enforce requires a clean check.
    pub fn can_enforce(&self) -> bool {
        self.warnings() == 0 && self.errors() == 0
    }

    pub fn summary(&self) -> String {
        format!(
            "{} 项正常，{} 项告警，{} 项错误",
            self.items.len() - self.warnings() - self.errors(),
            self.warnings(),
            self.errors()
        )
    }
}
