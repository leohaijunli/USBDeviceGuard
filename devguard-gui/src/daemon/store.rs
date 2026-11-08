//! Signals have one engine owner; the socket worker only sends immutable values.

use std::path::PathBuf;

use devguard_core::{
    protocol::{
        CheckResponse, DoctorResponse, LearnResponse, ListResponse, LogResponse, ShowResponse,
    },
    EventMessage, LogFilter,
};
use nsh_core::prelude::*;
use tokio::sync::mpsc;

use super::{model, Connection, DeviceFilter, DeviceRow, Fault};

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Inspection {
    Registered(ShowResponse),
    Candidate(LearnResponse),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Load<T> {
    pub value: Option<T>,
    pub error: Option<Fault>,
    pub pending: bool,
    ticket: u64,
    key: String,
}

impl<T> Default for Load<T> {
    fn default() -> Self {
        Self {
            value: None,
            error: None,
            pending: false,
            ticket: 0,
            key: String::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub(super) enum ReadKind {
    Inspect {
        key: String,
        name: Option<String>,
        devnode: Option<String>,
    },
    Check,
    Logs(LogFilter),
    Doctor,
    Refresh,
}

#[derive(Debug, Clone)]
pub(super) enum Payload {
    Inspection(Box<Inspection>),
    Check(CheckResponse),
    Logs(LogResponse),
    Doctor(DoctorResponse),
}

#[derive(Debug)]
pub(super) enum Command {
    Read {
        generation: u64,
        ticket: u64,
        kind: ReadKind,
    },
    Reconnect,
}

#[derive(Debug)]
pub(super) enum Message {
    Snapshot {
        generation: u64,
        snapshot: ListResponse,
    },
    Event(EventMessage),
    Connection(Connection),
    Reply {
        generation: u64,
        ticket: u64,
        result: Result<Payload, Fault>,
        kind: ReadKind,
    },
}

#[derive(Clone, Copy)]
pub(crate) struct Workspace {
    snapshot: Signal<Option<ListResponse>>,
    connection: Signal<Connection>,
    generation: Signal<u64>,
    revision: Signal<u64>,
    selection: Signal<Option<String>>,
    inspection: Signal<Load<Inspection>>,
    check: Signal<Load<CheckResponse>>,
    logs: Signal<Load<LogResponse>>,
    doctor: Signal<Load<DoctorResponse>>,
    feedback: Signal<SharedString>,
    refreshing: Signal<bool>,
    sender: Stored<Option<mpsc::Sender<Command>>>,
    next_ticket: Stored<u64>,
}

impl Workspace {
    pub(crate) fn connect(path: PathBuf) -> Self {
        let store = Self::empty();
        #[cfg(target_os = "linux")]
        effect(move || {
            // SAFETY: reads this process's credentials, without modifying them.
            if unsafe { libc::geteuid() } == 0 {
                store.receive(Message::Connection(Connection::Unavailable(Fault::new(
                    "gui_root_forbidden",
                    "The GUI must run as an ordinary user.",
                    "Remove sudo and sign in as a member of the devguard group.",
                    false,
                ))));
                return;
            }
            if !path.is_absolute() {
                store.receive(Message::Connection(Connection::Unavailable(Fault::new("socket_path_invalid",
                    "The daemon socket path must be absolute.", "Use /run/devguard/ctl.sock or an explicitly selected private development socket.", false))));
                return;
            }
            let (sender, mut receiver) = mpsc::channel(32);
            match super::transport::start(path.clone(), sender) {
                Ok(worker) => {
                    store.sender.replace(Some(worker.sender()));
                    cleanup(move || drop(worker));
                    spawn(async move {
                        while let Some(message) = receiver.recv().await {
                            store.receive(message);
                        }
                    });
                }
                Err(error) => store.receive(Message::Connection(Connection::Unavailable(error))),
            }
        });
        #[cfg(not(target_os = "linux"))]
        {
            let _ = path;
            store.receive(Message::Connection(Connection::Unavailable(Fault::new(
                "linux_only",
                "Daemon management is available on Linux only.",
                "Windows remains a read-only local diagnostic build.",
                false,
            ))));
        }
        store
    }

    fn empty() -> Self {
        Self {
            snapshot: signal(None),
            connection: signal(Connection::Connecting),
            generation: signal(0),
            revision: signal(0),
            selection: signal(None),
            inspection: signal(Load::default()),
            check: signal(Load::default()),
            logs: signal(Load::default()),
            doctor: signal(Load::default()),
            feedback: signal("Waiting for daemon data".into()),
            refreshing: signal(false),
            sender: stored(None),
            next_ticket: stored(0),
        }
    }

    pub(crate) fn connection(self) -> Connection {
        self.connection.get()
    }
    pub(crate) fn live(self) -> bool {
        self.connection.with(Connection::live)
    }
    pub(crate) fn generation(self) -> u64 {
        self.generation.get()
    }
    pub(crate) fn revision(self) -> u64 {
        self.revision.get()
    }
    pub(crate) fn snapshot(self) -> Option<ListResponse> {
        self.snapshot.get()
    }
    pub(crate) fn rows(self) -> Vec<DeviceRow> {
        self.snapshot
            .with(|s| s.as_ref().map(model::rows).unwrap_or_default())
    }
    pub(crate) fn ids(self, query: &str, filter: DeviceFilter) -> Vec<String> {
        self.rows()
            .into_iter()
            .filter(|r| r.matches(query, filter))
            .map(|r| r.key)
            .collect()
    }
    pub(crate) fn row(self, key: &str) -> Option<DeviceRow> {
        self.rows().into_iter().find(|r| r.key == key)
    }
    pub(crate) fn selection(self) -> Option<String> {
        self.selection.get()
    }
    pub(crate) fn select(self, key: String) {
        self.selection.set(Some(key));
    }
    pub(crate) fn inspection_for(self, key: &str) -> Load<Inspection> {
        self.inspection.with(|load| {
            if load.key == key {
                load.clone()
            } else {
                Load::default()
            }
        })
    }
    pub(crate) fn check(self) -> Load<CheckResponse> {
        self.check.get()
    }
    pub(crate) fn logs(self) -> Load<LogResponse> {
        self.logs.get()
    }
    pub(crate) fn doctor(self) -> Load<DoctorResponse> {
        self.doctor.get()
    }
    pub(crate) fn feedback(self) -> SharedString {
        self.feedback.get()
    }
    pub(crate) fn refreshing(self) -> bool {
        self.refreshing.get()
    }

    pub(crate) fn status(self) -> SharedString {
        self.snapshot
            .with(|s| {
                self.connection
                    .with(|c| c.label(s.as_ref().map(|s| &s.service)))
            })
            .into()
    }

    pub(crate) fn timestamp(self) -> SharedString {
        self.snapshot
            .with(|s| {
                s.as_ref()
                    .map(|s| format!("Snapshot: {}", s.generated_at))
                    .unwrap_or_else(|| "No verified snapshot".into())
            })
            .into()
    }

    pub(crate) fn reconnect(self) {
        if self.sender.with(|s| {
            s.as_ref()
                .is_some_and(|s| s.try_send(Command::Reconnect).is_ok())
        }) {
            self.connection.set(Connection::Connecting);
        }
    }

    pub(crate) fn refresh(self) {
        if !self.refreshing() {
            self.send(ReadKind::Refresh);
        }
    }
    pub(crate) fn inspect(self, row: &DeviceRow) {
        self.send(ReadKind::Inspect {
            key: row.key.clone(),
            name: row.config.as_ref().map(|c| c.name.clone()),
            devnode: row.online.first().map(|d| d.devnode.clone()),
        });
    }
    pub(crate) fn run_check(self) {
        self.send(ReadKind::Check);
    }
    pub(crate) fn query_logs(self, filter: LogFilter) {
        self.send(ReadKind::Logs(filter));
    }
    pub(crate) fn run_doctor(self) {
        self.send(ReadKind::Doctor);
    }

    fn send(self, kind: ReadKind) {
        if !self.live() {
            return;
        }
        let ticket = self.next_ticket.with(|id| {
            *id += 1;
            *id
        });
        match &kind {
            ReadKind::Inspect { key, .. } => begin(self.inspection, ticket, key.clone()),
            ReadKind::Check => begin(self.check, ticket, "check".into()),
            ReadKind::Logs(filter) => begin(
                self.logs,
                ticket,
                serde_json::to_string(filter).unwrap_or_default(),
            ),
            ReadKind::Doctor => begin(self.doctor, ticket, "doctor".into()),
            ReadKind::Refresh => self.refreshing.set(true),
        }
        let sent = self.sender.with(|s| {
            s.as_ref().is_some_and(|s| {
                s.try_send(Command::Read {
                    generation: self.generation.get_untracked(),
                    ticket,
                    kind: kind.clone(),
                })
                .is_ok()
            })
        });
        if !sent {
            self.receive(Message::Reply {
                generation: self.generation.get_untracked(),
                ticket,
                kind,
                result: Err(Fault::new(
                    "request_queue_full",
                    "The daemon request could not be queued.",
                    "Wait for the current request or reconnect.",
                    false,
                )),
            });
        }
    }

    fn receive(self, message: Message) {
        match message {
            Message::Snapshot {
                generation,
                snapshot,
            } => batch(|| {
                if self.generation.get_untracked() != generation {
                    self.clear_pending();
                }
                self.generation.set(generation);
                self.snapshot.set(Some(snapshot));
                self.refreshing.set(false);
                self.connection.set(Connection::Live);
                self.revision.update(|n| *n += 1);
                self.feedback
                    .set("Live daemon data · changes arrive through subscription".into());
            }),
            Message::Connection(connection) => batch(|| {
                if !connection.live() {
                    self.clear_pending();
                    self.refreshing.set(false);
                }
                self.connection.set(connection);
            }),
            Message::Event(message) => batch(|| {
                self.snapshot.update(|snapshot| {
                    let Some(snapshot) = snapshot else {
                        return;
                    };
                    use devguard_core::Event;
                    match message.event {
                        Event::DeviceAdded { device }
                        | Event::DeviceUpdated { device }
                        | Event::Blocked { device, .. }
                        | Event::AllowedDuringPause { device, .. } => {
                            snapshot.online.retain(|d| d.devnode != device.devnode);
                            snapshot.online.push(device);
                        }
                        Event::DeviceRemoved { devnode, .. } => {
                            snapshot.online.retain(|d| d.devnode != devnode)
                        }
                        Event::DriftDetected { name, .. } => {
                            for d in &mut snapshot.online {
                                if d.managed_by.as_deref() == Some(&name) {
                                    d.status = devguard_core::DeviceStatus::Drift;
                                }
                            }
                        }
                        _ => {}
                    }
                    snapshot.generated_at = message.ts;
                    snapshot.event_sequence = Some(message.seq);
                });
                self.revision.update(|n| *n += 1);
            }),
            Message::Reply {
                generation,
                ticket,
                kind,
                result,
            } => {
                if generation != self.generation.get_untracked() || !self.live() {
                    return;
                }
                match kind {
                    ReadKind::Inspect { .. } => finish(
                        self.inspection,
                        ticket,
                        result.and_then(|p| match p {
                            Payload::Inspection(v) => Ok(*v),
                            _ => Err(wrong_payload()),
                        }),
                    ),
                    ReadKind::Check => finish(
                        self.check,
                        ticket,
                        result.and_then(|p| match p {
                            Payload::Check(v) => Ok(v),
                            _ => Err(wrong_payload()),
                        }),
                    ),
                    ReadKind::Logs(_) => finish(
                        self.logs,
                        ticket,
                        result.and_then(|p| match p {
                            Payload::Logs(v) => Ok(v),
                            _ => Err(wrong_payload()),
                        }),
                    ),
                    ReadKind::Doctor => finish(
                        self.doctor,
                        ticket,
                        result.and_then(|p| match p {
                            Payload::Doctor(v) => Ok(v),
                            _ => Err(wrong_payload()),
                        }),
                    ),
                    ReadKind::Refresh => {
                        self.refreshing.set(false);
                        if let Err(error) = result {
                            self.feedback.set(error.display().into());
                        }
                    }
                }
            }
        }
    }

    fn clear_pending(self) {
        cancel(self.inspection);
        cancel(self.check);
        cancel(self.logs);
        cancel(self.doctor);
    }

    #[cfg(test)]
    pub(super) fn fixture(snapshot: ListResponse) -> (Self, mpsc::Receiver<Command>) {
        let store = Self::empty();
        let (sender, receiver) = mpsc::channel(16);
        store.sender.replace(Some(sender));
        store.receive(Message::Snapshot {
            generation: 1,
            snapshot,
        });
        (store, receiver)
    }

    #[cfg(test)]
    pub(super) fn inject(self, message: Message) {
        self.receive(message);
    }
}

fn wrong_payload() -> Fault {
    Fault::new(
        "response_invalid",
        "The daemon returned unexpected data.",
        "Update the daemon and GUI together.",
        false,
    )
}
fn begin<T: Clone + PartialEq + 'static>(state: Signal<Load<T>>, ticket: u64, key: String) {
    let mut load = state.get_untracked();
    if load.key != key {
        load.value = None;
    }
    load.key = key;
    load.ticket = ticket;
    load.pending = true;
    load.error = None;
    state.set(load);
}
fn finish<T: Clone + PartialEq + 'static>(
    state: Signal<Load<T>>,
    ticket: u64,
    result: Result<T, Fault>,
) {
    let mut load = state.get_untracked();
    if ticket != load.ticket {
        return;
    }
    load.pending = false;
    match result {
        Ok(value) => {
            load.value = Some(value);
            load.error = None;
        }
        Err(error) => load.error = Some(error),
    }
    state.set(load);
}
fn cancel<T: Clone + PartialEq + 'static>(state: Signal<Load<T>>) {
    let mut load = state.get_untracked();
    load.pending = false;
    load.ticket = 0;
    state.set(load);
}
