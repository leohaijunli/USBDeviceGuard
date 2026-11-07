//! Phase 2 decision-core tests.  These run on any host: the daemon state
//! machine is exercised through [`DeviceIo`] fakes, so Linux is only needed
//! for the real udev/socket/termios shell.

use devguard_core::{
    config::{DeviceId, DeviceKind, IdRole},
    identity::UdevFacts,
    journal::Journal,
    policy::{PauseKind, PolicyMode, ServiceState},
    protocol::{Event, ProtocolError, Request},
    Paths,
};

use devguardd::core::{Caller, Daemon, Effect, Handled};
use devguardd::io::{ApplyOutcome, DeviceIo};
use devguardd::rules;

fn facts(devnode: &str, vid: &str, pid: &str, serial: Option<&str>, path: Option<&str>) -> UdevFacts {
    UdevFacts {
        devnode: devnode.to_owned(),
        subsystem: "tty".into(),
        vid: vid.into(),
        pid: pid.into(),
        serial_short: serial.map(str::to_owned),
        id_path: path.map(str::to_owned),
        usb_path: Some(format!("/devices/usb1/{}", serial.or(path).unwrap_or(devnode))),
        driver: Some("cdc_acm".into()),
        is_cdc_acm: true,
        is_usb: true,
        ..UdevFacts::default()
    }
}

fn px4_config() -> devguard_core::DeviceConfig {
    devguard_core::DeviceConfig {
        name: "px4-main".into(),
        kind: DeviceKind::Px4,
        description: None,
        ids: vec![DeviceId {
            vid: "26ac".into(),
            pid: "0032".into(),
            serial: Some("SN-PX4".into()),
            path: None,
            role: IdRole::App,
        }],
        serial: devguard_core::config::SerialConfig {
            baud: devguard_core::BaudRate::NotApplicable,
            data_bits: 8,
            parity: devguard_core::Parity::N,
            stop_bits: 1,
            flow: devguard_core::FlowControl::None,
            raw: true,
        },
        access: Default::default(),
    }
}

/// A fake device bus: two online devices, apply always succeeds.
struct FakeIo {
    devices: Vec<UdevFacts>,
    apply_calls: Vec<String>,
    /// Refuse deauthorization for this USB path (composite safety).
    refuse_block: Option<String>,
    check_drift: bool,
}

impl FakeIo {
    fn with_px4_and_adapter() -> Self {
        Self {
            devices: vec![
                facts("/dev/ttyACM0", "26ac", "0032", Some("SN-PX4"), None),
                facts("/dev/ttyUSB0", "1a86", "7523", None, Some("pci-1")),
                // A motherboard UART: tty but never a managed device.
                {
                    let mut builtin = facts("/dev/ttyS0", "", "", None, None);
                    builtin.is_usb = false;
                    builtin.usb_path = None;
                    builtin.driver = Some("serial8250".into());
                    builtin
                },
            ],
            apply_calls: Vec::new(),
            refuse_block: None,
            check_drift: false,
        }
    }
}

impl DeviceIo for FakeIo {
    fn enumerate(&self) -> Result<Vec<UdevFacts>, String> {
        Ok(self.devices.clone())
    }

    fn facts_for_devnode(&self, devnode: &str) -> Result<Option<UdevFacts>, String> {
        Ok(self.devices.iter().find(|d| d.devnode == devnode).cloned())
    }

    fn apply(
        &mut self,
        config: &devguard_core::DeviceConfig,
        _facts: &UdevFacts,
    ) -> Result<ApplyOutcome, String> {
        self.apply_calls.push(config.name.clone());
        Ok(ApplyOutcome {
            devnode: "/dev/ttyACM0".into(),
            applied: vec![devguard_core::protocol::AppliedSetting {
                key: "termios.baud".into(),
                value: "n/a".into(),
            }],
            warnings: Vec::new(),
        })
    }

    fn check(
        &self,
        config: &devguard_core::DeviceConfig,
        _facts: &UdevFacts,
    ) -> devguard_core::CheckReport {
        let items = if self.check_drift {
            vec![devguard_core::CheckItem::warning(
                "fixture.mode", "mode differs", "0600", "apply the configuration",
            )]
        } else {
            Vec::new()
        };
        devguard_core::CheckReport::new(Some(config.name.clone()), items)
    }

    fn set_authorized(&mut self, usb_path: &str, _authorize: bool) -> Result<(), String> {
        if self.refuse_block.as_deref() == Some(usb_path) {
            Err("the USB parent serves non-serial functions".into())
        } else {
            Ok(())
        }
    }

    fn write_rules(&mut self, _devices: &[devguard_core::DeviceConfig]) -> Result<(), String> {
        Ok(())
    }

    fn doctor_probe(&self) -> Vec<devguard_core::DoctorItem> {
        Vec::new()
    }
}

fn daemon() -> Daemon {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::rooted(dir.path());
    paths.create_dirs().unwrap();
    Daemon::new(paths, Some(10_000))
}

fn group_caller() -> Caller {
    Caller {
        uid: 1000,
        user: "alice".into(),
        groups: vec![10_000],
        polkit_authorized: false,
    }
}

fn authorized_caller() -> Caller {
    Caller {
        uid: 1000,
        user: "alice".into(),
        groups: vec![10_000],
        polkit_authorized: true,
    }
}

fn error_of(handled: &Handled) -> &ProtocolError {
    handled
        .response
        .error
        .as_ref()
        .expect("expected an error response")
}

fn data_of<T: for<'de> serde::de::Deserialize<'de>>(handled: &Handled) -> T {
    let value = handled.response.data.as_ref().expect("expected data");
    serde_json::from_value(value.clone()).expect("data must match the response type")
}

#[test]
fn read_requests_need_the_devguard_group_or_polkit() {
    let mut daemon = daemon();
    let mut io = FakeIo::with_px4_and_adapter();
    let outsider = Caller {
        uid: 2000,
        user: "bob".into(),
        groups: vec![2000],
        polkit_authorized: false,
    };

    let denied = daemon.handle(&mut io, &outsider, Some(1), Request::List);
    assert_eq!(error_of(&denied).code, "unauthorized");
    assert!(denied.journal.iter().any(|entry| entry.event == "request_denied"));

    // Read-only for a group member is fine.
    let allowed = daemon.handle(&mut io, &group_caller(), Some(2), Request::List);
    assert!(allowed.response.ok);

    // A modification without polkit is refused even for a group member.
    let denied = daemon.handle(
        &mut io,
        &group_caller(),
        Some(3),
        Request::Pause {
            kind: PauseKind::Enforcement,
            minutes: 5,
            note: None,
        },
    );
    assert_eq!(error_of(&denied).code, "unauthorized");

    // Root bypasses both gates.
    let allowed = daemon.handle(&mut io, &Caller::root(), Some(4), Request::List);
    assert!(allowed.response.ok);
}

#[test]
fn learn_candidate_builds_a_draft_and_flags_physical_binding() {
    let mut daemon = daemon();
    let mut io = FakeIo::with_px4_and_adapter();

    let handled = daemon.handle(
        &mut io,
        &group_caller(),
        Some(1),
        Request::LearnCandidate {
            devnode: "/dev/ttyACM0".into(),
        },
    );
    let learn: devguard_core::protocol::LearnResponse = data_of(&handled);
    assert_eq!(learn.draft.kind, DeviceKind::Px4);
    assert_eq!(learn.draft.ids[0].serial.as_deref(), Some("SN-PX4"));
    assert_eq!(learn.draft.ids[0].role, IdRole::App);
    assert!(learn.warnings.iter().any(|warning| warning.contains("CDC-ACM")));

    // The CH340-style adapter has no serial: the draft binds to the port and
    // the warning says so.
    let handled = daemon.handle(
        &mut io,
        &group_caller(),
        Some(2),
        Request::LearnCandidate {
            devnode: "/dev/ttyUSB0".into(),
        },
    );
    let learn: devguard_core::protocol::LearnResponse = data_of(&handled);
    assert!(learn.draft.ids[0].serial.is_none());
    assert_eq!(
        learn.draft.ids[0].path.as_deref(),
        Some("pci-1")
    );
    assert!(learn
        .warnings
        .iter()
        .any(|warning| warning.contains("physical port")));

    // A devnode that is not a managed tty is refused with an actionable error.
    let handled = daemon.handle(
        &mut io,
        &group_caller(),
        Some(3),
        Request::LearnCandidate {
            devnode: "/dev/ttyS0".into(),
        },
    );
    assert_eq!(error_of(&handled).code, "invalid_request");

    // A devnode that does not exist at all is not found.
    let handled = daemon.handle(
        &mut io,
        &group_caller(),
        Some(4),
        Request::LearnCandidate {
            devnode: "/dev/null".into(),
        },
    );
    assert_eq!(error_of(&handled).code, "not_found");
}

#[test]
fn save_requires_polkit_and_emits_event_journal_and_rules_effect() {
    let mut daemon = daemon();
    let mut io = FakeIo::with_px4_and_adapter();

    // Group member without polkit: refused, nothing written.
    let denied = daemon.handle(
        &mut io,
        &group_caller(),
        Some(1),
        Request::SaveDevice {
            config: px4_config(),
            note: Some("no".into()),
        },
    );
    assert_eq!(error_of(&denied).code, "unauthorized");
    assert!(daemon.store().load().unwrap().devices.is_empty());

    // Authorized save: file written, event + journal + WriteRules effect.
    let handled = daemon.handle(
        &mut io,
        &authorized_caller(),
        Some(2),
        Request::SaveDevice {
            config: px4_config(),
            note: Some("registered on the bench".into()),
        },
    );
    assert!(handled.response.ok, "{:?}", handled.response.error);
    assert!(handled.effects.contains(&Effect::WriteRules));
    assert!(handled
        .events
        .iter()
        .any(|event| matches!(event, Event::ConfigChanged { .. })));
    assert!(handled
        .journal
        .iter()
        .any(|entry| entry.event == "config_changed" && entry.uid == Some(1000)));
    assert!(daemon.store().get("px4-main").is_ok());
}#[test]
fn enforce_blocks_unregistered_and_pause_releases() {
    let mut daemon = daemon();
    let mut io = FakeIo::with_px4_and_adapter();

    // Register the PX4 first.
    daemon.handle(
        &mut io,
        &authorized_caller(),
        Some(1),
        Request::SaveDevice {
            config: px4_config(),
            note: None,
        },
    );

    // Switch to enforce (authorized).
    let handled = daemon.handle(
        &mut io,
        &authorized_caller(),
        Some(2),
        Request::PolicySet {
            mode: PolicyMode::Enforce,
            note: Some("lock the bench".into()),
        },
    );
    assert!(handled.events.iter().any(|event| matches!(
        event,
        Event::PolicyChanged { after: PolicyMode::Enforce, .. }
    )));

    // The unregistered adapter is now blocked by its USB path.
    let adapter = facts("/dev/ttyUSB0", "1a86", "7523", None, Some("pci-1"));
    let handled = daemon.on_device_facts(&mut io, &adapter);
    assert!(handled
        .events
        .iter()
        .any(|event| matches!(event, Event::Blocked { .. })));
    assert_eq!(daemon.blocked.len(), 1);

    // A pause (enforcement) releases it and marks it allowed.
    let handled = daemon.handle(
        &mut io,
        &authorized_caller(),
        Some(3),
        Request::Pause {
            kind: PauseKind::Enforcement,
            minutes: 5,
            note: Some("flash another board".into()),
        },
    );
    assert!(daemon.blocked.is_empty());
    assert_eq!(daemon.allowed.len(), 1);
    assert!(handled
        .events
        .iter()
        .any(|event| matches!(event, Event::PauseStarted { .. })));

    // During the pause, policy changes are refused (section 4.2).
    let refused = daemon.handle(
        &mut io,
        &authorized_caller(),
        Some(4),
        Request::PolicySet {
            mode: PolicyMode::Audit,
            note: None,
        },
    );
    assert_eq!(error_of(&refused).code, "busy");

    // The registered PX4 is still maintained: apply works during enforcement.
    let applied = daemon.handle(
        &mut io,
        &authorized_caller(),
        Some(5),
        Request::Apply {
            name: Some("px4-main".into()),
            devnode: None,
        },
    );
    assert!(applied.response.ok, "{:?}", applied.response.error);
    assert_eq!(io.apply_calls, vec!["px4-main".to_owned()]);

    // Resume: the pause ends and the GUI is told which devices to prompt for.
    let handled = daemon.handle(
        &mut io,
        &authorized_caller(),
        Some(6),
        Request::Resume {
            note: Some("back to work".into()),
        },
    );
    let resume: devguard_core::protocol::ResumeResponse = data_of(&handled);
    assert!(matches!(resume.service, ServiceState::Running { .. }));
    assert_eq!(resume.allowed.len(), 1);
    assert!(handled
        .events
        .iter()
        .any(|event| matches!(event, Event::PauseEnded { .. })));
}

#[test]
fn pause_expiry_is_monotonic_and_a_new_pause_cannot_start_while_paused() {
    let mut daemon = daemon();
    let mut io = FakeIo::with_px4_and_adapter();

    let handled = daemon.handle(
        &mut io,
        &authorized_caller(),
        Some(1),
        Request::Pause {
            kind: PauseKind::Service,
            minutes: 5,
            note: None,
        },
    );
    assert!(handled.response.ok);

    // A second pause is busy.
    let busy = daemon.handle(
        &mut io,
        &authorized_caller(),
        Some(2),
        Request::Pause {
            kind: PauseKind::Enforcement,
            minutes: 5,
            note: None,
        },
    );
    assert_eq!(error_of(&busy).code, "busy");

    // Before the deadline nothing expires.
    assert!(daemon.expire_pause(&authorized_caller()).is_none());

    // Force the deadline into the past, then expire.
    if let Some(pause) = &mut daemon.pause {
        pause.until = chrono::Utc::now() - chrono::Duration::seconds(1);
    }
    let handled = daemon.expire_pause(&authorized_caller()).expect("must expire");
    assert!(daemon.pause.is_none());
    assert!(handled
        .events
        .iter()
        .any(|event| matches!(event, Event::PauseEnded { .. })));

    // And the service accepts a new pause afterwards.
    let handled = daemon.handle(
        &mut io,
        &authorized_caller(),
        Some(3),
        Request::Pause {
            kind: PauseKind::Service,
            minutes: 15,
            note: None,
        },
    );
    assert!(handled.response.ok);
}

#[test]
fn service_pause_stops_apply_but_registered_devices_stay_listed() {
    let mut daemon = daemon();
    let mut io = FakeIo::with_px4_and_adapter();

    daemon.handle(
        &mut io,
        &authorized_caller(),
        Some(1),
        Request::SaveDevice {
            config: px4_config(),
            note: None,
        },
    );
    daemon.handle(
        &mut io,
        &authorized_caller(),
        Some(2),
        Request::Pause {
            kind: PauseKind::Service,
            minutes: 5,
            note: None,
        },
    );

    // Apply is refused with a busy error naming the next step.
    let refused = daemon.handle(
        &mut io,
        &authorized_caller(),
        Some(3),
        Request::Apply {
            name: Some("px4-main".into()),
            devnode: None,
        },
    );
    assert_eq!(error_of(&refused).code, "busy");

    // Reads keep working.
    let listed = daemon.handle(&mut io, &group_caller(), Some(4), Request::List);
    let list: devguard_core::protocol::ListResponse = data_of(&listed);
    assert!(matches!(list.service, ServiceState::Paused { .. }));
    assert_eq!(list.devices.len(), 1);
}

#[test]
fn composite_refusal_keeps_the_device_allowed_and_audited() {
    let mut daemon = daemon();
    let mut io = FakeIo::with_px4_and_adapter();
    let adapter = facts("/dev/ttyUSB0", "1a86", "7523", None, Some("pci-1"));
    let usb_path = adapter.usb_path.clone().unwrap();
    io.refuse_block = Some(usb_path.clone());

    daemon.handle(
        &mut io,
        &authorized_caller(),
        Some(1),
        Request::PolicySet {
            mode: PolicyMode::Enforce,
            note: None,
        },
    );

    let handled = daemon.on_device_facts(&mut io, &adapter);
    // Never blocked blindly: the refusal is explicit and audited.
    assert!(daemon.blocked.is_empty());
    assert_eq!(daemon.allowed.len(), 1);
    assert!(handled
        .journal
        .iter()
        .any(|entry| entry.event == "block_refused"));
    assert!(handled
        .journal
        .iter()
        .any(|entry| entry
            .details
            .get("reason")
            .and_then(|reason| reason.as_str())
            .is_some_and(|reason| reason.contains("non-serial"))));
}

#[test]
fn shutdown_returns_the_effect_and_audits_the_operator() {
    let mut daemon = daemon();
    let mut io = FakeIo::with_px4_and_adapter();

    let handled = daemon.handle(
        &mut io,
        &authorized_caller(),
        Some(1),
        Request::Shutdown {
            release_blocked: true,
            note: Some("maintenance".into()),
        },
    );
    assert!(handled.response.ok);
    assert!(handled
        .effects
        .contains(&Effect::Shutdown { release_blocked: true }));
    assert!(handled
        .events
        .iter()
        .any(|event| matches!(event, Event::ServiceStopping { .. })));
    assert!(handled
        .journal
        .iter()
        .any(|entry| entry.event == "service_stopping" && entry.uid == Some(1000)));
}

#[test]
fn block_and_allow_are_polkit_gated_and_keyed_by_usb_path() {
    let mut daemon = daemon();
    let mut io = FakeIo::with_px4_and_adapter();

    // Without polkit: refused.
    let denied = daemon.handle(
        &mut io,
        &group_caller(),
        Some(1),
        Request::BlockDevice {
            devnode: "/dev/ttyUSB0".into(),
            note: None,
        },
    );
    assert_eq!(error_of(&denied).code, "unauthorized");

    // With polkit: blocked, then allowed again.
    let blocked = daemon.handle(
        &mut io,
        &authorized_caller(),
        Some(2),
        Request::BlockDevice {
            devnode: "/dev/ttyUSB0".into(),
            note: Some("suspicious adapter".into()),
        },
    );
    assert!(blocked.response.ok);
    assert_eq!(daemon.blocked.len(), 1);

    let allowed = daemon.handle(
        &mut io,
        &authorized_caller(),
        Some(3),
        Request::AllowDevice {
            devnode: "/dev/ttyUSB0".into(),
            note: Some("user says keep it".into()),
        },
    );
    assert!(allowed.response.ok);
    assert!(daemon.blocked.is_empty());
    assert_eq!(daemon.allowed.len(), 1);
}

#[test]
fn journal_persists_through_the_daemon_paths() {
    let mut daemon = daemon();
    let mut io = FakeIo::with_px4_and_adapter();

    daemon.handle(
        &mut io,
        &authorized_caller(),
        Some(1),
        Request::SaveDevice {
            config: px4_config(),
            note: None,
        },
    );

    // The shell wrote the journal entries the core returned.
    let journal = Journal::new(&daemon.paths);
    let entry = devguard_core::JournalEntry::new("config_changed").name("px4-main");
    journal.append(&entry).unwrap();
    let query = journal
        .query(&devguard_core::LogFilter {
            name: Some("px4-main".into()),
            ..devguard_core::LogFilter::default()
        })
        .unwrap();
    assert_eq!(query.entries.len(), 1);
}

#[test]
fn rules_generation_flags_unsafe_ids_instead_of_escaping_them() {
    let mut config = px4_config();
    config.ids[0].serial = Some("evil\"; SYMLINK".into());
    let generated = rules::generate(&[config], "dialout");
    assert!(generated
        .skipped
        .iter()
        .any(|note| note.contains("no safely matchable") || note.contains("no usable ids")));
    assert!(!generated.text.contains("evil"));
}

#[test]
fn pause_presets_are_enforced_by_the_core_not_only_the_gui() {
    let mut daemon = daemon();
    let mut io = FakeIo::with_px4_and_adapter();

    for minutes in [0u32, 61, 1000] {
        let refused = daemon.handle(
            &mut io,
            &authorized_caller(),
            Some(1),
            Request::Pause {
                kind: PauseKind::Enforcement,
                minutes,
                note: None,
            },
        );
        assert_eq!(error_of(&refused).code, "invalid_request", "{minutes}");
    }
}

#[test]
fn every_query_response_preserves_the_request_id() {
    let mut daemon = daemon();
    let mut io = FakeIo::with_px4_and_adapter();
    for request in [Request::List, Request::PolicyGet, Request::ServiceState, Request::Doctor] {
        let handled = daemon.handle(&mut io, &group_caller(), Some(73), request);
        assert!(handled.response.ok);
        assert_eq!(handled.response.id, Some(73));
    }
}

#[test]
fn check_returns_drift_events_for_subscribers() {
    let mut daemon = daemon();
    let mut io = FakeIo::with_px4_and_adapter();
    daemon.handle(&mut io, &authorized_caller(), Some(1), Request::SaveDevice {
        config: px4_config(), note: None,
    });
    io.check_drift = true;
    let handled = daemon.handle(&mut io, &group_caller(), Some(2), Request::Check { name: None });
    assert!(handled.events.iter().any(|event| matches!(event, Event::DriftDetected { .. })));
}
