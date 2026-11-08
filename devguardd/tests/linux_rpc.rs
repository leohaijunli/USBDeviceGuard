#![cfg(target_os = "linux")]

//! Real Unix sockets and storage; the external USB bus is an inert fixture.

use devguard_core::client::Client;
use devguard_core::protocol::LogResponse;
use devguard_core::{CheckReport, DeviceConfig, DeviceId, DeviceKind, DoctorItem, IdRole, LogFilter, Paths, Request, UdevFacts};
use devguardd::io::{ApplyOutcome, DeviceIo};
use devguardd::linux::{AccessPolicy, Server};
use tokio::sync::oneshot;

#[derive(Default)]
struct NoUsb { devices: Vec<UdevFacts>, inventory_error: Option<String>, rules_error: Option<String> }

impl DeviceIo for NoUsb {
    fn enumerate(&self) -> Result<Vec<UdevFacts>, String> {
        match &self.inventory_error { Some(error) => Err(error.clone()), None => Ok(self.devices.clone()) }
    }
    fn facts_for_devnode(&self, devnode: &str) -> Result<Option<UdevFacts>, String> { Ok(self.enumerate()?.into_iter().find(|device| device.devnode == devnode)) }
    fn apply(&mut self, _config: &DeviceConfig, _facts: &UdevFacts) -> Result<ApplyOutcome, String> { Err("no USB bus in this test".into()) }
    fn check(&self, config: &DeviceConfig, _facts: &UdevFacts) -> CheckReport { CheckReport::new(Some(config.name.clone()), Vec::new()) }
    fn set_authorized(&mut self, _path: &str, _authorize: bool) -> Result<(), String> { Err("no USB bus in this test".into()) }
    fn write_rules(&mut self, _devices: &[DeviceConfig]) -> Result<(), String> {
        match &self.rules_error { Some(error) => Err(error.clone()), None => Ok(()) }
    }
    fn doctor_probe(&self) -> Vec<DoctorItem> { Vec::new() }
}

fn config() -> DeviceConfig {
    DeviceConfig {
        name: "radio-1".into(), kind: DeviceKind::Radio, description: None,
        ids: vec![DeviceId { vid: "1a86".into(), pid: "7523".into(), serial: None, path: Some("pci-port-1".into()), role: IdRole::App }],
        serial: DeviceKind::Radio.default_serial(), access: Default::default(),
    }
}

/// Construct the inode of an abandoned socket without ever opening a
/// listening fd in the multithreaded test process. A concurrent fork can
/// briefly inherit a normal bind/drop fixture until the child's exec.
fn abandoned_socket(path: &std::path::Path) {
    use std::os::unix::ffi::OsStrExt;
    let pathname = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    // SAFETY: valid NUL-terminated path in this test's private temporary tree;
    // S_IFSOCK creates no hardware device node and opens no listening fd.
    assert_eq!(unsafe { libc::mknod(pathname.as_ptr(), libc::S_IFSOCK | 0o600, 0) }, 0,
        "could not create abandoned socket fixture: {}", std::io::Error::last_os_error());
    let error = std::os::unix::net::UnixStream::connect(path).unwrap_err();
    assert_eq!(error.raw_os_error(), Some(libc::ECONNREFUSED));
}

struct Harness {
    _dir: tempfile::TempDir,
    paths: Paths,
    stop: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
}

impl Harness {
    fn start(access: AccessPolicy) -> Self {
        Self::start_with(access, NoUsb::default())
    }

    fn start_with(access: AccessPolicy, io: NoUsb) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::rooted(dir.path());
        let server = Server::bind(paths.clone(), access).unwrap();
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(server.serve(io, async { let _ = stopped.await; }));
        Self { _dir: dir, paths, stop, task }
    }

    fn start_with_checker(access: AccessPolicy, io: NoUsb, program: std::path::PathBuf) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::rooted(dir.path());
        let server = Server::bind(paths.clone(), access).unwrap();
        let (stop, stopped) = oneshot::channel();
        let checker = devguardd::linux::Checker::checker_for(program, std::time::Duration::from_secs(5));
        let task = tokio::spawn(server.serve_with_checker(io, async { let _ = stopped.await; }, checker));
        Self { _dir: dir, paths, stop, task }
    }

    async fn finish(self) {
        let _ = self.stop.send(());
        tokio::time::timeout(std::time::Duration::from_secs(5), self.task).await.unwrap().unwrap().unwrap();
    }

    async fn restart(mut self) -> Self {
        let _ = self.stop.send(());
        tokio::time::timeout(std::time::Duration::from_secs(5), self.task).await.unwrap().unwrap().unwrap();
        let server = Server::bind(self.paths.clone(), AccessPolicy::sandbox()).unwrap();
        let (stop, stopped) = oneshot::channel();
        self.stop = stop;
        self.task = tokio::spawn(server.serve(NoUsb::default(), async { let _ = stopped.await; }));
        self
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn saved_configuration_note_is_retrievable_through_the_audit_api() {
    let harness = Harness::start(AccessPolicy::sandbox());
    let socket = harness.paths.socket();
    tokio::task::spawn_blocking(move || {
        let mut client = Client::connect(socket).unwrap();
        client.request(Request::SaveDevice { config: config(), note: Some("registered on the bench".into()) }).unwrap();
        let response = client.request(Request::LogQuery { filter: LogFilter { event: Some("config_changed".into()), ..LogFilter::default() } }).unwrap();
        let log: LogResponse = response.into_data().unwrap();
        assert_eq!(log.query.entries[0].details.get("note").and_then(|value| value.as_str()), Some("registered on the bench"));
    }).await.unwrap();
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn apply_is_routed_to_the_io_layer_instead_of_being_unsupported() {
    use devguard_core::client::ClientError;

    let harness = Harness::start(AccessPolicy::sandbox());
    let socket = harness.paths.socket();
    tokio::task::spawn_blocking(move || {
        let mut client = Client::connect(socket).unwrap();
        // No registered device: the request must reach the decision core and
        // come back as a normal not_found, not the phase-2
        // "request_unavailable" gate.
        let error = client.request(Request::Apply { name: Some("radio-1".into()), devnode: None }).unwrap_err();
        assert!(matches!(error, ClientError::Protocol(error) if error.code == "not_found"));
    }).await.unwrap();
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pause_stays_unavailable_until_its_phase_gate() {
    use devguard_core::client::ClientError;

    let harness = Harness::start(AccessPolicy::sandbox());
    let socket = harness.paths.socket();
    tokio::task::spawn_blocking(move || {
        let mut client = Client::connect(socket).unwrap();
        let error = client.request(Request::Pause { kind: devguard_core::PauseKind::Service, minutes: 5, note: None }).unwrap_err();
        assert!(matches!(error, ClientError::Protocol(error) if error.code == "unsupported"));
    }).await.unwrap();
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_responses_return_a_correlated_actionable_error() {
    use devguard_core::client::ClientError;

    let io = NoUsb { devices: vec![UdevFacts {
        devnode: "/dev/ttyUSB0".into(), subsystem: "tty".into(), vid: "1a86".into(), pid: "7523".into(), is_usb: true,
        model: Some("x".repeat(devguard_core::protocol::MAX_RESPONSE_BYTES + 1)), ..UdevFacts::default()
    }], ..NoUsb::default() };
    let harness = Harness::start_with(AccessPolicy::sandbox(), io);
    let socket = harness.paths.socket();
    tokio::task::spawn_blocking(move || {
        let mut client = Client::connect(socket).unwrap();
        client.request(Request::ServiceState).unwrap();
        let error = client.request(Request::List).unwrap_err();
        assert!(matches!(error, ClientError::Protocol(error) if error.code == "response_too_large" && error.next_step.is_some()));
    }).await.unwrap();
    harness.finish().await;
}

#[tokio::test]
async fn a_private_sandbox_rejects_parent_traversal_and_root_symlinks() {
    use std::fs;
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("nested")).unwrap();
    symlink(dir.path(), dir.path().join("alias")).unwrap();
    for root in [dir.path().join("nested/.."), dir.path().join("alias")] {
        assert!(Server::bind(Paths::rooted(root), AccessPolicy::sandbox()).is_err());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deletion_and_shutdown_notes_survive_a_daemon_restart() {
    let harness = Harness::start(AccessPolicy::sandbox());
    let socket = harness.paths.socket();
    tokio::task::spawn_blocking(move || {
        let mut client = Client::connect(socket).unwrap();
        client.request(Request::SaveDevice { config: config(), note: None }).unwrap();
        client.request(Request::DeleteDevice { name: "radio-1".into(), note: Some("fixture removed".into()) }).unwrap();
        client.request(Request::Shutdown { release_blocked: true, note: Some("test shutdown".into()) }).unwrap();
    }).await.unwrap();
    let harness = harness.restart().await;
    let socket = harness.paths.socket();
    tokio::task::spawn_blocking(move || {
        let mut client = Client::connect(socket).unwrap();
        let log: LogResponse = client.request(Request::LogQuery { filter: LogFilter::default() }).unwrap().into_data().unwrap();
        let notes: Vec<&str> = log.query.entries.iter().filter_map(|entry| entry.details.get("note").and_then(|value| value.as_str())).collect();
        assert!(notes.contains(&"fixture removed") && notes.contains(&"test shutdown"), "{notes:?}");
        let deleted = log.query.entries.iter().find(|entry| entry.details.get("deleted").and_then(|value| value.as_bool()) == Some(true)).unwrap();
        assert!(deleted.before.as_ref().and_then(|value| value.as_str()).unwrap().contains("name = \"radio-1\""));
        assert_eq!(deleted.after.as_ref().and_then(|value| value.as_str()), Some(""));
    }).await.unwrap();
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn startup_does_not_impersonate_a_gui_operator() {
    let harness = Harness::start(AccessPolicy::sandbox());
    let socket = harness.paths.socket();
    tokio::task::spawn_blocking(move || {
        let mut client = Client::connect(socket).unwrap();
        let log: LogResponse = client.request(Request::LogQuery { filter: LogFilter { event: Some("service_started".into()), ..LogFilter::default() } }).unwrap().into_data().unwrap();
        let start = &log.query.entries[0];
        assert!(start.uid.is_none() && start.user.is_none());
        assert_eq!(start.details.get("identity_source").and_then(|value| value.as_str()), Some("daemon_process"));
    }).await.unwrap();
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crud_and_identity_conflicts_are_observable_through_rpc() {
    let harness = Harness::start(AccessPolicy::sandbox());
    let socket = harness.paths.socket();
    tokio::task::spawn_blocking(move || {
        let mut client = Client::connect(socket).unwrap();
        client.request(Request::SaveDevice { config: config(), note: None }).unwrap();
        let show: devguard_core::protocol::ShowResponse = client.request(Request::Show { name: "radio-1".into() }).unwrap().into_data().unwrap();
        assert_eq!(show.device.name, "radio-1");
        let mut duplicate = config();
        duplicate.name = "radio-2".into();
        assert!(client.request(Request::SaveDevice { config: duplicate, note: None }).is_err());
        let list: devguard_core::protocol::ListResponse = client.request(Request::List).unwrap().into_data().unwrap();
        assert_eq!(list.devices.len(), 1);
        client.request(Request::DeleteDevice { name: "radio-1".into(), note: None }).unwrap();
        assert!(client.request(Request::Show { name: "radio-1".into() }).is_err());
    }).await.unwrap();
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn events_are_ordered_and_shutdown_flushes_the_final_event() {
    let harness = Harness::start(AccessPolicy::sandbox());
    let socket = harness.paths.socket();
    tokio::task::spawn_blocking(move || {
        let mut subscriber = Client::connect(&socket).unwrap();
        subscriber.subscribe().unwrap();
        let mut client = Client::connect(socket).unwrap();
        client.request(Request::SaveDevice { config: config(), note: None }).unwrap();
        let first = subscriber.next_event().unwrap().unwrap();
        assert!(matches!(first.event, devguard_core::Event::ConfigChanged { .. }));
        client.request(Request::Shutdown { release_blocked: true, note: None }).unwrap();
        let last = subscriber.next_event().unwrap().unwrap();
        assert!(last.seq > first.seq && matches!(last.event, devguard_core::Event::ServiceStopping { .. }));
        assert!(subscriber.next_event().unwrap().is_none());
    }).await.unwrap();
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_supplied_root_identity_is_ignored_and_denial_is_audited() {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    // An absent required gid, with the chown no-change sentinel in this fixture.
    assert_ne!(unsafe { libc::geteuid() }, 0, "run authorization tests as a normal user");
    let harness = Harness::start(AccessPolicy::System { gid: u32::MAX });
    let socket = harness.paths.socket();
    tokio::task::spawn_blocking(move || {
        let mut stream = UnixStream::connect(socket).unwrap();
        stream.set_read_timeout(Some(std::time::Duration::from_secs(3))).unwrap();
        stream.write_all(b"{\"v\":1,\"id\":7,\"op\":\"list\",\"uid\":0,\"user\":\"root\",\"polkit_authorized\":true}\n").unwrap();
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).unwrap();
        let response = devguard_core::ResponseEnvelope::parse(&line).unwrap();
        assert_eq!(response.id, Some(7));
        assert_eq!(response.error.unwrap().code, "unauthorized");
    }).await.unwrap();
    let harness = harness.restart().await;
    let socket = harness.paths.socket();
    tokio::task::spawn_blocking(move || {
        let mut client = Client::connect(socket).unwrap();
        let log: LogResponse = client.request(Request::LogQuery { filter: LogFilter { event: Some("request_denied".into()), ..LogFilter::default() } }).unwrap().into_data().unwrap();
        assert_eq!(log.query.entries[0].uid, Some(unsafe { libc::geteuid() }));
    }).await.unwrap();
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_polkit_denial_blocks_the_effect_and_audits_the_real_operator() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    assert_ne!(unsafe { libc::geteuid() }, 0, "run authorization tests as a normal user");
    // A checker that denies: the daemon must not save the device, must return
    // unauthorized, and must audit the denial under the true peer uid.
    let dir = tempfile::tempdir().unwrap();
    let checker = dir.path().join("pkcheck-denies");
    fs::write(&checker, "#!/bin/sh\nexit 1\n").unwrap();
    fs::set_permissions(&checker, fs::Permissions::from_mode(0o700)).unwrap();

    let harness = Harness::start_with_checker(AccessPolicy::System { gid: u32::MAX }, NoUsb::default(), checker);
    let socket = harness.paths.socket();
    tokio::task::spawn_blocking(move || {
        let mut client = Client::connect(socket).unwrap();
        let error = client.request(Request::SaveDevice { config: config(), note: None }).unwrap_err();
        assert!(matches!(error, devguard_core::client::ClientError::Protocol(ref e) if e.code == "unauthorized"), "{error}");
    }).await.unwrap();
    let harness = harness.restart().await;
    let socket = harness.paths.socket();
    tokio::task::spawn_blocking(move || {
        let mut client = Client::connect(socket).unwrap();
        let log: LogResponse = client.request(Request::LogQuery { filter: LogFilter { event: Some("request_denied".into()), ..LogFilter::default() } }).unwrap().into_data().unwrap();
        let entry = log.query.entries[0].clone();
        assert_eq!(entry.uid, Some(unsafe { libc::geteuid() }));
        assert_eq!(entry.details.get("op").and_then(|value| value.as_str()), Some("save_device"));
        assert_eq!(entry.details.get("action").and_then(|value| value.as_str()), Some("dev.devguard.modify"));
    }).await.unwrap();
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_polkit_grant_allows_a_mutation_and_audits_the_authorized_operator() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    assert_ne!(unsafe { libc::geteuid() }, 0, "run authorization tests as a normal user");
    let dir = tempfile::tempdir().unwrap();
    let checker = dir.path().join("pkcheck-grants");
    fs::write(&checker, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&checker, fs::Permissions::from_mode(0o700)).unwrap();

    let harness = Harness::start_with_checker(AccessPolicy::System { gid: u32::MAX }, NoUsb::default(), checker);
    let socket = harness.paths.socket();
    tokio::task::spawn_blocking(move || {
        let mut client = Client::connect(socket).unwrap();
        // With a positive polkit decision the device is saved (not denied),
        // reaching the decision core and persisting the config.
        let result = client.request(Request::SaveDevice { config: config(), note: Some("polkit-grant".into()) });
        if let Err(e) = result {
            panic!("grant should have been authorized, got: {e}");
        }
    }).await.unwrap();
    let harness = harness.restart().await;
    let socket = harness.paths.socket();
    tokio::task::spawn_blocking(move || {
        let mut client = Client::connect(socket).unwrap();
        let log: LogResponse = client.request(Request::LogQuery { filter: LogFilter { event: Some("config_changed".into()), ..LogFilter::default() } }).unwrap().into_data().unwrap();
        let entry = log.query.entries[0].clone();
        assert_eq!(entry.uid, Some(unsafe { libc::geteuid() }));
        assert_eq!(entry.details.get("note").and_then(|value| value.as_str()), Some("polkit-grant"));
    }).await.unwrap();
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stale_socket_is_taken_over_but_a_live_daemon_never_is() {

    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::rooted(dir.path());
    paths.create_dirs().unwrap();

    // A socket file left behind by a killed daemon: exists, nobody listens.
    abandoned_socket(&paths.socket());
    assert!(paths.socket().exists(), "the stale socket file must still exist");

    // The restarted daemon takes it over instead of crash-looping.
    let restarted = Server::bind(paths.clone(), AccessPolicy::sandbox());
    assert!(restarted.is_ok(), "stale socket takeover failed: {:?}", restarted.err());

    // A second bind while the first daemon is alive must still be refused.
    let second = Server::bind(paths, AccessPolicy::sandbox());
    assert!(second.is_err(), "a live daemon's socket must not be stolen");
}

#[tokio::test]
async fn a_listener_inherited_by_a_child_is_not_an_abandoned_socket() {
    use std::os::{fd::OwnedFd, unix::net::UnixListener as StdListener};
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::rooted(dir.path());
    paths.create_dirs().unwrap();
    let listener: OwnedFd = StdListener::bind(paths.socket()).unwrap().into();
    let mut child = tokio::process::Command::new("/bin/sleep").arg("60")
        .stdin(std::process::Stdio::from(listener)).kill_on_drop(true).spawn().unwrap();
    // The parent has closed its handle, but fd 0 still listens in the child.
    let error = Server::bind(paths.clone(), AccessPolicy::sandbox()).err().expect("live child listener must be preserved");
    assert_eq!(error.kind(), std::io::ErrorKind::AddrInUse);
    assert!(paths.socket().exists());
    child.start_kill().unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), child.wait()).await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn snapshots_expose_the_worker_event_barrier_without_breaking_cli_responses() {
    let harness = Harness::start(AccessPolicy::sandbox());
    let socket = harness.paths.socket();
    tokio::task::spawn_blocking(move || {
        let mut client = Client::connect(socket).unwrap();
        let before: devguard_core::protocol::ListResponse = client.request(Request::List).unwrap().into_data().unwrap();
        assert_eq!(before.event_sequence, Some(0));
        client.request(Request::SaveDevice { config: config(), note: None }).unwrap();
        let after: devguard_core::protocol::ListResponse = client.request(Request::List).unwrap().into_data().unwrap();
        assert_eq!(after.event_sequence, Some(1));
        assert_eq!(after.devices[0].name, "radio-1");
    }).await.unwrap();
    harness.finish().await;
}

#[tokio::test]
async fn startup_does_not_follow_or_replace_an_untrusted_instance_lock() {
    use std::fs;
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::rooted(dir.path());
    paths.create_dirs().unwrap();
    let other = dir.path().join("preserve-me");
    fs::write(&other, b"user-owned data").unwrap();
    let lock = paths.run.join("daemon.lock");
    symlink(&other, &lock).unwrap();

    assert!(Server::bind(paths.clone(), AccessPolicy::sandbox()).is_err());
    assert!(fs::symlink_metadata(&lock).unwrap().file_type().is_symlink());
    assert_eq!(fs::read(&other).unwrap(), b"user-owned data");
    fs::remove_file(&lock).unwrap();
    fs::hard_link(&other, &lock).unwrap();
    assert!(Server::bind(paths, AccessPolicy::sandbox()).is_err());
    assert_eq!(fs::read(&other).unwrap(), b"user-owned data");
}

#[tokio::test]
async fn occupied_non_socket_paths_are_preserved() {
    use std::fs;
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::rooted(dir.path());
    paths.create_dirs().unwrap();
    fs::write(paths.socket(), b"keep this file").unwrap();
    assert!(Server::bind(paths.clone(), AccessPolicy::sandbox()).is_err());
    assert_eq!(fs::read(paths.socket()).unwrap(), b"keep this file");

    fs::remove_file(paths.socket()).unwrap();
    symlink(dir.path().join("absent"), paths.socket()).unwrap();
    assert!(Server::bind(paths.clone(), AccessPolicy::sandbox()).is_err());
    assert!(fs::symlink_metadata(paths.socket()).unwrap().file_type().is_symlink());
}

#[tokio::test]
async fn socket_recovery_does_not_remove_a_hard_link_to_another_socket() {
    use std::fs;
    use std::os::unix::net::UnixListener as StdListener;

    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::rooted(dir.path());
    paths.create_dirs().unwrap();
    let other_path = dir.path().join("preserve-socket");
    drop(StdListener::bind(&other_path).unwrap());
    fs::hard_link(&other_path, paths.socket()).unwrap();
    assert!(Server::bind(paths.clone(), AccessPolicy::sandbox()).is_err());
    assert!(paths.socket().exists() && other_path.exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_concurrent_restarts_cannot_steal_each_others_socket() {
    use std::sync::{Arc, Barrier};

    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::rooted(dir.path());
    paths.create_dirs().unwrap();
    abandoned_socket(&paths.socket());
    let barrier = Arc::new(Barrier::new(2));
    let contenders: Vec<_> = (0..2).map(|_| {
        let paths = paths.clone();
        let barrier = barrier.clone();
        tokio::task::spawn_blocking(move || {
            barrier.wait();
            Server::bind(paths, AccessPolicy::sandbox())
        })
    }).collect();
    // Keep the winning server alive while the other contender finishes.
    let mut winners = Vec::new();
    for contender in contenders {
        if let Ok(server) = contender.await.unwrap() { winners.push(server); }
    }
    assert_eq!(winners.len(), 1);
    let mut client = Client::connect(paths.socket()).unwrap();
    let (stop, stopped) = oneshot::channel();
    let server = winners.pop().unwrap();
    let task = tokio::spawn(server.serve(NoUsb::default(), async { let _ = stopped.await; }));
    tokio::task::spawn_blocking(move || client.request(Request::ServiceState).unwrap()).await.unwrap();
    stop.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), task).await.unwrap().unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enumeration_and_rule_failures_are_not_reported_as_success() {
    use devguard_core::client::ClientError;
    let io = NoUsb { inventory_error: Some("fixture enumeration failed".into()), rules_error: Some("fixture reload failed".into()), ..NoUsb::default() };
    let harness = Harness::start_with(AccessPolicy::sandbox(), io);
    let socket = harness.paths.socket();
    tokio::task::spawn_blocking(move || {
        let mut client = Client::connect(socket).unwrap();
        // List is the diagnostics surface: a failed enumeration must show up
        // as a problem in an otherwise ok response, never as a silent
        // empty bus and never as an opaque transport error.
        let list: devguard_core::protocol::ListResponse = client.request(Request::List).unwrap().into_data().unwrap();
        assert!(list.online.is_empty());
        assert!(
            list.problems.iter().any(|problem| problem.severity == devguard_core::Severity::Error && problem.message.contains("fixture enumeration failed")),
            "{:?}",
            list.problems
        );
        let error = client.request(Request::SaveDevice { config: config(), note: None }).unwrap_err();
        assert!(matches!(error, ClientError::Protocol(error) if error.code == "rules_failed" && error.message.contains("configuration change is saved")));
        let log: LogResponse = client.request(Request::LogQuery { filter: LogFilter { event: Some("config_changed".into()), ..LogFilter::default() } }).unwrap().into_data().unwrap();
        assert_eq!(log.query.entries.len(), 1);
        assert!(matches!(client.request(Request::PolicySet { mode: devguard_core::PolicyMode::Enforce, note: None }), Err(ClientError::Protocol(error)) if error.code == "unsupported"));
    }).await.unwrap();
    harness.finish().await;
}
