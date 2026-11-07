//! Kernel peer credentials and local account lookups, never client JSON.

use std::ffi::{CStr, CString};
use std::fs;
use std::io;
use std::ptr;

use tokio::net::UnixStream;

use crate::Caller;

#[derive(Clone)]
pub(super) struct Peer {
    pub uid: u32,
    pub gid: u32,
    pub pid: u32,
    pub start_time: u64,
}

impl Peer {
    pub fn from_stream(stream: &UnixStream) -> io::Result<Self> {
        let cred = stream.peer_cred()?;
        let pid = cred.pid().ok_or_else(|| io::Error::other("SO_PEERCRED did not include a pid"))?;
        let pid = u32::try_from(pid).map_err(|_| io::Error::other("invalid peer pid"))?;
        Ok(Self { uid: cred.uid(), gid: cred.gid(), pid, start_time: process_start_time(pid)? })
    }

    pub fn caller(&self) -> io::Result<Caller> {
        if process_start_time(self.pid)? != self.start_time {
            return Err(io::Error::other("the socket peer process changed"));
        }
        let status = fs::read_to_string(format!("/proc/{}/status", self.pid))?;
        let fields = |key: &str| -> io::Result<Vec<u32>> {
            status.lines().find_map(|line| line.strip_prefix(key))
                .ok_or_else(|| io::Error::other(format!("missing peer {key}")))?
                .split_whitespace().map(|value| value.parse().map_err(|_| io::Error::other("invalid process credentials"))).collect()
        };
        if fields("Uid:")?.get(1) != Some(&self.uid) || fields("Gid:")?.get(1) != Some(&self.gid)
            || process_start_time(self.pid)? != self.start_time
        {
            return Err(io::Error::other("peer credentials no longer match SO_PEERCRED"));
        }
        let mut groups = fields("Groups:")?;
        groups.push(self.gid);
        groups.sort_unstable();
        groups.dedup();
        Ok(Caller { uid: self.uid, user: user_name(self.uid)?, groups, polkit_authorized: false })
    }

    pub fn polkit_subject(&self) -> String {
        format!("{},{},{}", self.pid, self.start_time, self.uid)
    }
}

fn process_start_time(pid: u32) -> io::Result<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let (_, fields) = stat.rsplit_once(") ").ok_or_else(|| io::Error::other("invalid process stat"))?;
    fields.split_whitespace().nth(19).ok_or_else(|| io::Error::other("missing process start time"))?
        .parse().map_err(|_| io::Error::other("invalid process start time"))
}

pub(super) fn user_name(uid: u32) -> io::Result<String> {
    let mut size = 16 * 1024;
    loop {
        let mut buffer = vec![0u8; size];
        let mut account = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut result = ptr::null_mut();
        // SAFETY: buffer and account remain alive until the lookup string is copied.
        let code = unsafe { libc::getpwuid_r(uid, account.as_mut_ptr(), buffer.as_mut_ptr().cast(), buffer.len(), &mut result) };
        if code == libc::ERANGE && size < 1024 * 1024 {
            size *= 2;
            continue;
        }
        if code != 0 {
            return Err(io::Error::from_raw_os_error(code));
        }
        if result.is_null() {
            return Ok(format!("uid:{uid}"));
        }
        // SAFETY: successful getpwuid_r initialized result and its NUL-terminated name.
        return Ok(unsafe { CStr::from_ptr((*result).pw_name) }.to_string_lossy().into_owned());
    }
}

pub fn group_id(name: &str) -> io::Result<Option<u32>> {
    let name = CString::new(name).map_err(|_| io::Error::other("invalid group name"))?;
    let mut size = 16 * 1024;
    loop {
        let mut buffer = vec![0u8; size];
        let mut group = std::mem::MaybeUninit::<libc::group>::uninit();
        let mut result = ptr::null_mut();
        // SAFETY: all output buffers are valid for the duration of the NSS lookup.
        let code = unsafe { libc::getgrnam_r(name.as_ptr(), group.as_mut_ptr(), buffer.as_mut_ptr().cast(), buffer.len(), &mut result) };
        if code == libc::ERANGE && size < 1024 * 1024 {
            size *= 2;
            continue;
        }
        if code != 0 {
            return Err(io::Error::from_raw_os_error(code));
        }
        // SAFETY: a non-null result from successful getgrnam_r is initialized.
        return Ok(if result.is_null() { None } else { Some(unsafe { (*result).gr_gid }) });
    }
}

pub(super) fn serial_group() -> Result<String, String> {
    for group in ["dialout", "uucp"] {
        if group_id(group).map_err(|error| error.to_string())?.is_some() {
            return Ok(group.into());
        }
    }
    Err("neither dialout nor uucp exists; create the serial access group before activating rules".into())
}
