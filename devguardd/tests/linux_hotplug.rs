#![cfg(target_os = "linux")]

//! Udev input and the external bus are isolated; observations use the real socket.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use devguard_core::{CheckReport, DeviceConfig, DoctorItem, Event, EventMessage, Paths, Request, RequestEnvelope, ResponseEnvelope, UdevFacts};
use devguardd::io::{ApplyOutcome, DeviceIo};
use devguardd::linux::{monitor::UdevMonitor, server::read_frame, AccessPolicy, Server};
use tokio::io::{AsyncWriteExt, BufReader, DuplexStream};
use tokio::net::{unix::OwnedReadHalf, UnixStream};
use tokio::sync::oneshot;

#[derive(Clone, Default)]
struct Bus(Arc<Mutex<Vec<UdevFacts>>>);

impl DeviceIo for Bus {
    fn enumerate(&self) -> Result<Vec<UdevFacts>, String> { Ok(self.0.lock().unwrap().clone()) }
    fn facts_for_devnode(&self, devnode: &str) -> Result<Option<UdevFacts>, String> { Ok(self.enumerate()?.into_iter().find(|device| device.devnode == devnode)) }
    fn apply(&mut self, _config: &DeviceConfig, _facts: &UdevFacts) -> Result<ApplyOutcome, String> { Err("fixture forbids device writes".into()) }
    fn check(&self, config: &DeviceConfig, _facts: &UdevFacts) -> CheckReport { CheckReport::new(Some(config.name.clone()), Vec::new()) }
    fn set_authorized(&mut self, _path: &str, _authorize: bool) -> Result<(), String> { Err("fixture forbids authorization writes".into()) }
    fn write_rules(&mut self, _devices: &[DeviceConfig]) -> Result<(), String> { Err("fixture forbids rules writes".into()) }
    fn doctor_probe(&self) -> Vec<DoctorItem> { Vec::new() }
}

fn device(node: &str) -> UdevFacts {
    UdevFacts { devnode: node.into(), subsystem: "tty".into(), vid: "1a86".into(), pid: "7523".into(),
        serial_short: Some("HOTPLUG-FIXTURE".into()), usb_path: Some("/devices/pci0/usb1/1-2".into()),
        is_usb: true, driver: Some("ch341-uart".into()), ..UdevFacts::default() }
}

struct Harness {
    _dir: tempfile::TempDir,
    paths: Paths,
    bus: Bus,
    input: DuplexStream,
    stop: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
}

impl Harness {
    async fn start() -> Self {
        Self::start_with(Vec::new()).await
    }

    async fn start_with(initial: Vec<UdevFacts>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::rooted(dir.path());
        let bus = Bus(Arc::new(Mutex::new(initial)));
        let (mut input, source) = tokio::io::duplex(128 * 1024);
        input.write_all(b"monitor will print the received events for:\nUDEV - the event which udev sends out after rule processing\n\n").await.unwrap();
        let monitor = UdevMonitor::from_reader(BufReader::new(source)).await.unwrap();
        let server = Server::bind(paths.clone(), AccessPolicy::sandbox()).unwrap();
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(server.serve_monitored(bus.clone(), monitor, async { let _ = stopped.await; }));
        Self { _dir: dir, paths, bus, input, stop, task }
    }

    async fn subscribe(&self) -> BufReader<OwnedReadHalf> {
        let stream = UnixStream::connect(self.paths.socket()).await.unwrap();
        let (read, mut write) = stream.into_split();
        let mut line = serde_json::to_vec(&RequestEnvelope::new(1, Request::SubscribeEvents)).unwrap();
        line.push(b'\n');
        write.write_all(&line).await.unwrap();
        let mut read = BufReader::new(read);
        let response = tokio::time::timeout(Duration::from_secs(3), read_frame(&mut read)).await.unwrap().unwrap().unwrap();
        assert!(ResponseEnvelope::parse(&response).unwrap().ok);
        read
    }

    async fn hint(&mut self, action: &str, node: &str, seq: u64) {
        let name = node.strip_prefix("/dev/").unwrap();
        let path = format!("/devices/pci0/usb1/1-2/{name}");
        let text = format!("UDEV  [123.456789] {action} {path} (tty)\nACTION={action}\nDEVPATH={path}\nSUBSYSTEM=tty\nDEVNAME={node}\nSEQNUM={seq}\n\n");
        self.input.write_all(text.as_bytes()).await.unwrap();
    }

    async fn finish(self) {
        let _ = self.stop.send(());
        tokio::time::timeout(Duration::from_secs(5), self.task).await.unwrap().unwrap().unwrap();
    }
}

async fn next_event(reader: &mut BufReader<OwnedReadHalf>) -> EventMessage {
    let line = tokio::time::timeout(Duration::from_secs(3), read_frame(reader)).await
        .expect("hotplug event was not delivered").unwrap().unwrap();
    serde_json::from_str(&line).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_udev_add_reaches_the_socket_subscriber() {
    let mut harness = Harness::start().await;
    let mut subscriber = harness.subscribe().await;
    *harness.bus.0.lock().unwrap() = vec![device("/dev/ttyUSB0")];
    harness.hint("add", "/dev/ttyUSB0", 41).await;
    assert!(matches!(next_event(&mut subscriber).await.event, Event::DeviceAdded { device } if device.devnode == "/dev/ttyUSB0"));
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn duplicate_hints_do_not_duplicate_added_events() {
    let mut harness = Harness::start().await;
    let mut subscriber = harness.subscribe().await;
    *harness.bus.0.lock().unwrap() = vec![device("/dev/ttyUSB0")];
    harness.hint("add", "/dev/ttyUSB0", 41).await;
    let added = next_event(&mut subscriber).await;
    assert!(matches!(added.event, Event::DeviceAdded { .. }));
    harness.hint("add", "/dev/ttyUSB0", 41).await;
    harness.hint("change", "/dev/ttyUSB0", 42).await;
    assert!(tokio::time::timeout(Duration::from_millis(100), read_frame(&mut subscriber)).await.is_err());
    harness.bus.0.lock().unwrap().clear();
    harness.hint("remove", "/dev/ttyUSB0", 43).await;
    let removed = next_event(&mut subscriber).await;
    assert!(removed.seq > added.seq && matches!(removed.event, Event::DeviceRemoved { devnode, .. } if devnode == "/dev/ttyUSB0"));
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tty_renumbering_removes_the_old_node_before_adding_the_new_one() {
    let mut harness = Harness::start().await;
    let mut subscriber = harness.subscribe().await;
    *harness.bus.0.lock().unwrap() = vec![device("/dev/ttyUSB0")];
    harness.hint("add", "/dev/ttyUSB0", 41).await;
    next_event(&mut subscriber).await;
    *harness.bus.0.lock().unwrap() = vec![device("/dev/ttyUSB7")];
    harness.hint("add", "/dev/ttyUSB7", 43).await;
    assert!(matches!(next_event(&mut subscriber).await.event, Event::DeviceRemoved { devnode, .. } if devnode == "/dev/ttyUSB0"));
    assert!(matches!(next_event(&mut subscriber).await.event, Event::DeviceAdded { device } if device.devnode == "/dev/ttyUSB7"));
    // A late remove hint cannot remove the current, freshly enumerated node.
    harness.hint("remove", "/dev/ttyUSB0", 42).await;
    assert!(tokio::time::timeout(Duration::from_millis(100), read_frame(&mut subscriber)).await.is_err());
    let socket = harness.paths.socket();
    let list: devguard_core::protocol::ListResponse = tokio::task::spawn_blocking(move || {
        devguard_core::client::Client::connect(socket).unwrap().request(Request::List).unwrap().into_data().unwrap()
    }).await.unwrap();
    assert_eq!(list.online.iter().map(|device| device.devnode.as_str()).collect::<Vec<_>>(), vec!["/dev/ttyUSB7"]);
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_initial_snapshot_is_not_replayed_as_a_hotplug_add() {
    let mut harness = Harness::start_with(vec![device("/dev/ttyUSB0")]).await;
    let mut subscriber = harness.subscribe().await;
    harness.hint("change", "/dev/ttyUSB0", 41).await;
    assert!(tokio::time::timeout(Duration::from_millis(100), read_frame(&mut subscriber)).await.is_err());
    harness.bus.0.lock().unwrap().clear();
    harness.hint("remove", "/dev/ttyUSB0", 42).await;
    assert!(matches!(next_event(&mut subscriber).await.event, Event::DeviceRemoved { devnode, .. } if devnode == "/dev/ttyUSB0"));
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn monitor_eof_stops_the_daemon_and_records_the_failure() {
    let mut harness = Harness::start().await;
    let mut subscriber = harness.subscribe().await;
    harness.input.shutdown().await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), &mut harness.task).await.unwrap().unwrap();
    assert!(result.is_err());
    assert!(tokio::time::timeout(Duration::from_secs(3), read_frame(&mut subscriber)).await.unwrap().unwrap().is_none());
    // Read the durable record through the same public RPC after restart.
    let server = Server::bind(harness.paths.clone(), AccessPolicy::sandbox()).unwrap();
    let (stop, stopped) = oneshot::channel();
    let task = tokio::spawn(server.serve(harness.bus.clone(), async { let _ = stopped.await; }));
    let socket = harness.paths.socket();
    let log: devguard_core::protocol::LogResponse = tokio::task::spawn_blocking(move || {
        devguard_core::client::Client::connect(socket).unwrap().request(Request::LogQuery {
            filter: devguard_core::LogFilter { event: Some("monitor_failed".into()), ..devguard_core::LogFilter::default() },
        }).unwrap().into_data().unwrap()
    }).await.unwrap();
    assert_eq!(log.query.entries.len(), 1);
    assert!(log.query.entries[0].details.get("reason").unwrap().as_str().unwrap().contains("output closed"));
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), task).await.unwrap().unwrap().unwrap();
}
