//! Read-only udev event source; fixtures cross the same byte-stream seam.

use std::io;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufRead, AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

use super::inventory::{parse_properties, serial_kernel_name};

const MAX_RECORD_BYTES: usize = 64 * 1024;
const READY: &str = "UDEV - the event which udev sends out after rule processing";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorEvent {
    pub action: String,
    pub devnode: String,
    pub devpath: String,
    pub sequence: Option<u64>,
}

pub struct UdevMonitor {
    reader: Box<dyn AsyncBufRead + Unpin + Send>,
    child: Option<Child>,
}

impl UdevMonitor {
    pub async fn start() -> io::Result<Self> {
        Self::start_command(Path::new("/usr/bin/udevadm")).await
    }

    /// Alternate executables are fixture adapters; the monitoring arguments stay fixed.
    pub async fn start_command(program: &Path) -> io::Result<Self> {
        let mut child = Command::new(program)
            .args(["monitor", "--udev", "--property", "--subsystem-match=tty"])
            .env("SYSTEMD_COLORS", "0")
            .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::inherit())
            .kill_on_drop(true).spawn()?;
        let output = child.stdout.take().ok_or_else(|| io::Error::other("udev monitor stdout was not piped"))?;
        let mut monitor = Self { reader: Box::new(BufReader::new(output)), child: Some(child) };
        tokio::time::timeout(Duration::from_secs(5), monitor.wait_ready()).await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "udev monitor did not become ready"))??;
        Ok(monitor)
    }

    pub async fn stop(mut self) -> io::Result<()> {
        if let Some(child) = &mut self.child {
            let _ = child.start_kill();
            tokio::time::timeout(Duration::from_secs(2), child.wait()).await
                .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "udev monitor did not exit"))??;
        }
        Ok(())
    }

    pub async fn from_reader<R>(reader: R) -> io::Result<Self>
    where R: AsyncBufRead + Unpin + Send + 'static {
        let mut monitor = Self { reader: Box::new(reader), child: None };
        monitor.wait_ready().await?;
        Ok(monitor)
    }

    async fn wait_ready(&mut self) -> io::Result<()> {
        let mut bytes = 0;
        loop {
            let line = bounded_line(&mut *self.reader).await?;
            bytes += line.len();
            if bytes > MAX_RECORD_BYTES { return Err(invalid("udev readiness output exceeds 64 KiB")); }
            if line.trim_end() == READY { return Ok(()); }
        }
    }

    pub async fn next_event(&mut self) -> io::Result<MonitorEvent> {
        loop {
            let header = bounded_line(&mut *self.reader).await?;
            if header.trim().is_empty() { continue; }
            if !header.starts_with("UDEV ") { return Err(invalid("unexpected udev event header")); }
            let (_, fields) = header.split_once(']').ok_or_else(|| invalid("udev header has no timestamp terminator"))?;
            let fields: Vec<&str> = fields.split_whitespace().collect();
            if fields.len() != 3 { return Err(invalid("invalid udev event header")); }
            let action = fields[0];
            let devpath = fields[1];
            let subsystem = fields[2].strip_prefix('(').and_then(|value| value.strip_suffix(')'))
                .ok_or_else(|| invalid("udev header has no subsystem"))?;
            if !matches!(action, "add" | "remove" | "change" | "move" | "online" | "offline" | "bind" | "unbind") {
                return Err(invalid("unknown udev action"));
            }
            let mut properties = String::new();
            let mut size = header.len();
            loop {
                let line = bounded_line(&mut *self.reader).await?;
                size += line.len();
                if size > MAX_RECORD_BYTES { return Err(invalid("udev event exceeds 64 KiB")); }
                if line.trim().is_empty() { break; }
                properties.push_str(&line);
            }
            let properties = parse_properties(&properties).map_err(invalid)?;
            for (key, expected) in [("ACTION", action), ("DEVPATH", devpath), ("SUBSYSTEM", subsystem)] {
                if properties.get(key).map(String::as_str) != Some(expected) {
                    return Err(invalid(format!("udev header and {key} disagree")));
                }
            }
            if subsystem != "tty" { continue; }
            if !devpath.starts_with("/devices/") || devpath.split('/').any(|part| matches!(part, "." | "..")) {
                return Err(invalid("udev DEVPATH must stay inside /sys/devices"));
            }
            let devnode = properties.get("DEVNAME").cloned()
                .unwrap_or_else(|| format!("/dev/{}", devpath.rsplit('/').next().unwrap_or_default()));
            if !devnode.strip_prefix("/dev/").is_some_and(serial_kernel_name) { continue; }
            let sequence = properties.get("SEQNUM").map(|value| value.parse::<u64>())
                .transpose().map_err(|_| invalid("invalid udev SEQNUM"))?;
            return Ok(MonitorEvent { action: action.into(), devnode, devpath: devpath.into(), sequence });
        }
    }
}

async fn bounded_line<R: AsyncBufRead + Unpin + ?Sized>(reader: &mut R) -> io::Result<String> {
    let mut line = Vec::new();
    loop {
        let buffer = reader.fill_buf().await?;
        if buffer.is_empty() {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "udev monitor output closed"));
        }
        let end = buffer.iter().position(|byte| *byte == b'\n').map(|index| index + 1);
        let count = end.unwrap_or(buffer.len());
        if line.len() + count > MAX_RECORD_BYTES { return Err(invalid("udev monitor line exceeds 64 KiB")); }
        line.extend_from_slice(&buffer[..count]);
        reader.consume(count);
        if end.is_some() {
            return String::from_utf8(line).map_err(|_| invalid("udev monitor output is not UTF-8"));
        }
    }
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}
