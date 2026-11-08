use nsh_base::{tabs, theme};
use nsh_core::prelude::*;

use crate::{
    daemon::{DeviceFilter, DeviceRow, Inspection, Workspace},
    ui::{self, tokens, ActionStyle},
};

pub(super) fn devices(
    store: Workspace,
    search: ElementRef,
    open_logs: impl Fn(String) + Copy + 'static,
) -> View {
    let query = signal(String::new());
    let filter = signal(DeviceFilter::All);
    let detail_open = signal(false);
    let list = element_ref();
    let ids = computed(move || store.ids(&query.get(), filter.get()));
    let selected = computed(move || {
        store
            .selection()
            .or_else(|| ids.with(|ids| ids.first().cloned()))
    });
    let current = computed(move || selected.get().and_then(|key| store.row(&key)));
    effect(move || {
        let _ = query.get();
        let _ = filter.get();
        after_answer(move || list.scroll_to_offset(tokens::space(0.0), false));
    });
    effect(move || {
        let _ = store.generation();
        // A new drift report may keep the same status/identity. Refresh its
        // details on pushed events, not on a device polling timer.
        let _ = store.revision();
        let live = store.live();
        let row = current.get();
        if live {
            if let Some(row) = row {
                untracked(|| store.inspect(&row));
            }
        }
    });
    view().flex_1().min_h_0().min_w_0().gap_3()
        .child(view().gap_1().child(ui::heading("USB serial devices"))
            .child(ui::hint("Online devices and saved registrations from devguardd. Selecting a device loads its inspection report.")))
        .child(view().flex_row().flex_wrap().gap_3().items_center()
            .child(ui::field("Search devices", "Name, description, VID:PID, serial or port", query).test_id("live-search-devices")
                .element_ref(&search).max_length(128).flex_1().min_w_48().on_focus(move || detail_open.set(false)))
            .child(tabs().flex_row().gap_1().p_1().rounded(move || theme().radius).bg(move || theme().muted)
                .child(ui::section_tab("All", move || filter.get() == DeviceFilter::All, move || filter.set(DeviceFilter::All)))
                .child(ui::section_tab("Needs attention", move || filter.get() == DeviceFilter::Attention, move || filter.set(DeviceFilter::Attention)))
                .child(ui::section_tab("Registered", move || filter.get() == DeviceFilter::Registered, move || filter.set(DeviceFilter::Registered)))))
        .child(when(move || store.snapshot().is_some_and(|s| !s.problems.is_empty()), move || {
            super::error(move || store.snapshot().map(|s| s.problems.iter().map(|p| {
                format!("{}{}\n{}", p.file.as_deref().map(|f| format!("{f}: ")).unwrap_or_default(), p.message, p.next_step.as_deref().unwrap_or("Ask an administrator to inspect the daemon configuration."))
            }).collect::<Vec<_>>().join("\n")).unwrap_or_default())
        }))
        .child(view().flex_1().min_h_0().min_w_0().flex_row().gap_3()
            .child(ui::panel().flex_1().style_when(move || !tokens::wide_layout() && detail_open.get(), |s| s.hidden())
                .child(view().px_4().py_2().flex_row().items_center().gap_2()
                    .child(text("Devices and registrations").text_sm().font_weight(FontWeight::MEDIUM).flex_1())
                    .child(ui::hint(move || format!("{} / {}", ids.with(Vec::len), store.rows().len())).tabular_nums()))
                .child(virtual_list(move || ids.with(Vec::len), move |position| {
                    let device = computed(move || ids.with(|ids| ids.get(position.index()).and_then(|id| store.row(id))));
                    row(device, selected, move |key| { store.select(key); detail_open.set(true); })
                }).test_id("live-device-list").element_ref(&list).keys(move || ids.get())
                    .estimated_item_size(move || tokens::length("row-estimate") * font_scale())
                    .content_padding_x(tokens::space(4.0)).content_padding_y(tokens::space(2.0)).row_gap(tokens::space(1.0))
                    .empty(view().p_6().gap_3()
                        .child(ui::heading(move || if store.snapshot().is_none() { "No verified device data" } else if store.rows().is_empty() { "No devices or registrations" } else { "No matching devices" }))
                        .child(ui::hint(move || if !store.live() { "Connect to devguardd to load real device data." } else { "Connect a USB serial device or adjust the filters." }))
                        .child(ui::action("Clear filters", ActionStyle::Standard, move || { query.set(String::new()); filter.set(DeviceFilter::All); }).self_start()))
                    .flex_1().min_h_0()))
            .child(ui::panel().w(move || tokens::length("inspector-width"))
                .style_when(move || !tokens::wide_layout(), |s| s.flex_1())
                .style_when(move || !tokens::wide_layout() && !detail_open.get(), |s| s.hidden())
                .child(view().px_4().py_2().flex_row().items_center().gap_2()
                    .child(text("Device details").text_sm().font_weight(FontWeight::MEDIUM).flex_1())
                    .child(when(move || !tokens::wide_layout(), move || ui::action("Back to list", ActionStyle::Quiet, move || detail_open.set(false)))))
                .child(scroll().flex_1().min_h_0().child(view().p_4().gap_3()
                    .child(when(move || current.with(Option::is_some), move || inspector(store, current, ids, open_logs))
                        .otherwise(move || view().gap_2().child(ui::heading("No device selected"))
                            .child(ui::hint(move || if store.selection().is_some() { "The selected device disconnected or its registration was removed. Selection was not transferred to another device." } else { "Choose an online device or a saved registration." }))))))))
}

fn row(
    device: Memo<Option<DeviceRow>>,
    selected: Memo<Option<String>>,
    select: impl Fn(String) + 'static,
) -> impl IntoElement {
    ui::list_row(
        move || device.with(|d| d.as_ref().map(|d| d.title.clone().into())),
        move || {
            device.with(|d| {
                d.as_ref()
                    .is_some_and(|d| selected.get().as_deref() == Some(&d.key))
            })
        },
        move || {
            if let Some(row) = device.get_untracked() {
                select(row.key);
            }
        },
    )
    .child(
        view()
            .flex_1()
            .min_w_0()
            .gap_1()
            .child(
                text(move || {
                    device.with(|d| d.as_ref().map(|d| d.title.clone()).unwrap_or_default())
                })
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .number_of_lines(1),
            )
            .child(
                ui::hint(move || {
                    device.with(|d| d.as_ref().map(DeviceRow::summary).unwrap_or_default())
                })
                .text_xs()
                .number_of_lines(1),
            ),
    )
    .child(
        text(move || device.with(|d| d.as_ref().map(DeviceRow::status).unwrap_or("")))
            .text_xs()
            .text_color(move || {
                if device.with(|d| d.as_ref().is_some_and(|d| d.status() != "Registered")) {
                    tokens::color_role("warning")
                } else {
                    theme().muted_foreground
                }
            }),
    )
}

fn inspector(
    store: Workspace,
    current: Memo<Option<DeviceRow>>,
    ids: Memo<Vec<String>>,
    open_logs: impl Fn(String) + Copy + 'static,
) -> View {
    let inspection = computed(move || {
        current.with(|r| {
            r.as_ref()
                .map(|r| store.inspection_for(&r.key))
                .unwrap_or_default()
        })
    });
    let config = computed(move || {
        inspection.with(|i| match &i.value {
            Some(Inspection::Registered(report)) => Some(report.device.clone()),
            Some(Inspection::Candidate(report)) => Some(report.draft.clone()),
            None => current.with(|r| r.as_ref().and_then(|r| r.config.clone())),
        })
    });
    let items = computed(move || {
        inspection.with(|i| match &i.value {
            Some(Inspection::Registered(report)) => report.check.items.clone(),
            _ => Vec::new(),
        })
    });
    view().gap_3()
        .child(ui::heading(move || current.with(|r| r.as_ref().map(|r| r.title.clone()).unwrap_or_default())).selectable(true))
        .child(ui::hint(move || current.with(|r| r.as_ref().map(|r| r.status()).unwrap_or(""))))
        .child(when(move || current.with(|r| r.as_ref().is_some_and(|r| !ids.with(|ids| ids.contains(&r.key)))), || ui::hint("This selection is outside the current filter.")))
        .child(ui::metadata("Device node", move || -> SharedString { current.with(|r| r.as_ref().map(DeviceRow::ports).unwrap_or_default()).into() }))
        .child(ui::metadata("Binding", move || -> SharedString { current.with(|r| r.as_ref().map(|r| {
            if r.online.iter().any(|d| d.serial.is_some()) || r.config.as_ref().is_some_and(|c| c.ids.iter().any(|id| id.serial.is_some())) { "Serial identity" } else { "Physical port (ID_PATH)" }
        }).unwrap_or("")).into() }))
        .child(ui::metadata("Stable link", move || -> SharedString { inspection.with(|i| match &i.value {
            Some(Inspection::Registered(report)) => report.symlink.clone(),
            _ => if current.with(|r| r.as_ref().is_some_and(|r| r.config.is_some())) { "Awaiting inspection" } else { "Not registered" }.into(),
        }).into() }))
        .child(ui::metadata("Serial", move || -> SharedString { current.with(|r| r.as_ref().and_then(|r| r.online.first()).and_then(|d| d.serial.clone()).unwrap_or_else(|| "Not reported; inspect saved identities below".into())).into() }))
        .child(ui::metadata("Driver", move || -> SharedString { current.with(|r| r.as_ref().and_then(|r| r.online.first()).and_then(|d| d.driver.clone()).unwrap_or_else(|| "Not available".into())).into() }))
        .child(ui::metadata("USB path", move || -> SharedString { current.with(|r| r.as_ref().and_then(|r| r.online.first()).and_then(|d| d.usb_path.clone()).unwrap_or_default()).into() }))
        .child(ui::hint("Inspection can open a tty to read termios and may assert DTR. This view does not write serial settings."))
        .child(when(move || inspection.with(|i| i.pending), || ui::hint("Loading inspection report")))
        .child(when(move || inspection.with(|i| i.error.is_some()), move || super::error(move || inspection.with(|i| i.error.as_ref().map(|e| e.display()).unwrap_or_default()))))
        .child(when(move || config.with(Option::is_some), move || view().gap_2().pt_3().border_t_1().border_color(move || theme().border)
            .child(text(move || current.with(|r| if r.as_ref().is_some_and(|r| r.config.is_some()) { "Saved configuration" } else { "Registration draft · not saved" })).text_sm().font_weight(FontWeight::MEDIUM))
            .child(ui::metadata("Baud", move || -> SharedString { config.with(|c| c.as_ref().map(|c| c.serial.baud.to_string()).unwrap_or_default()).into() }))
            .child(ui::metadata("Line format", move || -> SharedString { config.with(|c| c.as_ref().map(|c| format!("{} data bits · {:?} parity · {} stop bits · {:?} flow · raw={}", c.serial.data_bits, c.serial.parity, c.serial.stop_bits, c.serial.flow, c.serial.raw)).unwrap_or_default()).into() }))
            .child(ui::metadata("Access", move || -> SharedString { config.with(|c| c.as_ref().map(|c| format!("{} / {}", c.access.group, c.access.mode)).unwrap_or_default()).into() }))
            .child(text(move || config.with(|c| c.as_ref().and_then(|c| c.to_toml().ok()).unwrap_or_default())).text_xs().selectable(true))))
        .child(each(items, |item| item.id.clone(), move |item, _| view().gap_1().pt_2()
            .child(text(move || item.with(|i| format!("{} · {}", super::severity(i.severity), i.label))).text_sm().font_weight(FontWeight::MEDIUM))
            .child(ui::hint(move || item.with(|i| format!("Expected: {}\nActual: {}", i.expected.as_deref().unwrap_or("—"), i.actual.as_deref().unwrap_or("—")))).selectable(true))
            .child(ui::hint(move || item.with(|i| i.next_step.clone().unwrap_or_default())).selectable(true))))
        .child(ui::hint(move || inspection.with(|i| match &i.value { Some(Inspection::Candidate(report)) => report.warnings.join("\n"), _ => String::new() })))
        .child(view().flex_row().flex_wrap().gap_2()
            .child(ui::action("Reload inspection", ActionStyle::Standard, move || { if let Some(row) = current.get_untracked() { store.inspect(&row); } }).disabled(move || !store.live() || inspection.with(|i| i.pending)))
            .child(ui::action("Device logs", ActionStyle::Standard, move || { if let Some(name) = current.with_untracked(|r| r.as_ref().and_then(|r| r.config.as_ref()).map(|c| c.name.clone())) { open_logs(name); } }).disabled(move || current.with(|r| r.as_ref().is_none_or(|r| r.config.is_none()))))
            .child(ui::action("Copy details", ActionStyle::Quiet, move || {
                if let Some(row) = current.get_untracked() {
                    let info = inspection.get_untracked();
                    set_clipboard(format!("{}\n{}\n{}", row.title, row.summary(), info.value.as_ref().map(|v| match v {
                        Inspection::Registered(r) => serde_json::to_string_pretty(r).unwrap_or_default(),
                        Inspection::Candidate(r) => serde_json::to_string_pretty(r).unwrap_or_default(),
                    }).unwrap_or_default()));
                }
            })))
}
