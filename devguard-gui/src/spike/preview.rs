//! A UI-only countdown to exercise the pause dialog and banner. This is not
//! the daemon's service state or enforcement timer.

use std::time::Duration;

use devguard_core::{policy::PAUSE_PRESETS, PauseKind};
use nsh_core::{
    prelude::*,
    time::{now, Instant},
};

use crate::copy;

#[derive(Clone, PartialEq)]
struct Preview {
    kind: PauseKind,
    deadline: Instant,
}

#[derive(Clone, Copy)]
pub(crate) struct PausePreview {
    current: Signal<Option<Preview>>,
    remaining: Signal<u64>,
    outcome: Signal<SharedString>,
}

impl PausePreview {
    pub(crate) fn new() -> Self {
        let preview = Self {
            current: signal(None),
            remaining: signal(0),
            outcome: signal(SharedString::from(
                "UI validation only · USB control is not connected",
            )),
        };
        effect(move || {
            if let Some(current) = preview.current.get() {
                // Only a visible UI countdown ticks. The event receiver never polls.
                // Replacing/ending a preview disposes this effect's previous task.
                spawn(async move {
                    loop {
                        let remaining = current.deadline.saturating_duration_since(now());
                        let seconds =
                            remaining.as_secs() + u64::from(remaining.subsec_nanos() != 0);
                        if seconds == 0 {
                            preview.finish("Countdown finished · preview resumed automatically");
                            break;
                        }
                        preview.remaining.set(seconds);
                        timer(Duration::from_secs(1).min(remaining)).await;
                    }
                });
            }
        });
        preview
    }

    pub(crate) fn start(self, kind: PauseKind, minutes: u32) {
        if !PAUSE_PRESETS.contains(&minutes) {
            return;
        }
        let duration = Duration::from_secs(u64::from(minutes) * 60);
        batch(|| {
            self.remaining.set(duration.as_secs());
            self.current.set(Some(Preview {
                kind,
                deadline: now() + duration,
            }));
        });
    }

    pub(crate) fn active(self) -> bool {
        self.current.with(Option::is_some)
    }

    pub(crate) fn label(self) -> SharedString {
        if let Some(current) = self.current.get() {
            let seconds = self.remaining.get();
            format!(
                "Pause preview · {} · {:02}:{:02}",
                copy::pause_kind(current.kind),
                seconds / 60,
                seconds % 60
            )
            .into()
        } else {
            self.outcome.get()
        }
    }

    pub(crate) fn end(self) {
        self.finish("Pause preview ended");
    }

    fn finish(self, message: &'static str) {
        batch(|| {
            self.current.set(None);
            self.remaining.set(0);
            self.outcome.set(message.into());
        });
    }
}
