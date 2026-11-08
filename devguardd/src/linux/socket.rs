//! Exclusive daemon ownership and conservative recovery of abandoned sockets.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
use std::path::Path;

pub(crate) struct InstanceLock {
    // The lock file stays on disk. Unlinking it while another process opens
    // it could create two lock inodes, defeating exclusive ownership.
    _file: File,
}

impl InstanceLock {
    pub(crate) fn acquire(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).create(true)
            .truncate(false).mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(path)?;
        let metadata = file.metadata()?;
        // SAFETY: geteuid only reads the process credentials.
        if !metadata.is_file() || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0 || metadata.nlink() != 1
        {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied,
                "daemon.lock must be an owned, private regular file without extra hard links"));
        }
        // SAFETY: the fd remains owned by file. flock is released on close,
        // including after SIGKILL; CLOEXEC prevents child processes keeping it.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(io::Error::other("another daemon holds the instance lock"));
        }
        Ok(Self { _file: file })
    }
}

pub(crate) fn recover_socket(path: &Path) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    // SAFETY: geteuid only reads the process credentials.
    if !metadata.file_type().is_socket() || metadata.uid() != unsafe { libc::geteuid() } || metadata.nlink() != 1 {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied,
            "refusing to replace an unowned/hard-linked socket, symlink or non-socket file"));
    }
    match probe(path) {
        // Only ECONNREFUSED proves there is no listener. EAGAIN/EINPROGRESS
        // can mean a full listen queue, not an abandoned socket.
        Err(error) if error.raw_os_error() == Some(libc::ECONNREFUSED) => {}
        Ok(()) => return Err(io::Error::new(io::ErrorKind::AddrInUse,
            "another process is listening on the control socket")),
        Err(error) => return Err(error),
    }
    let current = fs::symlink_metadata(path)?;
    if !current.file_type().is_socket() || current.dev() != metadata.dev()
        || current.ino() != metadata.ino() || current.uid() != metadata.uid() || current.nlink() != 1
    {
        return Err(io::Error::other("the control socket changed during recovery; it was preserved"));
    }
    fs::remove_file(path)
}

fn probe(path: &Path) -> io::Result<()> {
    let bytes = path.as_os_str().as_bytes();
    // SAFETY: zero is a valid initial representation of sockaddr_un.
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if bytes.len() >= address.sun_path.len() || bytes.contains(&0) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid Unix socket path"));
    }
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (target, source) in address.sun_path.iter_mut().zip(bytes) { *target = *source as libc::c_char; }
    // SAFETY: socket has no pointer arguments; flags make the probe bounded
    // even if an unrelated process has saturated its listen queue.
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK, 0) };
    if fd < 0 { return Err(io::Error::last_os_error()); }
    // SAFETY: fd is newly created and uniquely owned.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let length = (std::mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1) as libc::socklen_t;
    // SAFETY: address is initialized and length includes the NUL-terminated
    // pathname within it. OwnedFd closes the probe on every exit path.
    if unsafe { libc::connect(fd.as_raw_fd(), (&address as *const libc::sockaddr_un).cast(), length) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
