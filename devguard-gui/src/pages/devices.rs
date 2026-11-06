//! Read-only USB serial device list. Windows diagnostics run locally; Linux
//! device management comes from devguardd in later phases.

use nsh_base::theme;
use nsh_core::prelude::*;

use crate::{
    hardware::HardwareStore,
    ui::{self, tokens, ActionStyle, Tone},
};

pub(crate) fn devices(store: HardwareStore, search: ElementRef) -> View {
    let query = signal(String::new());
    let picked = signal(None::<String>);
    let detail_open = signal(false);
    let ids = computed(move || store.ids(&query.get()));
    let selected = computed(move || {
        picked
            .get()
            .filter(|id| store.device(id).is_some())
            .or_else(|| ids.with(|ids| ids.first().cloned()))
    });
    let current = computed(move || selected.get().and_then(|id| store.device(&id)));
    let reset_filter = move || query.set(String::new());
    let scan_enabled = store.scan_enabled();

    view().flex_1().min_h_0().min_w_0().gap_4()
        .child(
            view().flex_row().flex_wrap().items_center().gap_3()
                .child(view().flex_1().min_w_48().gap_1()
                    .child(ui::heading("USB serial devices"))
                    .child(ui::hint("Read-only inventory. Permissions, baud rates and blocking are applied by devguardd on Linux.")))
                .child(ui::action("Refresh", ActionStyle::Standard, move || store.refresh())
                    .disabled(move || store.loading() || !scan_enabled)),
        )
        .child(
            view().flex_row().flex_wrap().gap_3().items_center()
                .child(ui::field("Search devices", "Search port, name, VID:PID or serial", query)
                    .test_id("search-devices").element_ref(&search).flex_1().min_w_48().max_length(128)
                    .on_focus(move || detail_open.set(false)))
                .child(ui::hint(move || store.source_label())),
        )
        .child(when(move || scan_enabled && store.error().is_some(), move || {
            view().p_3().gap_1().rounded(move || theme().radius)
                .bg(move || ui::tokens::color_role("warning-surface"))
                .border_1().border_color(move || theme().border)
                .child(text(move || store.error().unwrap_or_default()).text_sm()
                    .text_color(move || ui::tokens::color_role("warning")))
        }))
        .child(
            view().flex_1().min_h_0().min_w_0().flex_row().gap_4()
                .child(
                    ui::panel().flex_1()
                        .style_when(move || !tokens::wide_layout() && detail_open.get(), |s| s.hidden())
                        .child(view().px_4().py_3().flex_row().items_center().justify_between().gap_3()
                            .child(text("Detected").text_sm().font_weight(FontWeight::SEMIBOLD))
                            .child(ui::hint(move || -> SharedString {
                                let total = store.count();
                                let ignored = store.ignored_count();
                                let devices = if total == 1 { "device" } else { "devices" };
                                if ignored == 0 {
                                    format!("{total} USB {devices}").into()
                                } else {
                                    let ports = if ignored == 1 { "port" } else { "ports" };
                                    format!("{total} USB {devices} · {ignored} non-USB {ports} hidden").into()
                                }
                            }).tabular_nums()))
                        .child(
                            virtual_list(move || ids.with(Vec::len), move |position| {
                                let device = computed(move || {
                                    ids.with(|ids| ids.get(position.index()).and_then(|id| store.device(id)))
                                });
                                row(device, selected, move |id| {
                                    picked.set(Some(id));
                                    detail_open.set(true);
                                })
                            })
                            .test_id("device-list")
                            .keys(move || ids.get())
                            .estimated_item_size(move || tokens::length("row-estimate") * font_scale())
                            .content_padding(tokens::space(2.0))
                            .row_gap(tokens::space(1.0))
                            .empty(view().p_6().gap_3().child(
                                when(
                                    move || store.loading(),
                                    || view()
                                        .gap_2()
                                        .child(ui::heading("Detecting USB serial devices…"))
                                        .child(ui::hint("This lists the local machine only; it never opens a serial port.")),
                                )
                                .otherwise(move || {
                                    view()
                                        .gap_3()
                                        .child(ui::heading("No matching devices"))
                                        .child(ui::hint("Plug in a USB serial device or adjust the search."))
                                        .child(when(
                                            move || !query.with(|query| query.trim().is_empty()),
                                            move || ui::action("Clear filters", ActionStyle::Standard, reset_filter).self_start(),
                                        ))
                                }),
                            ))
                            .flex_1().min_h_0(),
                        )
                        .child(view().px_4().py_2().flex_row().items_center().gap_2()
                            .border_t_1().border_color(move || theme().border)
                            .child(ui::hint(move || store.updated_label()).flex_1().number_of_lines(1))),
                )
                .child(
                    ui::panel()
                        .w(move || tokens::length("inspector-width"))
                        .style_when(move || !tokens::wide_layout(), |s| s.flex_1())
                        .style_when(move || !tokens::wide_layout() && !detail_open.get(), |s| s.hidden())
                        .child(
                            view().px_4().py_3().flex_row().items_center().gap_2()
                                .child(text("Device details").text_sm().font_weight(FontWeight::SEMIBOLD).flex_1())
                                .child(when(move || !tokens::wide_layout(), move || {
                                    ui::action("Back to list", ActionStyle::Quiet, move || detail_open.set(false))
                                })),
                        )
                        .child(scroll().flex_1().min_h_0().child(
                            view().p_4().gap_5().child(when(move || current.with(Option::is_some), move || {
                                inspector(current)
                            }).otherwise(|| {
                                view().gap_4()
                                    .child(ui::badge("Read-only", Tone::Neutral))
                                    .child(ui::heading("Select a device"))
                                    .child(ui::hint("Details come from the operating system's device database. Nothing is written to the device."))
                            })),
                        )),
                ),
        )
}

fn row(
    device: Memo<Option<crate::hardware::UsbSerialDevice>>,
    selected: Memo<Option<String>>,
    select: impl Fn(String) + 'static,
) -> impl IntoElement {
    ui::list_row(
        move || {
            device.with(|device| {
                device
                    .as_ref()
                    .map(|device| SharedString::from(device.name.clone()))
            })
        },
        move || {
            device.with(|device| {
                device.as_ref().is_some_and(|device| {
                    selected.get().as_deref() == Some(device.instance_id.as_str())
                })
            })
        },
        move || {
            if let Some(device) = device.get_untracked() {
                select(device.instance_id);
            }
        },
    )
    .child(
        view()
            .w_16()
            .min_h_10()
            .justify_center()
            .items_center()
            .rounded(move || theme().radius)
            .bg(move || theme().muted)
            .child(
                text(move || {
                    device.with(|device| {
                        device
                            .as_ref()
                            .map(|device| device.port.clone())
                            .unwrap_or_default()
                    })
                })
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .tabular_nums(),
            ),
    )
    .child(
        view()
            .flex_1()
            .min_w_0()
            .gap_1()
            .child(
                text(move || {
                    device.with(|device| {
                        device
                            .as_ref()
                            .map(|device| device.name.clone())
                            .unwrap_or_default()
                    })
                })
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .number_of_lines(1),
            )
            .child(
                ui::hint(move || {
                    device.with(|device| {
                        device
                            .as_ref()
                            .map(|device| {
                                let mut label = format!(
                                    "{}:{} · {}",
                                    device.vid,
                                    device.pid,
                                    device
                                        .driver_service
                                        .clone()
                                        .unwrap_or_else(|| "no driver".into())
                                );
                                if let Some(serial) = &device.serial_candidate {
                                    label.push_str(&format!(" · {serial}"));
                                }
                                label
                            })
                            .unwrap_or_default()
                    })
                })
                .text_xs()
                .number_of_lines(1),
            ),
    )
    .child(
        when(
            move || device.with(|device| device.as_ref().is_some_and(|device| device.healthy())),
            || ui::badge("OK", Tone::Success),
        )
        .otherwise(|| ui::badge("Check", Tone::Warning)),
    )
    .child(ui::hint("›"))
}

fn inspector(current: Memo<Option<crate::hardware::UsbSerialDevice>>) -> View {
    view().gap_5()
        .child(when(move || current.with(|device| device.as_ref().is_some_and(|device| !device.healthy())), || {
            ui::badge("Reported problem", Tone::Warning)
        }).otherwise(|| ui::badge("Ready", Tone::Success)))
        .child(ui::heading(move || {
            current.with(|device| device.as_ref().map(|device| device.name.clone()).unwrap_or_default())
        }).selectable(true))
        .child(when(move || current.with(|device| device.as_ref().is_some_and(|device| device.composite_parent)), || {
            ui::hint("Composite USB parent: blocking this device would affect its other interfaces. The daemon must check every interface first.")
        }))
        .child(metadata("Port", move || current.with(|device| device.as_ref().map(|device| device.port.clone()).unwrap_or_default())))
        .child(metadata("VID:PID", move || current.with(|device| {
            device.as_ref().map(|device| format!("{}:{}", device.vid, device.pid)).unwrap_or_default()
        })))
        .child(metadata("Serial candidate", move || current.with(|device| {
            device.as_ref().and_then(|device| device.serial_candidate.clone()).unwrap_or_else(|| "not reported".into())
        })))
        .child(metadata("Status", move || current.with(|device| device.as_ref().map(|device| device.status.clone()).unwrap_or_default())))
        .child(metadata("Driver", move || current.with(|device| {
            device.as_ref().map(|device| {
                let service = device.driver_service.clone().unwrap_or_else(|| "none".into());
                match &device.driver_version {
                    Some(version) => format!("{service} ({version})"),
                    None => service,
                }
            }).unwrap_or_default()
        })))
        .child(metadata("Instance ID", move || current.with(|device| device.as_ref().map(|device| device.instance_id.clone()).unwrap_or_default())))
        .child(view().pt_4().gap_3().border_t_1().border_color(move || theme().border)
            .child(text("Binding notes").text_sm().font_weight(FontWeight::MEDIUM))
            .child(ui::hint("A serial candidate from the USB parent still needs confirmation as ID_SERIAL_SHORT on Linux. The instance tail of a composite interface is never used as a serial number."))
            .child(metadata("USB parent", move || current.with(|device| device.as_ref().map(|device| device.usb_instance_id.clone()).unwrap_or_default()))))
        .child(ui::action("Copy details", ActionStyle::Standard, move || {
            if let Some(device) = current.get_untracked() {
                set_clipboard(format!(
                    "{}\nPort: {}\nVID:PID: {}:{}\nSerial: {}\nStatus: {}\nDriver: {}\nInstance: {}\nUSB parent: {}",
                    device.name, device.port, device.vid, device.pid,
                    device.serial_candidate.as_deref().unwrap_or("not reported"),
                    device.status,
                    device.driver_service.as_deref().unwrap_or("none"),
                    device.instance_id, device.usb_instance_id
                ));
            }
        }).self_start())
}

fn metadata(label: &'static str, value: impl Fn() -> String + 'static) -> View {
    view().gap_1().child(ui::hint(label).text_xs()).child(
        text(move || SharedString::from(value()))
            .text_sm()
            .selectable(true),
    )
}
