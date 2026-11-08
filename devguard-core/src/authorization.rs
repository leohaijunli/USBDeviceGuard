//! Domain action IDs shared by the daemon, policy files and future GUI control.
//!
//! Domain service-start/autostart intent is not authorization to manage
//! systemd units. The systemd manager performs its own polkit checks.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolkitAction {
    Modify,
    Pause,
    ServiceStop,
    ServiceStart,
    ServiceAutostart,
}

impl PolkitAction {
    pub const ALL: [Self; 5] = [Self::Modify, Self::Pause, Self::ServiceStop, Self::ServiceStart, Self::ServiceAutostart];

    pub const fn id(self) -> &'static str {
        match self {
            Self::Modify => "dev.devguard.modify",
            Self::Pause => "dev.devguard.pause",
            Self::ServiceStop => "dev.devguard.service-stop",
            Self::ServiceStart => "dev.devguard.service-start",
            Self::ServiceAutostart => "dev.devguard.service-autostart",
        }
    }
}

pub const SYSTEMD_MANAGE_UNITS: &str = "org.freedesktop.systemd1.manage-units";
pub const SYSTEMD_MANAGE_UNIT_FILES: &str = "org.freedesktop.systemd1.manage-unit-files";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_actions_have_unique_stable_ids() {
        let ids: Vec<_> = PolkitAction::ALL.iter().map(|action| action.id()).collect();
        assert_eq!(ids, ["dev.devguard.modify", "dev.devguard.pause", "dev.devguard.service-stop",
                         "dev.devguard.service-start", "dev.devguard.service-autostart"]);
        assert_eq!(ids.iter().collect::<std::collections::BTreeSet<_>>().len(), ids.len());
    }

    #[test]
    fn systemd_actions_are_distinct_from_domain_intents() {
        for action in PolkitAction::ALL {
            assert_ne!(action.id(), SYSTEMD_MANAGE_UNITS);
            assert_ne!(action.id(), SYSTEMD_MANAGE_UNIT_FILES);
        }
    }
}
