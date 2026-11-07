use nsh_base::{button, input, radio, tab, theme, ButtonElement, Input, RadioElement, TabElement};
use nsh_core::prelude::*;

use super::tokens;

#[derive(Clone, Copy)]
pub(crate) enum ActionStyle {
    Standard,
    Primary,
    Quiet,
}

pub(crate) fn action(
    label: impl IntoReactive<SharedString>,
    style: ActionStyle,
    on_press: impl Fn() + 'static,
) -> ButtonElement {
    button()
        .on_press(on_press)
        .min_h_9()
        .px_3()
        .py_2()
        .flex_row()
        .items_center()
        .justify_center()
        .gap_2()
        .rounded(move || theme().radius)
        .border_1()
        .border_color(move || match style {
            ActionStyle::Standard => theme().border,
            ActionStyle::Primary => theme().primary,
            ActionStyle::Quiet => theme().border.opacity(0.0),
        })
        .bg(move || match style {
            ActionStyle::Standard => theme().card,
            ActionStyle::Primary => theme().primary,
            ActionStyle::Quiet => theme().card.opacity(0.0),
        })
        .text_color(move || match style {
            ActionStyle::Primary => theme().primary_foreground,
            _ => theme().foreground,
        })
        .hover(move |s| {
            s.bg(move || match style {
                ActionStyle::Primary => theme().primary.opacity(0.9),
                _ => theme().accent,
            })
        })
        .active(|s| s.opacity(0.75))
        .focus(|s| {
            s.style_with(|s| s.outline(tokens::length("focus-width")))
                .outline_color(move || theme().ring)
        })
        .disabled_style(|s| s.opacity(0.45))
        .child(text(label).text_sm().font_weight(FontWeight::MEDIUM))
}

/// Controlled input. Callers keep the draft; CJK composition is never rewritten.
pub(crate) fn field(
    label: &'static str,
    placeholder: &'static str,
    value: Signal<String>,
) -> Input {
    input()
        .label(label)
        .value(move || value.get())
        .on_change(move |next: &str| value.set(next.to_owned()))
        .placeholder(placeholder)
        .placeholder_color(|| color(theme().muted_foreground))
        .caret_color(|| color(theme().primary))
        .min_h_10()
        .px_3()
        .py_2()
        .text_sm()
        .rounded(move || theme().radius)
        .border_1()
        .border_color(move || theme().input)
        .bg(move || theme().card)
        .text_color(move || theme().foreground)
        .focus(|s| s.border_color(move || theme().ring))
}

pub(crate) fn section_tab(
    label: &'static str,
    selected: impl Fn() -> bool + Copy + 'static,
    on_press: impl Fn() + 'static,
) -> TabElement {
    tab()
        .selected(selected)
        .on_press(on_press)
        .min_h_9()
        .px_3()
        .py_2()
        .rounded(move || theme().radius)
        .selected_style(|s| s.bg(move || theme().card))
        .hover(|s| s.bg(move || theme().accent))
        .active(|s| s.opacity(0.75))
        .focus(|s| {
            s.style_with(|s| s.outline(tokens::length("focus-width")))
                .outline_color(move || theme().ring)
        })
        .child(
            text(label)
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || {
                    if selected() {
                        theme().foreground
                    } else {
                        theme().muted_foreground
                    }
                }),
        )
}

pub(crate) fn list_row(
    label: impl Fn() -> Option<SharedString> + 'static,
    selected: impl Fn() -> bool + 'static,
    on_press: impl Fn() + 'static,
) -> ButtonElement {
    button()
        .accessibility_label(label)
        .selected(selected)
        .on_press(on_press)
        .flex_row()
        .items_center()
        .gap_3()
        .px_3()
        .py_2()
        .min_w_0()
        .rounded(move || theme().radius)
        .bg(move || theme().card)
        .selected_style(|s| s.bg(move || theme().accent))
        .hover(|s| s.bg(move || theme().muted))
        .active(|s| s.opacity(0.75))
        .focus(|s| {
            s.style_with(|s| s.outline(tokens::length("focus-width")))
                .outline_color(move || theme().ring)
        })
}

/// A radio styled as a clearly selected option; the group owns keyboard behavior.
pub(crate) fn choice<T: Copy + PartialEq + 'static>(
    label: &'static str,
    description: Option<&'static str>,
    selected: Signal<T>,
    value: T,
) -> RadioElement {
    radio()
        .accessibility_label(label)
        .checked(move || selected.get() == value)
        .on_change(move |_: &bool| selected.set(value))
        .min_h_10()
        .p_3()
        .gap_1()
        .rounded(move || theme().radius)
        .border_1()
        .border_color(move || theme().border)
        .bg(move || theme().card)
        .checked_style(|s| {
            s.border_color(move || theme().primary)
                .bg(move || theme().accent)
        })
        .hover(|s| s.border_color(move || theme().ring))
        .active(|s| s.opacity(0.75))
        .focus(|s| {
            s.style_with(|s| s.outline(tokens::length("focus-width")))
                .outline_color(move || theme().ring)
        })
        .child(text(label).text_sm().font_weight(FontWeight::MEDIUM))
        .child(description.map(|description| {
            text(description)
                .text_sm()
                .text_color(move || theme().muted_foreground)
        }))
}
