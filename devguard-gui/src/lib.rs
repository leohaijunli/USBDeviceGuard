//! SerialGuard's Rust-first desktop. Linux uses the real daemon; Windows
//! retains its read-only diagnostics and explicitly labelled validation tools.

mod copy;
#[cfg(any(target_os = "linux", test))]
mod daemon;
mod dialogs;
mod hardware;
#[cfg(any(target_os = "linux", test))]
mod live;
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
    registry.register::<ValidationApp>();
}

/// Platform workspace. No simulated device data is used in the Linux surface.
#[component]
pub fn App() -> impl IntoElement {
    ui::tokens::install();
    #[cfg(any(target_os = "linux", test))]
    let workspace = if os() == nsh_core::platform::Os::Linux {
        live::shell()
    } else {
        shell::shell()
    };
    #[cfg(not(any(target_os = "linux", test)))]
    let workspace = shell::shell();
    view()
        .flex_1()
        .bg(move || theme().background)
        .text_color(move || theme().foreground)
        .p_safe_area()
        .child(workspace)
}

/// Explicit phase-0 tool, never a fallback when the daemon is unavailable.
#[component]
pub fn ValidationApp() -> impl IntoElement {
    ui::tokens::install();
    view()
        .flex_1()
        .bg(move || theme().background)
        .text_color(move || theme().foreground)
        .p_safe_area()
        .child(shell::shell())
}
