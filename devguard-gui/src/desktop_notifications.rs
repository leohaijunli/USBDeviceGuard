//! The app-owned tray bridges notifications across the Win32 and engine threads.
//! No pointer or device operation crosses a window message: it only wakes a
//! bounded queue. Clicking a native notice restores the existing window.

use std::sync::{mpsc, Arc, Mutex};

use nsh_platform_windows::windows::Win32::{
    Foundation::{HWND, LPARAM, WPARAM},
    UI::{
        Shell::{
            Shell_NotifyIconW, NIF_INFO, NIIF_INFO, NIIF_NOSOUND, NIM_MODIFY, NOTIFYICONDATAW,
        },
        WindowsAndMessaging::{IsIconic, IsWindowVisible, PostMessageW, WM_APP},
    },
};
use tokio::sync::mpsc::{Receiver, Sender};

use crate::hardware::UsbSerialDevice;

pub(crate) const NOTICE_MESSAGE: u32 = WM_APP + 2;

pub(crate) enum DesktopEvent {
    Visible(bool),
    OpenDevice(String),
}

struct Arrival {
    id: String,
    summary: String,
    count: usize,
}

struct Bridge {
    window: isize,
    arrivals: mpsc::SyncSender<Arrival>,
    events: Mutex<Option<Receiver<DesktopEvent>>>,
}

// This binary has one app-owned top-level host. Registration is removed before
// its window is destroyed; headless tests and non-embedded roots have no bridge.
static BRIDGE: Mutex<Option<Arc<Bridge>>> = Mutex::new(None);

pub(crate) struct TrayNotifications {
    bridge: Arc<Bridge>,
    arrivals: mpsc::Receiver<Arrival>,
    events: Sender<DesktopEvent>,
    latest_id: Option<String>,
}

impl TrayNotifications {
    pub(crate) fn new(window: HWND) -> Self {
        let (send, arrivals) = mpsc::sync_channel(8);
        let (events, receive) = tokio::sync::mpsc::channel(32);
        let bridge = Arc::new(Bridge {
            window: window.0 as isize,
            arrivals: send,
            events: Mutex::new(Some(receive)),
        });
        *BRIDGE.lock().expect("notification bridge lock poisoned") = Some(bridge.clone());
        Self {
            bridge,
            arrivals,
            events,
            latest_id: None,
        }
    }

    pub(crate) fn visible(&self, visible: bool) {
        let _ = self.events.try_send(DesktopEvent::Visible(visible));
    }

    pub(crate) fn show_pending(&mut self, mut icon: NOTIFYICONDATAW) {
        let Some(arrival) = self.arrivals.try_iter().last() else {
            return;
        };
        self.latest_id = Some(arrival.id);
        icon.uFlags = NIF_INFO;
        icon.dwInfoFlags = NIIF_INFO | NIIF_NOSOUND;
        write_utf16(
            &mut icon.szInfoTitle,
            if arrival.count == 1 {
                "New USB serial device"
            } else {
                "New USB serial devices"
            },
        );
        let body = if arrival.count == 1 {
            format!("{}\nClick to view details", arrival.summary)
        } else {
            format!(
                "{} devices detected. Latest: {}\nClick to view details",
                arrival.count, arrival.summary
            )
        };
        write_utf16(&mut icon.szInfo, &body);
        // SAFETY: the icon and its host window are live on this Win32 UI thread.
        if !unsafe { Shell_NotifyIconW(NIM_MODIFY, &icon) }.as_bool() {
            eprintln!("devguard: Windows did not accept the device notification");
        }
    }

    pub(crate) fn open_latest(&self) {
        if let Some(id) = &self.latest_id {
            let _ = self.events.try_send(DesktopEvent::OpenDevice(id.clone()));
        }
    }
}

impl Drop for TrayNotifications {
    fn drop(&mut self) {
        if let Ok(mut registered) = BRIDGE.lock() {
            if registered
                .as_ref()
                .is_some_and(|bridge| Arc::ptr_eq(bridge, &self.bridge))
            {
                *registered = None;
            }
        }
    }
}

/// Engine-side input is awaited, never polled, and remains owned by the shell.
pub(crate) fn take_events() -> Option<Receiver<DesktopEvent>> {
    let bridge = BRIDGE.lock().ok()?.as_ref()?.clone();
    let events = bridge.events.lock().ok()?.take();
    events
}

/// Windows supplies the background notification; the in-app toast remains
/// available after a manual restore. Notification settings may suppress Windows UI.
pub(crate) fn notify_if_hidden(devices: &[UsbSerialDevice]) {
    let Some(device) = devices.last() else { return };
    let Some(bridge) = BRIDGE.lock().ok().and_then(|bridge| bridge.clone()) else {
        return;
    };
    let window = HWND(bridge.window as *mut std::ffi::c_void);
    // SAFETY: this handle was registered by the host; querying visibility does
    // not mutate the window or touch its thread-owned userdata.
    if unsafe { IsWindowVisible(window) }.as_bool() && !unsafe { IsIconic(window) }.as_bool() {
        return;
    }
    let arrival = Arrival {
        id: device.instance_id.clone(),
        summary: format!(
            "{} · {} · {}:{}",
            device.port, device.name, device.vid, device.pid
        ),
        count: devices.len(),
    };
    if bridge.arrivals.try_send(arrival).is_ok() {
        // SAFETY: message has no pointers; only the host drains the typed queue.
        let _ = unsafe { PostMessageW(Some(window), NOTICE_MESSAGE, WPARAM(0), LPARAM(0)) };
    }
}

fn write_utf16(buffer: &mut [u16], text: &str) {
    buffer.fill(0);
    let mut used = 0;
    for character in text.chars() {
        let mut units = [0; 2];
        let encoded = character.encode_utf16(&mut units);
        if used + encoded.len() >= buffer.len() {
            break;
        }
        buffer[used..used + encoded.len()].copy_from_slice(encoded);
        used += encoded.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_text_is_terminated_without_splitting_surrogate_pairs() {
        let mut buffer = [1; 4];
        write_utf16(&mut buffer, "a😀b");
        assert_eq!(String::from_utf16(&buffer[..3]).unwrap(), "a😀");
        assert_eq!(buffer[3], 0);
        write_utf16(&mut buffer, "ok");
        assert_eq!(buffer, [b'o' as u16, b'k' as u16, 0, 0]);
    }
}
