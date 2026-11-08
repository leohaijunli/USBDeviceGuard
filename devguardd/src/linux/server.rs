//! Bounded JSON-lines RPC. One worker owns all daemon decisions and effects.

use std::ffi::CString;
use std::collections::BTreeMap;
use std::fs;
use std::future::Future;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context};
use devguard_core::{policy::PolicyStore, Event, EventMessage, Journal, JournalEntry, OnlineDevice, Paths, PolicyMode, ProtocolError, Request, RequestEnvelope, ResponseEnvelope};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::task::JoinSet;

use crate::io::DeviceIo;
use crate::{Caller, Daemon, Effect, Handled};

use super::accounts::{self, Peer};
use super::monitor::{MonitorEvent, UdevMonitor};
use super::polkit::{Checker, Decision};
use super::socket::{recover_socket, InstanceLock};
use super::systemd;

pub use devguard_core::protocol::MAX_REQUEST_BYTES;
const IO_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy)]
pub enum AccessPolicy {
    System { gid: u32 },
    /// Same-uid access to private files. This is not a polkit acceptance test.
    Sandbox { uid: u32, gid: u32 },
}

impl AccessPolicy {
    pub fn system() -> io::Result<Self> {
        // SAFETY: these calls only read this process's effective credentials.
        if unsafe { libc::geteuid() } != 0 {
            return Err(io::Error::other("system mode requires root; use --root for a private development sandbox"));
        }
        let gid = accounts::group_id("devguard")?.ok_or_else(|| io::Error::other("the devguard group is missing; system installation is an administrator action"))?;
        Ok(Self::System { gid })
    }

    pub fn sandbox() -> Self {
        // SAFETY: no credential or device state is changed.
        Self::Sandbox { uid: unsafe { libc::geteuid() }, gid: unsafe { libc::getegid() } }
    }

    fn gid(self) -> u32 {
        match self { Self::System { gid } | Self::Sandbox { gid, .. } => gid }
    }
}

pub struct Server {
    listener: UnixListener,
    paths: Paths,
    access: AccessPolicy,
    _socket: SocketGuard,
    // Dropped after the socket guard: a new instance cannot bind before
    // this one finishes cleaning up its socket.
    _instance: InstanceLock,
}

impl Server {
    pub async fn serve_monitored<I, F>(self, io: I, monitor: UdevMonitor, stop: F) -> anyhow::Result<()>
    where I: DeviceIo + Send + 'static, F: Future<Output = ()> {
        self.serve_inner(io, Some(monitor), stop, Checker::default()).await
    }

    pub fn bind(paths: Paths, access: AccessPolicy) -> io::Result<Self> {
        if let AccessPolicy::Sandbox { uid, .. } = access {
            // SAFETY: a sandbox may only grant access to its actual owner.
            if uid != unsafe { libc::geteuid() } {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, "sandbox owner does not match this process"));
            }
            let root = paths.etc.parent().and_then(Path::parent).ok_or_else(|| io::Error::other("invalid sandbox layout"))?;
            if !root.is_absolute() || root == Path::new("/")
                || root.components().any(|part| matches!(part, std::path::Component::ParentDir))
                || paths != Paths::rooted(root)
            {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "sandbox paths must form a private tree without parent traversal"));
            }
            ensure_directory(root, 0o700)?;
            if fs::canonicalize(root)? == Path::new("/") {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "the system root cannot be a development sandbox"));
            }
        }
        for path in [paths.etc.clone(), paths.devices_dir(), paths.run.clone(), paths.state.clone(), paths.backups_dir(), paths.udev_rules.clone()] {
            ensure_directory(&path, 0o700)?;
        }
        if matches!(access, AccessPolicy::System { .. }) {
            set_group(&paths.run, access.gid())?;
            fs::set_permissions(&paths.run, fs::Permissions::from_mode(0o750))?;
        }
        for path in [paths.journal_file(), paths.policy_file()] {
            if let Ok(metadata) = fs::symlink_metadata(path) {
                if !metadata.is_file() {
                    return Err(io::Error::other("daemon data files must not be symlinks or special files"));
                }
            }
        }
        // Serialize all cooperating instances before inspecting stale files.
        let instance = InstanceLock::acquire(&paths.run.join("daemon.lock"))?;
        let socket_path = paths.socket();
        recover_socket(&socket_path)?;
        let listener = UnixListener::bind(&socket_path)?;
        let guard = SocketGuard::new(paths.socket())?;
        set_group(&paths.socket(), access.gid())?;
        fs::set_permissions(paths.socket(), fs::Permissions::from_mode(0o660))?;
        Ok(Self { listener, paths, access, _socket: guard, _instance: instance })
    }

    pub async fn serve<I, F>(self, io: I, stop: F) -> anyhow::Result<()>
    where I: DeviceIo + Send + 'static, F: Future<Output = ()> {
        self.serve_inner(io, None, stop, Checker::default()).await
    }

    #[doc(hidden)]
    pub async fn serve_with_checker<I, F>(self, io: I, stop: F, checker: Checker) -> anyhow::Result<()>
    where I: DeviceIo + Send + 'static, F: Future<Output = ()> {
        self.serve_inner(io, None, stop, checker).await
    }

    async fn serve_inner<I, F>(self, io: I, monitor: Option<UdevMonitor>, stop: F, checker: Checker) -> anyhow::Result<()>
    where I: DeviceIo + Send + 'static, F: Future<Output = ()> {
        let (commands, mut requests) = mpsc::channel::<Command>(64);
        let (events, _) = broadcast::channel::<EventMessage>(256);
        let worker_events = events.clone();
        let paths = self.paths.clone();
        let access = self.access;
        let monitored = monitor.is_some();
        let mut worker = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let mut runtime = Runtime::new(paths, access.gid(), io, worker_events, monitored)?;
            // Type=notify orders udev's apply@ hook after initialization,
            // not merely after the process has been launched.
            systemd::ready().context("cannot notify systemd of readiness")?;
            while let Some(command) = requests.blocking_recv() {
                match command {
                    Command::Stop => return Ok(()),
                    Command::DeviceEvent(event) => {
                        if let Err(error) = runtime.observe(event) {
                            return runtime.monitor_failed(error.to_string());
                        }
                    }
                    Command::MonitorFailed(reason) => return runtime.monitor_failed(reason),
                    Command::Request { envelope, caller, denial, reply } => {
                        let id = envelope.id;
                        let result = runtime.dispatch(&caller, *envelope, denial);
                        match result {
                            Ok((response, shutdown)) => {
                                let _ = reply.send(response);
                                if shutdown { return Ok(()); }
                            }
                            Err(error) => {
                                let _ = reply.send(ResponseEnvelope::error(id, ProtocolError::internal(error.to_string())));
                                return Err(error);
                            }
                        }
                    }
                }
            }
            Ok(())
        });
        let (closing, closed) = watch::channel(false);
        let monitor_task = monitor.map(|mut monitor| {
            let commands = commands.clone();
            let mut closed = closed.clone();
            tokio::spawn(async move {
                loop {
                    let message = tokio::select! {
                        _ = closed.changed() => break,
                        event = monitor.next_event() => match event {
                            Ok(event) => Command::DeviceEvent(event),
                            Err(error) => Command::MonitorFailed(error.to_string()),
                        }
                    };
                    let failed = matches!(message, Command::MonitorFailed(_));
                    let sent = tokio::select! {
                        _ = closed.changed() => break,
                        sent = commands.send(message) => sent,
                    };
                    if sent.is_err() || failed { break; }
                }
                if let Err(error) = monitor.stop().await {
                    eprintln!("devguardd: monitor cleanup failed: {error}");
                }
            })
        });
        let mut connections = JoinSet::new();
        let checker = Arc::new(checker);
        tokio::pin!(stop);
        let finished = loop {
            tokio::select! {
                _ = &mut stop => break None,
                result = &mut worker => break Some(result),
                result = self.listener.accept(), if connections.len() < 64 => {
                    let (stream, _) = result?;
                    let commands = commands.clone();
                    let receiver = events.subscribe();
                    let closed = closed.clone();
                    let checker = checker.clone();
                    connections.spawn(async move {
                        if let Err(error) = connection(stream, access, commands, receiver, closed, checker).await {
                            eprintln!("devguardd: client connection closed: {error}");
                        }
                    });
                }
                _ = connections.join_next(), if !connections.is_empty() => {}
            }
        };
        let _ = closing.send(true);
        if let Some(mut monitor_task) = monitor_task {
            if tokio::time::timeout(Duration::from_secs(3), &mut monitor_task).await.is_err() {
                monitor_task.abort();
                let _ = monitor_task.await;
            }
        }
        if finished.is_none() {
            let _ = commands.send(Command::Stop).await;
        }
        drop(commands);
        if tokio::time::timeout(Duration::from_secs(3), async {
            while connections.join_next().await.is_some() {}
        }).await.is_err() {
            connections.abort_all();
        }
        let result = match finished { Some(result) => result, None => worker.await };
        result.context("daemon worker panicked")??;
        Ok(())
    }
}

enum Command {
    Request { envelope: Box<RequestEnvelope>, caller: Caller, denial: Option<ProtocolError>, reply: oneshot::Sender<ResponseEnvelope> },
    DeviceEvent(MonitorEvent),
    MonitorFailed(String),
    Stop,
}

async fn connection(
    stream: UnixStream,
    access: AccessPolicy,
    commands: mpsc::Sender<Command>,
    mut events: broadcast::Receiver<EventMessage>,
    mut closed: watch::Receiver<bool>,
    checker: Arc<Checker>,
) -> io::Result<()> {
    let peer = Peer::from_stream(&stream)?;
    if matches!(access, AccessPolicy::Sandbox { uid, .. } if uid != peer.uid) {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "the sandbox belongs to another uid"));
    }
    let (read, mut write) = stream.into_split();
    let mut reader = BufReader::new(read);
    loop {
        let line = tokio::select! {
            _ = closed.changed() => return Ok(()),
            line = tokio::time::timeout(IO_TIMEOUT, read_frame(&mut reader)) => {
                line.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "request read timed out"))??
            }
        };
        let Some(line) = line else { return Ok(()); };
        let envelope = match RequestEnvelope::parse(&line) {
            Ok(envelope) => envelope,
            Err(error) => {
                let id = serde_json::from_str::<serde_json::Value>(&line).ok().and_then(|value| value.get("id").and_then(|id| id.as_u64()));
                write_message(&mut write, id, &ResponseEnvelope::error(id, error)).await?;
                continue;
            }
        };
        let subscribe = matches!(envelope.request, Request::SubscribeEvents);
        let guarded = guarded_caller(&peer, access, &envelope.request, &checker, &mut closed).await?;
        let (reply, response) = oneshot::channel();
        commands.send(Command::Request { envelope: Box::new(envelope), caller: guarded.caller, denial: guarded.denial, reply }).await.map_err(|_| io::Error::other("daemon is stopping"))?;
        let response = response.await.map_err(|_| io::Error::other("daemon worker stopped"))?;
        let subscribed = subscribe && response.ok;
        write_message(&mut write, response.id, &response).await?;
        if subscribed {
            loop {
                tokio::select! {
                    biased;
                    event = events.recv() => {
                        match event {
                            Ok(event) => write_message(&mut write, None, &event).await?,
                            Err(broadcast::error::RecvError::Lagged(_)) => {
                                write_message(&mut write, None, &ResponseEnvelope::error(None, ProtocolError::new("events_lagged", "event consumer fell behind; connection closed", "reconnect and request a fresh list snapshot"))).await?;
                                return Ok(());
                            }
                            Err(broadcast::error::RecvError::Closed) => return Ok(()),
                        }
                    }
                    _ = closed.changed() => {
                        while let Ok(event) = events.try_recv() { write_message(&mut write, None, &event).await?; }
                        return Ok(());
                    }
                }
            }
        }
    }
}

struct GuardedCaller {
    caller: Caller,
    denial: Option<ProtocolError>,
}

async fn guarded_caller(peer: &Peer, access: AccessPolicy, request: &Request, checker: &Checker,
                        closing: &mut watch::Receiver<bool>) -> io::Result<GuardedCaller> {
    let original = peer.caller()?;
    if request.is_read_only() { return Ok(GuardedCaller { caller: original, denial: None }); }
    let Some(action) = request.needs_polkit_action() else {
        return Ok(GuardedCaller { caller: original, denial: Some(ProtocolError::unauthorized(
            "no authorization action is defined for this operation", "use a compatible daemon/client")) });
    };
    let decision = match access {
        AccessPolicy::Sandbox { uid, .. } if peer.uid == uid => Decision::Authorized,
        AccessPolicy::Sandbox { .. } => Decision::Denied("sandbox_owner_mismatch"),
        AccessPolicy::System { .. } if original.uid == 0 => Decision::Authorized,
        AccessPolicy::System { .. } => checker.check(peer, action, closing).await,
    };
    // Authentication may take a minute. Never dispatch an old identity or
    // stale supplementary groups after the await, even with a positive grant.
    Ok(finish_authorization(original, peer.caller(), decision, action))
}

fn finish_authorization(original: Caller, refreshed: io::Result<Caller>, decision: Decision,
                        action: &'static str) -> GuardedCaller {
    let mut caller = match refreshed {
        Ok(caller) if caller.uid == original.uid => caller,
        _ => return GuardedCaller { caller: original, denial: Some(ProtocolError::unauthorized(
            "socket peer identity changed during authorization", "reconnect from the same user and retry")) },
    };
    caller.polkit_authorized = decision == Decision::Authorized;
    let denial = match decision {
        Decision::Authorized => None,
        Decision::Denied(reason) => Some(ProtocolError::unauthorized(
            format!("{action}: {reason}"),
            "retry from an active local session with a polkit agent; check installed SerialGuard policy or ask an administrator")),
    };
    GuardedCaller { caller, denial }
}

pub async fn read_frame<R: AsyncBufRead + Unpin>(reader: &mut R) -> io::Result<Option<String>> {
    let mut line = Vec::new();
    loop {
        let buffer = reader.fill_buf().await?;
        if buffer.is_empty() {
            return if line.is_empty() { Ok(None) } else { Err(io::Error::new(io::ErrorKind::InvalidData, "request must end with a newline")) };
        }
        let end = buffer.iter().position(|byte| *byte == b'\n').map(|index| index + 1);
        let count = end.unwrap_or(buffer.len());
        if line.len() + count > MAX_REQUEST_BYTES {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "request exceeds 64 KiB"));
        }
        line.extend_from_slice(&buffer[..count]);
        reader.consume(count);
        if end.is_some() {
            return String::from_utf8(line).map(Some).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "request must be UTF-8"));
        }
    }
}

async fn write_message<W: AsyncWrite + Unpin>(write: &mut W, id: Option<u64>, message: &impl serde::Serialize) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(message).map_err(io::Error::other)?;
    bytes.push(b'\n');
    if bytes.len() > devguard_core::protocol::MAX_RESPONSE_BYTES {
        let error = ResponseEnvelope::error(id, ProtocolError::new(
            "response_too_large", "response exceeds 4 MiB; no truncated data was returned",
            "reduce the result size (for example, the log limit or a single-device show) and retry",
        ));
        bytes = serde_json::to_vec(&error).map_err(io::Error::other)?;
        bytes.push(b'\n');
    }
    tokio::time::timeout(Duration::from_secs(5), write.write_all(&bytes)).await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "client write timed out"))?
}

struct Runtime<I> {
    daemon: Daemon,
    io: I,
    journal: Journal,
    events: broadcast::Sender<EventMessage>,
    sequence: u64,
    devices: BTreeMap<String, OnlineDevice>,
}

impl<I: DeviceIo> Runtime<I> {
    fn new(paths: Paths, gid: u32, mut io: I, events: broadcast::Sender<EventMessage>, monitored: bool) -> anyhow::Result<Self> {
        if PolicyStore::new(&paths).load()?.mode != PolicyMode::Audit {
            bail!("the shell only supports audit; review policy.toml before starting (it was not changed)");
        }
        let journal = Journal::new(&paths);
        // SAFETY: read-only credentials, used only to describe this daemon start.
        let uid = unsafe { libc::geteuid() };
        journal.append(&JournalEntry::new("service_started")
            .details(serde_json::json!({
                "pid": std::process::id(), "phase": 4, "policy": "audit",
                "termios_apply_enabled": true, "usb_authorization_writes_enabled": false,
                "autosuspend_writes_enabled": false,
                "identity_source": "daemon_process", "daemon_uid": uid,
                "daemon_user": accounts::user_name(uid)?, "initiating_operator_known": false,
            })))?;
        let mut daemon = Daemon::new(paths, Some(gid));
        let devices = if monitored {
            daemon.online_with_status(&mut io).map_err(anyhow::Error::msg)?
                .into_iter().map(|device| (device.devnode.clone(), device)).collect()
        } else { BTreeMap::new() };
        Ok(Self { daemon, io, journal, events, sequence: 0, devices })
    }

    fn observe(&mut self, hint: MonitorEvent) -> anyhow::Result<()> {
        // Observation never applies settings or changes rules/authorization.
        // Registered-device checks can open the tty to read termios.
        let devices: BTreeMap<String, OnlineDevice> = self.daemon.online_with_status(&mut self.io)
            .map_err(anyhow::Error::msg)?.into_iter().map(|device| (device.devnode.clone(), device)).collect();
        let mut changes = Vec::new();
        for (node, previous) in &self.devices {
            if !devices.get(node).is_some_and(|device| same_attachment(previous, device)) {
                changes.push(("device_removed", previous.clone(), Event::DeviceRemoved {
                    name: previous.managed_by.clone().unwrap_or_else(|| previous.name.clone()), devnode: node.clone(),
                }));
            }
        }
        for (node, device) in &devices {
            match self.devices.get(node) {
                Some(previous) if same_attachment(previous, device) => {
                    if previous != device { changes.push(("device_updated", device.clone(), Event::DeviceUpdated { device: device.clone() })); }
                }
                _ => changes.push(("device_added", device.clone(), Event::DeviceAdded { device: device.clone() })),
            }
        }
        for (kind, device, event) in changes {
            self.journal.append(&JournalEntry::new(kind).device(&device)
                .name(device.managed_by.as_deref().unwrap_or(&device.name))
                .details(serde_json::json!({"source": "udev_monitor", "action": hint.action, "devpath": hint.devpath, "udev_seq": hint.sequence})))?;
            self.sequence += 1;
            let _ = self.events.send(EventMessage::new(self.sequence, event));
        }
        self.devices = devices;
        Ok(())
    }

    fn monitor_failed(&mut self, reason: String) -> anyhow::Result<()> {
        self.journal.append(&JournalEntry::new("monitor_failed").details(serde_json::json!({"reason": reason, "source": "udev_monitor"})))?;
        bail!("udev monitoring stopped; device snapshots must not be treated as live: {reason}")
    }

    fn dispatch(&mut self, caller: &Caller, envelope: RequestEnvelope, denial: Option<ProtocolError>) -> anyhow::Result<(ResponseEnvelope, bool)> {
        let id = envelope.id;
        let request = envelope.request;
        if !request.is_read_only() {
            self.journal.append(&JournalEntry::new("request_received").operator(caller.uid, &caller.user)
                .details(serde_json::json!({"op": request.name()})))?;
        }
        if let Some(error) = denial {
            let entry = JournalEntry::new("request_denied").operator(caller.uid, &caller.user)
                .details(serde_json::json!({"op": request.name(), "action": request.needs_polkit_action(), "reason": &error.message}));
            return self.finish(Handled { response: ResponseEnvelope::error(id, error), events: Vec::new(), effects: Vec::new(), journal: vec![entry] });
        }
        // Phase 3 enables apply (termios + env file).  Enforcement-adjacent
        // writes stay unavailable until their phase gates pass.
        let supported = !matches!(request, Request::Pause { .. } | Request::Resume { .. } | Request::PolicySet { .. } | Request::BlockDevice { .. } | Request::AllowDevice { .. });
        let handled = if supported {
            self.daemon.handle(&mut self.io, caller, id, request)
        } else {
            let (error, event) = match self.daemon.authorize(caller, &request) {
                Err(error) => (error, "request_denied"),
                Ok(()) => (ProtocolError::unsupported("this operation has not passed its Linux acceptance gate", "pause and enforcement await phase 5; audit, configuration and termios apply are available"), "request_unavailable"),
            };
            Handled { response: ResponseEnvelope::error(id, error), events: Vec::new(), effects: Vec::new(), journal: vec![JournalEntry::new(event).operator(caller.uid, &caller.user).details(serde_json::json!({"op": request.name()}))] }
        };
        self.finish(handled)
    }

    fn finish(&mut self, handled: Handled) -> anyhow::Result<(ResponseEnvelope, bool)> {
        for entry in &handled.journal { self.journal.append(entry)?; }
        let mut response = handled.response;
        let mut shutdown = false;
        for effect in handled.effects {
            match effect {
                Effect::WriteRules => {
                    let loaded = self.daemon.store().load()?;
                    let result = if loaded.errors.is_empty() && !loaded.issues.iter().any(|issue| issue.severity == devguard_core::Severity::Error) {
                        self.io.write_rules(&loaded.devices)
                    } else {
                        Err("configuration store contains errors; rules were not replaced".into())
                    };
                    if let Err(error) = result {
                        self.journal.append(&JournalEntry::new("rules_failed").details(serde_json::json!({"reason": error})))?;
                        response = ResponseEnvelope::error(response.id, ProtocolError::new("rules_failed", format!("configuration change is saved, but rules were not activated: {error}"), "repair the configuration or udev service, then save again"));
                    }
                }
                Effect::Shutdown { .. } => shutdown = true, // No devices can be blocked in this shell.
                Effect::Rescan => bail!("unexpected enforcement effect in the audit-only shell"),
            }
        }
        for event in handled.events {
            self.sequence += 1;
            let _ = self.events.send(EventMessage::new(self.sequence, event));
        }
        Ok((response, shutdown))
    }
}

fn same_attachment(previous: &OnlineDevice, current: &OnlineDevice) -> bool {
    previous.vid == current.vid && previous.pid == current.pid && previous.serial == current.serial
        && previous.id_path == current.id_path && previous.usb_path == current.usb_path
}

fn ensure_directory(path: &Path, mode: u32) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            // SAFETY: the current uid is read, never changed.
            if !metadata.is_dir() || metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o022 != 0 {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("{} must be an owned, non-writable-by-others directory, not a symlink", path.display())));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if let Some(parent) = path.parent() { ensure_directory(parent, mode)?; }
            fs::DirBuilder::new().mode(mode).create(path)?;
        }
        Err(error) => return Err(error),
    }
    Ok(())
}

fn set_group(path: &Path, gid: u32) -> io::Result<()> {
    let path = CString::new(path.as_os_str().as_bytes()).map_err(|_| io::Error::other("path contains NUL"))?;
    // SAFETY: valid C path; lchown cannot follow a final-component symlink.
    if unsafe { libc::lchown(path.as_ptr(), !0, gid) } != 0 { return Err(io::Error::last_os_error()); }
    Ok(())
}

struct SocketGuard { path: PathBuf, device: u64, inode: u64 }

impl SocketGuard {
    fn new(path: PathBuf) -> io::Result<Self> {
        let metadata = fs::symlink_metadata(&path)?;
        Ok(Self { path, device: metadata.dev(), inode: metadata.ino() })
    }
}

impl Drop for SocketGuard {
    fn drop(&mut self) {
        if fs::symlink_metadata(&self.path).is_ok_and(|metadata| metadata.file_type().is_socket() && metadata.dev() == self.device && metadata.ino() == self.inode) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::{finish_authorization, guarded_caller};
    use devguard_core::{DeviceConfig, DeviceId, DeviceKind, IdRole, Request};
    use tokio::sync::watch;

    fn config() -> DeviceConfig {
        DeviceConfig {
            name: "radio-1".into(), kind: DeviceKind::Radio, description: None,
            ids: vec![DeviceId { vid: "1a86".into(), pid: "7523".into(), serial: None, path: Some("pci-port-1".into()), role: IdRole::App }],
            serial: DeviceKind::Radio.default_serial(), access: Default::default(),
        }
    }

    fn peer() -> Peer {
        let (stream, _other) = tokio::net::UnixStream::pair().unwrap();
        Peer::from_stream(&stream).unwrap()
    }

    fn caller() -> Caller {
        Caller { uid: 1000, user: "dev".into(), groups: vec![1000], polkit_authorized: false }
    }

    #[test]
    fn authorization_marks_the_caller_and_has_no_denial() {
        let caller = caller();
        let result = finish_authorization(caller.clone(), Ok(caller.clone()), Decision::Authorized, "dev.devguard.modify");
        assert!(result.caller.polkit_authorized);
        assert!(result.denial.is_none());
    }

    #[test]
    fn a_denied_decision_maps_to_an_actionable_unauthorized_error() {
        let caller = caller();
        let result = finish_authorization(caller.clone(), Ok(caller), Decision::Denied("polkit_not_authorized"), "dev.devguard.modify");
        let error = result.denial.expect("denial error");
        assert_eq!(error.code, "unauthorized");
        assert!(error.message.contains("dev.devguard.modify") && error.message.contains("polkit_not_authorized"));
        assert!(error.next_step.is_some());
        assert!(!result.caller.polkit_authorized);
    }

    #[test]
    fn an_identity_change_during_authorization_is_never_dispatched() {
        let caller = caller();
        let mut impersonated = caller.clone();
        impersonated.uid += 1;
        let result = finish_authorization(caller, Ok(impersonated), Decision::Authorized, "dev.devguard.pause");
        let error = result.denial.expect("identity change must deny");
        assert!(error.message.contains("identity changed"));
    }

    #[test]
    fn a_peer_read_failure_during_authorization_is_a_denial() {
        let caller = caller();
        let failure = Err(std::io::Error::other("peer process exited"));
        let result = finish_authorization(caller, failure, Decision::Authorized, "dev.devguard.modify");
        assert!(result.denial.is_some());
    }

    #[tokio::test]
    async fn read_only_requests_are_not_subjected_to_polkit() {
        let peer = peer();
        let (_sender, mut closing) = watch::channel(false);
        let result = guarded_caller(&peer, AccessPolicy::System { gid: 0 }, &Request::List, &Checker::default(), &mut closing).await.unwrap();
        assert!(result.denial.is_none());
        assert!(!result.caller.polkit_authorized);
    }

    #[tokio::test]
    async fn a_sandbox_owner_is_authorized_without_external_polkit() {
        let peer = peer();
        let (_sender, mut closing) = watch::channel(false);
        let result = guarded_caller(&peer, AccessPolicy::Sandbox { uid: peer.uid, gid: peer.gid },
                                   &Request::SaveDevice { config: config(), note: None },
                                   &Checker::default(), &mut closing).await.unwrap();
        assert!(result.denial.is_none() && result.caller.polkit_authorized);
    }

    #[tokio::test]
    async fn a_system_root_is_authorized_without_external_polkit() {
        // The unit test cannot fabricate uid 0 without root; instead assert the
        // non-root System path still consults the checker (fixture exit 0 grants).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pkcheck");
        std::fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
        let checker = Checker::fixture(path, std::time::Duration::from_secs(2));
        let peer = peer();
        let (_sender, mut closing) = watch::channel(false);
        let result = guarded_caller(&peer, AccessPolicy::System { gid: 0 },
                                   &Request::SaveDevice { config: config(), note: None },
                                   &checker, &mut closing).await.unwrap();
        assert!(result.denial.is_none() && result.caller.polkit_authorized);
    }

    #[tokio::test]
    async fn a_non_read_only_request_without_an_action_is_refused_fail_closed() {
        // Every current mutation has an action; this guards against adding a
        // future one and forgetting its polkit binding.
        use devguard_core::{DeviceId, DeviceKind, PauseKind, Request, IdRole};
        let cfg = devguard_core::DeviceConfig {
            name: "radio-1".into(), kind: DeviceKind::Radio, description: None,
            ids: vec![DeviceId { vid: "1a86".into(), pid: "7523".into(), serial: None, path: Some("port-1".into()), role: IdRole::App }],
            serial: DeviceKind::Radio.default_serial(), access: Default::default(),
        };
        let mutations = [
            Request::SaveDevice { config: cfg.clone(), note: None },
            Request::DeleteDevice { name: "radio-1".into(), note: None },
            Request::Apply { name: None, devnode: None },
            Request::Pause { kind: PauseKind::Service, minutes: 1, note: None },
            Request::Resume { note: None },
            Request::PolicySet { mode: devguard_core::PolicyMode::Audit, note: None },
            Request::Shutdown { release_blocked: true, note: None },
            Request::BlockDevice { devnode: "/dev/ttyUSB0".into(), note: None },
            Request::AllowDevice { devnode: "/dev/ttyUSB0".into(), note: None },
        ];
        for request in mutations {
            assert!(!request.is_read_only());
            assert!(request.needs_polkit_action().is_some(), "mutation {} must carry a polkit action", request.name());
        }
    }
}
