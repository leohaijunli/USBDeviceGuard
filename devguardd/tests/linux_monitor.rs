#![cfg(target_os = "linux")]

//! The udev byte stream is the external input; no real USB is changed.

use devguardd::linux::monitor::UdevMonitor;
use tokio::io::BufReader;

#[tokio::test]
async fn a_ready_udev_stream_yields_the_usb_serial_event() {
    let text = concat!(
        "monitor will print the received events for:\n",
        "UDEV - the event which udev sends out after rule processing\n\n",
        "UDEV  [123.456789] add      /devices/pci0/usb1/1-2/ttyUSB0 (tty)\n",
        "ACTION=add\nDEVPATH=/devices/pci0/usb1/1-2/ttyUSB0\n",
        "SUBSYSTEM=tty\nDEVNAME=/dev/ttyUSB0\nSEQNUM=41\n\n",
    );
    let mut monitor = UdevMonitor::from_reader(BufReader::new(text.as_bytes())).await.unwrap();
    let event = monitor.next_event().await.unwrap();
    assert_eq!((event.action.as_str(), event.devnode.as_str(), event.sequence), ("add", "/dev/ttyUSB0", Some(41)));
}

#[tokio::test]
async fn stopping_a_monitor_reaps_its_owned_process() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("udevadm-fixture");
    let pid_file = dir.path().join("pid");
    let script = format!("#!/usr/bin/python3\nimport os, pathlib, signal\npathlib.Path({:?}).write_text(str(os.getpid()))\nprint('UDEV - the event which udev sends out after rule processing', flush=True)\nsignal.pause()\n", pid_file.to_string_lossy());
    fs::write(&program, script).unwrap();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
    let monitor = UdevMonitor::start_command(&program).await.unwrap();
    let pid: i32 = fs::read_to_string(pid_file).unwrap().parse().unwrap();
    monitor.stop().await.unwrap();
    // SAFETY: signal 0 only probes the fixture pid; it never delivers a signal.
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
}

#[tokio::test]
async fn irrelevant_subsystems_are_skipped_without_selecting_them() {
    let text = concat!(
        "UDEV - the event which udev sends out after rule processing\n\n",
        "UDEV [1.0] add /devices/usb1/1-2/input0 (input)\n",
        "ACTION=add\nDEVPATH=/devices/usb1/1-2/input0\nSUBSYSTEM=input\nDEVNAME=/dev/input/event0\n\n",
        "UDEV [1.1] remove /devices/usb1/1-3/ttyACM2 (tty)\n",
        "ACTION=remove\nDEVPATH=/devices/usb1/1-3/ttyACM2\nSUBSYSTEM=tty\nSEQNUM=99\n\n",
    );
    let mut monitor = UdevMonitor::from_reader(BufReader::new(text.as_bytes())).await.unwrap();
    let event = monitor.next_event().await.unwrap();
    assert_eq!((event.action.as_str(), event.devnode.as_str()), ("remove", "/dev/ttyACM2"));
}

#[tokio::test]
async fn malformed_and_truncated_records_are_not_accepted() {
    for record in [
        "UDEV [1.0] add /devices/usb1/ttyUSB0 (tty)\nACTION=remove\nDEVPATH=/devices/usb1/ttyUSB0\nSUBSYSTEM=tty\n\n",
        "UDEV [1.0] add /devices/../../ttyUSB0 (tty)\nACTION=add\nDEVPATH=/devices/../../ttyUSB0\nSUBSYSTEM=tty\n\n",
        "UDEV [1.0] add /devices/usb1/ttyUSB0 (tty)\nACTION=add\n",
    ] {
        let text = format!("UDEV - the event which udev sends out after rule processing\n\n{record}");
        let mut monitor = UdevMonitor::from_reader(BufReader::new(std::io::Cursor::new(text.into_bytes()))).await.unwrap();
        assert!(monitor.next_event().await.is_err());
    }
}

#[tokio::test]
async fn oversized_monitor_output_is_bounded() {
    let text = format!("UDEV - the event which udev sends out after rule processing\n\n{}\n", "x".repeat(64 * 1024 + 1));
    let mut monitor = UdevMonitor::from_reader(BufReader::new(std::io::Cursor::new(text.into_bytes()))).await.unwrap();
    assert_eq!(monitor.next_event().await.unwrap_err().kind(), std::io::ErrorKind::InvalidData);
}
