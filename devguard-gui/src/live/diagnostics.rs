use nsh_core::prelude::*;

use crate::{
    daemon::Workspace,
    ui::{self, tokens, ActionStyle},
};

pub(super) fn diagnostics(store: Workspace, visible: impl Fn() -> bool + Copy + 'static) -> View {
    effect(move || {
        let live = store.live();
        let shown = visible();
        let _ = store.generation();
        if live && shown {
            untracked(|| {
                store.run_check();
                store.run_doctor();
            });
        }
    });
    let checks = computed(move || {
        store
            .check()
            .value
            .map(|r| r.overall.items)
            .unwrap_or_default()
    });
    let doctor = computed(move || {
        store
            .doctor()
            .value
            .map(|r| r.report.items)
            .unwrap_or_default()
    });
    view().flex_1().min_h_0().min_w_0().gap_3()
        .child(ui::heading("Policy and diagnostics"))
        .child(ui::metadata("Reported policy", move || -> SharedString { store.snapshot().map(|s| match s.policy { devguard_core::PolicyMode::Audit => "audit · records only", devguard_core::PolicyMode::Enforce => "enforce · reported by daemon" }).unwrap_or("Unknown").into() }))
        .child(ui::hint("Policy changes, pause/resume and service controls are not enabled in this GUI slice. Diagnostics never authorize an enforce switch."))
        .child(view().flex_row().flex_wrap().items_center().gap_2()
            .child(ui::action("Run check", ActionStyle::Standard, move || store.run_check()).disabled(move || !store.live() || store.check().pending))
            .child(ui::action("Run doctor", ActionStyle::Standard, move || store.run_doctor()).disabled(move || !store.live() || store.doctor().pending))
            .child(ui::hint("Checking termios may assert DTR when opening a tty.")))
        .child(when(move || store.check().error.is_some(), move || super::error(move || store.check().error.map(|e| e.display()).unwrap_or_default())))
        .child(when(move || store.doctor().error.is_some(), move || super::error(move || store.doctor().error.map(|e| e.display()).unwrap_or_default())))
        .child(view().flex_1().min_h_0().min_w_0().flex_row().gap_3()
            .child(ui::panel().flex_1()
                .child(view().px_4().py_2().gap_1().child(text("Configuration check").text_sm().font_weight(FontWeight::MEDIUM))
                    .child(ui::hint(move || if store.check().pending { "Checking devices".into() } else {
                        store.check().value.map(|r| format!("{} · {} warning(s), {} error(s)", r.overall.checked_at, r.overall.warnings(), r.overall.errors()))
                            .unwrap_or_else(|| "Not checked".into())
                    })))
                .child(virtual_list(move || checks.with(Vec::len), move |position| {
                    let item = computed(move || checks.with(|items| items.get(position.index()).cloned()));
                    view().gap_1().py_2()
                        .child(text(move || item.with(|i| i.as_ref().map(|i| format!("{} · {}", super::severity(i.severity), i.label)).unwrap_or_default())).text_sm().font_weight(FontWeight::MEDIUM))
                        .child(ui::hint(move || item.with(|i| i.as_ref().map(|i| format!("Expected: {}\nActual: {}", i.expected.as_deref().unwrap_or("—"), i.actual.as_deref().unwrap_or("—"))).unwrap_or_default())).selectable(true))
                        .child(ui::hint(move || item.with(|i| i.as_ref().and_then(|i| i.next_step.clone()).unwrap_or_default())).selectable(true))
                }).test_id("live-check-list").keys(move || checks.with(|items| items.iter().map(|i| i.id.clone()).collect::<Vec<_>>()))
                    .estimated_item_size(move || tokens::space(24.0) * font_scale()).content_padding_x(tokens::space(4.0)).content_padding_y(tokens::space(2.0))
                    .empty(view().p_4().child(ui::hint("No check items. An empty report is not proof that connected hardware passed."))).flex_1().min_h_0()))
            .child(ui::panel().flex_1()
                .child(view().px_4().py_2().gap_1().child(text("Host diagnostics").text_sm().font_weight(FontWeight::MEDIUM))
                    .child(ui::hint(move || if store.doctor().pending { "Running doctor".into() } else {
                        store.doctor().value.map(|r| format!("{} · {} warning(s), {} error(s)", r.report.checked_at, r.report.warnings(), r.report.errors()))
                            .unwrap_or_else(|| "Not checked".into())
                    })))
                .child(virtual_list(move || doctor.with(Vec::len), move |position| {
                    let item = computed(move || doctor.with(|items| items.get(position.index()).cloned()));
                    view().gap_1().py_2()
                        .child(text(move || item.with(|i| i.as_ref().map(|i| format!("{:?} · {}", i.status, i.title)).unwrap_or_default())).text_sm().font_weight(FontWeight::MEDIUM))
                        .child(ui::hint(move || item.with(|i| i.as_ref().map(|i| i.detail.clone()).unwrap_or_default())).selectable(true))
                        .child(ui::hint(move || item.with(|i| i.as_ref().and_then(|i| i.next_step.clone()).unwrap_or_default())).selectable(true))
                }).test_id("live-doctor-list").keys(move || doctor.with(|items| items.iter().map(|i| i.id.clone()).collect::<Vec<_>>()))
                    .estimated_item_size(move || tokens::space(24.0) * font_scale()).content_padding_x(tokens::space(4.0)).content_padding_y(tokens::space(2.0))
                    .empty(view().p_4().child(ui::hint("No diagnostics yet. Run doctor when connected."))).flex_1().min_h_0())))
}
