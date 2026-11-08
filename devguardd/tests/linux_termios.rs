#![cfg(target_os = "linux")]

//! Phase-3 gate: termios set/read-back/drift against real pty pairs, plus
//! `LinuxIo::apply`/`check` on a pty-backed device.  No USB hardware here.

use std::fs;
use std::os::raw::c_int;

use devguard_core::config::{BaudRate, DeviceConfig, DeviceId, DeviceKind, FlowControl, IdRole, Parity, SerialConfig};
use devguard_core::identity::UdevFacts;
use devguard_core::runtime_env::RuntimeEnv;
use devguard_core::termios::{self, LineSettings};
use devguard_core::Paths;
use devguardd::io::DeviceIo;
use devguardd::linux::LinuxIo;

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

fn config_8n1(baud: BaudRate) -> SerialConfig {
    SerialConfig { baud, data_bits: 8, parity: Parity::N, stop_bits: 1, flow: FlowControl::None, raw: true }
}

fn set_master_baud(master: c_int, baud: u32) {
    let mut termios = unsafe { std::mem::zeroed::<libc::termios>() };
    // SAFETY: master is a valid tty fd.
    assert_eq!(unsafe { libc::tcgetattr(master, &mut termios) }, 0);
    let constant = match baud {
        9600 => libc::B9600,
        115200 => libc::B115200,
        other => panic!("unsupported test baud {other}"),
    };
    // SAFETY: termios is initialized.
    assert_eq!(unsafe { libc::cfsetospeed(&mut termios, constant) }, 0);
    assert_eq!(unsafe { libc::cfsetispeed(&mut termios, constant) }, 0);
    // SAFETY: master is a valid tty fd and termios is initialized.
    assert_eq!(unsafe { libc::tcsetattr(master, libc::TCSANOW, &termios) }, 0);
}

#[test]
fn apply_then_readback_matches_the_configuration() {
    let (_master, slave, path) = pty_pair();
    let _keep_slave_open = slave;
    let actual = termios::apply(&path, &config_8n1(BaudRate::Rate(115200))).expect("apply on a pty");
    assert_eq!(actual.baud, Some(115200));
    assert_eq!(actual.data_bits, 8);
    assert_eq!(actual.parity, Parity::N);
    assert_eq!(actual.stop_bits, 1);
    assert_eq!(actual.flow, FlowControl::None);
    assert!(actual.raw);
    let reread = termios::read(&path).expect("read back");
    assert_eq!(reread, actual);
    assert!(termios::drift(&config_8n1(BaudRate::Rate(115200)), &reread).is_empty());
}

#[test]
fn parity_stop_bits_and_flow_round_trip() {
    // Measured pty limitations (2026-11-07, Ubuntu 26.04 / 6.6 kernel):
    // the pty driver forces CSIZE to CS8 and drops (or rejects) PARENB, so
    // parity and data-bits 5-7 cannot round-trip here.  Baud, stop bits,
    // flow and raw mode do round-trip.  Parity/CSIZE verification belongs
    // to the real-device acceptance (cdc_acm/ftdi support them).
    let (_master, slave, path) = pty_pair();
    let _keep_slave_open = slave;
    let config = SerialConfig {
        baud: BaudRate::Rate(57600),
        data_bits: 8,
        parity: Parity::N,
        stop_bits: 2,
        flow: FlowControl::Software,
        raw: false,
    };
    let actual = termios::apply(&path, &config).expect("apply 8N2 software flow");
    assert_eq!(actual.baud, Some(57600));
    assert_eq!(actual.stop_bits, 2);
    assert_eq!(actual.flow, FlowControl::Software);
    assert!(!actual.raw);
    assert!(termios::drift(&config, &actual).is_empty());

    let hardware = SerialConfig { flow: FlowControl::Hardware, ..config };
    let actual = termios::apply(&path, &hardware).expect("apply hardware flow");
    assert_eq!(actual.flow, FlowControl::Hardware, "a pty stores CRTSCTS as a flag; real drivers may differ and their acceptance is a hardware gate");
}

#[test]
fn a_pty_never_claims_character_sizes_it_cannot_hold() {
    // Measured (2026-11-07, Ubuntu 26.04 / 6.6 kernel): the pty driver
    // forces CSIZE to CS8 regardless of the requested CS5-7, so an apply
    // of 7 data bits must fail on the read-back instead of claiming
    // success.  Parity (PARENB), stop bits, flow and baud do round-trip on
    // a pty; CSIZE 5-7 verification belongs to the real-device acceptance.
    let (_master, slave, path) = pty_pair();
    let _keep_slave_open = slave;
    let config = SerialConfig { data_bits: 7, parity: Parity::E, ..config_8n1(BaudRate::Rate(115200)) };
    let error = termios::apply(&path, &config).expect_err("a pty cannot hold CS7");
    assert!(error.contains("termios.data_bits") || error.contains("tcsetattr"), "{error}");
    let readback = termios::read(&path).expect("the pty still readable");
    assert_eq!(readback.data_bits, 8, "the pty keeps reporting CS8");
}

#[test]
fn baud_na_leaves_the_line_speed_untouched() {
    let (_master, slave, path) = pty_pair();
    let _keep_slave_open = slave;
    set_master_baud(_master, 9600);
    let actual = termios::apply(&path, &config_8n1(BaudRate::NotApplicable)).expect("apply with baud n/a");
    assert_eq!(actual.baud, Some(9600), "CDC-ACM mode must not change the speed");
    assert!(termios::drift(&config_8n1(BaudRate::NotApplicable), &actual).is_empty(), "n/a baud is never compared");
}

#[test]
fn drift_is_detected_after_an_external_change() {
    let (_master, slave, path) = pty_pair();
    let _keep_slave_open = slave;
    termios::apply(&path, &config_8n1(BaudRate::Rate(115200))).expect("apply");
    set_master_baud(_master, 9600);
    let actual = termios::read(&path).expect("read back after external change");
    let drift = termios::drift(&config_8n1(BaudRate::Rate(115200)), &actual);
    assert_eq!(drift, vec![("termios.baud".to_owned(), "115200".to_owned(), "9600".to_owned())]);
}

fn device_config(name: &str, baud: BaudRate) -> DeviceConfig {
    DeviceConfig {
        name: name.into(),
        kind: DeviceKind::Radio,
        description: None,
        ids: vec![DeviceId { vid: "1a86".into(), pid: "7523".into(), serial: None, path: Some("pty-port".into()), role: IdRole::App }],
        serial: config_8n1(baud),
        access: Default::default(),
    }
}

fn pty_facts(devnode: &str) -> UdevFacts {
    UdevFacts {
        devnode: devnode.into(),
        subsystem: "tty".into(),
        is_usb: true,
        driver: Some("pty".into()),
        ..UdevFacts::default()
    }
}

#[test]
fn linuxio_apply_verifies_and_writes_the_runtime_env_file() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::rooted(dir.path());
    let mut io = LinuxIo::new(paths.clone(), true);
    let (_master, slave, path) = pty_pair();
    let _keep_slave_open = slave;
    let config = device_config("radio-1", BaudRate::Rate(57600));
    let outcome = io.apply(&config, &pty_facts(&path)).expect("apply via LinuxIo");
    assert_eq!(outcome.devnode, path);
    let keys: Vec<&str> = outcome.applied.iter().map(|setting| setting.key.as_str()).collect();
    for key in ["termios.device", "termios.baud", "termios.data_bits", "termios.parity", "termios.stop_bits", "termios.flow", "termios.raw"] {
        assert!(keys.contains(&key), "missing {key} in {:?}", outcome.applied);
    }
    assert!(outcome.warnings.iter().any(|warning| warning.contains("autosuspend")));
    let env = RuntimeEnv::parse(&fs::read_to_string(paths.runtime_env("radio-1")).unwrap()).unwrap();
    assert_eq!(env.device, path);
    assert_eq!(env.serial, config_8n1(BaudRate::Rate(57600)));
}

#[test]
fn linuxio_check_reports_real_termios_drift() {
    let dir = tempfile::tempdir().unwrap();
    let io = LinuxIo::new(Paths::rooted(dir.path()), true);
    let (_master, slave, path) = pty_pair();
    let _keep_slave_open = slave;
    let config = device_config("radio-1", BaudRate::Rate(115200));
    termios::apply(&path, &config_8n1(BaudRate::Rate(115200))).expect("apply");
    let facts = pty_facts(&path);

    let report = io.check(&config, &facts);
    let item = report.items.iter().find(|item| item.id.ends_with(".termios")).expect("termios check item");
    assert_eq!(format!("{:?}", item.severity), "Ok", "{item:?}");

    set_master_baud(_master, 9600);
    let report = io.check(&config, &facts);
    let item = report.items.iter().find(|item| item.id.ends_with(".termios")).expect("termios check item");
    assert!(item.actual.as_deref().unwrap_or_default().contains("termios.baud=9600"), "{item:?}");
}

#[test]
fn check_surfaces_an_uninspectable_tty_instead_of_a_fake_pass() {
    let dir = tempfile::tempdir().unwrap();
    let plain = dir.path().join("plain-file");
    fs::write(&plain, b"not a tty").unwrap();
    let io = LinuxIo::new(Paths::rooted(dir.path()), true);
    let config = device_config("radio-1", BaudRate::Rate(115200));
    let report = io.check(&config, &pty_facts(plain.to_str().unwrap()));
    let item = report.items.iter().find(|item| item.id.ends_with(".termios")).expect("termios check item");
    assert!(item.actual.as_deref().unwrap_or_default().contains("tcgetattr"), "{item:?}");
}

#[test]
fn line_settings_defaults_are_explicit() {
    let settings = LineSettings {
        baud: None,
        data_bits: 8,
        parity: Parity::N,
        stop_bits: 1,
        flow: FlowControl::None,
        raw: true,
    };
    assert!(termios::drift(&config_8n1(BaudRate::NotApplicable), &settings).is_empty());
    assert_eq!(termios::drift(&config_8n1(BaudRate::Rate(9600)), &settings).len(), 1);
}
