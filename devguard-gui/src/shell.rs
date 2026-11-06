//! The phase 0 shell: stable navigation and shared dialogs; pages own their drafts.

use nsh_base::{tabs, theme};
use nsh_core::{platform::Appearance, prelude::*};

use crate::{
    copy,
    dialogs::{self, OpenDialog},
    hardware::HardwareStore,
    pages,
    spike::{PausePreview, SpikeStore},
    ui::{self, ActionStyle, Tone},
};

#[derive(Clone, Copy, PartialEq)]
enum Section {
    Devices,
    Records,
    Guide,
}

/// Headless tests must not spawn a PowerShell watcher per engine instance.
fn diagnostics_enabled() -> bool {
    std::env::var_os("DEVGUARD_GUI_NO_HW_SCAN").is_none()
}

pub(crate) fn shell() -> View {
    let store = SpikeStore::new();
    let preview = PausePreview::new();
    let hardware = HardwareStore::new(diagnostics_enabled());
    let section = signal(Section::Devices);
    let open = signal(None::<OpenDialog>);
    let search = element_ref();
    let device_search = element_ref();
    let add_button = element_ref();
    let pause_button = element_ref();
    let focus_search = move |_: &str| {
        if open.get_untracked().is_none() {
            section.set(Section::Devices);
            device_search.focus();
        }
    };
    on_key("mod+k", focus_search);
    on_key("mod+f", focus_search);
    on_key("mod+n", move |_| {
        if open.get_untracked().is_none() && !store.pending() {
            section.set(Section::Records);
            open.set(Some(OpenDialog::Record));
        }
    });

    view()
        .flex_1()
        .min_h_0()
        .min_w_0()
        .child(
            view()
                .px_6()
                .py_4()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap_3()
                .child(
                    view()
                        .flex_1()
                        .min_w_48()
                        .gap_1()
                        .child(
                            text("devguard")
                                .text_2xl()
                                .font_weight(FontWeight::SEMIBOLD),
                        )
                        .child(ui::hint("Device management, made clear")),
                )
                .child(ui::badge("Phase 0 · UI validation", Tone::Neutral))
                .child(ui::action(
                    move || SharedString::from(if dark() { "Light mode" } else { "Dark mode" }),
                    ActionStyle::Quiet,
                    || {
                        set_color_scheme(if dark() {
                            Appearance::Light
                        } else {
                            Appearance::Dark
                        });
                    },
                ))
                .child(
                    ui::action("Pause preview…", ActionStyle::Standard, move || {
                        open.set(Some(OpenDialog::Pause))
                    })
                    .element_ref(&pause_button),
                ),
        )
        .child(
            view()
                .px_6()
                .py_3()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap_3()
                .bg(move || {
                    if preview.active() {
                        ui::tokens::color_role("warning-surface")
                    } else {
                        theme().card
                    }
                })
                .border_t_1()
                .border_b_1()
                .border_color(move || theme().border)
                .child(
                    text(move || if preview.active() { "◷" } else { "●" }).text_color(move || {
                        if preview.active() {
                            ui::tokens::color_role("warning")
                        } else {
                            theme().primary
                        }
                    }),
                )
                .child(
                    text(move || preview.label())
                        .test_id("status-banner")
                        .flex_1()
                        .min_w_48()
                        .text_sm()
                        .tabular_nums(),
                )
                .child(when(
                    move || preview.active(),
                    move || ui::action("End preview", ActionStyle::Standard, move || preview.end()),
                )),
        )
        .child(
            view()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .p_6()
                .gap_4()
                .child(
                    tabs()
                        .flex_row()
                        .self_start()
                        .gap_1()
                        .p_1()
                        .rounded(move || theme().radius)
                        .bg(move || theme().muted)
                        .child(ui::section_tab(
                            "Devices",
                            move || section.get() == Section::Devices,
                            move || section.set(Section::Devices),
                        ))
                        .child(ui::section_tab(
                            "Records",
                            move || section.get() == Section::Records,
                            move || section.set(Section::Records),
                        ))
                        .child(ui::section_tab(
                            "Validation guide",
                            move || section.get() == Section::Guide,
                            move || section.set(Section::Guide),
                        )),
                )
                .child(
                    pages::devices(hardware, device_search)
                        .style_when(move || section.get() != Section::Devices, |s| s.hidden()),
                )
                .child(
                    pages::records(store, open, search, add_button)
                        .style_when(move || section.get() != Section::Records, |s| s.hidden()),
                )
                .child(
                    pages::guide(store, move || section.set(Section::Records))
                        .style_when(move || section.get() != Section::Guide, |s| s.hidden()),
                ),
        )
        .child(
            view()
                .px_6()
                .py_3()
                .gap_1()
                .border_t_1()
                .border_color(move || theme().border)
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .gap_3()
                        .child(ui::hint(move || store.feedback()).flex_1())
                        .child(
                            ui::hint(move || copy::local_record_count(store.count()))
                                .test_id("record-count")
                                .tabular_nums(),
                        ),
                )
                .child(when(
                    move || store.error().is_some(),
                    move || {
                        text(move || store.error().unwrap_or_default())
                            .text_sm()
                            .text_color(move || theme().destructive)
                    },
                )),
        )
        .child(when(
            move || open.get() == Some(OpenDialog::Record),
            move || dialogs::record_form(store, open, add_button),
        ))
        .child(when(
            move || open.get() == Some(OpenDialog::Pause),
            move || dialogs::pause_form(preview, open, pause_button),
        ))
}
