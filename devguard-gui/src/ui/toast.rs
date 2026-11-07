//! Non-modal USB arrival notices. Lifecycle belongs to NSH's ToastManager;
//! device identity and navigation remain owned by the store and shell.

use std::time::Duration;

use nsh_base::{
    theme, toast, toast_stack, Anchor, ToastManager, ToastMotion, ToastOptions, ToastStackState,
    ToastTransitionStatus,
};
use nsh_core::{prelude::*, time::now};

use crate::{hardware::HardwareStore, ui};

const NOTICE_TIMEOUT: Duration = Duration::from_secs(6);

#[derive(Clone, PartialEq)]
struct Notice {
    id: String,
    label: SharedString,
    status: ToastTransitionStatus,
}

/// A handle the shell keeps: the toast host lives as long as the shell does.
#[derive(Clone, Copy)]
pub(crate) struct ToastHost {
    manager: Stored<ToastManager<String, SharedString>>,
    shown: Signal<Vec<Notice>>,
    stack_state: ToastStackState,
}

impl ToastHost {
    /// A modal pauses notices without changing its focus or dismissing its draft.
    pub(crate) fn new(
        store: HardwareStore,
        blocked: impl Fn() -> bool + Copy + 'static,
        visible: impl Fn() -> bool + Copy + 'static,
    ) -> Self {
        let host = Self {
            manager: stored(ToastManager::<String, SharedString>::new(
                ToastMotion::sonner(),
            )),
            shown: signal(Vec::new()),
            stack_state: ToastStackState::new(),
        };

        effect(move || {
            let due = store.take_new_devices();
            if due.is_empty() {
                return;
            }
            untracked(|| {
                #[cfg(target_os = "windows")]
                crate::desktop_notifications::notify_if_hidden(&due);
                host.manager.with(|m| {
                    for device in due {
                        let label = format!(
                            "{} · {} · {}:{}",
                            device.port, device.name, device.vid, device.pid
                        );
                        m.push(
                            device.instance_id,
                            label.into(),
                            ToastOptions::timeout(Some(NOTICE_TIMEOUT)),
                            now(),
                        );
                    }
                });
                host.sync();
            });
        });

        // The timer exists only while notices are mounted. It drives animation
        // and dismissal, never device detection or queue polling.
        let active = computed(move || host.shown.with(|notices| !notices.is_empty()));
        effect(move || {
            let visible = visible();
            if !active.get() {
                return;
            }
            // Discard the elapsed hidden-window time on hide/restore. No toast
            // timer runs in a hidden window; visibility arrives over a channel.
            untracked(|| {
                if host.manager.with(|m| m.advance(now(), true).changed) {
                    host.sync();
                }
            });
            if visible {
                set_interval(Duration::from_millis(50), move || {
                    let paused = blocked() || host.stack_state.is_expanded();
                    if host.manager.with(|m| m.advance(now(), paused).changed) {
                        host.sync();
                    }
                });
            }
        });
        host
    }

    fn sync(self) {
        self.shown.set(self.manager.with(|m| {
            m.visible(3)
                .map(|(id, value, status)| Notice {
                    id: id.clone(),
                    label: value.clone(),
                    status,
                })
                .collect()
        }));
    }

    fn dismiss(self, id: &str) {
        if self.manager.with(|m| m.dismiss(&id.to_owned(), now())) {
            self.sync();
        }
    }

    fn dismiss_newest(self) {
        let newest = self.shown.with_untracked(|notices| {
            notices
                .iter()
                .rev()
                .find(|notice| notice.status != ToastTransitionStatus::Ending)
                .map(|notice| notice.id.clone())
        });
        if let Some(id) = newest {
            self.dismiss(&id);
        }
    }
}

/// The layer itself; mounted by the shell at the window edge. Tapping "View"
/// dismisses the toast and asks the shell to open that device's details.
pub(crate) fn layer(
    host: ToastHost,
    store: HardwareStore,
    blocked: impl Fn() -> bool + Copy + 'static,
    focus_device: impl Fn(String) + Copy + 'static,
) -> impl IntoElement {
    toast_stack()
        .placement(Anchor::BottomRight)
        .state(host.stack_state)
        .close_on_escape(move || !blocked())
        .on_cancel(move || host.dismiss_newest())
        .absolute()
        .right_4()
        .bottom_12()
        .w(move || ui::tokens::length("notice-width").min(window().width - ui::tokens::space(8.0)))
        .child(each(
            host.shown,
            |notice| notice.id.clone(),
            move |item, _| {
                let connected = move || store.device(&item.get().id).is_some();
                let dismiss = move || host.dismiss(&item.get_untracked().id);
                let open_device = move || {
                    let id = item.get_untracked().id;
                    if !blocked() && store.device(&id).is_some() {
                        host.dismiss(&id);
                        focus_device(id);
                    }
                };
                toast()
                    .transition_status(move || item.get().status)
                    .accessibility_label(move || {
                        Some(format!("New USB serial device: {}", item.get().label).into())
                    })
                    .on_dismiss_request(dismiss)
                    .rounded(move || ui::tokens::length("radius-panel"))
                    .px_4()
                    .py_3()
                    .gap_2()
                    .bg(move || theme().popover)
                    .text_color(move || theme().popover_foreground)
                    .border_1()
                    .border_color(move || theme().border)
                    .shadow_lg()
                    .transition_opacity()
                    .duration(move || -> f32 {
                        if reduce_motion() {
                            0.0
                        } else {
                            200.0
                        }
                    })
                    .starting_class("opacity-0 translate-y-2")
                    .ending_class("opacity-0 scale-95")
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(
                                text("New USB serial device")
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .flex_1(),
                            )
                            .child(
                                ui::action("View", ui::ActionStyle::Quiet, open_device)
                                    .disabled(move || blocked() || !connected()),
                            ),
                    )
                    .child(ui::hint(move || item.get().label))
                    .child(when(
                        move || !connected(),
                        || ui::hint("Device disconnected"),
                    ))
                    .child(ui::action("Dismiss", ui::ActionStyle::Quiet, dismiss).self_end())
            },
        ))
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use nsh_engine::{Clock, EngineConfig, Root};
    use nsh_testing::Headless;

    use super::*;

    #[component]
    fn VisibilityFixture() -> impl IntoElement {
        ui::tokens::install();
        let visible = signal(true);
        let store = HardwareStore::new(false);
        let host = ToastHost::new(store, || false, move || visible.get());
        view()
            .flex_1()
            .child(ui::action(
                "Fixture: show notice",
                ui::ActionStyle::Standard,
                move || {
                    host.manager.with(|m| {
                        m.push(
                            "fixture".into(),
                            "Isolated visibility test".into(),
                            ToastOptions::timeout(Some(NOTICE_TIMEOUT)),
                            now(),
                        );
                    });
                    host.sync();
                },
            ))
            .child(ui::action(
                "Fixture: hide",
                ui::ActionStyle::Standard,
                move || visible.set(false),
            ))
            .child(ui::action(
                "Fixture: restore",
                ui::ActionStyle::Standard,
                move || visible.set(true),
            ))
            .child(
                view()
                    .relative()
                    .flex_1()
                    .style_when(move || !visible.get(), |s| s.hidden())
                    .child(layer(host, store, || false, |_| {})),
            )
    }

    #[test]
    fn hidden_window_retains_the_notice_until_restore_then_resumes_its_timeout(
    ) -> Result<(), String> {
        let mut app = Headless::start(EngineConfig {
            viewport: Size::new(px(900.0), px(700.0)),
            clock: Clock::manual(SystemTime::UNIX_EPOCH),
            ..EngineConfig::new(Root::of::<VisibilityFixture>())
        })
        .map_err(|error| error.to_string())?;
        app.wait_for("role:button[Fixture: show notice]")?;
        app.press("role:button[Fixture: show notice]")?;
        app.wait_for("role:alert")?;
        app.run_for(Duration::from_secs(1))?;
        app.press("role:button[Fixture: hide]")?;
        app.fast_forward(Duration::from_secs(600))?;
        app.press("role:button[Fixture: restore]")?;
        app.wait_for("role:alert")?;
        app.run_for(Duration::from_secs(1))?;
        app.fast_forward(Duration::from_secs(8))?;
        app.run_for(Duration::from_secs(1))?;
        app.wait_for_gone("role:alert", Duration::from_secs(2))?;
        Ok(())
    }
}
