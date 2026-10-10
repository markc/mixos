// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tooltips (chrome specification §3.20). egui's own tooltips already meet
//! the specification under the chrome style: the popup frame (`card`, a
//! 1 pt `card_border`, `radius`, a 6 pt margin and the popup shadow), Body
//! text, at most 280 pt wide, 4 pt below the widget, shown after the pointer
//! rests 0.35 s, with a 0.2 s grace period for neighbours. What is left is
//! the wording: a tooltip that carries a shortcut reads "Label (Shortcut)".

use crate::command::Command;
use crate::strings::Strings;

/// A tooltip's text: `label`, then `shortcut` in parentheses when it has one.
pub fn text(label: &str, shortcut: Option<&str>) -> String {
    match shortcut {
        Some(shortcut) if !shortcut.is_empty() => format!("{label} ({shortcut})"),
        _ => label.to_owned(),
    }
}

/// The tooltip of `command`: its label and its shortcut in the platform's
/// form.
pub fn for_command<S>(ctx: &egui::Context, command: &Command<S>, strings: &Strings) -> String {
    let shortcut = command.shortcut.map(|s| ctx.format_shortcut(&s));
    text(
        &crate::command::label_text(strings, command.label),
        shortcut.as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shortcut_follows_in_parentheses() {
        assert_eq!(
            text("Search commands", Some("Ctrl+K")),
            "Search commands (Ctrl+K)"
        );
        assert_eq!(text("Close", None), "Close");
        assert_eq!(text("Close", Some("")), "Close");
    }
}
