//! devguard's Rust-first NSH desktop surface. Phase 0 validates native UI and
//! background event delivery before the daemon integration gate.

mod copy;
mod dialogs;
mod hardware;
mod pages;
mod shell;
mod spike;
mod ui;

#[cfg(target_os = "windows")]
mod desktop_notifications;
#[cfg(target_os = "windows")]
pub mod desktop_windows;

use nsh_base::theme;
use nsh_core::prelude::*;

#[native_module("devguard/native")]
fn native(registry: &mut Registry) {
    registry.register::<App>();
}

/// The devguard desktop surface; currently used for the phase 0 NSH spike.
#[component]
pub fn App() -> impl IntoElement {
    ui::tokens::install();
    view()
        .flex_1()
        .bg(move || theme().background)
        .text_color(move || theme().foreground)
        .p_safe_area()
        .child(shell::shell())
}
