//! Transient drafts belong to the modal's owner and are disposed on cancel.

use devguard_core::PauseKind;
use nsh_base::{radio_group, theme};
use nsh_core::prelude::*;

use crate::{
    spike::{PausePreview, SpikeStore},
    ui::{self, ActionStyle},
};

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum OpenDialog {
    Record,
    Pause,
}

pub(crate) fn record_form(
    store: SpikeStore,
    open: Signal<Option<OpenDialog>>,
    return_focus: ElementRef,
) -> impl IntoElement {
    let title = signal(String::new());
    let note = signal(String::new());
    let valid = computed(move || {
        !title.with(|title| title.trim().is_empty())
            && title.with(|title| title.chars().count() <= 64)
            && note.with(|note| note.chars().count() <= 240)
    });
    let close = move || {
        open.set(None);
        return_focus.focus();
    };
    let submit = move || {
        if valid.get_untracked() && store.submit_form(title.get_untracked(), note.get_untracked()) {
            close();
        }
    };
    ui::dialog_surface(
        "Add validation record",
        "A background thread will send this record to the list through the event channel.",
        close,
        view()
            .gap_4()
            .child(
                view()
                    .gap_2()
                    .child(text("Name").text_sm().font_weight(FontWeight::MEDIUM))
                    .child(
                        ui::field("Record name", "e.g. Firmware check", title)
                            .test_id("record-title")
                            .max_length(64)
                            .auto_focus(true)
                            .on_submit(move |_: &str| submit()),
                    )
                    .child(ui::hint("Required. Up to 64 characters.")),
            )
            .child(
                view()
                    .gap_2()
                    .child(
                        text("Note (optional)")
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM),
                    )
                    .child(
                        ui::field("Record note", "Describe the interaction to test", note)
                            .test_id("record-note")
                            .multiline(true)
                            .max_length(240)
                            .min_h_24(),
                    )
                    .child(ui::hint(move || {
                        format!("{}/240", note.with(|note| note.chars().count()))
                    })),
            )
            .child(when(
                move || store.error().is_some(),
                move || {
                    text(move || store.error().unwrap_or_default())
                        .text_sm()
                        .text_color(move || theme().destructive)
                },
            )),
        view()
            .flex_row()
            .flex_wrap()
            .gap_2()
            .child(ui::action("Cancel", ActionStyle::Standard, close))
            .child(
                ui::action("Send record", ActionStyle::Primary, submit)
                    .disabled(move || !valid.get() || store.pending()),
            ),
    )
}

pub(crate) fn pause_form(
    preview: PausePreview,
    open: Signal<Option<OpenDialog>>,
    return_focus: ElementRef,
) -> impl IntoElement {
    let kind = signal(PauseKind::Enforcement);
    let minutes = signal(5_u32);
    let close = move || {
        open.set(None);
        return_focus.focus();
    };
    let start = move || {
        preview.start(kind.get_untracked(), minutes.get_untracked());
        close();
    };
    ui::dialog_surface(
        "Pause preview",
        "Try the pause options and countdown. This preview does not control the USB service.",
        close,
        view()
            .gap_4()
            .child(
                view()
                    .gap_2()
                    .child(text("Pause scope").text_sm().font_weight(FontWeight::MEDIUM))
                    .child(
                        radio_group()
                            .gap_2()
                            .child(ui::choice(
                                "Enforcement only",
                                Some("Allow unregistered devices temporarily; keep maintaining registered devices."),
                                kind,
                                PauseKind::Enforcement,
                            ))
                            .child(ui::choice(
                                "Whole service",
                                Some("Pause blocking and configuration maintenance; keep the daemon running."),
                                kind,
                                PauseKind::Service,
                            )),
                    ),
            )
            .child(
                view()
                    .gap_2()
                    .child(text("Duration").text_sm().font_weight(FontWeight::MEDIUM))
                    .child(
                        radio_group().flex_row().flex_wrap().gap_2().children(
                            [
                                (5, "5 min"),
                                (15, "15 min"),
                                (30, "30 min"),
                                (60, "60 min"),
                            ]
                            .into_iter()
                            .map(move |(value, label)| {
                                ui::choice(label, None, minutes, value).flex_1()
                            }),
                        ),
                    )
                    .child(ui::hint("Resumes automatically. Up to 60 minutes; end the preview at any time.")),
            ),
        view()
            .flex_row()
            .flex_wrap()
            .gap_2()
            .child(ui::action("Cancel", ActionStyle::Standard, close))
            .child(ui::action("Start preview", ActionStyle::Primary, start)),
    )
}
