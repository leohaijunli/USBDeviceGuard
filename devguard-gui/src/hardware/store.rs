use nsh_core::{prelude::*, time::now};

use super::arrivals::Arrivals;
use super::types::{HardwareEvent, UsbSerialDevice, UsbSnapshot};

/// Read-only device facts. This store never calls devguard's modify protocol.
#[derive(Clone, Copy)]
pub(crate) struct HardwareStore {
    snapshot: Signal<Option<UsbSnapshot>>,
    error: Signal<Option<SharedString>>,
    loading: Signal<bool>,
    watching: Signal<bool>,
    enabled: bool,
    arrivals: Stored<Arrivals>,
    pending_notices: Signal<Vec<UsbSerialDevice>>,
}

impl HardwareStore {
    pub(crate) fn new(enabled: bool) -> Self {
        let store = Self {
            snapshot: signal(None),
            error: signal(None),
            loading: signal(enabled),
            watching: signal(false),
            arrivals: stored(Arrivals::default()),
            pending_notices: signal(Vec::new()),
            enabled,
        };
        if enabled {
            #[cfg(target_os = "windows")]
            {
                effect(move || {
                    let (sender, mut receiver) = tokio::sync::mpsc::channel::<HardwareEvent>(8);
                    match super::windows::spawn_watch(sender) {
                        Ok(watcher) => cleanup(move || drop(watcher)),
                        Err(error) => {
                            store.receive(HardwareEvent::Error {
                                message: format!(
                                    "Could not start USB detection: {error}. Use Refresh to retry."
                                ),
                            });
                            return;
                        }
                    }
                    spawn(async move {
                        while let Some(event) = receiver.recv().await {
                            store.receive(event);
                        }
                        batch(|| {
                            store.watching.set(false);
                            store.loading.set(false);
                        });
                    });
                });
            }
            #[cfg(not(target_os = "windows"))]
            {
                store.loading.set(false);
                store.error.set(Some(
                    "This read-only diagnostic view uses Windows PnP. Linux USB management requires devguardd; use Records for phase 0 UI validation.".into(),
                ));
            }
        }
        store
    }

    pub(crate) fn scan_enabled(self) -> bool {
        self.enabled
    }

    pub(crate) fn loading(self) -> bool {
        self.loading.get()
    }

    pub(crate) fn error(self) -> Option<SharedString> {
        self.error.get()
    }

    pub(crate) fn count(self) -> usize {
        self.snapshot.with(|snapshot| {
            snapshot
                .as_ref()
                .map_or(0, |snapshot| snapshot.devices.len())
        })
    }

    pub(crate) fn ignored_count(self) -> usize {
        self.snapshot.with(|snapshot| {
            snapshot
                .as_ref()
                .map_or(0, |snapshot| snapshot.ignored_non_usb_ports.len())
        })
    }

    pub(crate) fn device(self, instance: &str) -> Option<UsbSerialDevice> {
        self.snapshot.with(|snapshot| {
            snapshot.as_ref().and_then(|snapshot| {
                snapshot
                    .devices
                    .iter()
                    .find(|device| device.instance_id.eq_ignore_ascii_case(instance))
                    .cloned()
            })
        })
    }

    pub(crate) fn ids(self, query: &str) -> Vec<String> {
        self.snapshot.with(|snapshot| {
            snapshot
                .as_ref()
                .map(|snapshot| {
                    snapshot
                        .devices
                        .iter()
                        .filter(|device| device.matches(query))
                        .map(|device| device.instance_id.clone())
                        .collect()
                })
                .unwrap_or_default()
        })
    }

    /// The newly appeared USB serial devices not yet announced, oldest first.
    /// Consuming clears them, so a toast host can announce each exactly once.
    pub(crate) fn take_new_devices(self) -> Vec<UsbSerialDevice> {
        let devices = self.pending_notices.get();
        if !devices.is_empty() {
            self.pending_notices.set(Vec::new());
        }
        devices
    }

    pub(crate) fn updated_label(self) -> SharedString {
        self.snapshot.with(|snapshot| {
            snapshot
                .as_ref()
                .map(|snapshot| {
                    format!(
                        "Updated at: {} · {}",
                        snapshot.captured_at, snapshot.computer
                    )
                    .into()
                })
                .unwrap_or_else(|| "No device snapshot yet".into())
        })
    }

    pub(crate) fn source_label(self) -> &'static str {
        if self.loading() {
            "Detecting USB serial devices…"
        } else if !self.enabled {
            "Live diagnostics disabled for this test session"
        } else if self.watching.get() {
            "Watching device changes"
        } else {
            "Automatic updates unavailable · Refresh to rescan"
        }
    }

    pub(crate) fn refresh(self) {
        if self.loading() || !self.enabled {
            return;
        }
        #[cfg(target_os = "windows")]
        {
            self.loading.set(true);
            self.error.set(None);
            let (sender, mut receiver) = tokio::sync::mpsc::channel::<HardwareEvent>(4);
            if let Err(error) = super::windows::spawn_scan(sender) {
                self.receive(HardwareEvent::Error {
                    message: format!(
                        "Could not start USB detection: {error}. Use Refresh to retry."
                    ),
                });
                return;
            }
            spawn(async move {
                while let Some(event) = receiver.recv().await {
                    self.receive(event);
                }
                self.loading.set(false);
            });
        }
    }

    fn receive(self, event: HardwareEvent) {
        match event {
            HardwareEvent::Snapshot { snapshot } => batch(|| {
                self.watching.set(snapshot.automatic_updates);
                let due = self
                    .arrivals
                    .with(|arrivals| arrivals.observe(&snapshot.devices, now()));
                if !due.is_empty() {
                    let mut pending = self.pending_notices.get_untracked();
                    pending.extend(due);
                    self.pending_notices.set(pending);
                }
                self.snapshot.set(Some(snapshot));
                self.error.set(None);
                self.loading.set(false);
            }),
            HardwareEvent::WatchWarning { message } | HardwareEvent::Error { message } => {
                batch(|| {
                    self.error.set(Some(message.into()));
                    self.loading.set(false);
                    self.watching.set(false);
                })
            }
        }
    }
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
