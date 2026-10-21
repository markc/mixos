// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Modal dialogs (chrome specification §3.13, §3.21).
//!
//! To AccessKit a dialog is one node of role Dialog, modal, labelled with
//! its title, with the title, body and buttons inside it.
//!
//! A dialog is the popup frame (`card`, a 1 pt `card_border`, `radius`, a
//! 6 pt margin and the popup shadow), 380 to 440 pt wide by default, over
//! an undimmed window: images behind it keep their true contrast, and a
//! click outside does nothing. Top to bottom: the title in Inter SemiBold
//! 15 (not selectable); 4 pt, a full-width `separator` hairline and 8 pt;
//! the body; the button row at the right edge.
//!
//! It opens centred, then keeps its top-left where it is, so a body that
//! grows extends down and right; dragging the title row moves it, kept on
//! screen. Esc cancels (the topmost dialog only, and only while no popup is
//! open); Enter runs the default button while no widget had keyboard focus
//! when the frame began (a focused button or field takes Enter itself).
//!
//! **Button order.** Buttons are given in the Windows and Linux order, the
//! default first ("OK, Cancel, Apply"; "Yes, No, Cancel"). On macOS the
//! default goes last with Cancel beside it and the alternates further left.
//! Either way they are laid out left to right, so Tab follows reading order.

use crate::bars;
use crate::button::PushButton;
use crate::chrome::Chrome;
use egui::{
    Align2, Area, Color32, Context, Id, Key, Label, Modal, Modifiers, Order, Pos2, Rangef,
    RichText, Sense, Ui, UiKind, Vec2, os::OperatingSystem,
};

/// Title size, SemiBold (§2.1), and the gaps round the hairline (§3.21).

/// How far below the window's centre a dialog first opens. The
/// specification says only "centred" (§3.21); the evidence centres every
/// theme's dialog 1.75 pt low (frames of 282.5 and 284.5 pt both centred
/// on y 401.75 of 800), as if centred in a window 3.5 pt shorter at the
/// top. The cause is not determined (§5.4); this matches it.

/// The most of the window a resizable dialog's body may fill.

/// The default width range (§3.21).

/// The gap above the button row, between its buttons, and each button's
/// least width. Undetermined by the specification (§5.4); the evidence's
/// OK and Cancel are 84 pt wide and 10 pt apart.

/// What a dialog button does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// The default action: the primary button, run by Enter.
    Default,
    /// Run by Esc.
    Cancel,
    /// An alternate ("Apply", "No").
    Other,
}

/// One dialog button.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub label: String,
    pub role: Role,
    pub enabled: bool,
}

impl Choice {
    pub fn new(label: impl Into<String>, role: Role) -> Self {
        Self {
            label: label.into(),
            role,
            enabled: true,
        }
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

/// `choices` in the order the platform lays them out, as indices into
/// `choices` (given in the Windows and Linux order).
pub fn order(choices: &[Choice], os: OperatingSystem) -> Vec<usize> {
    let all = 0..choices.len();
    if os != OperatingSystem::Mac {
        return all.collect();
    }
    let of = |role: Role| all.clone().filter(move |&i| choices[i].role == role);
    of(Role::Other)
        .chain(of(Role::Cancel))
        .chain(of(Role::Default))
        .collect()
}

/// What happened in a dialog this frame.
pub struct DialogResponse<R> {
    pub inner: R,
    /// The index (into the choices given) of the button chosen by click,
    /// Enter or Esc.
    pub chosen: Option<usize>,
    /// Esc on a dialog with no Cancel button.
    pub dismissed: bool,
    /// The dialog's frame.
    pub rect: egui::Rect,
}

/// Where a dialog sits: unplaced until it has been shown centred once.
#[derive(Clone, Copy, Debug, Default)]
struct Placement {
    at: Option<Pos2>,
    shown: bool,
}

/// A modal dialog.
pub struct Dialog {
    id: Id,
    title: String,
    width: Option<Rangef>,
    /// The body's size when the person may resize it: (default, minimum).
    /// The size is kept by egui for the id, so it is remembered while the
    /// app runs. None keeps the fixed width range.
    resize: Option<(Vec2, Vec2)>,
    choices: Vec<Choice>,
}

impl Dialog {
    pub fn new(id: impl Into<Id>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            width: None,
            resize: None,
            choices: Vec::new(),
        }
    }

    /// A wider range, for complex dialogs (600) or a new-document one (800).
    pub fn width(mut self, width: Rangef) -> Self {
        self.width = Some(width);
        self
    }

    /// The body can be resized from its bottom-right corner, starting at
    /// `default` and never below `min`. Replaces the fixed width range.
    pub fn resizable(mut self, default: Vec2, min: Vec2) -> Self {
        self.resize = Some((default, min));
        self
    }

    /// The buttons, default first.
    pub fn buttons(mut self, choices: Vec<Choice>) -> Self {
        self.choices = choices;
        self
    }

    /// Forget where the dialog was, so it opens centred next time.
    pub fn reset(ctx: &Context, id: impl Into<Id>) {
        ctx.data_mut(|d| d.remove::<Placement>(id.into()));
    }

    pub fn show<R>(self, ctx: &Context, body: impl FnOnce(&mut Ui) -> R) -> DialogResponse<R> {
        let Self {
            id,
            title,
            width,
            resize,
            choices,
        } = self;
        let tokens = Chrome::of(ctx).style;
        let width = width.unwrap_or(Rangef { min: tokens.dialog_min_width as f32, max: tokens.dialog_max_width as f32 });
        let mut place: Placement = ctx.data(|d| d.get_temp(id)).unwrap_or_default();
        let mut area = Area::new(id)
            .kind(UiKind::Modal)
            .sense(Sense::hover())
            .order(Order::Foreground)
            .interactable(true);
        area = match place.at {
            Some(at) => area.fixed_pos(at),
            None => area.anchor(Align2::CENTER_CENTER, Vec2::new(0.0, crate::chrome::Chrome::of(ctx).style.dialog_centre_drop as f32)),
        };
        let os = ctx.os();
        // Read before the body draws: a text or value field gives up focus
        // on Enter without consuming it, so after the body nothing looks
        // focused and the key would run the default button too.
        let focused_before = ctx.memory(|m| m.focused()).is_some();
        let mut chosen = None;
        let mut title_drag = Vec2::ZERO;
        let modal = Modal::new(id)
            .area(area)
            .backdrop_color(Color32::TRANSPARENT)
            .show(ctx, |ui| {
                // To assistive technology (and the drive layer) the frame is a
                // modal dialog named by its title, its controls its children.
                let name = title.clone();
                ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
                    node.set_role(egui::accesskit::Role::Dialog);
                    node.set_modal();
                    node.set_label(name);
                });
                if resize.is_none() {
                    ui.set_min_width(width.min);
                    ui.set_max_width(width.max);
                }
                let font = crate::fonts::bound(ui.ctx(), crate::fonts::heading(crate::chrome::Chrome::of(ctx).style.dialog_title_size as f32));
                let ink = Chrome::of(ui.ctx()).palette.text;
                let title = ui
                    .add(Label::new(RichText::new(&title).font(font).color(ink)).selectable(false));
                let row =
                    egui::Rect::from_x_y_ranges(ui.max_rect().x_range(), title.rect.y_range());
                title_drag = ui
                    .interact(row, id.with("title"), Sense::drag())
                    .drag_delta();
                // 4 pt beyond the item spacing (measured: the rule sits 11.5 pt
                // under the title's glyphs); 8 pt exactly below it.
                ui.add_space(crate::chrome::Chrome::of(ctx).style.dialog_above_rule as f32);
                bars::hairline(ui);
                ui.add_space(crate::chrome::Chrome::of(ctx).style.dialog_below_rule as f32 - ui.spacing().item_spacing.y);
                let inner = match resize {
                    Some((default, min)) => {
                        // The body keeps the size the person left it at, up to
                        // most of the window; the buttons stay below it.
                        let max = ui.ctx().content_rect().size() * crate::chrome::Chrome::of(ctx).style.dialog_resize_max_share as f32;
                        egui::Resize::default()
                            .id_salt(id.with("size"))
                            .default_size(default)
                            .min_size(min)
                            .max_size(max)
                            .show(ui, body)
                    }
                    None => body(ui),
                };
                if !choices.is_empty() {
                    ui.add_space(crate::chrome::Chrome::of(ctx).style.dialog_above_buttons as f32);
                    chosen = button_row(ui, &choices, os);
                }
                inner
            });
        let rect = modal.response.rect;
        if place.shown || place.at.is_some() {
            let screen = ctx.content_rect();
            let at = place.at.unwrap_or(rect.min) + title_drag;
            let max = (screen.max - rect.size()).max(screen.min);
            place.at = Some(at.clamp(screen.min, max));
        }
        place.shown = true;
        ctx.data_mut(|d| d.insert_temp(id, place));

        let mut dismissed = false;
        if chosen.is_none() && modal.is_top_modal && !modal.any_popup_open {
            let find = |role: Role| choices.iter().position(|c| c.role == role && c.enabled);
            if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                chosen = find(Role::Cancel);
                dismissed = chosen.is_none();
            } else if !focused_before
                && ctx.memory(|m| m.focused().is_none())
                && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter))
            {
                chosen = find(Role::Default);
            }
        }
        DialogResponse {
            inner: modal.inner,
            chosen,
            dismissed,
            rect,
        }
    }
}

/// The button row, right-aligned and laid out left to right.
fn button_row(ui: &mut Ui, choices: &[Choice], os: OperatingSystem) -> Option<usize> {
    let font = crate::fonts::bound(ui.ctx(), crate::fonts::medium(crate::button::push_size_of(ui.ctx())));
    let ordered = order(choices, os);
    let widths: Vec<f32> = ordered
        .iter()
        .map(|&i| {
            let galley = ui.painter().layout_no_wrap(
                choices[i].label.clone(),
                font.clone(),
                Color32::PLACEHOLDER,
            );
            (galley.size().x + 28.0).max(crate::chrome::Chrome::of(ui.ctx()).style.dialog_button_min as f32)
        })
        .collect();
    let total = widths.iter().sum::<f32>() + crate::chrome::Chrome::of(ui.ctx()).style.dialog_button_gap as f32 * (widths.len().saturating_sub(1)) as f32;
    let height = Chrome::of(ui.ctx()).metrics.push_height;
    let (_, row) = ui.allocate_space(egui::vec2(ui.available_width(), height));
    // Placed left to right, so focus order is reading order, flush right.
    let mut x = row.right() - total;
    let mut chosen = None;
    for (&index, width) in ordered.iter().zip(&widths) {
        let choice = &choices[index];
        let button = match choice.role {
            Role::Default => PushButton::primary(&choice.label),
            Role::Cancel | Role::Other => PushButton::secondary(&choice.label),
        };
        let rect = egui::Rect::from_min_size(egui::pos2(x, row.top()), egui::vec2(*width, height));
        let mut cell = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        if !choice.enabled {
            cell.disable();
        }
        if cell.add(button.min_width(*width)).clicked() {
            chosen = Some(index);
        }
        x += width + crate::chrome::Chrome::of(ui.ctx()).style.dialog_button_gap as f32;
    }
    chosen
}


/// The installed value of the former `WIDTH` constant.
pub fn width_of(ctx: &egui::Context) -> Rangef { Rangef { min: crate::chrome::Chrome::of(ctx).style.dialog_min_width as f32, max: crate::chrome::Chrome::of(ctx).style.dialog_max_width as f32 } }

#[cfg(test)]
mod tests {
    use super::*;

    fn choices() -> Vec<Choice> {
        vec![
            Choice::new("OK", Role::Default),
            Choice::new("Cancel", Role::Cancel),
            Choice::new("Apply", Role::Other),
        ]
    }

    #[test]
    fn the_default_comes_first_except_on_macos() {
        assert_eq!(order(&choices(), OperatingSystem::Nix), [0, 1, 2]);
        assert_eq!(order(&choices(), OperatingSystem::Windows), [0, 1, 2]);
        assert_eq!(
            order(&choices(), OperatingSystem::Mac),
            [2, 1, 0],
            "alternates, Cancel, then the default"
        );
    }
}
