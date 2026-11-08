//! Fixtures are confined to tests; no fake-data controls ship in the GUI.

use devguard_core::{
    protocol::ListResponse, DeviceConfig, DeviceId, DeviceKind, IdRole, PolicyMode, ServiceState,
    UdevFacts,
};

use super::model::*;

pub(super) fn config(name: &str) -> DeviceConfig {
    DeviceConfig {
        name: name.into(),
        description: Some("Bench device".into()),
        kind: DeviceKind::Radio,
        ids: vec![DeviceId {
            vid: "1209".into(),
            pid: "0001".into(),
            serial: Some(name.into()),
            path: None,
            role: IdRole::App,
        }],
        serial: DeviceKind::Radio.default_serial(),
        access: Default::default(),
    }
}

pub(super) fn device(name: &str, node: &str) -> devguard_core::OnlineDevice {
    UdevFacts {
        devnode: node.into(),
        subsystem: "tty".into(),
        vid: "1209".into(),
        pid: "0001".into(),
        serial_short: Some(name.into()),
        model: Some(format!("Probe {name}")),
        is_usb: true,
        ..UdevFacts::default()
    }
    .to_online()
}

pub(super) fn snapshot(sequence: u64) -> ListResponse {
    ListResponse {
        service: ServiceState::running(PolicyMode::Audit),
        policy: PolicyMode::Audit,
        devices: vec![config("radio-a")],
        online: vec![device("b", "/dev/ttyUSB0")],
        problems: vec![],
        paused_allowed: vec![],
        generated_at: "2026-11-08T00:00:00Z".into(),
        event_sequence: Some(sequence),
    }
}

#[test]
fn snapshots_and_events_have_an_exact_non_guessed_barrier() {
    assert!(Cursor::snapshot(None).is_err());
    let mut cursor = Cursor::snapshot(Some(10)).unwrap();
    assert!(!cursor.accept(9).unwrap());
    assert!(!cursor.accept(10).unwrap());
    assert!(cursor.accept(11).unwrap());
    assert!(cursor.accept(13).is_err());
}

#[test]
fn duplicate_new_events_are_not_silently_ignored() {
    let mut cursor = Cursor::snapshot(Some(10)).unwrap();
    assert!(cursor.accept(11).unwrap());
    assert!(cursor.accept(11).is_err());
}

#[test]
fn reconnect_backoff_is_bounded() {
    assert_eq!(
        (0..8).map(retry_seconds).collect::<Vec<_>>(),
        [1, 2, 4, 8, 16, 30, 30, 30]
    );
}

#[test]
fn saved_offline_devices_and_unregistered_online_devices_both_remain_visible() {
    let rows = rows(&snapshot(0));
    assert_eq!(rows.len(), 2);
    assert!(rows
        .iter()
        .any(|r| r.status() == "Offline" && r.title == "radio-a"));
    assert!(rows.iter().any(|r| r.status() == "Unregistered"));
}

#[test]
fn devnode_reuse_never_rebinds_a_selection_to_a_different_serial() {
    let a = device("a", "/dev/ttyUSB0");
    let b = device("b", "/dev/ttyUSB0");
    assert_ne!(online_key(&a), online_key(&b));
    let mut renamed = a.clone();
    renamed.devnode = "/dev/ttyUSB1".into();
    assert_eq!(online_key(&a), online_key(&renamed));
}

#[test]
fn serial_identity_follows_ports_but_path_bound_identity_does_not() {
    let mut a = device("a", "/dev/ttyUSB0");
    a.id_path = Some("physical-port-1".into());
    let mut b = a.clone();
    b.id_path = Some("physical-port-2".into());
    assert_eq!(online_key(&a), online_key(&b));
    a.serial = None;
    b.serial = None;
    assert_ne!(online_key(&a), online_key(&b));
}

#[test]
fn an_unknown_connection_is_not_mislabelled_as_stopped_or_protected() {
    let fault = Fault::new("test", "test", "test", false);
    assert!(Connection::Unavailable(fault)
        .label(None)
        .contains("unknown"));
    assert!(Connection::Live
        .label(Some(&ServiceState::running(PolicyMode::Audit)))
        .contains("no blocking"));
}

mod ui_flows {
    use devguard_core::{
        protocol::{CheckResponse, DoctorResponse, LearnResponse, LogResponse, ShowResponse},
        CheckItem, CheckReport, DoctorItem, DoctorStatus, JournalEntry, JournalQuery,
    };
    use nsh_base::theme;
    use nsh_core::{
        platform::{Idiom, Os},
        prelude::*,
    };
    use nsh_engine::{Clock, EngineConfig, Environment, Root};
    use nsh_testing::Headless;
    use std::time::{Duration, SystemTime};

    use super::*;
    use crate::{
        daemon::{
            store::{Command, Message, Payload, ReadKind},
            Inspection, Workspace,
        },
        ui::{self, ActionStyle},
    };

    #[component]
    fn FixtureApp() -> impl IntoElement {
        ui::tokens::install();
        let (store, mut commands) = Workspace::fixture(snapshot(10));
        let hold = signal(false);
        let hold_refresh = signal(false);
        let held = stored(None::<Message>);
        spawn(async move {
            while let Some(command) = commands.recv().await {
                match command {
                    Command::Reconnect => store.inject(Message::Snapshot {
                        generation: store.generation() + 1,
                        snapshot: snapshot(20),
                    }),
                    Command::Read {
                        generation,
                        ticket,
                        kind,
                    } => {
                        if matches!(kind, ReadKind::Refresh) {
                            let response = Message::Snapshot {
                                generation,
                                snapshot: snapshot(30),
                            };
                            if hold_refresh.get_untracked() {
                                held.replace(Some(response));
                            } else {
                                store.inject(response);
                            }
                            continue;
                        }
                        let payload = match &kind {
                            ReadKind::Inspect {
                                name: Some(name), ..
                            } => Payload::Inspection(Box::new(Inspection::Registered(
                                ShowResponse {
                                    device: config(name),
                                    online: vec![],
                                    check: CheckReport::new(
                                        Some(name.clone()),
                                        vec![CheckItem::warning(
                                            "offline",
                                            "Device is offline",
                                            "Not connected",
                                            "Connect the device.",
                                        )],
                                    ),
                                    symlink: format!("/dev/devguard/{name}"),
                                    runtime_env: format!("/run/devguard/{name}.env"),
                                    toml: config(name).to_toml().unwrap(),
                                },
                            ))),
                            ReadKind::Inspect {
                                devnode: Some(node),
                                ..
                            } => Payload::Inspection(Box::new(Inspection::Candidate(
                                LearnResponse {
                                    facts: UdevFacts {
                                        devnode: node.clone(),
                                        ..UdevFacts::default()
                                    },
                                    draft: config("candidate-b"),
                                    warnings: vec![
                                        "This registration draft has not been saved.".into()
                                    ],
                                },
                            ))),
                            ReadKind::Inspect { .. } => unreachable!(),
                            ReadKind::Check => Payload::Check(CheckResponse {
                                overall: CheckReport::new(
                                    None,
                                    vec![CheckItem::warning(
                                        "offline",
                                        "Fixture check result",
                                        "Device offline",
                                        "Connect device",
                                    )],
                                ),
                                reports: vec![],
                            }),
                            ReadKind::Doctor => Payload::Doctor(DoctorResponse {
                                report: devguard_core::DoctorReport::new(vec![DoctorItem::new(
                                    "driver",
                                    "Fixture driver diagnostic",
                                    DoctorStatus::Warning,
                                    "Needs inspection",
                                )]),
                            }),
                            ReadKind::Logs(filter) => {
                                let entry = JournalEntry::new("config_changed")
                                    .name("radio-a")
                                    .operator(1000, "fixture-user")
                                    .note("Authorized fixture change");
                                let entries = if filter
                                    .matches(&entry, &serde_json::to_string(&entry).unwrap())
                                {
                                    vec![entry]
                                } else {
                                    vec![]
                                };
                                Payload::Logs(LogResponse {
                                    query: JournalQuery {
                                        entries,
                                        skipped: 1,
                                        total_seen: 2,
                                    },
                                    export_path: None,
                                })
                            }
                            ReadKind::Refresh => unreachable!(),
                        };
                        let hold_this = hold.get_untracked()
                            && matches!(kind, ReadKind::Inspect { name: None, .. });
                        let message = Message::Reply {
                            generation,
                            ticket,
                            kind,
                            result: Ok(payload),
                        };
                        if hold_this {
                            held.replace(Some(message));
                        } else {
                            store.inject(message);
                        }
                    }
                }
            }
        });
        view()
            .flex_1()
            .bg(move || theme().background)
            .text_color(move || theme().foreground)
            .child(crate::live::shell_with(store))
            .child(
                view()
                    .flex_row()
                    .flex_wrap()
                    .gap_1()
                    .child(ui::action(
                        "Fixture: reuse port",
                        ActionStyle::Standard,
                        move || {
                            store.inject(Message::Event(devguard_core::EventMessage::new(
                                11,
                                devguard_core::Event::DeviceRemoved {
                                    name: "b".into(),
                                    devnode: "/dev/ttyUSB0".into(),
                                },
                            )));
                            store.inject(Message::Event(devguard_core::EventMessage::new(
                                12,
                                devguard_core::Event::DeviceAdded {
                                    device: device("c", "/dev/ttyUSB0"),
                                },
                            )));
                        },
                    ))
                    .child(ui::action(
                        "Fixture: stopped",
                        ActionStyle::Standard,
                        move || store.inject(Message::Connection(Connection::Stopped)),
                    ))
                    .child(ui::action(
                        "Fixture: interrupted",
                        ActionStyle::Standard,
                        move || {
                            store.inject(Message::Connection(Connection::Reconnecting {
                                seconds: 1,
                                fault: Fault::new(
                                    "fixture",
                                    "Fixture connection lost",
                                    "Reconnect for fresh data",
                                    true,
                                ),
                            }))
                        },
                    ))
                    .child(ui::action(
                        "Fixture: hold refresh",
                        ActionStyle::Standard,
                        move || hold_refresh.set(true),
                    ))
                    .child(ui::action(
                        "Fixture: hold inspection",
                        ActionStyle::Standard,
                        move || hold.set(true),
                    ))
                    .child(ui::action(
                        "Fixture: release stale reply",
                        ActionStyle::Standard,
                        move || {
                            if let Some(reply) = held.replace(None) {
                                store.inject(reply);
                            }
                        },
                    )),
            )
    }

    fn start(width: f32) -> Result<Headless, String> {
        let viewport = Size::new(px(width), px(800.0));
        let mut app = Headless::start(EngineConfig {
            viewport,
            environment: Environment {
                window: viewport,
                os: Os::Linux,
                idiom: Idiom::Desktop,
                ..Environment::default()
            },
            clock: Clock::manual(SystemTime::UNIX_EPOCH),
            ..EngineConfig::new(Root::of::<FixtureApp>())
        })
        .map_err(|e| e.to_string())?;
        app.wait_for("SerialGuard")?;
        app.wait_for("role:button[Probe b]")?;
        Ok(app)
    }

    #[test]
    fn live_workspace_does_not_claim_audit_is_protection_or_offer_writes() -> Result<(), String> {
        let app = start(1120.0)?;
        assert!(app.shows("Running · audit · no blocking"));
        assert!(app.shows("radio-a"));
        assert!(app.shows("Probe b"));
        assert!(!app.shows("Pause preview…"));
        assert!(!app.shows("Send event"));
        assert!(!app.shows("Apply configuration"));
        Ok(())
    }

    #[test]
    fn search_filter_and_resize_preserve_device_identity() -> Result<(), String> {
        for width in [900.0, 1120.0] {
            let mut app = start(width)?;
            app.press("role:button[radio-a]")?;
            app.wait_for(r"text:/^\/dev\/devguard\/radio-a$/")?;
            app.set_viewport(Size::new(px(1120.0), px(800.0)))?;
            app.type_text("id:live-search-devices", "Probe b")?;
            app.wait_for("This selection is outside the current filter.")?;
            app.press("role:button[Copy details]")?;
            assert!(app
                .screen()
                .clipboard()
                .unwrap_or_default()
                .starts_with("radio-a\n"));
            app.set_environment("large text", |e| e.font_scale = 1.5)?;
            app.clear("id:live-search-devices")?;
            app.press("role:button[radio-a]")?;
            app.wait_for("role:button[Back to list]")?;
        }
        Ok(())
    }

    #[test]
    fn unplugging_then_reusing_a_port_does_not_select_the_new_device() -> Result<(), String> {
        let mut app = start(1120.0)?;
        app.press("role:button[Probe b]")?;
        app.press("role:button[Fixture: reuse port]")?;
        app.wait_for("role:button[Probe c]")?;
        assert!(!app.screen().selected("role:button[Probe c]")?);
        assert!(app.shows("No device selected"));
        Ok(())
    }

    #[test]
    fn stale_inspection_response_cannot_overwrite_a_new_selection() -> Result<(), String> {
        let mut app = start(1120.0)?;
        app.press("role:button[radio-a]")?;
        app.wait_for(r"text:/^\/dev\/devguard\/radio-a$/")?;
        app.press("role:button[Fixture: hold inspection]")?;
        app.press("role:button[Probe b]")?;
        app.wait_for("Loading inspection report")?;
        app.press("role:button[radio-a]")?;
        app.wait_for(r"text:/^\/dev\/devguard\/radio-a$/")?;
        app.press("role:button[Fixture: release stale reply]")?;
        app.press("role:button[Copy details]")?;
        let copied = app.screen().clipboard().unwrap_or_default();
        assert!(copied.contains("/dev/devguard/radio-a"));
        assert!(!copied.contains("candidate-b"));
        Ok(())
    }

    #[test]
    fn stopped_and_unknown_states_disable_queries_and_keep_a_stale_snapshot() -> Result<(), String>
    {
        let mut app = start(1120.0)?;
        app.press("role:button[Fixture: interrupted]")?;
        app.wait_for("Connection interrupted · retry in 1s")?;
        assert!(!app.shows("Stopped · unprotected"));
        app.press("role:button[Fixture: stopped]")?;
        app.wait_for("Stopped · unprotected")?;
        assert!(app.shows("radio-a"));
        assert!(!app.screen().enabled("role:button[Refresh snapshot]")?);
        app.fast_forward(Duration::from_secs(120))?;
        assert!(app.shows("Stopped · unprotected"));
        app.press("role:button[Reconnect]")?;
        app.wait_for("Running · audit · no blocking")?;
        Ok(())
    }

    #[test]
    fn refresh_is_disabled_until_its_snapshot_arrives() -> Result<(), String> {
        let mut app = start(1120.0)?;
        app.press("role:button[Fixture: hold refresh]")?;
        app.press("role:button[Refresh snapshot]")?;
        assert!(!app.screen().enabled("role:button[Refresh snapshot]")?);
        app.press("role:button[Fixture: release stale reply]")?;
        app.wait_until("refresh enabled", Duration::from_secs(2), |screen| {
            screen
                .enabled("role:button[Refresh snapshot]")
                .unwrap_or(false)
        })?;
        Ok(())
    }

    #[test]
    fn logs_validate_time_and_copy_the_daemon_operator_not_a_gui_identity() -> Result<(), String> {
        let mut app = start(1120.0)?;
        app.press("role:tab[Logs]")?;
        app.wait_for("role:button[config_changed]")?;
        app.wait_for("text:/1 matching records/")?;
        app.type_text("id:log-since", "yesterday")?;
        app.press("role:button[Query logs]")?;
        app.wait_for("text:/Use RFC3339/")?;
        app.clear("id:log-since")?;
        app.press("role:button[Query logs]")?;
        app.press("role:button[Copy JSONL]")?;
        let text = app.screen().clipboard().unwrap_or_default();
        assert!(text.contains("fixture-user"));
        assert!(text.contains("Authorized fixture change"));
        assert!(text.ends_with('\n'));
        Ok(())
    }

    #[test]
    fn diagnostics_render_received_check_and_doctor_results_without_enabling_enforce(
    ) -> Result<(), String> {
        let mut app = start(1120.0)?;
        app.press("role:tab[Policy and diagnostics]")?;
        app.wait_for("Warning · Fixture check result")?;
        app.wait_for("Warning · Fixture driver diagnostic")?;
        assert!(!app.shows("Enable enforce"));
        assert!(!app.shows("Start preview"));
        app.press("role:button[Dark mode]")?;
        app.wait_for("role:button[Light mode]")?;
        Ok(())
    }
}
