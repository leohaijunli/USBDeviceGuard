//! Windows USB serial diagnostics, read-only.
//!
//! `nsh_core::background` must not be called from the engine thread: in NSH
//! 0.4.0's TSX headless runner there is no Tokio reactor there and it panics.
//! Every child process therefore runs on its own OS thread and talks to the
//! engine through the same bounded channel pattern as the spike worker; the
//! engine side only ever awaits `Receiver::recv()`.

use std::{
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    sync::{Arc, Condvar, Mutex},
    thread,
    time::Duration,
};

use base64::{engine::general_purpose::STANDARD, Engine};
use tokio::sync::mpsc::Sender;

use super::types::{HardwareEvent, UsbSnapshot};

const SCRIPT: &str = include_str!("../../assets/windows-usb.ps1");
const STARTUP_TIMEOUT: Duration = Duration::from_secs(25);
const MAX_LINE: usize = 1024 * 1024;

fn command(watch: bool) -> Command {
    use std::os::windows::process::CommandExt;

    let script = format!("& {{\n{SCRIPT}\n}}{}", if watch { " -Watch" } else { "" });
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let powershell = std::env::var_os("SystemRoot")
        .map(|root| {
            std::path::PathBuf::from(root).join("System32/WindowsPowerShell/v1.0/powershell.exe")
        })
        .unwrap_or_else(|| {
            std::path::PathBuf::from(
                "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe",
            )
        });
    let mut command = Command::new(powershell);
    command
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-EncodedCommand",
            &STANDARD.encode(bytes),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // CREATE_NO_WINDOW: the diagnostic child must not flash a console.
    command.creation_flags(0x08000000);
    command
}

fn spawn_thread(name: &'static str, body: impl FnOnce() + Send + 'static) -> std::io::Result<()> {
    thread::Builder::new()
        .name(name.to_owned())
        .spawn(body)
        .map(|_| ())
}

fn send(sender: &Sender<HardwareEvent>, event: HardwareEvent) -> bool {
    sender.blocking_send(event).is_ok()
}

fn spawn_error(error: std::io::Error) -> HardwareEvent {
    HardwareEvent::Error {
        message: format!("Could not start Windows USB diagnostics: {error}. Use Refresh to retry."),
    }
}

/// A kill switch shared with the reader: if the child produces nothing within
/// `STARTUP_TIMEOUT`, the watchdog kills it and the read loop ends.
struct Guard {
    child: Arc<Mutex<Child>>,
    done: Arc<(Mutex<bool>, Condvar)>,
}

impl Guard {
    fn new(child: Child) -> Self {
        Self {
            child: Arc::new(Mutex::new(child)),
            done: Arc::new((Mutex::new(false), Condvar::new())),
        }
    }

    fn arm(&self) {
        let child = Arc::clone(&self.child);
        let done = Arc::clone(&self.done);
        thread::spawn(move || {
            let (lock, cvar) = &*done;
            let guard = lock.lock().expect("watchdog lock poisoned");
            let (done, _) = cvar
                .wait_timeout_while(guard, STARTUP_TIMEOUT, |done| !*done)
                .expect("watchdog condvar poisoned");
            if !*done {
                if let Ok(mut child) = child.lock() {
                    let _ = child.kill();
                }
            }
        });
    }

    fn finish(&self) {
        let (lock, cvar) = &*self.done;
        *lock.lock().expect("watchdog lock poisoned") = true;
        cvar.notify_all();
    }

    fn output(&self) -> Option<BufReader<std::process::ChildStdout>> {
        let mut child = self.child.lock().expect("child lock poisoned");
        child.stdout.take().map(BufReader::new)
    }

    fn kill(&self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
        }
    }

    fn wait(&self) -> bool {
        matches!(
            self.child.lock().expect("child lock poisoned").wait(),
            Ok(status) if status.success()
        )
    }
}

/// The watcher owns the child for as long as the engine reads its channel; a
/// dropped receiver ends `blocking_send`, and the thread kills the child.
pub(crate) fn spawn_watch(sender: Sender<HardwareEvent>) -> std::io::Result<()> {
    spawn_thread("devguard-usb-watch", move || {
        let child = match command(true).spawn() {
            Ok(child) => child,
            Err(error) => {
                let _ = send(&sender, spawn_error(error));
                return;
            }
        };
        let guard = Guard::new(child);
        guard.arm();
        let mut first = true;
        if let Some(reader) = guard.output() {
            for line in reader.lines() {
                let Ok(line) = line else { break };
                if first {
                    guard.finish();
                    first = false;
                }
                if !forward_line(&sender, &line) {
                    guard.kill();
                    return;
                }
            }
        }
        if first {
            guard.finish();
            guard.kill();
            let _ = send(
                &sender,
                HardwareEvent::Error {
                    message: "USB detection took too long. Use Refresh to retry.".into(),
                },
            );
            return;
        }
        if !guard.wait() {
            let _ = send(
                &sender,
                HardwareEvent::Error {
                    message: "Windows USB diagnostics stopped unexpectedly. Use Refresh to retry."
                        .into(),
                },
            );
        }
    })
}

/// One-shot scan; the engine receives exactly one event, then stops reading.
pub(crate) fn spawn_scan(sender: Sender<HardwareEvent>) -> std::io::Result<()> {
    spawn_thread("devguard-usb-scan", move || {
        let child = match command(false).spawn() {
            Ok(child) => child,
            Err(error) => {
                let _ = send(&sender, spawn_error(error));
                return;
            }
        };
        let guard = Guard::new(child);
        guard.arm();
        let mut snapshot: Option<UsbSnapshot> = None;
        if let Some(reader) = guard.output() {
            for line in reader.lines().map_while(Result::ok) {
                guard.finish();
                match decode(&line) {
                    Ok(HardwareEvent::Snapshot { snapshot: found }) => {
                        snapshot = Some(found);
                        break;
                    }
                    Ok(HardwareEvent::Error { message }) => {
                        let _ = send(&sender, HardwareEvent::Error { message });
                        guard.kill();
                        return;
                    }
                    Ok(HardwareEvent::WatchWarning { .. }) => {}
                    Err(message) => {
                        let _ = send(&sender, HardwareEvent::Error { message });
                        guard.kill();
                        return;
                    }
                }
            }
        }
        guard.finish();
        guard.wait();
        match snapshot {
            Some(snapshot) => {
                let _ = send(&sender, HardwareEvent::Snapshot { snapshot });
            }
            None => {
                let _ = send(
                    &sender,
                    HardwareEvent::Error {
                        message: "USB diagnostics returned no snapshot. Use Refresh to retry."
                            .into(),
                    },
                );
            }
        }
    })
}

fn forward_line(sender: &Sender<HardwareEvent>, line: &str) -> bool {
    match decode(line) {
        Ok(event) => send(sender, event),
        Err(message) => send(sender, HardwareEvent::Error { message }),
    }
}

fn decode(line: &str) -> Result<HardwareEvent, String> {
    if line.trim().is_empty() {
        return Err("USB diagnostics produced an empty event.".into());
    }
    if line.len() > MAX_LINE {
        return Err("USB diagnostic output exceeded the size limit.".into());
    }
    serde_json::from_str(line)
        .map_err(|error| format!("Invalid USB diagnostic event: {error}. Use Refresh to retry."))
}
