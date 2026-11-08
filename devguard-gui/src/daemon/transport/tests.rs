//! Real Unix socket/daemon integration in private trees. Device I/O is inert.

use devguard_core::{
    client::Client, protocol::LogResponse, CheckReport, DeviceConfig, DoctorItem, Paths, UdevFacts,
};
use devguardd::{
    io::{ApplyOutcome, DeviceIo},
    linux::{AccessPolicy, Server},
};
use tokio::sync::oneshot;

use super::*;
use crate::daemon::tests::config;

struct InertIo;
impl DeviceIo for InertIo {
    fn enumerate(&self) -> Result<Vec<UdevFacts>, String> {
        Ok(vec![])
    }
    fn facts_for_devnode(&self, _: &str) -> Result<Option<UdevFacts>, String> {
        Ok(None)
    }
    fn apply(&mut self, _: &DeviceConfig, _: &UdevFacts) -> Result<ApplyOutcome, String> {
        panic!("GUI attempted a hardware write")
    }
    fn set_authorized(&mut self, _: &str, _: bool) -> Result<(), String> {
        panic!("GUI attempted a USB authorization write")
    }
    fn check(&self, c: &DeviceConfig, _: &UdevFacts) -> CheckReport {
        CheckReport::new(Some(c.name.clone()), vec![])
    }
    fn write_rules(&mut self, _: &[DeviceConfig]) -> Result<(), String> {
        Ok(())
    }
    fn doctor_probe(&self) -> Vec<DoctorItem> {
        vec![]
    }
}

struct TestDaemon {
    stop: Option<oneshot::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
}
impl TestDaemon {
    fn start(paths: Paths, access: AccessPolicy) -> Self {
        let (stop, stopped) = oneshot::channel();
        let (ready, ready_receiver) = std::sync::mpsc::channel();
        let thread = thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let server = Server::bind(paths, access).unwrap();
                ready.send(()).unwrap();
                server
                    .serve(InertIo, async {
                        let _ = stopped.await;
                    })
                    .await
                    .unwrap();
            });
        });
        ready_receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        Self {
            stop: Some(stop),
            thread: Some(thread),
        }
    }
    fn stop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}
impl Drop for TestDaemon {
    fn drop(&mut self) {
        self.stop();
    }
}

async fn receive(
    receiver: &mut mpsc::Receiver<Message>,
    matches: impl Fn(&Message) -> bool,
) -> Message {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let message = receiver.recv().await.expect("worker exited");
            if matches(&message) {
                return message;
            }
        }
    })
    .await
    .expect("expected socket update did not arrive")
}

fn seed(paths: &Paths) {
    devguard_core::store::DeviceStore::new(paths.clone())
        .save(&config("radio-a"), None)
        .unwrap();
}

#[tokio::test]
async fn real_daemon_populates_all_queries_and_pushes_configuration_changes() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::rooted(dir.path());
    seed(&paths);
    let _daemon = TestDaemon::start(paths.clone(), AccessPolicy::sandbox());
    let (updates, mut receiver) = mpsc::channel(32);
    let worker = start(paths.socket(), updates).unwrap();
    let sender = worker.sender();
    let message = receive(&mut receiver, |m| matches!(m, Message::Snapshot { .. })).await;
    let Message::Snapshot {
        generation,
        snapshot,
    } = message
    else {
        unreachable!()
    };
    assert_eq!(snapshot.devices[0].name, "radio-a");
    assert_eq!(snapshot.event_sequence, Some(0));
    let kinds = [
        ReadKind::Inspect {
            key: "registered:radio-a".into(),
            name: Some("radio-a".into()),
            devnode: None,
        },
        ReadKind::Check,
        ReadKind::Doctor,
        ReadKind::Logs(devguard_core::LogFilter::default()),
    ];
    for (index, kind) in kinds.into_iter().enumerate() {
        let ticket = index as u64 + 1;
        sender
            .send(Command::Read {
                generation,
                ticket,
                kind,
            })
            .await
            .unwrap();
        let response = receive(
            &mut receiver,
            |m| matches!(m, Message::Reply { ticket: id, .. } if *id == ticket),
        )
        .await;
        assert!(
            matches!(response, Message::Reply { result: Ok(_), .. }),
            "{response:?}"
        );
    }
    Client::connect(paths.socket())
        .unwrap()
        .request(Request::SaveDevice {
            config: config("radio-new"),
            note: None,
        })
        .unwrap();
    let changed = receive(&mut receiver, |m| matches!(m, Message::Event(e) if matches!(&e.event, Event::ConfigChanged { name, .. } if name == "radio-new"))).await;
    let Message::Event(changed) = changed else {
        unreachable!()
    };
    let message = receive(&mut receiver, |m| matches!(m, Message::Snapshot { snapshot, .. } if snapshot.devices.iter().any(|d| d.name == "radio-new"))).await;
    assert!(
        matches!(message, Message::Snapshot { snapshot, .. } if snapshot.event_sequence == Some(changed.seq))
    );
    drop(worker);
    // Closing the GUI worker never sends shutdown or touches device settings.
    let mut client = Client::connect(paths.socket()).unwrap();
    client.request(Request::List).unwrap();
    let log: LogResponse = client
        .request(Request::LogQuery {
            filter: Default::default(),
        })
        .unwrap()
        .into_data()
        .unwrap();
    assert!(!log
        .query
        .entries
        .iter()
        .any(|e| matches!(e.event.as_str(), "service_stopping" | "applied")));
}

#[tokio::test]
async fn a_known_clean_stop_does_not_reconnect_until_explicitly_requested() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::rooted(dir.path());
    let mut daemon = TestDaemon::start(paths.clone(), AccessPolicy::sandbox());
    let (updates, mut receiver) = mpsc::channel(32);
    let worker = start(paths.socket(), updates).unwrap();
    receive(&mut receiver, |m| matches!(m, Message::Snapshot { .. })).await;
    Client::connect(paths.socket())
        .unwrap()
        .request(Request::Shutdown {
            release_blocked: true,
            note: None,
        })
        .unwrap();
    receive(&mut receiver, |m| {
        matches!(m, Message::Connection(Connection::Stopped))
    })
    .await;
    daemon.stop();
    let _restarted = TestDaemon::start(paths.clone(), AccessPolicy::sandbox());
    assert!(
        tokio::time::timeout(Duration::from_millis(150), receiver.recv())
            .await
            .is_err(),
        "worker reconnected after a known stop"
    );
    worker.sender().send(Command::Reconnect).await.unwrap();
    receive(
        &mut receiver,
        |m| matches!(m, Message::Snapshot { generation, .. } if *generation > 1),
    )
    .await;
}

#[tokio::test]
async fn unexpected_disconnect_reconnects_to_a_new_daemon_and_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::rooted(dir.path());
    let mut daemon = TestDaemon::start(paths.clone(), AccessPolicy::sandbox());
    let (updates, mut receiver) = mpsc::channel(32);
    let _worker = start(paths.socket(), updates).unwrap();
    receive(&mut receiver, |m| matches!(m, Message::Snapshot { .. })).await;
    daemon.stop();
    receive(&mut receiver, |m| {
        matches!(m, Message::Connection(Connection::Reconnecting { .. }))
    })
    .await;
    seed(&paths);
    let _restarted = TestDaemon::start(paths.clone(), AccessPolicy::sandbox());
    receive(&mut receiver, |m| matches!(m, Message::Snapshot { generation, snapshot } if *generation > 1 && !snapshot.devices.is_empty())).await;
}

#[tokio::test]
async fn read_authorization_denial_is_not_a_successful_empty_inventory() {
    assert_ne!(
        unsafe { libc::geteuid() },
        0,
        "run GUI tests as an ordinary user"
    );
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::rooted(dir.path());
    let _daemon = TestDaemon::start(paths.clone(), AccessPolicy::System { gid: u32::MAX });
    let (updates, mut receiver) = mpsc::channel(32);
    let _worker = start(paths.socket(), updates).unwrap();
    let failure = receive(&mut receiver, |m| {
        matches!(m, Message::Connection(Connection::Unavailable(_)))
    })
    .await;
    assert!(
        matches!(failure, Message::Connection(Connection::Unavailable(f)) if f.code == "unauthorized" && !f.retryable)
    );
}

#[tokio::test]
async fn framed_transport_rejects_truncation_non_utf8_and_oversized_responses() {
    for bytes in [
        b"no newline".to_vec(),
        vec![255, b'\n'],
        vec![b'x'; MAX_RESPONSE_BYTES + 1],
    ] {
        let (mut writer, reader) = tokio::io::duplex(bytes.len() + 1);
        writer.write_all(&bytes).await.unwrap();
        drop(writer);
        assert!(frame(&mut BufReader::new(reader)).await.is_err());
    }
}

#[tokio::test]
async fn mismatched_response_id_is_rejected_before_payload_use() {
    let (client, server) = UnixStream::pair().unwrap();
    let mut server = BufReader::new(server);
    let task = tokio::spawn(async move {
        frame(&mut server).await.unwrap();
        server
            .get_mut()
            .write_all(b"{\"v\":1,\"id\":99,\"ok\":true,\"data\":{}}\n")
            .await
            .unwrap();
    });
    assert_eq!(
        exchange(&mut BufReader::new(client), 1, Request::List)
            .await
            .unwrap_err()
            .code,
        "response_invalid"
    );
    task.await.unwrap();
}

#[derive(Clone, PartialEq, nsh_core::prelude::Props)]
struct RealWorkspaceProps {
    /// Private test socket; never uses the installed service.
    socket: nsh_core::prelude::Prop<String>,
}

#[nsh_core::prelude::component]
fn RealWorkspace(props: RealWorkspaceProps) -> impl nsh_core::prelude::IntoElement {
    crate::ui::tokens::install();
    crate::live::shell_with(super::super::Workspace::connect(
        props.socket.get_untracked().into(),
    ))
}

#[test]
fn real_daemon_changes_reach_the_headless_gui_and_closing_it_leaves_daemon_running(
) -> Result<(), String> {
    use nsh_core::{
        platform::{Idiom, Os},
        prelude::*,
    };
    use nsh_engine::{EngineConfig, Environment, Root};
    use nsh_testing::Headless;
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::rooted(dir.path());
    seed(&paths);
    let _daemon = TestDaemon::start(paths.clone(), AccessPolicy::sandbox());
    let socket = paths.socket().to_string_lossy().to_string();
    let viewport = Size::new(px(1120.0), px(800.0));
    let mut app = Headless::start(EngineConfig {
        viewport,
        environment: Environment {
            os: Os::Linux,
            idiom: Idiom::Desktop,
            window: viewport,
            ..Environment::default()
        },
        ..EngineConfig::new(Root::new::<RealWorkspace>(move || RealWorkspaceProps {
            socket: socket.clone().into(),
        }))
    })
    .map_err(|e| e.to_string())?;
    app.wait_for("Running · audit · no blocking")?;
    app.wait_for("role:button[radio-a]")?;
    app.press("role:button[radio-a]")?;
    app.wait_for(r"text:/^\/dev\/devguard\/radio-a$/")?;
    Client::connect(paths.socket())
        .unwrap()
        .request(Request::SaveDevice {
            config: config("radio-new"),
            note: Some("real-gui-test".into()),
        })
        .unwrap();
    app.wait_for("role:button[radio-new]")?;
    app.press("role:tab[Logs]")?;
    app.wait_for("role:button[config_changed]")?;
    app.press("role:button[Copy JSONL]")?;
    assert!(app
        .screen()
        .clipboard()
        .unwrap_or_default()
        .contains("real-gui-test"));
    drop(app);
    Client::connect(paths.socket())
        .unwrap()
        .request(Request::ServiceState)
        .unwrap();
    Ok(())
}
