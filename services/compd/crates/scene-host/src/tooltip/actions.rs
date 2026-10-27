// SPDX-License-Identifier: MIT OR Apache-2.0
use crate::view::tip::{self, Candidate, Hover, Outcome, Tip, TipAt};
use std::time::Instant;
use world::state::Loop;

#[derive(Default)]
pub(crate) struct Actions {
    hover: Hover,
    shown: Option<(String, TipAt)>,
    wake: Option<Instant>,
    ctx: egui::Context,
    appearance: Option<u64>,
}

impl Actions {
    pub fn update(
        &mut self,
        state: &Loop,
        port: &crate::port::Port,
        prepared: Option<&crate::appearance::Look>,
        generation: u64,
        suppressed: bool,
    ) -> Option<((String, TipAt), (f32, f32, f32, f32))> {
        let target = decor::seat::action_tip().filter(|_| !suppressed);
        let output = state.inner.current_output();
        let space = &state.inner.space_state().state;
        let output_rect = space.output_geometry(&output)?;
        let had_target = target.is_some();
        let candidate = target.and_then(|(window, rect, text, key, pressed)| {
            let loc = space.element_location(&window)?;
            let ext = decor::window::extents(&window)?;
            let x = loc.x as f32 - ext.left + rect.x - output_rect.loc.x as f32;
            let y = loc.y as f32 - ext.top + rect.y - output_rect.loc.y as f32;
            let bounds = egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(output_rect.size.w as f32, output_rect.size.h as f32),
            );
            let rect = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(rect.w, rect.h));
            if !bounds.contains(rect.center()) {
                return None;
            }
            Some((
                Candidate {
                    key,
                    rect,
                    tip: Tip {
                        title: text,
                        ..Default::default()
                    },
                },
                pressed,
            ))
        });
        if had_target && candidate.is_none() {
            return None;
        }
        let pressed = candidate.as_ref().is_some_and(|(_, pressed)| *pressed);
        let now = Instant::now();
        let ctx = self.ctx.clone();
        let theme = prepared
            .map(|look| look.theme.clone())
            .unwrap_or_else(|| std::sync::Arc::new(toolkit::theme::Theme::embedded()));
        if self.appearance != Some(generation) {
            crate::appearance::install(
                &ctx,
                &theme,
                prepared.map_or(1.0, |look| look.text_scale),
                false,
            );
            self.appearance = Some(generation);
        }
        let size = super::ui_size(prepared, &theme);
        let measure = |text: &str, font: &egui::FontId| {
            let size = ctx.fonts_mut(|fonts| {
                fonts
                    .layout_no_wrap(text.into(), font.clone(), egui::Color32::WHITE)
                    .size()
            });
            (size.x, size.y)
        };
        let mut outcome = Outcome::Same;
        let _ = ctx.run(Default::default(), |_| {
            outcome = self.hover.pass(
                now,
                candidate.clone().map(|(candidate, _)| candidate),
                pressed,
                &|tip| tip::card(tip, size, &measure),
            );
        });
        match outcome {
            Outcome::Show(at) => {
                self.shown = Some(("__window_actions".into(), at));
                self.wake = None;
            }
            Outcome::Hide => {
                self.shown = None;
                self.wake = None;
            }
            Outcome::Wait(delay) => {
                if self.wake.is_none_or(|wake| wake <= now) {
                    port.wake_after(delay);
                    self.wake = Some(now + delay);
                }
            }
            Outcome::Same => {}
        }
        self.shown.clone().map(|(key, mut at)| {
            let rect = at.anchor;
            at.anchor = (0.0, 0.0, rect.2, rect.3);
            ((key, at), rect)
        })
    }
}
