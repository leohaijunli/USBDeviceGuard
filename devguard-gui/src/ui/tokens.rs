//! One source for colors and the few geometry values outside NSH's scale.

use nsh_base::{
    ScrollbarLook, ScrollbarMode, ScrollbarTheme, set_scrollbar_theme, theme, Theme,
};
use nsh_core::prelude::*;

pub(crate) const THEME_JSON: &str = include_str!("../../theme.json");

pub(crate) fn install() {
    let theme =
        Theme::from_json(THEME_JSON).expect("the bundled theme must pass its build-time test");
    set_tokens(theme.into_tokens());
    install_scrollbars();
}

/// Every scroll view and list gets a bar that stays visible and can be
/// dragged, so a short window never hides content behind an invisible
/// indicator. Set once per app instance: the theme is engine state.
fn install_scrollbars() {
    let installed = stored(false);
    if installed.get() {
        return;
    }
    installed.replace(true);
    let mut vertical = ScrollbarLook::default();
    vertical.class = Some("absolute top-0 right-0 bottom-0 w-3 p-0.5".into());
    vertical.thumb_class =
        Some("w-full rounded-full bg-muted-foreground/40 hover:bg-muted-foreground/70".into());
    vertical.accessibility_label = Some("Scroll".into());
    let mut horizontal = ScrollbarLook::default();
    horizontal.class = Some("absolute left-0 right-0 bottom-0 h-3 p-0.5".into());
    horizontal.thumb_class =
        Some("h-full rounded-full bg-muted-foreground/40 hover:bg-muted-foreground/70".into());
    horizontal.accessibility_label = Some("Scroll horizontally".into());
    let mut bars = ScrollbarTheme::default();
    bars.mode = ScrollbarMode::Always;
    bars.vertical = Some(vertical);
    bars.horizontal = Some(horizontal);
    set_scrollbar_theme(Some(bars));
}

pub(crate) fn color_role(name: &str) -> AdaptiveColor {
    theme().color(name).unwrap_or_else(|| theme().destructive)
}

pub(crate) fn length(name: &str) -> Points {
    px(theme().number(name).unwrap_or(0.0))
}

/// NSH's 4pt scale for APIs that accept Points instead of spacing builders.
pub(crate) fn space(steps: f32) -> Points {
    px(steps * 4.0)
}

pub(crate) fn wide_layout() -> bool {
    // Large text gets the same single-pane fallback as a narrow window.
    window().width >= length("two-pane-breakpoint") * font_scale().max(1.0)
}
