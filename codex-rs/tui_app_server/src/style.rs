use crate::color::is_light;
use crate::terminal_palette::default_bg;
use ratatui::style::Color;
use ratatui::style::Style;

pub fn user_message_style() -> Style {
    user_message_style_for(default_bg())
}

pub fn proposed_plan_style() -> Style {
    proposed_plan_style_for(default_bg())
}

/// Returns the style for a user-authored message using the provided terminal background.
pub fn user_message_style_for(terminal_bg: Option<(u8, u8, u8)>) -> Style {
    match terminal_bg {
        Some(bg) => Style::default()
            .bg(user_message_bg(bg))
            .fg(user_message_fg(bg)),
        // Some terminals do not expose their default background color. Keep a
        // visible gray fallback so user-authored prompts are still easy to spot.
        None => Style::default().bg(Color::DarkGray).fg(Color::White),
    }
}

pub fn proposed_plan_style_for(terminal_bg: Option<(u8, u8, u8)>) -> Style {
    match terminal_bg {
        Some(bg) => Style::default()
            .bg(proposed_plan_bg(bg))
            .fg(user_message_fg(bg)),
        None => Style::default().bg(Color::DarkGray).fg(Color::White),
    }
}

#[allow(clippy::disallowed_methods)]
pub fn user_message_bg(terminal_bg: (u8, u8, u8)) -> Color {
    // Prefer a stronger fixed gray so user prompts remain obvious even on
    // terminals whose palette mapping would otherwise make the background too subtle.
    if is_light(terminal_bg) {
        Color::Rgb(224, 224, 224)
    } else {
        Color::Rgb(58, 58, 58)
    }
}

#[allow(clippy::disallowed_methods)]
fn user_message_fg(terminal_bg: (u8, u8, u8)) -> Color {
    if is_light(terminal_bg) {
        Color::Black
    } else {
        Color::White
    }
}

#[allow(clippy::disallowed_methods)]
pub fn proposed_plan_bg(terminal_bg: (u8, u8, u8)) -> Color {
    user_message_bg(terminal_bg)
}

// 编号（如：1）：修改
// 主要修改内容：为用户消息和计划块改用更明显的固定灰底、前景色兜底，以及无终端背景探测时的可见默认样式。
// 修改目的：让用户发出的内容在深浅主题和背景探测失败时都能被一眼识别出来。
