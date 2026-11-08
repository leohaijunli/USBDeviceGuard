#![cfg(target_os = "linux")]

//! `devguardctl run` end-to-end on a real pty: the child receives the
//! device as its stdio with the configured line discipline already set.

use std::fs;
use std::os::fd::FromRawFd;
use std::os::raw::c_int;
use std::process::{Command, Stdio};

use devguard_core::config::{BaudRate, FlowControl, Parity, SerialConfig};
use devguard_core::runtime_env::RuntimeEnv;

fn pty_pair() -> (c_int, c_int, String) {
    let mut master: c_int = -1;
    let mut slave: c_int = -1;
    // SAFETY: both pointers are valid c_int outputs.
    if unsafe { libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) } != 0 {
        panic!("openpty: {}", std::io::Error::last_os_error());
    }
    let mut buffer = [0u8; 128];
    // SAFETY: buffer is writable for the full length.  ptsname_r takes the
    // master fd, not the slave.
    if unsafe { libc::ptsname_r(master, buffer.as_mut_ptr().cast(), buffer.len()) } != 0 {
        panic!("ptsname_r: {}", std::io::Error::last_os_error());
    }
    let path = std::ffi::CStr::from_bytes_until_nul(&buffer)
        .expect("ptsname_r NUL-terminates")
        .to_string_lossy()
        .into_owned();
    (master, slave, path)
}

#[test]
fn run_mode_sets_termios_and_the_child_sees_it_on_stdio() {
    let dir = tempfile::tempdir().unwrap();
    let (master, slave, path) = pty_pair();
    let env = RuntimeEnv::new(
        &path,
        SerialConfig { baud: BaudRate::Rate(115200), data_bits: 8, parity: Parity::N, stop_bits: 1, flow: FlowControl::None, raw: true },
    );
    let env_file = dir.path().join("radio-1.env");
    fs::write(&env_file, env.render()).unwrap();
    // Close our slave fd now: the open master keeps the pty (and its
    // /dev/pts path) alive, and the child opens its own.  If we kept it
    // open, the master read below would never see EOF.
    // SAFETY: slave came from openpty and is still open.
    unsafe { libc::close(slave) };

    let mut child = Command::new(env!("CARGO_BIN_EXE_devguardctl"))
        .args(["run", "radio-1", "--env"])
        .arg(&env_file)
        .args(["--", "/bin/sh", "-c", "stty -a"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn devguardctl run");
    let status = child.wait().expect("wait for run child");
    assert!(status.success(), "devguardctl run exited with {status}");

    // The child's stdio is the pty slave; read what `stty -a` printed.
    let mut output = String::new();
    let mut master_file = unsafe { fs::File::from_raw_fd(master) };
    use std::io::Read;
    match master_file.read_to_string(&mut output) {
        Ok(_) => {}
        Err(error) if error.raw_os_error() == Some(libc::EIO) => {}
        Err(error) => panic!("reading pty master: {error}"),
    }
    assert!(output.contains("115200"), "child did not see the applied baud: {output:?}");
    assert!(output.contains("cs8"), "child did not see 8 data bits: {output:?}");
    assert!(output.contains("-parenb"), "child did not see no parity: {output:?}");
}

#[test]
fn run_fails_clearly_when_the_env_device_is_gone() {
    let dir = tempfile::tempdir().unwrap();
    let env = RuntimeEnv::new(
        "/dev/this-tty-does-not-exist",
        SerialConfig { baud: BaudRate::Rate(115200), data_bits: 8, parity: Parity::N, stop_bits: 1, flow: FlowControl::None, raw: true },
    );
    let env_file = dir.path().join("radio-1.env");
    fs::write(&env_file, env.render()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_devguardctl"))
        .args(["run", "radio-1", "--env"])
        .arg(&env_file)
        .args(["--", "/bin/true"])
        .stderr(Stdio::piped())
        .output()
        .expect("run devguardctl");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    // termios::apply is the first thing that touches the device, so its
    // message surfaces; either way the devnode name is in the error.
    assert!(stderr.contains("/dev/this-tty-does-not-exist"), "{stderr}");
}
