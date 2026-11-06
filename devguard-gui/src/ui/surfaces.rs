use nsh_base::{dialog, theme};
use nsh_core::prelude::*;

use super::tokens;

#[derive(Clone, Copy)]
pub(crate) enum Tone {
    Neutral,
    Success,
    Warning,
}

pub(crate) fn heading(title: impl IntoReactive<SharedString>) -> Text {
    text(title)
        .text_lg()
        .font_weight(FontWeight::SEMIBOLD)
        .role(AccessibilityRole::Header)
}

pub(crate) fn hint(label: impl IntoReactive<SharedString>) -> Text {
    text(label)
        .text_sm()
        .text_color(move || theme().muted_foreground)
}

pub(crate) fn panel() -> View {
    view()
        .min_w_0()
        .min_h_0()
        .bg(move || theme().card)
        .border_1()
        .border_color(move || theme().border)
        .rounded(move || tokens::length("radius-panel"))
}

pub(crate) fn badge(label: impl IntoReactive<SharedString>, tone: Tone) -> View {
    view()
        .self_start()
        .px_2()
        .py_1()
        .rounded(move || theme().radius)
        .bg(move || match tone {
            Tone::Neutral => theme().muted,
            Tone::Success => tokens::color_role("success-surface"),
            Tone::Warning => tokens::color_role("warning-surface"),
        })
        .child(
            text(label)
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || match tone {
                    Tone::Neutral => theme().muted_foreground,
                    Tone::Success => tokens::color_role("success"),
                    Tone::Warning => tokens::color_role("warning"),
                }),
        )
}

/// The native modal supplies focus trapping and Escape; this layer supplies its look.
pub(crate) fn dialog_surface(
    title: &'static str,
    description: &'static str,
    close: impl Fn() + Copy + 'static,
    body: impl IntoElement,
    actions: impl IntoElement,
) -> Modal {
    dialog()
        .animation(ModalAnimation::Fade)
        .on_cancel(close)
        .flex_1()
        .p_4()
        .bg(move || tokens::color_role("overlay"))
        .child(
            view()
                .m(auto())
                .w(move || tokens::length("dialog-width"))
                .max_w(relative(1.0))
                .max_h(relative(1.0))
                .rounded(move || tokens::length("radius-panel"))
                .bg(move || theme().popover)
                .text_color(move || theme().popover_foreground)
                .border_1()
                .border_color(move || theme().border)
                .shadow_lg()
                .child(
                    view()
                        .p_5()
                        .gap_2()
                        .child(heading(title))
                        .child(hint(description)),
                )
                .child(
                    scroll()
                        .min_h_0()
                        .child(view().px_5().pb_5().gap_4().child(body)),
                )
                .child(
                    view()
                        .p_4()
                        .flex_row()
                        .flex_wrap()
                        .justify_end()
                        .gap_2()
                        .border_t_1()
                        .border_color(move || theme().border)
                        .child(actions),
                ),
        )
}
