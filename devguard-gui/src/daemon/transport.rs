//! An owned OS thread hosts Tokio. NSH's engine thread needs no Tokio reactor.
//! Two sockets separate the push stream from bounded, correlated read requests.

use std::{
    io,
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use devguard_core::{
    protocol::{ListResponse, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES, PROTOCOL_VERSION},
    Event, EventMessage, Request, RequestEnvelope, ResponseEnvelope,
};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
    sync::{mpsc, watch},
};

use super::{
    model::{retry_seconds, Cursor},
    store::{Command, Message, Payload, ReadKind},
    Connection, Fault, Inspection,
};

pub(super) struct Worker {
    sender: mpsc::Sender<Command>,
    cancel: watch::Sender<bool>,
    _thread: thread::JoinHandle<()>,
}
impl Worker {
    pub(super) fn sender(&self) -> mpsc::Sender<Command> {
        self.sender.clone()
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.cancel.send(true);
    }
}

pub(super) fn start(path: PathBuf, updates: mpsc::Sender<Message>) -> Result<Worker, Fault> {
    let (sender, commands) = mpsc::channel(16);
    let (cancel, mut cancelled) = watch::channel(false);
    let worker = thread::Builder::new()
        .name("serialguard-gui-socket".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = updates.blocking_send(Message::Connection(Connection::Unavailable(
                        io_fault(error),
                    )));
                    return;
                }
            };
            runtime.block_on(async move {
                tokio::select! { biased;
                    _ = cancelled.changed() => {},
                    _ = supervise(path, commands, updates) => {},
                }
            });
        })
        .map_err(io_fault)?;
    Ok(Worker {
        sender,
        cancel,
        _thread: worker,
    })
}

enum Exit {
    Stopped,
    Retry,
    Closed,
}

async fn supervise(
    path: PathBuf,
    mut commands: mpsc::Receiver<Command>,
    updates: mpsc::Sender<Message>,
) {
    let mut generation = 0;
    let mut attempt = 0;
    loop {
        generation += 1;
        if updates
            .send(Message::Connection(Connection::Connecting))
            .await
            .is_err()
        {
            return;
        }
        let result = session(&path, generation, &mut commands, &updates).await;
        let wait = match result {
            Ok(Exit::Closed) => return,
            Ok(Exit::Retry) => {
                attempt = 0;
                continue;
            }
            Ok(Exit::Stopped) => {
                if updates
                    .send(Message::Connection(Connection::Stopped))
                    .await
                    .is_err()
                {
                    return;
                }
                None
            }
            Err(fault) if !fault.retryable => {
                if updates
                    .send(Message::Connection(Connection::Unavailable(fault)))
                    .await
                    .is_err()
                {
                    return;
                }
                None
            }
            Err(fault) => {
                let seconds = retry_seconds(attempt);
                attempt += 1;
                if updates
                    .send(Message::Connection(Connection::Reconnecting {
                        seconds,
                        fault,
                    }))
                    .await
                    .is_err()
                {
                    return;
                }
                Some(Duration::from_secs(seconds))
            }
        };
        // No polling: await either the backoff or an explicit reconnect. Never
        // call systemd, and never replay a command across daemon generations.
        let deadline = async {
            match wait {
                Some(delay) => tokio::time::sleep(delay).await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                _ = &mut deadline => break,
                command = commands.recv() => match command {
                    None => return,
                    Some(Command::Reconnect) => { attempt = 0; break; },
                    Some(Command::Read { generation, ticket, kind }) => {
                        let fault = Fault::new("not_connected", "The daemon connection is not live.", "Reconnect before retrying the query.", false);
                        if updates.send(Message::Reply { generation, ticket, kind, result: Err(fault) }).await.is_err() { return; }
                    }
                }
            }
        }
    }
}

struct EventTask(tokio::task::JoinHandle<()>);
impl Drop for EventTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn session(
    path: &Path,
    generation: u64,
    commands: &mut mpsc::Receiver<Command>,
    updates: &mpsc::Sender<Message>,
) -> Result<Exit, Fault> {
    let stream = tokio::time::timeout(Duration::from_secs(5), UnixStream::connect(path))
        .await
        .map_err(|_| timeout_fault())?
        .map_err(io_fault)?;
    let credentials = stream.peer_cred().map_err(io_fault)?;
    let peer = (credentials.pid(), credentials.uid());
    if peer.0.is_none() {
        return Err(Fault::new(
            "peer_identity_missing",
            "The daemon peer has no process identity.",
            "Use a local Linux Unix socket.",
            false,
        ));
    }
    let mut reader = BufReader::new(stream);
    exchange(&mut reader, 1, Request::SubscribeEvents).await?;
    let (event_sender, mut events) = mpsc::channel(64);
    let _reader_task = EventTask(tokio::spawn(async move {
        loop {
            let event = read_event(&mut reader).await;
            let end = !matches!(event, Ok(Some(_)));
            if event_sender.send(event).await.is_err() || end {
                return;
            }
        }
    }));
    let mut cursor = snapshot(path, peer, generation, updates).await?;
    loop {
        tokio::select! {
            event = events.recv() => {
                let Some(event) = event else { return Err(disconnected()); };
                let Some(event) = event? else { return Err(disconnected()); };
                if !cursor.accept(event.seq)? { continue; }
                if matches!(event.event, Event::ServiceStopping { .. }) { return Ok(Exit::Stopped); }
                let reconcile = matches!(event.event, Event::ConfigChanged { .. } | Event::PolicyChanged { .. }
                    | Event::ServiceStarted { .. } | Event::StartupReconcile { .. } | Event::PauseStarted { .. } | Event::PauseEnded { .. });
                if updates.send(Message::Event(event)).await.is_err() { return Ok(Exit::Closed); }
                if reconcile { cursor = snapshot(path, peer, generation, updates).await?; }
            }
            command = commands.recv() => match command {
                None => return Ok(Exit::Closed),
                Some(Command::Reconnect) => return Ok(Exit::Retry),
                Some(Command::Read { generation: expected, ticket, kind }) => {
                    if expected != generation { continue; }
                    if matches!(kind, ReadKind::Refresh) {
                        cursor = snapshot(path, peer, generation, updates).await?;
                        continue;
                    }
                    let request = request(&kind)?;
                    let result = rpc(path, peer, request).await.and_then(|response| payload(&kind, response));
                    if result.as_ref().is_err_and(|error| error.retryable) { return Err(result.err().unwrap()); }
                    if updates.send(Message::Reply { generation, ticket, kind, result }).await.is_err() { return Ok(Exit::Closed); }
                }
            }
        }
    }
}

async fn snapshot(
    path: &Path,
    peer: (Option<i32>, u32),
    generation: u64,
    updates: &mpsc::Sender<Message>,
) -> Result<Cursor, Fault> {
    let value: ListResponse = rpc(path, peer, Request::List)
        .await?
        .into_data()
        .map_err(protocol_fault)?;
    let cursor = Cursor::snapshot(value.event_sequence)?;
    updates
        .send(Message::Snapshot {
            generation,
            snapshot: value,
        })
        .await
        .map_err(|_| disconnected())?;
    Ok(cursor)
}

fn request(kind: &ReadKind) -> Result<Request, Fault> {
    Ok(match kind {
        ReadKind::Inspect {
            name: Some(name), ..
        } => Request::Show { name: name.clone() },
        ReadKind::Inspect {
            devnode: Some(devnode),
            ..
        } => Request::LearnCandidate {
            devnode: devnode.clone(),
        },
        ReadKind::Inspect { .. } => {
            return Err(Fault::new(
                "device_unavailable",
                "The selected device is no longer available.",
                "Select an online device or an existing registration.",
                false,
            ))
        }
        ReadKind::Check => Request::Check { name: None },
        ReadKind::Logs(filter) => Request::LogQuery {
            filter: filter.clone(),
        },
        ReadKind::Doctor => Request::Doctor,
        ReadKind::Refresh => Request::List,
    })
}

fn payload(kind: &ReadKind, response: ResponseEnvelope) -> Result<Payload, Fault> {
    match kind {
        ReadKind::Inspect { name: Some(_), .. } => response
            .into_data()
            .map(|v| Payload::Inspection(Box::new(Inspection::Registered(v)))),
        ReadKind::Inspect { .. } => response
            .into_data()
            .map(|v| Payload::Inspection(Box::new(Inspection::Candidate(v)))),
        ReadKind::Check => response.into_data().map(Payload::Check),
        ReadKind::Logs(_) => response.into_data().map(Payload::Logs),
        ReadKind::Doctor => response.into_data().map(Payload::Doctor),
        ReadKind::Refresh => unreachable!("snapshots have their own response path"),
    }
    .map_err(protocol_fault)
}

async fn rpc(
    path: &Path,
    peer: (Option<i32>, u32),
    request: Request,
) -> Result<ResponseEnvelope, Fault> {
    tokio::time::timeout(Duration::from_secs(90), async {
        let stream = UnixStream::connect(path).await.map_err(io_fault)?;
        let credentials = stream.peer_cred().map_err(io_fault)?;
        if (credentials.pid(), credentials.uid()) != peer {
            return Err(disconnected());
        }
        exchange(&mut BufReader::new(stream), 1, request).await
    })
    .await
    .map_err(|_| timeout_fault())?
}

async fn exchange(
    reader: &mut BufReader<UnixStream>,
    id: u64,
    request: Request,
) -> Result<ResponseEnvelope, Fault> {
    debug_assert!(
        request.is_read_only(),
        "this GUI bridge exposes no mutations"
    );
    let mut bytes = serde_json::to_vec(&RequestEnvelope::new(id, request))
        .map_err(|e| invalid(e.to_string()))?;
    bytes.push(b'\n');
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err(invalid("Request exceeds 64 KiB."));
    }
    reader.get_mut().write_all(&bytes).await.map_err(io_fault)?;
    let line = tokio::time::timeout(Duration::from_secs(90), frame(reader))
        .await
        .map_err(|_| timeout_fault())??
        .ok_or_else(disconnected)?;
    let response = ResponseEnvelope::parse(&line).map_err(protocol_fault)?;
    if response.id != Some(id) {
        return Err(invalid("Response ID did not match the query."));
    }
    if !response.ok {
        return Err(response
            .error
            .map(protocol_fault)
            .unwrap_or_else(|| invalid("Missing daemon error.")));
    }
    if response.error.is_some() || response.data.is_none() {
        return Err(invalid("Inconsistent daemon response."));
    }
    Ok(response)
}

async fn read_event(reader: &mut BufReader<UnixStream>) -> Result<Option<EventMessage>, Fault> {
    let Some(line) = frame(reader).await? else {
        return Ok(None);
    };
    let event: EventMessage = serde_json::from_str(&line).map_err(|e| {
        ResponseEnvelope::parse(&line)
            .ok()
            .and_then(|r| r.error)
            .map(protocol_fault)
            .unwrap_or_else(|| invalid(e.to_string()))
    })?;
    if event.v != PROTOCOL_VERSION {
        return Err(invalid("Unsupported event protocol version."));
    }
    Ok(Some(event))
}

async fn frame(reader: &mut (impl AsyncBufRead + Unpin)) -> Result<Option<String>, Fault> {
    let mut bytes = Vec::new();
    loop {
        let buffer = reader.fill_buf().await.map_err(io_fault)?;
        if buffer.is_empty() {
            return if bytes.is_empty() {
                Ok(None)
            } else {
                Err(invalid("Truncated daemon frame."))
            };
        }
        let end = buffer.iter().position(|b| *b == b'\n').map(|i| i + 1);
        let count = end.unwrap_or(buffer.len());
        if bytes.len() + count > MAX_RESPONSE_BYTES {
            return Err(invalid("Daemon frame exceeds 4 MiB."));
        }
        bytes.extend_from_slice(&buffer[..count]);
        reader.consume(count);
        if end.is_some() {
            return String::from_utf8(bytes)
                .map(Some)
                .map_err(|e| invalid(e.to_string()));
        }
    }
}

fn protocol_fault(error: devguard_core::ProtocolError) -> Fault {
    let retryable = error.code == "events_lagged";
    Fault {
        code: error.code,
        message: error.message,
        next: error
            .next_step
            .unwrap_or_else(|| "Reconnect with a matching daemon version.".into()),
        retryable,
    }
}
fn io_fault(error: io::Error) -> Fault {
    if error.kind() == io::ErrorKind::PermissionDenied {
        Fault::new("permission_denied", "Access to the daemon socket was denied.", "Ask an administrator to check devguard group membership, then sign in again. Do not run the GUI as root.", false)
    } else {
        Fault::new(
            "connection_unavailable",
            format!("Daemon connection failed: {error}"),
            "The last snapshot is stale. The GUI will reconnect without starting the service.",
            true,
        )
    }
}
fn disconnected() -> Fault {
    Fault::new(
        "connection_closed",
        "The daemon connection closed unexpectedly.",
        "Reconnecting without starting the service; previous data is stale.",
        true,
    )
}
fn timeout_fault() -> Fault {
    Fault::new(
        "request_timeout",
        "The daemon query timed out.",
        "Reconnecting for a fresh snapshot.",
        true,
    )
}
fn invalid(message: impl Into<String>) -> Fault {
    Fault::new(
        "response_invalid",
        message,
        "Update the daemon and GUI together, then reconnect.",
        false,
    )
}

#[cfg(test)]
mod tests;
