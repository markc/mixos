// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! `SetupOverlay` — the capture setup phase. A full-screen screen-space
//! instance: a dim mask with a clear hole over the prospective capture
//! target, plus a chooser (target kind + media + Confirm/Cancel). For the two
//! region kinds the hole is drawn by dragging; for Windows it mirrors the
//! pre-selected canvas windows' bbox; for FullScreen it's the whole output.
//!
//! The overlay is authoritative for the chosen kind/media/draft — the interface
//! reads them off the instance on Confirm (it reads the live canvas selection
//! itself for the Windows case).

use crate::capture::session::message::{
    CaptureMedia, CaptureMessage, OverlayPoint, OverlayRect, TargetKind,
};
use crate::strings;
use ui::UiProgram;

use crate::surface::draw::capture::{mask, style};

pub struct SetupOverlay {
    screen_w: i32,
    screen_h: i32,
    kind: TargetKind,
    media: CaptureMedia,
    /// In-progress region rect for the region kinds (screen pixels).
    draft: Option<OverlayRect>,
    /// Bbox of the pre-selected canvas windows (screen pixels), if any.
    preselect: Option<OverlayRect>,
    /// Transparent-background option (window/world targets). Default off.
    no_background: bool,
    dragging: bool,
    drag_origin: (i32, i32),
    cursor: (i32, i32),
}

impl SetupOverlay {
    pub fn new(
        screen_w: i32,
        screen_h: i32,
        media: CaptureMedia,
        kind: TargetKind,
        preselect: Option<OverlayRect>,
    ) -> Self {
        Self {
            screen_w,
            screen_h,
            kind,
            media,
            draft: None,
            preselect,
            no_background: false,
            dragging: false,
            drag_origin: (0, 0),
            cursor: (0, 0),
        }
    }

    pub fn kind(&self) -> TargetKind {
        self.kind
    }
    pub fn media(&self) -> CaptureMedia {
        self.media
    }
    pub fn draft(&self) -> Option<OverlayRect> {
        self.draft
    }
    pub fn no_background(&self) -> bool {
        self.no_background
    }

    /// The no-background toggle is only meaningful for window/world targets
    /// (screen/full-screen capture the composited screen, backdrop included).
    fn supports_no_background(&self) -> bool {
        matches!(self.kind, TargetKind::Windows | TargetKind::WorldRegion)
    }

    fn is_region(&self) -> bool {
        matches!(
            self.kind,
            TargetKind::WorldRegion | TargetKind::ScreenRegion
        )
    }

    /// The hole to clear in the mask for the current selection.
    fn hole(&self) -> Option<OverlayRect> {
        match self.kind {
            TargetKind::Windows => self.preselect,
            TargetKind::WorldRegion | TargetKind::ScreenRegion => self.draft,
            TargetKind::FullScreen => Some(OverlayRect {
                x: 0,
                y: 0,
                w: self.screen_w,
                h: self.screen_h,
            }),
        }
    }
}

fn rect_from(a: (i32, i32), b: (i32, i32)) -> OverlayRect {
    let x = a.0.min(b.0);
    let y = a.1.min(b.1);
    OverlayRect {
        x,
        y,
        w: (a.0 - b.0).abs(),
        h: (a.1 - b.1).abs(),
    }
}

impl UiProgram for SetupOverlay {
    type Message = CaptureMessage;

    fn update(&mut self, message: Self::Message) {
        match message {
            CaptureMessage::DragMove(OverlayPoint { x, y }) => {
                self.cursor = (x as i32, y as i32);
                if self.dragging && self.is_region() {
                    self.draft = Some(rect_from(self.drag_origin, self.cursor));
                }
            }
            CaptureMessage::DragStart => {
                if self.is_region() {
                    self.dragging = true;
                    self.drag_origin = self.cursor;
                    self.draft = Some(rect_from(self.cursor, self.cursor));
                }
            }
            CaptureMessage::DragEnd => {
                self.dragging = false;
            }
            CaptureMessage::SelectKind(k) => {
                self.kind = k;
            }
            CaptureMessage::SelectMedia(m) => {
                self.media = m;
            }
            CaptureMessage::SetNoBackground(v) => {
                self.no_background = v;
            }
            // Confirm/Cancel and everything else are handled by the interface.
            _ => {}
        }
    }

    fn setup(&mut self, ctx: &egui::Context) {
        style::install(ctx);
    }

    fn show(&mut self, ui: &mut egui::Ui, emit: &mut Vec<CaptureMessage>) {
        style::keep_up(ui.ctx());
        let accent = style::accent(ui);
        // Bottom layer: the mask with a clear hole, painted first.
        mask::mask_with_hole(
            ui.painter(),
            self.screen_w,
            self.screen_h,
            self.hole(),
            style::SETUP_MASK,
            Some((accent, style::BORDER_WIDTH)),
        );
        // The whole area drives the region drag. The chooser is an area above
        // it, so it takes its own clicks first and a drag only starts on the
        // masked or clear background.
        let background = ui.interact(
            ui.max_rect(),
            egui::Id::unique("capture-setup-background"),
            egui::Sense::click_and_drag(),
        );
        let (pointer, pressed, released) = ui.ctx().input(|input| {
            (
                input.pointer.hover_pos(),
                input.pointer.primary_pressed(),
                input.pointer.primary_released(),
            )
        });
        // Report motion only when it moved: `update` stores it, so the next
        // pass sees no change and emits nothing (no message loop).
        if let Some(at) = pointer
            && (at.x as i32, at.y as i32) != self.cursor
        {
            emit.push(CaptureMessage::DragMove(OverlayPoint { x: at.x, y: at.y }));
        }
        if pressed && background.hovered() {
            emit.push(CaptureMessage::DragStart);
        }
        if released && self.dragging {
            emit.push(CaptureMessage::DragEnd);
        }
        let (escape, enter) = ui.ctx().input(|input| {
            (
                input.key_pressed(egui::Key::Escape),
                input.key_pressed(egui::Key::Enter),
            )
        });
        if escape {
            emit.push(CaptureMessage::Cancel);
        } else if enter {
            emit.push(CaptureMessage::Confirm);
        }
        egui::Area::new(egui::Id::unique("capture-setup-chooser"))
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 28.0))
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| self.chooser(ui, accent, emit));
    }
}

impl SetupOverlay {
    fn chooser(&self, ui: &mut egui::Ui, accent: egui::Color32, emit: &mut Vec<CaptureMessage>) {
        egui::Frame::popup(ui.style())
            .inner_margin(20.0)
            .stroke(egui::Stroke::new(1.0, accent))
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(8.0, 12.0);
                    ui.label(egui::RichText::new(strings::text("capture-setup-title")).size(18.0));
                    ui.horizontal(|ui| {
                        for (key, kind) in [
                            ("capture-kind-windows", TargetKind::Windows),
                            ("capture-kind-world", TargetKind::WorldRegion),
                            ("capture-kind-screen", TargetKind::ScreenRegion),
                            ("capture-kind-full", TargetKind::FullScreen),
                        ] {
                            if style::choice(ui, strings::text(key), self.kind == kind).clicked() {
                                emit.push(CaptureMessage::SelectKind(kind));
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        for (key, media) in [
                            ("capture-media-screenshot", CaptureMedia::Screenshot),
                            ("capture-media-video", CaptureMedia::Video),
                        ] {
                            if style::choice(ui, strings::text(key), self.media == media).clicked()
                            {
                                emit.push(CaptureMessage::SelectMedia(media));
                            }
                        }
                    });
                    // Transparent-background toggle (window/world only).
                    if self.supports_no_background() {
                        let mut on = self.no_background;
                        if ui
                            .checkbox(&mut on, strings::text("capture-transparent"))
                            .changed()
                        {
                            emit.push(CaptureMessage::SetNoBackground(on));
                        }
                    }
                    let hint = match self.kind {
                        TargetKind::Windows => "capture-hint-windows",
                        TargetKind::WorldRegion => "capture-hint-world",
                        TargetKind::ScreenRegion => "capture-hint-screen",
                        TargetKind::FullScreen => "capture-hint-full",
                    };
                    ui.label(
                        egui::RichText::new(strings::text(hint))
                            .size(13.0)
                            .color(style::weak(ui)),
                    );
                    ui.horizontal(|ui| {
                        if style::button(ui, strings::text("capture-confirm"), Some(accent))
                            .clicked()
                        {
                            emit.push(CaptureMessage::Confirm);
                        }
                        if style::button(ui, strings::text("capture-cancel"), None).clicked() {
                            emit.push(CaptureMessage::Cancel);
                        }
                    });
                });
            });
    }
}

/// What the capture driver reads off this overlay when the user commits.
#[derive(Clone, Debug)]
pub struct SetupSnapshot {
    pub kind: TargetKind,
    pub media: CaptureMedia,
    pub draft: Option<OverlayRect>,
    pub no_background: bool,
}

/// These fields only move on a click, which has already been through a full
/// update+render cycle by the time the driver reads them, so the one frame of
/// staleness an off-thread snapshot carries is not observable.
impl ui::UiSnapshot for SetupOverlay {
    type Snapshot = SetupSnapshot;
    fn snapshot(&self) -> SetupSnapshot {
        SetupSnapshot {
            kind: self.kind,
            media: self.media,
            draft: self.draft,
            no_background: self.no_background,
        }
    }
}
