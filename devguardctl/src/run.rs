//! `devguardctl run <name> -- <cmd>` (Linux): read the runtime env file
//! written by `apply`, set the same line discipline on the device, verify
//! the read-back and then exec the command with the device as its
//! stdin/stdout/stderr.  The daemon is not contacted: the env file is the
//! contract, so a stale devnode fails with a clear error instead of
//! silently reconfiguring something else.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::os::raw::c_int;

use devguard_core::runtime_env::RuntimeEnv;
use devguard_core::termios;

/// Default env location in system mode; `--env` overrides it (the sandbox
/// layout is `<root>/run/devguard/<name>.env`).
pub fn env_path(name: &str, override_path: Option<&Path>) -> PathBuf {
    override_path
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("/run/devguard").join(format!("{name}.env")))
}

fn open_device(devnode: &str) -> Result<c_int, String> {
    let path = std::ffi::CString::new(devnode)
        .map_err(|_| format!("`{devnode}` contains a NUL byte"))?;
    // SAFETY: path is a valid NUL-terminated string.
    let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDWR | libc::O_NOCTTY | libc::O_NONBLOCK) };
    if fd < 0 {
        return Err(format!(
            "cannot open {devnode}: {} (run apply on the current devnode?)",
            std::io::Error::last_os_error()
        ));
    }
    Ok(fd)
}

fn clear_nonblocking(fd: c_int) -> Result<(), String> {
    // SAFETY: fd is a valid open file descriptor.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    // SAFETY: same fd.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}

/// Set the line discipline and exec.  On success this never returns: the
/// process becomes `command` with the device on fds 0/1/2.  A termios
/// read-back mismatch aborts before the exec — `run` never hands a program
/// a wrongly configured port.
pub fn execute(env_file: &Path, command: &[OsString]) -> anyhow::Result<()> {
    let Some(program) = command.first() else {
        anyhow::bail!("run needs a command after `--`, e.g. `devguardctl run px4-main -- mavproxy ...`");
    };
    let text = std::fs::read_to_string(env_file)
        .map_err(|error| anyhow::anyhow!("cannot read {}: {error}; run `devguardctl apply <name>` first", env_file.display()))?;
    let env = RuntimeEnv::parse(&text).map_err(|error| {
        anyhow::anyhow!("{} is not a valid runtime env file: {error}", env_file.display())
    })?;
    termios::apply(&env.device, &env.serial)
        .map_err(|error| anyhow::anyhow!("cannot set the line discipline on {}: {error}", env.device))?;
    let fd = open_device(&env.device).map_err(anyhow::Error::msg)?;
    clear_nonblocking(fd).map_err(anyhow::Error::msg)?;
    // SAFETY: fd is a valid open descriptor; dup2 onto stdio is the classic
    // exec redirection.  dup2 clears CLOEXEC on the duplicates.
    for target in [0, 1, 2] {
        if unsafe { libc::dup2(fd, target) } < 0 {
            return Err(anyhow::anyhow!("dup2 to fd {target}: {}", std::io::Error::last_os_error()));
        }
    }
    // SAFETY: the original fd is still open and no longer needed.
    unsafe { libc::close(fd) };
    let mut process = std::process::Command::new(program);
    process.args(&command[1..]);
    use std::os::unix::process::CommandExt;
    let error = process.exec();
    Err(anyhow::anyhow!("cannot exec {}: {error}", program.to_string_lossy()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_env_path_follows_the_documented_layout() {
        assert_eq!(env_path("px4-main", None), PathBuf::from("/run/devguard/px4-main.env"));
        assert_eq!(env_path("px4-main", Some(Path::new("/tmp/x.env"))), PathBuf::from("/tmp/x.env"));
    }

    #[test]
    fn empty_command_is_rejected_before_touching_the_device() {
        let dir = tempfile::tempdir().unwrap();
        let error = execute(&dir.path().join("missing.env"), &[]).unwrap_err();
        assert!(error.to_string().contains("run needs a command"), "{error}");
    }

    #[test]
    fn missing_env_file_names_apply_as_the_next_step() {
        let dir = tempfile::tempdir().unwrap();
        let error = execute(&dir.path().join("missing.env"), &[OsString::from("true")]).unwrap_err();
        assert!(error.to_string().contains("run `devguardctl apply <name>` first"), "{error}");
    }
}
