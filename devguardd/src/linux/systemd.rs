//! Notify systemd only after the RPC worker and initial snapshot are ready.

use std::ffi::OsStr;
use std::io;
use std::os::linux::net::SocketAddrExt;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::{SocketAddr, UnixDatagram};
use std::time::Duration;

pub(crate) fn ready() -> io::Result<()> {
    notify(std::env::var_os("NOTIFY_SOCKET").as_deref())
}

fn notify(destination: Option<&OsStr>) -> io::Result<()> {
    let Some(destination) = destination else { return Ok(()); };
    let bytes = destination.as_bytes();
    let address = if let Some(name) = bytes.strip_prefix(b"@") {
        if name.is_empty() { return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty notify socket")); }
        SocketAddr::from_abstract_name(name)?
    } else if bytes.starts_with(b"/") {
        SocketAddr::from_pathname(destination)?
    } else {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "NOTIFY_SOCKET must be absolute or abstract"));
    };
    let socket = UnixDatagram::unbound()?;
    socket.set_write_timeout(Some(Duration::from_secs(1)))?;
    socket.send_to_addr(b"READY=1\nSTATUS=Audit mode; USB serial monitoring ready", &address)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_start_does_not_need_a_notify_socket() {
        notify(None).unwrap();
    }

    #[test]
    fn readiness_is_sent_to_a_filesystem_socket() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notify.sock");
        let receiver = UnixDatagram::bind(&path).unwrap();
        receiver.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        notify(Some(path.as_os_str())).unwrap();
        let mut bytes = [0; 128];
        let size = receiver.recv(&mut bytes).unwrap();
        assert!(bytes[..size].starts_with(b"READY=1\nSTATUS="));
    }

    #[test]
    fn readiness_supports_systemds_abstract_socket() {
        let name = format!("devguard-notify-{}", std::process::id());
        let receiver = UnixDatagram::bind_addr(&SocketAddr::from_abstract_name(name.as_bytes()).unwrap()).unwrap();
        receiver.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        notify(Some(OsStr::new(&format!("@{name}")))).unwrap();
        let mut bytes = [0; 128];
        let size = receiver.recv(&mut bytes).unwrap();
        assert!(bytes[..size].starts_with(b"READY=1\n"));
    }

    #[test]
    fn invalid_or_unreachable_destinations_are_errors() {
        assert!(notify(Some(OsStr::new("relative.sock"))).is_err());
        assert!(notify(Some(OsStr::new("@"))).is_err());
        let dir = tempfile::tempdir().unwrap();
        assert!(notify(Some(dir.path().join("missing.sock").as_os_str())).is_err());
    }
}
