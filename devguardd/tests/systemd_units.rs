//! Offline packaging contracts; systemd execution is separately accepted on Linux.

const DAEMON: &str = include_str!("../../packaging/systemd/devguardd.service");
const APPLY: &str = include_str!("../../packaging/systemd/devguard-apply@.service");

fn value<'a>(unit: &'a str, section: &str, key: &str) -> Option<&'a str> {
    let mut in_section = false;
    for line in unit.lines().map(str::trim) {
        if line.starts_with('[') {
            in_section = line == format!("[{section}]");
        } else if in_section && !line.starts_with('#') {
            if let Some((name, value)) = line.split_once('=') {
                if name == key { return Some(value); }
            }
        }
    }
    None
}

#[test]
fn daemon_reports_readiness_and_restarts_only_after_failure() {
    assert_eq!(value(DAEMON, "Service", "Type"), Some("notify"));
    assert_eq!(value(DAEMON, "Service", "NotifyAccess"), Some("main"));
    assert_eq!(value(DAEMON, "Service", "Restart"), Some("on-failure"));
    assert_eq!(value(DAEMON, "Service", "KillMode"), Some("control-group"));
    assert_eq!(value(DAEMON, "Service", "ExecStart"), Some("/usr/bin/devguardd"));
}

#[test]
fn hotplug_hook_never_pulls_in_a_stopped_daemon() {
    assert_eq!(value(APPLY, "Service", "Type"), Some("oneshot"));
    assert_eq!(value(APPLY, "Service", "ExecCondition"), Some("/usr/bin/systemctl is-active --quiet devguardd.service"));
    assert_eq!(value(APPLY, "Unit", "Wants"), None);
    assert_eq!(value(APPLY, "Unit", "Requires"), None);
    assert_eq!(value(APPLY, "Unit", "After"), Some("devguardd.service systemd-udevd.service"));
    assert_eq!(value(APPLY, "Service", "ExecStart"), Some("/usr/bin/devguardctl --socket /run/devguard/ctl.sock apply --devnode /dev/%i"));
}
