//! Synchronous Unix-socket client for CLI use and GUI background workers.

use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use crate::protocol::{MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES};
use crate::{EventMessage, ProtocolError, Request, RequestEnvelope, ResponseEnvelope, PROTOCOL_VERSION};

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("socket I/O: {0}")]
    Io(#[from] io::Error),
    #[error("{0}")]
    Protocol(#[from] ProtocolError),
}

pub struct Client {
    stream: BufReader<UnixStream>,
    next_id: u64,
    subscribed: bool,
    sequence: u64,
}

impl Client {
    pub fn connect(path: impl AsRef<Path>) -> Result<Self, ClientError> {
        let stream = UnixStream::connect(path)?;
        stream.set_read_timeout(Some(Duration::from_secs(90)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        Ok(Self { stream: BufReader::new(stream), next_id: 1, subscribed: false, sequence: 0 })
    }

    pub fn request(&mut self, request: Request) -> Result<ResponseEnvelope, ClientError> {
        if self.subscribed {
            return Err(ProtocolError::invalid("event streams do not accept more RPC requests", "open a separate request connection").into());
        }
        let subscribe = matches!(request, Request::SubscribeEvents);
        let id = self.next_id;
        self.next_id = id.checked_add(1).ok_or_else(|| ProtocolError::internal("request id exhausted"))?;
        let mut line = serde_json::to_vec(&RequestEnvelope::new(id, request)).map_err(|error| ProtocolError::internal(error.to_string()))?;
        line.push(b'\n');
        if line.len() > MAX_REQUEST_BYTES {
            return Err(ProtocolError::invalid("request exceeds 64 KiB", "reduce the configuration size").into());
        }
        self.stream.get_mut().write_all(&line)?;
        let line = read_line(&mut self.stream)?.ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "daemon closed before replying"))?;
        let response = ResponseEnvelope::parse(&line)?;
        if response.id != Some(id) {
            return Err(ProtocolError::invalid("response id does not match the request", "reconnect to a compatible devguardd").into());
        }
        if !response.ok {
            return Err(response.error.unwrap_or_else(|| ProtocolError::internal("daemon returned ok=false without an error")).into());
        }
        if response.error.is_some() || response.data.is_none() {
            return Err(ProtocolError::internal("daemon returned an inconsistent response envelope").into());
        }
        if subscribe {
            self.subscribed = true;
            self.stream.get_mut().set_read_timeout(None)?;
        }
        Ok(response)
    }

    pub fn subscribe(&mut self) -> Result<ResponseEnvelope, ClientError> {
        self.request(Request::SubscribeEvents)
    }

    pub fn next_event(&mut self) -> Result<Option<EventMessage>, ClientError> {
        if !self.subscribed {
            return Err(ProtocolError::invalid("this connection is not subscribed", "call subscribe before reading events").into());
        }
        let Some(line) = read_line(&mut self.stream)? else { return Ok(None); };
        let event: EventMessage = match serde_json::from_str(&line) {
            Ok(event) => event,
            Err(error) => {
                if let Ok(response) = ResponseEnvelope::parse(&line) {
                    if let Some(error) = response.error { return Err(error.into()); }
                }
                return Err(ProtocolError::invalid(format!("cannot parse event: {error}"), "reconnect and request a fresh snapshot").into());
            }
        };
        if event.v != PROTOCOL_VERSION || event.seq <= self.sequence {
            return Err(ProtocolError::invalid("event version or sequence is invalid", "reconnect and request a fresh snapshot").into());
        }
        self.sequence = event.seq;
        Ok(Some(event))
    }
}

fn read_line(reader: &mut impl BufRead) -> io::Result<Option<String>> {
    let mut line = Vec::new();
    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            return if line.is_empty() { Ok(None) } else { Err(io::Error::new(io::ErrorKind::InvalidData, "response must end with a newline")) };
        }
        let end = buffer.iter().position(|byte| *byte == b'\n').map(|index| index + 1);
        let count = end.unwrap_or(buffer.len());
        if line.len() + count > MAX_RESPONSE_BYTES {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "response exceeds 4 MiB"));
        }
        line.extend_from_slice(&buffer[..count]);
        reader.consume(count);
        if end.is_some() {
            return String::from_utf8(line).map(Some).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "response must be UTF-8"));
        }
    }
}
