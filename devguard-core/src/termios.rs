//! Termios setting, read-back and drift comparison (Linux only; shared by
//! the daemon shell and `devguardctl run`).
//!
//! Phase-3 gate: everything here is exercised against real pty pairs in
//! `devguardd/tests/linux_termios.rs` before any real device is touched.  `apply`
//! opens the devnode with `O_RDWR|O_NOCTTY|O_NONBLOCK`, sets the line
//! discipline with `TCSANOW` (no flush of pending data) and reads the
//! settings back; `read` opens `O_RDONLY` for inspection only.  Reading
//! termios necessarily opens the tty, which some drivers treat as asserting
//! DTR — the same side effect `stty` has; no bytes are written to the line.

use std::ffi::CString;
use std::os::raw::c_int;

use crate::config::{FlowControl, Parity, SerialConfig};

/// What the kernel currently reports for one tty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineSettings {
    /// `None` when the kernel reports a speed without a `Bxxx` constant.
    pub baud: Option<u32>,
    pub data_bits: u8,
    pub parity: Parity,
    pub stop_bits: u8,
    pub flow: FlowControl,
    /// Canonical processing fully off (`cfmakeraw` state).
    pub raw: bool,
}

/// Per-field drift: `(key, expected, actual)`.
pub type Drift = Vec<(String, String, String)>;

fn c_path(path: &str) -> Result<CString, String> {
    CString::new(path).map_err(|_| format!("`{path}` contains a NUL byte"))
}

fn open_tty(devnode: &str, writable: bool) -> Result<c_int, String> {
    let path = c_path(devnode)?;
    let flags = if writable {
        libc::O_RDWR | libc::O_NOCTTY | libc::O_NONBLOCK
    } else {
        libc::O_RDONLY | libc::O_NOCTTY | libc::O_NONBLOCK
    };
    // SAFETY: `path` is a valid NUL-terminated string; open(2) has no other
    // invariants.  The fd is closed by the caller-provided scope.
    let fd = unsafe { libc::open(path.as_ptr(), flags) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(fd)
}

fn get_termios(fd: c_int) -> Result<libc::termios, String> {
    let mut termios = unsafe { std::mem::zeroed::<libc::termios>() };
    // SAFETY: fd is a valid tty fd and termios is a valid, zeroed buffer.
    if unsafe { libc::tcgetattr(fd, &mut termios) } != 0 {
        return Err(format!(
            "tcgetattr: {} (is this a tty?)",
            std::io::Error::last_os_error()
        ));
    }
    Ok(termios)
}

fn set_termios(fd: c_int, termios: &libc::termios) -> Result<(), String> {
    // TCSANOW: apply immediately without discarding pending input/output.
    // SAFETY: fd is a valid tty fd and termios points to initialized data.
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, termios) } != 0 {
        return Err(format!("tcsetattr: {}", std::io::Error::last_os_error()));
    }
    Ok(())
}

fn speed_constant(baud: u32) -> Option<libc::speed_t> {
    let speed = match baud {
        300 => libc::B300,
        600 => libc::B600,
        1200 => libc::B1200,
        1800 => libc::B1800,
        2400 => libc::B2400,
        4800 => libc::B4800,
        9600 => libc::B9600,
        19200 => libc::B19200,
        38400 => libc::B38400,
        57600 => libc::B57600,
        115200 => libc::B115200,
        230400 => libc::B230400,
        460800 => libc::B460800,
        500000 => libc::B500000,
        576000 => libc::B576000,
        921600 => libc::B921600,
        1_000_000 => libc::B1000000,
        1_152_000 => libc::B1152000,
        1_500_000 => libc::B1500000,
        2_000_000 => libc::B2000000,
        2_500_000 => libc::B2500000,
        3_000_000 => libc::B3000000,
        3_500_000 => libc::B3500000,
        4_000_000 => libc::B4000000,
        _ => return None,
    };
    Some(speed)
}

fn baud_of_speed(speed: libc::speed_t) -> Option<u32> {
    Some(match speed {
        libc::B0 => 0,
        libc::B300 => 300,
        libc::B600 => 600,
        libc::B1200 => 1200,
        libc::B1800 => 1800,
        libc::B2400 => 2400,
        libc::B4800 => 4800,
        libc::B9600 => 9600,
        libc::B19200 => 19200,
        libc::B38400 => 38400,
        libc::B57600 => 57600,
        libc::B115200 => 115200,
        libc::B230400 => 230400,
        libc::B460800 => 460800,
        libc::B500000 => 500000,
        libc::B576000 => 576000,
        libc::B921600 => 921600,
        libc::B1000000 => 1_000_000,
        libc::B1152000 => 1_152_000,
        libc::B1500000 => 1_500_000,
        libc::B2000000 => 2_000_000,
        libc::B2500000 => 2_500_000,
        libc::B3000000 => 3_000_000,
        libc::B3500000 => 3_500_000,
        libc::B4000000 => 4_000_000,
        _ => return None,
    })
}

fn settings_of(termios: &libc::termios) -> LineSettings {
    let baud = baud_of_speed(unsafe { libc::cfgetospeed(termios) });
    let data_bits = match termios.c_cflag & libc::CSIZE {
        libc::CS5 => 5,
        libc::CS6 => 6,
        libc::CS7 => 7,
        _ => 8,
    };
    let parity = if termios.c_cflag & libc::PARENB == 0 {
        Parity::N
    } else if termios.c_cflag & libc::PARODD != 0 {
        Parity::O
    } else {
        Parity::E
    };
    let stop_bits = if termios.c_cflag & libc::CSTOPB != 0 { 2 } else { 1 };
    let flow = if termios.c_cflag & libc::CRTSCTS != 0 {
        FlowControl::Hardware
    } else if termios.c_iflag & (libc::IXON | libc::IXOFF) != 0 {
        FlowControl::Software
    } else {
        FlowControl::None
    };
    let raw = termios.c_lflag & (libc::ICANON | libc::ECHO | libc::ISIG) == 0
        && termios.c_iflag & (libc::BRKINT | libc::ICRNL) == 0;
    LineSettings { baud, data_bits, parity, stop_bits, flow, raw }
}

fn configure(termios: &mut libc::termios, config: &SerialConfig) -> Result<(), String> {
    if config.raw {
        // SAFETY: termios points to initialized data.
        unsafe { libc::cfmakeraw(termios) };
    }
    termios.c_cflag = (termios.c_cflag & !libc::CSIZE)
        | match config.data_bits {
            5 => libc::CS5,
            6 => libc::CS6,
            7 => libc::CS7,
            8 => libc::CS8,
            other => return Err(format!("unsupported data bits {other} (5-8)")),
        };
    termios.c_cflag &= !libc::PARENB;
    match config.parity {
        Parity::N => {}
        Parity::E => termios.c_cflag |= libc::PARENB,
        Parity::O => termios.c_cflag |= libc::PARENB | libc::PARODD,
    }
    termios.c_cflag &= !libc::CSTOPB;
    if config.stop_bits == 2 {
        termios.c_cflag |= libc::CSTOPB;
    }
    termios.c_cflag &= !libc::CRTSCTS;
    termios.c_iflag &= !(libc::IXON | libc::IXOFF | libc::IXANY);
    match config.flow {
        FlowControl::None => {}
        FlowControl::Hardware => termios.c_cflag |= libc::CRTSCTS,
        FlowControl::Software => termios.c_iflag |= libc::IXON | libc::IXOFF,
    }
    if let Some(rate) = config.baud.rate() {
        let speed = speed_constant(rate)
            .ok_or_else(|| format!("baud {rate} has no Linux Bxxx constant"))?;
        // SAFETY: termios points to initialized data.
        unsafe {
            if libc::cfsetispeed(termios, speed) != 0 || libc::cfsetospeed(termios, speed) != 0 {
                return Err(format!(
                    "cfset{rate} speed: {}",
                    std::io::Error::last_os_error()
                ));
            }
        }
    }
    // Baud n/a (CDC-ACM): leave the line speed untouched on purpose.
    Ok(())
}

/// Set the line discipline described by `config` on `devnode`, read the
/// settings back and verify them.  Returns the verified read-back; a
/// mismatch is an error — the caller never gets an unverified "success".
/// `BaudRate::NotApplicable` leaves the speed as-is (and never compares it);
/// every other field is always written and verified.
pub fn apply(devnode: &str, config: &SerialConfig) -> Result<LineSettings, String> {
    let fd = open_tty(devnode, true)?;
    let result: Result<LineSettings, String> = (|| {
        let mut termios = get_termios(fd)?;
        configure(&mut termios, config)?;
        set_termios(fd, &termios)?;
        let readback = get_termios(fd)?;
        Ok(settings_of(&readback))
    })();
    // SAFETY: fd came from open above and is still open.
    unsafe { libc::close(fd) };
    let actual = result?;
    let drift = drift(config, &actual);
    if !drift.is_empty() {
        return Err(format!(
            "termios read-back differs from the configuration: {}",
            describe_drift(&drift)
        ));
    }
    Ok(actual)
}

/// One-line description of read-back drift for errors and check reports.
pub fn describe_drift(drift: &Drift) -> String {
    drift
        .iter()
        .map(|(key, expected, got)| format!("{key}: expected {expected}, got {got}"))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Read the current line discipline without changing it.
pub fn read(devnode: &str) -> Result<LineSettings, String> {
    let fd = open_tty(devnode, false)?;
    let result = get_termios(fd).map(|termios| settings_of(&termios));
    // SAFETY: fd came from open above and is still open.
    unsafe { libc::close(fd) };
    result
}

/// Compare a configuration against read-back settings.  `baud = "n/a"` is
/// never compared (CDC-ACM does not report a verified line speed).
pub fn drift(config: &SerialConfig, actual: &LineSettings) -> Drift {
    let mut drift = Vec::new();
    if let Some(expected) = config.baud.rate() {
        if actual.baud != Some(expected) {
            drift.push((
                "termios.baud".into(),
                expected.to_string(),
                actual.baud.map(|b| b.to_string()).unwrap_or_else(|| "unknown".into()),
            ));
        }
    }
    if config.data_bits != actual.data_bits {
        drift.push((
            "termios.data_bits".into(),
            config.data_bits.to_string(),
            actual.data_bits.to_string(),
        ));
    }
    if config.parity != actual.parity {
        drift.push((
            "termios.parity".into(),
            config.parity.serial_key().to_owned(),
            actual.parity.serial_key().to_owned(),
        ));
    }
    if config.stop_bits != actual.stop_bits {
        drift.push((
            "termios.stop_bits".into(),
            config.stop_bits.to_string(),
            actual.stop_bits.to_string(),
        ));
    }
    if config.flow != actual.flow {
        drift.push((
            "termios.flow".into(),
            config.flow.to_string(),
            actual.flow.to_string(),
        ));
    }
    if config.raw != actual.raw {
        drift.push((
            "termios.raw".into(),
            u8::from(config.raw).to_string(),
            u8::from(actual.raw).to_string(),
        ));
    }
    drift
}
