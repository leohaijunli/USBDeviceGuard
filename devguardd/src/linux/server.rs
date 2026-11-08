//! Bounded JSON-lines RPC. One worker owns all daemon decisions and effects.

use std::ffi::CString;
use std::collections::BTreeMap;
use std::fs;
use std::future::Future;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;
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
}

impl Server {
    pub async fn serve_monitored<I, F>(self, io: I, monitor: UdevMonitor, stop: F) -> anyhow::Result<()>
    where I: DeviceIo + Send + 'static, F: Future<Output = ()> {
        self.serve_inner(io, Some(monitor), stop).await
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
        // Never unlink an unfamiliar file or another daemon's socket.
        let listener = UnixListener::bind(paths.socket())?;
        let guard = SocketGuard::new(paths.socket())?;
        set_group(&paths.socket(), access.gid())?;
        fs::set_permissions(paths.socket(), fs::Permissions::from_mode(0o660))?;
        Ok(Self { listener, paths, access, _socket: guard })
    }

    pub async fn serve<I, F>(self, io: I, stop: F) -> anyhow::Result<()>
    where I: DeviceIo + Send + 'static, F: Future<Output = ()> {
        self.serve_inner(io, None, stop).await
    }

    async fn serve_inner<I, F>(self, io: I, monitor: Option<UdevMonitor>, stop: F) -> anyhow::Result<()>
    where I: DeviceIo + Send + 'static, F: Future<Output = ()> {
        let (commands, mut requests) = mpsc::channel::<Command>(64);
        let (events, _) = broadcast::channel::<EventMessage>(256);
        let worker_events = events.clone();
        let paths = self.paths.clone();
        let access = self.access;
        let monitored = monitor.is_some();
        let mut worker = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let mut runtime = Runtime::new(paths, access.gid(), io, worker_events, monitored)?;
            while let Some(command) = requests.blocking_recv() {
                match command {
                    Command::Stop => return Ok(()),
                    Command::DeviceEvent(event) => {
                        if let Err(error) = runtime.observe(event) {
                            return runtime.monitor_failed(error.to_string());
                        }
                    }
                    Command::MonitorFailed(reason) => return runtime.monitor_failed(reason),
                    Command::Request { envelope, caller, reply } => {
                        let id = envelope.id;
                        let result = runtime.dispatch(&caller, *envelope);
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
                    connections.spawn(async move {
                        if let Err(error) = connection(stream, access, commands, receiver, closed).await {
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
    Request { envelope: Box<RequestEnvelope>, caller: Caller, reply: oneshot::Sender<ResponseEnvelope> },
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
        let mut caller = peer.caller()?;
        if !envelope.request.is_read_only() {
            caller.polkit_authorized = match access {
                AccessPolicy::Sandbox { uid, .. } => peer.uid == uid,
                AccessPolicy::System { .. } => {
                    if caller.uid == 0 { true } else { polkit(&peer, &envelope.request).await }
                }
            };
        }
        let (reply, response) = oneshot::channel();
        commands.send(Command::Request { envelope: Box::new(envelope), caller, reply }).await.map_err(|_| io::Error::other("daemon is stopping"))?;
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

async fn polkit(peer: &Peer, request: &Request) -> bool {
    let Some(action) = request.needs_polkit_action() else { return false; };
    let child = tokio::process::Command::new("/usr/bin/pkcheck")
        .args(["--action-id", action, "--process", &peer.polkit_subject(), "--allow-user-interaction"])
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).kill_on_drop(true).spawn();
    let Ok(mut child) = child else { return false; };
    matches!(tokio::time::timeout(Duration::from_secs(60), child.wait()).await, Ok(Ok(status)) if status.success())
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
            bail!("phase-2 shell only supports audit; review policy.toml before starting (it was not changed)");
        }
        let journal = Journal::new(&paths);
        // SAFETY: read-only credentials, used only to describe this daemon start.
        let uid = unsafe { libc::geteuid() };
        journal.append(&JournalEntry::new("service_started")
            .details(serde_json::json!({
                "pid": std::process::id(), "phase": 2, "hardware_writes": false,
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
        // Observation never calls apply, rule reload, termios or authorized.
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

    fn dispatch(&mut self, caller: &Caller, envelope: RequestEnvelope) -> anyhow::Result<(ResponseEnvelope, bool)> {
        let id = envelope.id;
        let request = envelope.request;
        if !request.is_read_only() {
            self.journal.append(&JournalEntry::new("request_received").operator(caller.uid, &caller.user)
                .details(serde_json::json!({"op": request.name()})))?;
        }
        // Phase 3 enables apply (termios + env file).  Enforcement-adjacent
        // writes stay unavailable until their phase gates pass.
        let supported = !matches!(request, Request::Pause { .. } | Request::Resume { .. } | Request::PolicySet { .. } | Request::BlockDevice { .. } | Request::AllowDevice { .. });
        let handled = if supported {
            self.daemon.handle(&mut self.io, caller, id, request)
        } else {
            let (error, event) = match self.daemon.authorize(caller, &request) {
                Err(error) => (error, "request_denied"),
                Ok(()) => (ProtocolError::unsupported("this hardware operation has not passed its Linux acceptance gate", "complete termios/composite-safety acceptance; this shell only supports audit and configuration"), "request_unavailable"),
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
