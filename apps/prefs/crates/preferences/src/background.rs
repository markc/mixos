// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Carry the wallpaper editor's calls through Preferences' effect queue.
use crate::wallpaper::{Call, Kind};
use crate::{Effect, Engine};
use citizen::{CallError, Reply};

impl Engine {
    fn backdrop_call(&mut self, call: Option<Call>) {
        if let Some(call) = call {
            self.effects.push(Effect::Backdrop {
                ticket: call.ticket,
                command: call.command,
                body: call.body,
            });
        }
    }
    pub fn read_wallpaper(&mut self) {
        if !self.connected || self.quitting {
            return;
        }
        let ticket = self.next();
        let call = self.wallpaper.read(ticket);
        self.backdrop_call(call);
    }
    pub fn can_edit_wallpaper(&self) -> bool {
        self.connected
            && !self.quitting
            && self.ui.dialog.is_none()
            && !self.picker.open
            && self.wallpaper.can_edit()
    }
    pub fn can_apply_wallpaper(&self) -> bool {
        self.can_edit_wallpaper() && self.wallpaper.can_apply() && !self.ui.wallpaper_invalid
    }
    pub fn apply_wallpaper(&mut self) {
        if self.can_apply_wallpaper() {
            let ticket = self.next();
            let call = self.wallpaper.apply(ticket);
            self.backdrop_call(call);
        }
    }
    pub fn can_revert_wallpaper(&self) -> bool {
        self.connected
            && !self.quitting
            && self.ui.dialog.is_none()
            && !self.picker.open
            && !self.wallpaper.working()
            && (self.wallpaper.dirty()
                || self.ui.wallpaper_kind.is_some()
                || self.ui.wallpaper_invalid)
    }
    fn clear_wallpaper_inputs(&mut self) {
        self.ui.wallpaper_kind = None;
        self.ui.wallpaper_colour = None;
        self.ui.wallpaper_image = None;
        self.ui.wallpaper_invalid = false;
    }
    pub fn revert_wallpaper(&mut self) {
        if self.can_revert_wallpaper() {
            self.wallpaper.revert();
            self.clear_wallpaper_inputs();
        }
    }
    pub fn wallpaper_target(&mut self, target: Option<String>) {
        if self.can_edit_wallpaper() {
            self.wallpaper.select_target(target);
            self.clear_wallpaper_inputs();
        }
    }
    pub fn wallpaper_kind(&mut self, kind: Kind) {
        if !self.can_edit_wallpaper() {
            return;
        }
        self.ui.wallpaper_kind = Some(kind);
        match kind {
            Kind::Theme => {
                self.ui.wallpaper_invalid = self.wallpaper.edit_kind(kind).is_err();
            }
            Kind::Colour => {
                let colour = self
                    .ui
                    .wallpaper_colour
                    .clone()
                    .or_else(|| self.wallpaper.fill()["colour"].as_str().map(str::to_owned))
                    .unwrap_or_else(|| "#000000".into());
                self.wallpaper_colour(colour);
            }
            Kind::Image => {
                let image = self
                    .ui
                    .wallpaper_image
                    .clone()
                    .or_else(|| self.wallpaper.fill()["image"].as_str().map(str::to_owned))
                    .unwrap_or_default();
                self.wallpaper_image(image);
            }
        }
    }
    pub fn wallpaper_colour(&mut self, colour: String) {
        if self.can_edit_wallpaper() {
            self.ui.wallpaper_colour = Some(colour.clone());
            self.ui.wallpaper_invalid = self.wallpaper.edit_colour(colour).is_err();
        }
    }
    pub fn wallpaper_image(&mut self, image: String) {
        if self.can_edit_wallpaper() {
            self.ui.wallpaper_image = Some(image.clone());
            self.ui.wallpaper_invalid = self.wallpaper.edit_image(image).is_err();
        }
    }
    pub fn wallpaper_finished(&mut self, ticket: u64, result: Result<Reply, CallError>) {
        let next = self.next();
        let call = self.wallpaper.complete(ticket, result, next);
        self.backdrop_call(call);
        if !self.wallpaper.dirty() && !self.ui.wallpaper_invalid {
            self.clear_wallpaper_inputs();
        }
        if self.quitting && !self.busy() {
            self.close_now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn label(key: &str, _: &[(&str, &str)]) -> String {
        key.into()
    }
    fn loaded() -> Engine {
        let mut e = Engine::new(label);
        let release = match e.take_effects().pop().unwrap() {
            Effect::Releases { ticket, .. } => ticket,
            _ => panic!("listing"),
        };
        e.released(
            release,
            Ok(Reply {
                rc: 0,
                body: "[]".into(),
                error: None,
            }),
        );
        e.take_effects();
        e.read_wallpaper();
        let ticket = match e.take_effects().pop().unwrap() {
            Effect::Backdrop { ticket, .. } => ticket,
            _ => panic!("read"),
        };
        e.wallpaper_finished(ticket,Ok(Reply{rc:0,body:json!({"schema":"backdrop.v1","config":{"schema":1,"default":{"colour":"#112233"}},"outputs":[],"pending":null}).to_string(),error:None}));
        e
    }
    #[test]
    fn incomplete_input_cannot_apply_the_previous_valid_value() {
        let mut e = loaded();
        e.wallpaper_kind(Kind::Colour);
        e.wallpaper_colour("#445566".into());
        assert!(e.can_apply_wallpaper());
        e.wallpaper_colour("#4455".into());
        assert!(!e.can_apply_wallpaper());
        assert_eq!(e.ui.wallpaper_colour.as_deref(), Some("#4455"));
        e.revert_wallpaper();
        assert!(!e.ui.wallpaper_invalid);
        assert!(!e.wallpaper.dirty());
    }
    #[test]
    fn closing_waits_for_the_accepted_wallpaper_operation() {
        let mut e = loaded();
        e.wallpaper_colour("#445566".into());
        e.apply_wallpaper();
        let ticket = match e.take_effects().pop().unwrap() {
            Effect::Backdrop { ticket, .. } => ticket,
            _ => panic!("preflight"),
        };
        e.quit();
        assert!(e.busy());
        assert!(!e.take_effects().iter().any(|f| matches!(f, Effect::Exit)));
        e.wallpaper_finished(
            ticket,
            Err(CallError {
                message: "lost".into(),
                outcome_unknown: false,
            }),
        );
        assert!(e.take_effects().iter().any(|f| matches!(f, Effect::Exit)));
    }
}
