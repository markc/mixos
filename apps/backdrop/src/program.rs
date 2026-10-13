// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The wallpaper on each output's Background surface. The program draws only
//! from a snapshot the controller publishes ([`View`]): it never decodes and
//! never polls. It draws when the layer loop asks (a configure, a scale, or a
//! repaint the controller requested through a context handed over in
//! [`LayerProgram::setup`]).
//!
//! Texture rules:
//!
//! - a context owns at most one image texture, in its own temporary data, so
//!   one output never reuses another's texture;
//! - every texture is paid for from one aggregate [`Budget`] ([`TEXTURE_BUDGET`]).
//!   A reservation is held by the slot that owns the texture and released when
//!   the slot is replaced, removed or its context goes;
//! - replacing a texture reserves the new size while the old is still held, so
//!   the overlap is counted. A texture that does not fit leaves its output on
//!   its fallback, with the reason in the output's status;
//! - a fallback or theme colour is painted beneath every image, so an alpha
//!   image shows what is beneath it, not a hole.

use egui::{Color32, ColorImage, CornerRadius, Id, Rect, TextureHandle, TextureOptions, pos2};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use toolkit::layer::{LayerProgram, SurfaceInfo};
use wallpaper::decode::Decoded;
use wallpaper::{Config, Rgb, Wallpaper, fit};

/// The most texture memory, in bytes, all outputs' images may hold together.
/// At 3840 × 3840 RGBA (59 MiB) that is four full-size images. A smaller
/// image costs what it is.
pub const TEXTURE_BUDGET: u64 = 256 * 1024 * 1024;

/// The owner key a context carries in its own data: which wakers entry is it.
const OWNER: &str = "backdrop-owner";
/// The slot a context keeps its one image texture in.
const SLOT: &str = "backdrop-wallpaper-texture";

/// The egui contexts of the live surfaces, each with its owner token, which the
/// controller wakes on a change.
pub type Wakers = Arc<Mutex<Vec<(u64, egui::Context)>>>;

/// A decoded image the controller keeps, with a serial that changes whenever
/// the image does (so a context knows to re-upload).
pub struct Resident {
    pub serial: u64,
    pub image: Decoded,
}

/// What the program shows: the applied configuration and the images it names.
#[derive(Default)]
pub struct View {
    pub revision: u64,
    pub config: Config,
    pub images: BTreeMap<PathBuf, Arc<Resident>>,
}

/// One output as the program last drew it.
#[derive(Debug, Clone, PartialEq)]
pub struct OutputStatus {
    pub revision: u64,
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    /// What the output shows: `theme`, `colour`, `image`, or `fallback` while
    /// its image is not decoded or does not fit the texture budget.
    pub shows: &'static str,
    /// Why the output is not showing its image, if it is not.
    pub error: Option<String>,
}

/// State shared with the controller.
#[derive(Default)]
pub struct Shared {
    pub view: Arc<View>,
    pub outputs: BTreeMap<String, OutputStatus>,
}

/// Lock, reading through a poisoned lock: the data is plain and still valid.
pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The aggregate texture budget: bytes reserved by live textures.
#[derive(Debug)]
pub struct Budget {
    limit: u64,
    used: Mutex<u64>,
}

impl Budget {
    pub fn new(limit: u64) -> Arc<Self> {
        Arc::new(Self {
            limit,
            used: Mutex::new(0),
        })
    }

    /// Reserve `bytes`, or `None` when they would take the budget over its limit.
    pub fn reserve(self: &Arc<Self>, bytes: u64) -> Option<Reservation> {
        let mut used = lock(&self.used);
        let total = used.checked_add(bytes)?;
        if total > self.limit {
            return None;
        }
        *used = total;
        Some(Reservation {
            budget: self.clone(),
            bytes,
        })
    }

    /// Bytes reserved now.
    pub fn used(&self) -> u64 {
        *lock(&self.used)
    }
}

/// Bytes held against a [`Budget`]; released when dropped.
#[derive(Debug)]
pub struct Reservation {
    budget: Arc<Budget>,
    bytes: u64,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let mut used = lock(&self.budget.used);
        *used = used.saturating_sub(self.bytes);
    }
}

/// The image texture a context owns, with the bytes it is paid for in.
#[derive(Clone)]
struct Slot {
    serial: u64,
    texture: TextureHandle,
    reservation: Arc<Reservation>,
}

pub struct Backdrop {
    theme: toolkit::Theme,
    shared: Arc<Mutex<Shared>>,
    /// The live surfaces' contexts, with their owner tokens.
    wakers: Wakers,
    budget: Arc<Budget>,
    /// The next owner token handed to a context.
    next_owner: u64,
    /// What the last pass showed and why, for the status reported after it.
    shows: &'static str,
    revision: u64,
    error: Option<String>,
}

impl Backdrop {
    pub fn new(theme: toolkit::Theme, shared: Arc<Mutex<Shared>>, wakers: Wakers) -> Self {
        Self {
            theme,
            shared,
            wakers,
            budget: Budget::new(TEXTURE_BUDGET),
            next_owner: 0,
            shows: "theme",
            revision: 0,
            error: None,
        }
    }
}

fn colour(rgb: Rgb) -> Color32 {
    let [r, g, b] = rgb.0;
    Color32::from_rgb(r, g, b)
}

fn paint(ui: &egui::Ui, rect: Rect, fill: Color32) {
    ui.painter().rect_filled(rect, CornerRadius::ZERO, fill);
}

/// The decoded pixels as egui's image (one copy, made when a context uploads).
fn upload(image: &Decoded) -> ColorImage {
    let [w, h] = image.size;
    ColorImage::from_rgba_unmultiplied([w as usize, h as usize], image.rgba.as_raw())
}

/// Draw `resident` over `rect` on `ui`'s context, centre-cropped. Err when the
/// texture does not fit the budget; the context then holds no image texture.
fn draw_image(
    ui: &egui::Ui,
    resident: &Resident,
    rect: Rect,
    budget: &Arc<Budget>,
) -> Result<(), String> {
    let ctx = ui.ctx().clone();
    let slot_id = Id::new(SLOT);
    let previous = ctx.data(|d| d.get_temp::<Slot>(slot_id));
    let texture = match previous {
        Some(slot) if slot.serial == resident.serial => slot.texture,
        previous => {
            // The new size is reserved while the old one is still held, so the
            // overlap is counted against the budget.
            let bytes = resident.image.bytes();
            let Some(reservation) = budget.reserve(bytes) else {
                ctx.data_mut(|d| d.remove::<Slot>(slot_id));
                return Err(format!(
                    "texture budget: {bytes} bytes will not fit beside {} of {} bytes in use",
                    budget.used(),
                    budget.limit
                ));
            };
            let texture = match previous {
                Some(mut slot) => {
                    slot.texture.set(upload(&resident.image), TextureOptions::LINEAR);
                    slot.texture
                }
                None => ctx.load_texture(
                    "backdrop-wallpaper",
                    upload(&resident.image),
                    TextureOptions::LINEAR,
                ),
            };
            // Replacing the slot drops the old reservation, after the new one.
            ctx.data_mut(|d| {
                d.insert_temp(
                    slot_id,
                    Slot {
                        serial: resident.serial,
                        texture: texture.clone(),
                        reservation: Arc::new(reservation),
                    },
                )
            });
            texture
        }
    };
    let [w, h] = resident.image.size;
    let uv = fit::centre_crop([w, h], [rect.width(), rect.height()]);
    let uv = Rect::from_min_max(pos2(uv.min[0], uv.min[1]), pos2(uv.max[0], uv.max[1]));
    ui.painter().image(texture.id(), rect, uv, Color32::WHITE);
    Ok(())
}

impl LayerProgram for Backdrop {
    fn setup(&mut self, ctx: &egui::Context) {
        toolkit::install(ctx, &self.theme);
        // The owner token lives in the context's own data, so `removed` can find
        // this context's entry again without comparing contexts.
        self.next_owner += 1;
        let owner = self.next_owner;
        ctx.data_mut(|d| d.insert_temp(Id::new(OWNER), owner));
        lock(&self.wakers).push((owner, ctx.clone()));
    }

    fn show(&mut self, ui: &mut egui::Ui, surface: &SurfaceInfo) {
        let view = lock(&self.shared).view.clone();
        self.revision = view.revision;
        let rect = ui.max_rect();
        let theme = ui.visuals().panel_fill;
        let mut error = None;
        self.shows = match view.config.for_output(surface.output.as_deref()) {
            Wallpaper::Theme => {
                paint(ui, rect, theme);
                "theme"
            }
            Wallpaper::Colour(rgb) => {
                paint(ui, rect, colour(*rgb));
                "colour"
            }
            Wallpaper::Image { path, fallback } => {
                // Beneath every image: the fallback, else the theme.
                paint(ui, rect, fallback.map_or(theme, colour));
                match view.images.get(path) {
                    Some(resident) => match draw_image(ui, resident, rect, &self.budget) {
                        Ok(()) => "image",
                        Err(reason) => {
                            error = Some(reason);
                            "fallback"
                        }
                    },
                    None => "fallback",
                }
            }
        };
        self.error = error;
        // Any shown image that is not this pass's leaves the context: a solid
        // fill or the theme holds no texture (its memory goes back to the budget).
        if self.shows != "image" {
            ui.ctx().data_mut(|d| d.remove::<Slot>(Id::new(SLOT)));
        }
    }

    /// Record what a pass drew, once it was presented.
    fn presented(&mut self, surface: &SurfaceInfo) {
        let Some(name) = surface.output.clone() else {
            return;
        };
        lock(&self.shared).outputs.insert(
            name,
            OutputStatus {
                revision: self.revision,
                width: surface.width,
                height: surface.height,
                scale: surface.scale,
                shows: self.shows,
                error: self.error.clone(),
            },
        );
    }

    /// The surface is going away: forget its context, free its texture (its
    /// reservation goes back to the budget) and its output's status.
    fn removed(&mut self, ctx: &egui::Context, surface: &SurfaceInfo) {
        if let Some(owner) = ctx.data(|d| d.get_temp::<u64>(Id::new(OWNER))) {
            lock(&self.wakers).retain(|(token, _)| *token != owner);
        }
        ctx.data_mut(|d| d.remove::<Slot>(Id::new(SLOT)));
        if let Some(name) = &surface.output {
            lock(&self.shared).outputs.remove(name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reservation_is_released_when_dropped_and_overlap_is_counted() {
        let budget = Budget::new(150);
        let old = budget.reserve(100).expect("100 of 150 fits");
        // A replacement reserved while the old is still held must not fit.
        assert!(budget.reserve(100).is_none(), "overlap counts");
        assert_eq!(budget.used(), 100);
        drop(old);
        assert_eq!(budget.used(), 0, "the old reservation is released");
        let new = budget.reserve(100).expect("the released bytes are free again");
        assert!(budget.reserve(51).is_none(), "the limit still holds");
        assert!(budget.reserve(50).is_some(), "exactly the limit is allowed");
        drop(new);
    }

    #[test]
    fn a_shared_slot_releases_its_bytes_only_when_the_last_copy_goes() {
        let budget = Budget::new(64);
        let slot = Arc::new(budget.reserve(64).unwrap());
        let copy = slot.clone();
        drop(slot);
        assert_eq!(budget.used(), 64, "a copy still holds the texture");
        drop(copy);
        assert_eq!(budget.used(), 0);
    }
}
