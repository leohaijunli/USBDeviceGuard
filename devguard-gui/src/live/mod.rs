//! A real daemon workbench, separate from Windows' phase-0 validation pages.

mod devices;
mod diagnostics;
mod logs;

use nsh_base::{tabs, theme};
use nsh_core::{platform::Appearance, prelude::*};

use crate::{
    daemon::{Connection, Workspace},
    ui::{self, ActionStyle},
};

#[derive(Clone, Copy, PartialEq)]
enum Section {
    Devices,
    Logs,
    Diagnostics,
}

pub(crate) fn shell() -> View {
    let socket = std::env::var_os("DEVGUARD_GUI_SOCKET")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "/run/devguard/ctl.sock".into());
    shell_with(Workspace::connect(socket))
}

pub(crate) fn shell_with(store: Workspace) -> View {
    let section = signal(Section::Devices);
    let search = element_ref();
    let log_device = signal(None::<String>);
    on_key("mod+k", move |_| {
        section.set(Section::Devices);
        search.focus();
    });
    on_key("mod+f", move |_| {
        section.set(Section::Devices);
        search.focus();
    });
    let open_logs = move |name: String| {
        log_device.set(Some(name));
        section.set(Section::Logs);
    };
    view().flex_1().min_h_0().min_w_0()
        .child(view().px_4().py_3().flex_row().flex_wrap().items_center().gap_3()
            .child(ui::heading("SerialGuard").flex_1())
            .child(ui::hint("Linux · viewing and diagnostics"))
            .child(ui::action(move || SharedString::from(if dark() { "Light mode" } else { "Dark mode" }), ActionStyle::Quiet,
                || set_color_scheme(if dark() { Appearance::Light } else { Appearance::Dark }))))
        .child(view().px_4().py_2().gap_1().border_t_1().border_b_1().border_color(move || theme().border)
            .child(view().flex_row().flex_wrap().items_center().gap_3()
                .child(text(move || store.status()).test_id("daemon-status").text_sm().font_weight(FontWeight::MEDIUM).flex_1()
                    .text_color(move || match store.connection() {
                        Connection::Stopped | Connection::Unavailable(_) => theme().destructive,
                        Connection::Reconnecting { .. } => ui::tokens::color_role("warning"),
                        _ => theme().foreground,
                    }))
                .child(ui::action("Refresh snapshot", ActionStyle::Quiet, move || store.refresh()).disabled(move || !store.live() || store.refreshing()))
                .child(ui::action("Reconnect", ActionStyle::Standard, move || store.reconnect())))
            .child(ui::hint(move || if store.live() {
                "Event subscription active. No blocking, pause or device writes are available in this view."
            } else {
                "Previous data may be stale. Reconnect never starts the service; start it externally for now."
            })))
        .child(when(move || matches!(store.connection(), Connection::Unavailable(_) | Connection::Reconnecting { .. }), move || {
            error(move || match store.connection() {
                Connection::Unavailable(fault) | Connection::Reconnecting { fault, .. } => fault.display(), _ => String::new(),
            }).px_4().py_2()
        }))
        .child(view().flex_1().min_h_0().min_w_0().p_4().gap_3()
            .child(tabs().flex_row().self_start().gap_1().p_1().rounded(move || theme().radius).bg(move || theme().muted)
                .child(ui::section_tab("Devices", move || section.get() == Section::Devices, move || section.set(Section::Devices)))
                .child(ui::section_tab("Logs", move || section.get() == Section::Logs, move || section.set(Section::Logs)))
                .child(ui::section_tab("Policy and diagnostics", move || section.get() == Section::Diagnostics, move || section.set(Section::Diagnostics))))
            .child(devices::devices(store, search, open_logs).style_when(move || section.get() != Section::Devices, |s| s.hidden()))
            .child(logs::logs(store, log_device, move || section.get() == Section::Logs).style_when(move || section.get() != Section::Logs, |s| s.hidden()))
            .child(diagnostics::diagnostics(store, move || section.get() == Section::Diagnostics).style_when(move || section.get() != Section::Diagnostics, |s| s.hidden())))
        .child(view().px_4().py_2().border_t_1().border_color(move || theme().border).gap_1()
            .child(ui::hint(move || store.timestamp()).test_id("daemon-timestamp").tabular_nums())
            .child(ui::hint(move || store.feedback()).number_of_lines(2)))
}

pub(super) fn error(message: impl Fn() -> String + 'static) -> View {
    view().gap_1().child(
        text(message)
            .text_sm()
            .selectable(true)
            .text_color(move || theme().destructive),
    )
}

pub(super) fn severity(severity: devguard_core::Severity) -> &'static str {
    match severity {
        devguard_core::Severity::Ok => "OK",
        devguard_core::Severity::Warning => "Warning",
        devguard_core::Severity::Error => "Error",
    }
}
