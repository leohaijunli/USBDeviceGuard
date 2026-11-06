//! Phase 1 integration tests for the platform-independent core: the device
//! store, the journal and the policy file, exercised on real temporary
//! directories. Linux-only behavior (udev, termios, sysfs) is phase 2+.

use std::fs;

use devguard_core::{
    config::{AccessConfig, DeviceConfig, DeviceId, DeviceKind, IdRole, SerialConfig},
    journal::{Journal, JournalEntry, LogFilter},
    paths::Paths,
    policy::{PauseKind, PauseState, PolicyMode, PolicyStore, MAX_PAUSE_MINUTES},
    protocol::{
        Event, EventMessage, Operator, Request, RequestEnvelope, ResponseEnvelope,
        PROTOCOL_VERSION,
    },
    store::{DeviceStore, StoreError},
    BaudRate, ValidationIssue,
};
use tempfile::TempDir;

fn px4(name: &str, vid: &str, pid: &str, serial: &str) -> DeviceConfig {
    DeviceConfig {
        name: name.to_owned(),
        kind: DeviceKind::Px4,
        description: Some("integration fixture".into()),
        ids: vec![DeviceId {
            vid: vid.to_owned(),
            pid: pid.to_owned(),
            serial: Some(serial.to_owned()),
            path: None,
            role: IdRole::App,
        }],
        serial: SerialConfig {
            baud: BaudRate::NotApplicable,
            data_bits: 8,
            parity: devguard_core::Parity::N,
            stop_bits: 1,
            flow: devguard_core::FlowControl::None,
            raw: true,
        },
        access: AccessConfig::default(),
    }
}

fn radio(name: &str, vid: &str, pid: &str, path: &str) -> DeviceConfig {
    DeviceConfig {
        name: name.to_owned(),
        kind: DeviceKind::Radio,
        description: None,
        ids: vec![DeviceId {
            vid: vid.to_owned(),
            pid: pid.to_owned(),
            serial: None,
            path: Some(path.to_owned()),
            role: IdRole::App,
        }],
        serial: SerialConfig {
            baud: BaudRate::Rate(57600),
            data_bits: 8,
            parity: devguard_core::Parity::N,
            stop_bits: 1,
            flow: devguard_core::FlowControl::None,
            raw: true,
        },
        access: AccessConfig::default(),
    }
}

#[test]
fn store_roundtrips_backup_and_rejects_conflicts() {
    let dir = TempDir::new().unwrap();
    let paths = Paths::rooted(dir.path());
    paths.create_dirs().unwrap();
    let store = DeviceStore::new(paths.clone());

    // Empty store loads cleanly.
    let loaded = store.load().unwrap();
    assert!(loaded.devices.is_empty() && loaded.errors.is_empty());

    // Save creates a readable file whose name matches its config.
    let first = px4("px4-main", "26ac", "0032", "SN-1");
    let outcome = store.save(&first, Some("initial registration")).unwrap();
    assert!(outcome.backup.is_none());
    let text = fs::read_to_string(&outcome.path).unwrap();
    assert!(text.contains("name = \"px4-main\""), "{text}");
    assert!(text.contains("baud = \"n/a\""), "{text}");

    // Editing backs up the previous content.
    let mut edited = first.clone();
    edited.description = Some("moved to the bench".into());
    let outcome = store.save(&edited, Some("edit")).unwrap();
    let backup = outcome.backup.expect("edit must back up the old file");
    let backup_text = fs::read_to_string(&backup).unwrap();
    assert!(backup_text.contains("integration fixture"), "{backup_text}");

    // A second device with the same identity is rejected before writing.
    let conflicting = px4("px4-clone", "26ac", "0032", "SN-1");
    let error = store.save(&conflicting, None).unwrap_err();
    match &error {
        StoreError::Invalid { message, issues } => {
            assert!(message.contains("already belongs"), "{message}");
            assert!(issues.iter().any(|issue| issue.severity
                == devguard_core::check::Severity::Error));
        }
        other => panic!("expected a validation error, got {other:?}"),
    }
    assert!(!paths.device_file("px4-clone").exists());

    // Saving under an existing name is an edit: the file is replaced, and the
    // identity may change (the operator re-registered different hardware).
    // Cross-device identity conflicts are still rejected, as asserted above.
    let duplicate_name = px4("px4-main", "1a86", "7523", "CH340");
    store.save(&duplicate_name, Some("replaced hardware")).unwrap();
    let loaded = store.load().unwrap();
    assert_eq!(loaded.devices.len(), 1);
    assert_eq!(loaded.devices[0].ids[0].vid, "1a86");

    // A radio bound to a physical port coexists with the PX4.
    let second = radio("radio-1", "1a86", "7523", "pci-0000:00:14.0-usb-0:2:1.0");
    store.save(&second, None).unwrap();
    let loaded = store.load().unwrap();
    assert_eq!(loaded.devices.len(), 2);
    assert_eq!(loaded.devices[0].name, "px4-main");
    assert_eq!(loaded.devices[1].kind, DeviceKind::Radio);
    assert_eq!(loaded.devices[1].ids[0].path.as_deref(), Some("pci-0000:00:14.0-usb-0:2:1.0"));

    // Delete backs up and removes the file.
    let backup = store.delete("radio-1").unwrap().expect("delete backs up");
    assert!(backup.to_string_lossy().contains("radio-1-deleted-"));
    assert!(!paths.device_file("radio-1").exists());
    assert!(matches!(
        store.delete("radio-1"),
        Err(StoreError::NotFound(_))
    ));

    // The reloaded store keeps the replaced px4-main with the new hardware.
    let loaded = store.load().unwrap();
    assert_eq!(loaded.devices.len(), 1);
    assert_eq!(loaded.devices[0].ids[0].vid, "1a86");
    assert_eq!(
        loaded.devices[0].description.as_deref(),
        Some("integration fixture")
    );
}

#[test]
fn store_rejects_invalid_names_and_missing_bindings() {
    let dir = TempDir::new().unwrap();
    let store = DeviceStore::new(Paths::rooted(dir.path()));

    let mut device = px4("px4-main", "26ac", "0032", "SN-1");
    device.name = "PX4 Main".into();
    assert!(matches!(store.save(&device, None), Err(StoreError::InvalidName(_))));

    device.name = "../escape".into();
    assert!(matches!(store.save(&device, None), Err(StoreError::InvalidName(_))));

    device.name = "px4-main".into();
    device.ids[0].serial = None;
    let issues: Vec<ValidationIssue> = device.validate();
    assert!(issues
        .iter()
        .any(|issue| issue.message.contains("neither serial nor path")));
}

#[test]
fn journal_append_query_and_export() {
    let dir = TempDir::new().unwrap();
    let journal = Journal::at(dir.path().join("journal.jsonl"));

    let registration = JournalEntry::new("config_changed")
        .name("px4-main")
        .devnode("/dev/ttyACM0")
        .details(serde_json::json!({ "note": "registered" }))
        .operator(1000, "alice")
        .change("none", "26ac:0032");
    journal.append(&registration).unwrap();

    let pause = JournalEntry::new("pause_started")
        .details(serde_json::json!({ "kind": "enforcement", "minutes": 5 }))
        .operator(1000, "alice");
    journal.append(&pause).unwrap();

    // Unknown events are still records; the audit trail never silently drops.
    let blocked = JournalEntry::new("blocked")
        .devnode("/dev/ttyUSB3")
        .details(serde_json::json!({ "vid": "1a86", "pid": "7523" }))
        .operator(0, "root");
    journal.append(&blocked).unwrap();

    // Everything, oldest first.
    let all = journal.query(&LogFilter::default()).unwrap();
    assert_eq!(all.entries.len(), 3);
    assert_eq!(all.skipped, 0);
    assert_eq!(all.entries[0].event, "config_changed");
    assert_eq!(all.entries[0].uid, Some(1000));
    assert_eq!(all.entries[0].user.as_deref(), Some("alice"));
    assert_eq!(
        all.entries[0].after.as_ref().and_then(|v| v.as_str()),
        Some("26ac:0032")
    );

    // Prefix filters and device filters compose.
    let pauses = journal
        .query(&LogFilter {
            event: Some("pause_*".into()),
            ..LogFilter::default()
        })
        .unwrap();
    assert_eq!(pauses.entries.len(), 1);
    assert_eq!(
        pauses.entries[0]
            .details
            .get("kind")
            .and_then(|kind| kind.as_str()),
        Some("enforcement")
    );

    let by_device = journal
        .query(&LogFilter {
            name: Some("px4-main".into()),
            ..LogFilter::default()
        })
        .unwrap();
    assert_eq!(by_device.entries.len(), 1);

    // The limit keeps the newest records.
    let limited = journal
        .query(&LogFilter {
            limit: Some(2),
            ..LogFilter::default()
        })
        .unwrap();
    assert_eq!(limited.entries.len(), 2);
    assert_eq!(limited.entries[0].event, "pause_started");

    // Export writes exactly the filtered records as JSONL.
    let export = dir.path().join("export.jsonl");
    let count = journal
        .export(
            &LogFilter {
                event: Some("blocked".into()),
                ..LogFilter::default()
            },
            &export,
        )
        .unwrap();
    assert_eq!(count, 1);
    let exported = fs::read_to_string(&export).unwrap();
    let line = exported.lines().next().unwrap();
    let entry: JournalEntry = serde_json::from_str(line).unwrap();
    assert_eq!(entry.event, "blocked");
    assert_eq!(entry.devnode.as_deref(), Some("/dev/ttyUSB3"));
}

#[test]
fn journal_survives_corrupt_lines_without_losing_the_rest() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("journal.jsonl");
    let journal = Journal::at(&path);
    journal
        .append(&JournalEntry::new("service_started").operator(0, "root"))
        .unwrap();
    // A torn write from a power cut: half a line, no trailing newline.
    use std::io::Write;
    let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
    write!(file, "{{\"ts\":\"2026-11-06T1").unwrap();
    drop(file);

    let query = journal.query(&LogFilter::default()).unwrap();
    assert_eq!(query.entries.len(), 1);
    assert_eq!(query.skipped, 1);
}

#[test]
fn policy_store_defaults_to_audit_and_roundtrips() {
    let dir = TempDir::new().unwrap();
    let paths = Paths::rooted(dir.path());
    paths.create_dirs().unwrap();
    let policy = PolicyStore::new(&paths);

    // A missing file is the documented default: audit.
    assert_eq!(policy.load().unwrap().mode, PolicyMode::Audit);
    assert!(!paths.policy_file().exists());

    policy.set(PolicyMode::Enforce).unwrap();
    let text = fs::read_to_string(paths.policy_file()).unwrap();
    assert!(text.contains("mode = \"enforce\""), "{text}");
    assert_eq!(policy.load().unwrap().mode, PolicyMode::Enforce);
}

#[test]
fn pause_state_is_bounded_and_monotonic() {
    let pause = PauseState::new(PauseKind::Enforcement, 5).unwrap();
    assert_eq!(pause.kind, PauseKind::Enforcement);
    assert_eq!(pause.minutes, 5);
    assert!(pause.until > pause.started_at);
    assert!(pause.remaining_secs() > 0 && pause.remaining_secs() <= 300);

    // No permanent pause, no zero pause, only the documented presets matter.
    assert!(PauseState::new(PauseKind::Service, 0).is_err());
    assert!(PauseState::new(PauseKind::Service, MAX_PAUSE_MINUTES + 1).is_err());
    assert!(PauseState::new(PauseKind::Service, 60).is_ok());
}

#[test]
fn protocol_envelopes_reject_versions_and_unknown_ops() {
    // Requests carry the version; a mismatch is refused, not guessed.
    let error = RequestEnvelope::parse("{\"v\":2,\"op\":\"list\"}").unwrap_err();
    assert_eq!(error.code, "unsupported");
    assert!(error.next_step.is_some());

    let error = RequestEnvelope::parse("{\"op\":\"list\"}").unwrap_err();
    assert_eq!(error.code, "invalid_request");

    // A shutdown request keeps its options through the round trip.
    let request = Request::Shutdown {
        release_blocked: false,
        note: Some("maintenance window".into()),
    };
    let line = serde_json::to_string(&RequestEnvelope::new(9, request.clone())).unwrap();
    let parsed = RequestEnvelope::parse(&line).unwrap();
    assert_eq!(parsed.request, request);
    assert_eq!(parsed.id, Some(9));
    assert_eq!(parsed.request.name(), "shutdown");
    assert!(!parsed.request.is_read_only());
    assert_eq!(
        parsed.request.needs_polkit_action(),
        Some("dev.devguard.service-stop")
    );

    // Read-only classification drives the socket's authorization split.
    assert!(Request::Doctor.is_read_only());
    assert!(!Request::PolicySet {
        mode: PolicyMode::Enforce,
        note: None
    }
    .is_read_only());

    // Responses separate ok and error payloads with actionable next steps.
    let ok = ResponseEnvelope::ok(
        Some(3),
        devguard_core::protocol::ServiceStateResponse {
            service: devguard_core::ServiceState::Running {
                mode: PolicyMode::Audit,
            },
        },
    );
    let line = serde_json::to_string(&ok).unwrap();
    let parsed = ResponseEnvelope::parse(&line).unwrap();
    assert!(parsed.ok);
    let state: devguard_core::protocol::ServiceStateResponse = parsed.into_data().unwrap();
    assert_eq!(
        state.service,
        devguard_core::ServiceState::Running {
            mode: PolicyMode::Audit
        }
    );

    let failure = ResponseEnvelope::error(
        Some(4),
        devguard_core::ProtocolError::unauthorized(
            "polkit refused dev.devguard.pause",
            "ask an administrator or run from a member of the devguard group",
        ),
    );
    let line = serde_json::to_string(&failure).unwrap();
    let parsed = ResponseEnvelope::parse(&line).unwrap();
    assert!(!parsed.ok);
    let error = parsed.into_data::<serde_json::Value>().unwrap_err();
    assert_eq!(error.code, "unauthorized");
    assert!(error.next_step.unwrap().contains("administrator"));

    // Events are versioned and timestamped at construction.
    let message = EventMessage::new(
        41,
        Event::ServiceStarted {
            pid: 4321,
            reconcile: devguard_core::StartupReconcile {
                added: Vec::new(),
                removed: vec!["radio-1".into()],
                pause_cleared: true,
                note: "daemon restarted".into(),
            },
        },
    );
    assert_eq!(message.v, PROTOCOL_VERSION);
    let line = serde_json::to_string(&message).unwrap();
    assert!(line.contains("\"event\":\"service_started\""), "{line}");
    let parsed: EventMessage = serde_json::from_str(&line).unwrap();
    assert_eq!(parsed.seq, 41);
    assert!(parsed
        .event
        .name()
        .starts_with("service_"));
    let operator = Operator::new(1000, "alice\n");
    assert_eq!(operator.user, "alice?"); // control characters cannot corrupt JSONL
}
