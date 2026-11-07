use nsh_base::theme;
use nsh_core::prelude::*;

use crate::{
    spike::SpikeStore,
    ui::{self, ActionStyle, Tone},
};

pub(crate) fn guide(store: SpikeStore, show_records: impl Fn() + Copy + 'static) -> View {
    ui::panel().flex_1()
        .child(scroll().flex_1().child(
            view().p_5().gap_5()
                .child(view().gap_2().child(ui::heading("NSH local validation"))
                    .child(ui::hint("Test native controls and background event delivery. USB management will be connected in later phases.")))
                .child(item("Lists and search", "Filter by name, note or ID. Select a record to see its details in the same workspace."))
                .child(item("Forms and dialogs", "Names support Unicode and notes can span multiple lines. Cancel discards the draft; an empty name cannot be submitted."))
                .child(item("Events and threads", "Records come from real background threads. Channel delivery wakes NSH to update signals on the UI thread."))
                .child(item("Pause interaction", "Choose either pause scope and one of four durations. The preview ends automatically when the countdown expires."))
                .child(view().gap_3()
                    .child(item("Long-list validation", "Send 1,000 records, scroll to the end, then search and select another record to check recycled rows."))
                    .child(ui::action("Send 1,000 records", ActionStyle::Standard, move || {
                        store.emit_background(1000);
                        show_records();
                    }).self_start().disabled(move || store.pending())))
                .child(view().pt_4().gap_3().border_t_1().border_color(move || theme().border)
                    .child(ui::badge("Linux acceptance pending", Tone::Warning))
                    .child(ui::hint("GTK 4.14+, Hyprland/Wayland, input methods and window-close behavior still need verification on Arch/Omarchy.")))
                .child(item("Keyboard shortcuts", "Ctrl+K / Ctrl+F: Search\nCtrl+N: Add record\nEscape: Dismiss the current dialog\nTab / Enter: Navigate and activate controls")),
        ))
}

fn item(title: &'static str, detail: &'static str) -> View {
    view()
        .gap_1()
        .child(text(title).text_sm().font_weight(FontWeight::SEMIBOLD))
        .child(ui::hint(detail))
}
