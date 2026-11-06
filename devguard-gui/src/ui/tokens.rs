//! One source for colors and the few geometry values outside NSH's scale.

use nsh_base::{theme, Theme};
use nsh_core::prelude::*;

pub(crate) const THEME_JSON: &str = include_str!("../../theme.json");

pub(crate) fn install() {
    let theme =
        Theme::from_json(THEME_JSON).expect("the bundled theme must pass its build-time test");
    set_tokens(theme.into_tokens());
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
