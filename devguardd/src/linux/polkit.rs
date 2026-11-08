//! Bounded pkcheck invocation. Only exit 0 authorizes; failures deny safely.

use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use tokio::sync::watch;

use super::accounts::Peer;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Authorized,
    Denied(&'static str),
}

pub struct Checker {
    program: PathBuf,
    deadline: Duration,
}

impl Default for Checker {
    fn default() -> Self {
        Self { program: PathBuf::from("/usr/bin/pkcheck"), deadline: Duration::from_secs(60) }
    }
}

impl Checker {
    /// Test seam replacing the system authorization checker with an arbitrary
    /// executable and deadline. Marked hidden because it is not a production
    /// configuration surface; the daemon still fails closed on any non-zero
    /// exit or a missing program.
    #[doc(hidden)]
    pub fn checker_for(program: PathBuf, deadline: Duration) -> Self {
        Self { program, deadline }
    }

    pub(super) async fn check(&self, peer: &Peer, action: &'static str, closing: &mut watch::Receiver<bool>) -> Decision {
        if *closing.borrow() { return Decision::Denied("service_stopping"); }
        let child = tokio::process::Command::new(&self.program)
            // pid,start-time,uid is required: bare PID authorization is racy.
            .args(["--action-id", action, "--process", &peer.polkit_subject(), "--allow-user-interaction"])
            // Use the subject's registered agent; never start a root-run text agent.
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
            .kill_on_drop(true).spawn();
        let Ok(mut child) = child else { return Decision::Denied("polkit_unavailable"); };
        let decision = tokio::select! {
            biased;
            _ = closing.changed() => Decision::Denied("service_stopping"),
            result = tokio::time::timeout(self.deadline, child.wait()) => match result {
                Ok(Ok(status)) => return classify(status),
                Ok(Err(_)) => Decision::Denied("polkit_check_failed"),
                Err(_) => Decision::Denied("polkit_timeout"),
            },
        };
        // Timeout/closing/wait error: explicitly kill and reap our helper.
        // kill_on_drop remains a fallback if the whole connection is aborted.
        let _ = child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
        decision
    }

    #[cfg(test)]
    pub(super) fn fixture(program: PathBuf, deadline: Duration) -> Self {
        Self { program, deadline }
    }
}

fn classify(status: ExitStatus) -> Decision {
    match status.code() {
        Some(0) => Decision::Authorized,
        Some(1) => Decision::Denied("polkit_not_authorized"),
        Some(2) => Decision::Denied("polkit_no_authentication_agent"),
        Some(3) => Decision::Denied("polkit_authentication_dismissed"),
        _ => Decision::Denied("polkit_check_failed"),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    use super::*;

    fn helper(directory: &Path, body: &str) -> PathBuf {
        let path = directory.join("pkcheck-fixture");
        fs::write(&path, format!("#!/bin/sh\nprintf '%s\\n' \"$$\" > \"$0.pid\"\nprintf '%s\\n' \"$@\" > \"$0.args\"\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    fn peer() -> Peer {
        let (stream, _other) = tokio::net::UnixStream::pair().unwrap();
        Peer::from_stream(&stream).unwrap()
    }

    #[tokio::test]
    async fn success_checks_the_exact_action_and_kernel_subject() {
        let dir = tempfile::tempdir().unwrap();
        let path = helper(dir.path(), "exit 0");
        let checker = Checker::fixture(path.clone(), Duration::from_secs(2));
        let subject = peer();
        let (_sender, mut closing) = watch::channel(false);
        assert_eq!(checker.check(&subject, "dev.devguard.modify", &mut closing).await, Decision::Authorized);
        let args = fs::read_to_string(path.with_extension("args")).unwrap();
        assert_eq!(args.lines().collect::<Vec<_>>(), ["--action-id", "dev.devguard.modify", "--process",
                                                    &subject.polkit_subject(), "--allow-user-interaction"]);
    }

    #[tokio::test]
    async fn all_nonzero_exits_deny_even_if_stdout_claims_authorization() {
        let dir = tempfile::tempdir().unwrap();
        for (code, reason) in [(1, "polkit_not_authorized"), (2, "polkit_no_authentication_agent"),
                               (3, "polkit_authentication_dismissed"), (126, "polkit_check_failed"),
                               (127, "polkit_check_failed"), (42, "polkit_check_failed")] {
            let path = helper(dir.path(), &format!("printf 'AUTHORIZED=yes\\n'; exit {code}"));
            let checker = Checker::fixture(path, Duration::from_secs(2));
            let (_sender, mut closing) = watch::channel(false);
            assert_eq!(checker.check(&peer(), "dev.devguard.pause", &mut closing).await, Decision::Denied(reason));
        }
    }

    #[tokio::test]
    async fn a_missing_checker_never_authorizes() {
        let dir = tempfile::tempdir().unwrap();
        let checker = Checker::fixture(dir.path().join("missing"), Duration::from_secs(1));
        let (_sender, mut closing) = watch::channel(false);
        assert_eq!(checker.check(&peer(), "dev.devguard.modify", &mut closing).await, Decision::Denied("polkit_unavailable"));
    }

    #[tokio::test]
    async fn a_checker_terminated_by_signal_never_authorizes() {
        let dir = tempfile::tempdir().unwrap();
        let checker = Checker::fixture(helper(dir.path(), "kill -TERM $$"), Duration::from_secs(2));
        let (_sender, mut closing) = watch::channel(false);
        assert_eq!(checker.check(&peer(), "dev.devguard.modify", &mut closing).await, Decision::Denied("polkit_check_failed"));
    }

    #[tokio::test]
    async fn timed_out_helper_is_killed_and_reaped() {
        let dir = tempfile::tempdir().unwrap();
        let path = helper(dir.path(), "exec /bin/sleep 60");
        let checker = Checker::fixture(path.clone(), Duration::from_millis(250));
        let (_sender, mut closing) = watch::channel(false);
        assert_eq!(checker.check(&peer(), "dev.devguard.modify", &mut closing).await, Decision::Denied("polkit_timeout"));
        let pid: u32 = fs::read_to_string(path.with_extension("pid")).unwrap().trim().parse().unwrap();
        assert!(!Path::new(&format!("/proc/{pid}")).exists(), "helper was not reaped");
    }

    #[tokio::test]
    async fn shutdown_cancels_a_pending_helper_without_waiting_for_its_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let path = helper(dir.path(), "exec /bin/sleep 60");
        let checker = Checker::fixture(path.clone(), Duration::from_secs(60));
        let subject = peer();
        let (sender, mut closing) = watch::channel(false);
        let task = tokio::spawn(async move { checker.check(&subject, "dev.devguard.modify", &mut closing).await });
        tokio::time::timeout(Duration::from_secs(2), async {
            while !path.with_extension("pid").exists() { tokio::time::sleep(Duration::from_millis(5)).await; }
        }).await.unwrap();
        sender.send(true).unwrap();
        let result = tokio::time::timeout(Duration::from_secs(3), task).await.unwrap().unwrap();
        assert_eq!(result, Decision::Denied("service_stopping"));
        let pid: u32 = fs::read_to_string(path.with_extension("pid")).unwrap().trim().parse().unwrap();
        assert!(!Path::new(&format!("/proc/{pid}")).exists());
    }

    #[tokio::test]
    async fn a_stopping_service_does_not_spawn_a_checker() {
        let dir = tempfile::tempdir().unwrap();
        let path = helper(dir.path(), "exit 0");
        let checker = Checker::fixture(path.clone(), Duration::from_secs(2));
        let (_sender, mut closing) = watch::channel(true);
        assert_eq!(checker.check(&peer(), "dev.devguard.modify", &mut closing).await, Decision::Denied("service_stopping"));
        assert!(!path.with_extension("pid").exists());
    }
}
