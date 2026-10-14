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
//! - a context holds at most two image textures in its own temporary data: the
//!   current one, and the outgoing one while a fade runs. One output never
//!   reuses another's texture;
//! - every texture is paid for from one aggregate [`Budget`] ([`TEXTURE_BUDGET`]).
//!   A reservation is held by the slot that owns the texture and released when
//!   the slot is replaced, removed or its context goes;
//! - a new current texture is reserved while the old one is still held, so the
//!   overlap is counted. An outgoing texture that a new fade no longer needs is
//!   released before that reservation, so a context never holds a third. A
//!   texture that does not fit releases its old slot and leaves its output on
//!   its fallback, with the reason in the output's status;
//! - a fallback or theme colour is painted beneath every image, so an alpha
//!   image shows what is beneath it, not a hole. A fade paints the outgoing
//!   image over its own fallback, then fades the current fallback and image in.

use egui::{
    Color32, ColorImage, CornerRadius, Id, Rect, TextureHandle, TextureOptions, TextureWrapMode,
    pos2, vec2,
};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};
use toolkit::layer::{LayerProgram, SurfaceInfo};
use wallpaper::decode::Decoded;
use wallpaper::fit::{self, Fit};
use wallpaper::{Config, Rgb, Wallpaper};

/// The most texture memory, in bytes, all outputs' images may hold together,
/// overlap included. A full-size image (3840 × 3840 × 4 bytes) is 56.25 MiB; a
/// complete replacement on four outputs holds four old and four new textures
/// while it swaps, 450 MiB, which this covers. A smaller image costs what it is.
pub const TEXTURE_BUDGET: u64 = 512 * 1024 * 1024;

/// The owner key a context carries in its own data: which wakers entry is it.
const OWNER: &str = "backdrop-owner";
/// How long a changed slideshow image takes to fade in.
pub const FADE: Duration = Duration::from_millis(500);
/// The slots a context keeps its textures in.
const SLOTS: &str = "backdrop-wallpaper-slots";

/// The egui contexts of the live surfaces, each with its owner token, which the
/// controller wakes on a change.
pub type Wakers = Arc<Mutex<Vec<(u64, egui::Context)>>>;

/// A decoded image the controller keeps, with a serial that changes whenever
/// the image does (so a context knows to re-upload).
pub struct Resident {
    pub serial: u64,
    pub image: Decoded,
    /// CPU bytes remain reserved until every view snapshot releases the pixels.
    pub _resident_bytes: Option<Arc<Reservation>>,
}

/// What the program shows: the applied configuration and the images it names.
#[derive(Default)]
pub struct View {
    pub revision: u64,
    pub config: Config,
    pub images: BTreeMap<PathBuf, Arc<Resident>>,
    /// The images the previous slide showed, kept for the fade.
    pub previous: BTreeMap<PathBuf, Arc<Resident>>,
    /// The configuration before the change a fade is for, `None` when no fade runs.
    pub previous_config: Option<Config>,
    /// When the fade started, on the monotonic clock the program reads.
    pub fade_started: Option<Instant>,
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
    /// Set when a reservation releases bytes; taken once by the program, which
    /// then wakes the outputs that may have been starved of them.
    freed: AtomicBool,
}

impl Budget {
    pub fn new(limit: u64) -> Arc<Self> {
        Arc::new(Self {
            limit,
            used: Mutex::new(0),
            freed: AtomicBool::new(false),
        })
    }

    /// Whether bytes were released since the last call (reported once).
    pub fn take_freed(&self) -> bool {
        self.freed.swap(false, Ordering::AcqRel)
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
        {
            let mut used = lock(&self.budget.used);
            *used = used.saturating_sub(self.bytes);
        }
        // Only a real release counts; no callback runs here (drops happen inside
        // egui's data lock), the program wakes outputs after its own pass.
        self.budget.freed.store(true, Ordering::Release);
    }
}

/// An image texture a context owns, with the bytes it is paid for in. The
/// reservation is held for its drop: it releases the bytes when the slot goes.
#[derive(Clone)]
struct Slot {
    serial: u64,
    texture: TextureHandle,
    /// The pixel size and fit it was uploaded for, to place it again.
    original: [u32; 2],
    fit: Fit,
    base: Color32,
    _reservation: Arc<Reservation>,
}

/// A context's textures: the current image and, during a fade, the outgoing one.
#[derive(Clone, Default)]
struct Slots {
    current: Option<Slot>,
    previous: Option<Slot>,
}

impl Slots {
    fn clear(&mut self) {
        *self = Self::default();
    }
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

/// One upload copy, composited over this output's fallback. Making the texture
/// opaque lets two ordinary egui images crossfade without applying alpha twice
/// to transparent pixels. Source decoding still runs exclusively on the worker.
fn upload(image: &Decoded, fallback: Color32) -> ColorImage {
    let [w, h] = image.size;
    let base: egui::Rgba = fallback.into();
    let pixels = image
        .rgba
        .as_raw()
        .chunks_exact(4)
        .map(|p| match p[3] {
            255 => Color32::from_rgb(p[0], p[1], p[2]),
            0 => fallback,
            _ => {
                let source: egui::Rgba =
                    Color32::from_rgba_unmultiplied(p[0], p[1], p[2], p[3]).into();
                Color32::from(source + base * (1.0 - source.a()))
            }
        })
        .collect();
    ColorImage::new([w as usize, h as usize], pixels)
}

/// Tiles repeat across the surface; every other image clamps at its edge.
fn texture_options(fit: Fit) -> TextureOptions {
    let wrap_mode = if fit == Fit::Tile {
        TextureWrapMode::Repeat
    } else {
        TextureWrapMode::ClampToEdge
    };
    TextureOptions {
        wrap_mode,
        ..TextureOptions::LINEAR
    }
}

/// Take the context's textures out of its data. The caller drops them, so their
/// bytes return to the budget outside egui's data lock.
fn take_slots(ctx: &egui::Context) -> Slots {
    ctx.data_mut(|d| d.remove_temp::<Slots>(Id::new(SLOTS)))
        .unwrap_or_default()
}

fn store_slots(ctx: &egui::Context, slots: Slots) {
    ctx.data_mut(|d| d.insert_temp(Id::new(SLOTS), slots));
}

/// Make `resident` the context's current texture. Nothing is uploaded while the
/// current texture already holds it. Otherwise the outgoing texture goes first,
/// then the new size is reserved while the old current one is still held. A
/// fade keeps that old one as the outgoing texture; any other change drops it.
/// On failure release old slots so a full budget can make progress on retry.
fn current_texture(
    ctx: &egui::Context,
    slots: &mut Slots,
    resident: &Resident,
    fit: Fit,
    base: Color32,
    budget: &Arc<Budget>,
    fading: bool,
) -> Result<TextureHandle, String> {
    let maximum = ctx.input(|input| input.max_texture_side);
    if resident
        .image
        .size
        .iter()
        .any(|side| *side as usize > maximum)
    {
        slots.clear();
        return Err(format!(
            "image texture {} × {} exceeds the device's {maximum}-pixel side limit",
            resident.image.size[0], resident.image.size[1]
        ));
    }
    if let Some(slot) = slots
        .current
        .as_mut()
        .filter(|slot| slot.serial == resident.serial)
    {
        if slot.fit != fit || slot.base != base {
            slot.texture
                .set(upload(&resident.image, base), texture_options(fit));
            slot.fit = fit;
            slot.base = base;
        }
        return Ok(slot.texture.clone());
    }
    // The outgoing texture of an earlier fade is obsolete now: release it before
    // the new size is reserved, so the context never holds a third.
    slots.previous = None;
    let bytes = resident.image.bytes();
    let Some(reservation) = budget.reserve(bytes) else {
        let reason = format!(
            "texture budget: {bytes} bytes will not fit beside {} of {} bytes in use",
            budget.used(),
            budget.limit
        );
        slots.clear();
        return Err(reason);
    };
    let texture = ctx.load_texture(
        "backdrop-wallpaper",
        upload(&resident.image, base),
        texture_options(fit),
    );
    let slot = Slot {
        serial: resident.serial,
        texture: texture.clone(),
        original: resident.image.original_size,
        fit,
        base,
        _reservation: Arc::new(reservation),
    };
    // Replacing the current slot drops the old reservation, after the new one.
    let outgoing = slots.current.replace(slot);
    slots.previous = if fading { outgoing } else { None };
    Ok(texture)
}

/// Paint `texture` over `rect` at `alpha`, placed by `fit` for an image of
/// `original` pixels on the output's physical scale.
fn paint_image(
    ui: &egui::Ui,
    texture: &TextureHandle,
    original: [u32; 2],
    fit: Fit,
    rect: Rect,
    alpha: f32,
) {
    let place = fit::placement(
        original,
        [rect.width(), rect.height()],
        ui.ctx().pixels_per_point(),
        fit,
    );
    let dest = Rect::from_min_max(
        rect.min + vec2(place.min[0], place.min[1]),
        rect.min + vec2(place.max[0], place.max[1]),
    );
    let uv = Rect::from_min_max(
        pos2(place.uv.min[0], place.uv.min[1]),
        pos2(place.uv.max[0], place.uv.max[1]),
    );
    let tint = Color32::from_white_alpha((alpha.clamp(0.0, 1.0) * 255.0).round() as u8);
    ui.painter().image(texture.id(), dest, uv, tint);
}

/// Fade the fallback only in the incoming image's bars. Inside its rectangle
/// the opaque incoming texture alone blends with the outgoing image.
fn paint_bars(ui: &egui::Ui, rect: Rect, original: [u32; 2], mode: Fit, fill: Color32) {
    let place = fit::placement(
        original,
        [rect.width(), rect.height()],
        ui.ctx().pixels_per_point(),
        mode,
    );
    let min = rect.min + vec2(place.min[0], place.min[1]);
    let max = rect.min + vec2(place.max[0], place.max[1]);
    for bar in [
        Rect::from_min_max(rect.min, pos2(rect.max.x, min.y)),
        Rect::from_min_max(pos2(rect.min.x, max.y), rect.max),
        Rect::from_min_max(pos2(rect.min.x, min.y), pos2(min.x, max.y)),
        Rect::from_min_max(pos2(max.x, min.y), pos2(rect.max.x, max.y)),
    ] {
        if bar.width() > 0.0 && bar.height() > 0.0 {
            paint(ui, bar, fill);
        }
    }
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
        self.show_pass(ui, surface);
        self.wake_starved();
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
        // The textures go with the surface; dropping them releases their bytes.
        drop(take_slots(ctx));
        if let Some(name) = &surface.output {
            lock(&self.shared).outputs.remove(name);
        }
        self.wake_starved();
    }
}

impl Backdrop {
    /// Once bytes were released, wake every live context once, so an output
    /// starved of texture memory draws again. Nothing wakes when nothing was
    /// freed, so an idle or fitting set never loops. The handles are cloned out
    /// first: no repaint request runs under the wakers lock.
    fn wake_starved(&self) {
        if !self.budget.take_freed() {
            return;
        }
        let contexts: Vec<egui::Context> = lock(&self.wakers)
            .iter()
            .map(|(_, context)| context.clone())
            .collect();
        for context in contexts {
            context.request_repaint();
        }
    }

    fn show_pass(&mut self, ui: &mut egui::Ui, surface: &SurfaceInfo) {
        let view = lock(&self.shared).view.clone();
        self.revision = view.revision;
        let ctx = ui.ctx().clone();
        let rect = ui.max_rect();
        let theme = ui.visuals().panel_fill;
        let name = surface.output.as_deref();
        let config = view.config.for_output(name);
        let outgoing = view
            .previous_config
            .as_ref()
            .map(|old| old.for_output(name));
        let mut slots = take_slots(&ctx);
        // A fade runs only where this output's image changes path, and only
        // until its time is up; once it has run out the outgoing texture goes.
        let fade = match (view.fade_started, config, outgoing) {
            (
                Some(start),
                Wallpaper::Image { path, .. },
                Some(Wallpaper::Image { path: old, .. }),
            ) if old != path => {
                let elapsed = Instant::now().saturating_duration_since(start);
                Some((elapsed.as_secs_f32() / FADE.as_secs_f32()).min(1.0))
            }
            _ => None,
        }
        .filter(|progress| *progress < 1.0);
        if fade.is_none() {
            slots.previous = None;
        }
        let (shows, error) = match config {
            Wallpaper::Theme => {
                slots.clear();
                paint(ui, rect, theme);
                ("theme", None)
            }
            Wallpaper::Colour(rgb) => {
                slots.clear();
                paint(ui, rect, colour(*rgb));
                ("colour", None)
            }
            Wallpaper::Slideshow { .. } => {
                // The controller publishes the resolved image, never a slideshow.
                slots.clear();
                paint(ui, rect, theme);
                ("fallback", Some("slideshow was not resolved".to_owned()))
            }
            Wallpaper::Image {
                path,
                fallback,
                fit,
            } => {
                // Beneath every image: the fallback, else the theme.
                let base = fallback.map_or(theme, colour);
                let mut error = None;
                let current = match view.images.get(path) {
                    None => {
                        slots.clear();
                        None
                    }
                    Some(resident) => match current_texture(
                        &ctx,
                        &mut slots,
                        resident,
                        *fit,
                        base,
                        &self.budget,
                        fade.is_some(),
                    ) {
                        Ok(texture) => Some((texture, resident.image.original_size, *fit)),
                        Err(reason) => {
                            error = Some(reason);
                            None
                        }
                    },
                };
                match (current, fade) {
                    (None, _) => {
                        paint(ui, rect, base);
                        ("fallback", error)
                    }
                    (Some((texture, original, fit)), Some(progress)) => {
                        // The outgoing image, full over its own fallback; then the
                        // current fallback and image fade in over it.
                        if let Some(Wallpaper::Image {
                            path: old_path,
                            fallback: old_base,
                            ..
                        }) = outgoing
                        {
                            paint(ui, rect, old_base.map_or(theme, colour));
                            let held = slots.previous.as_ref().filter(|slot| {
                                view.previous
                                    .get(old_path)
                                    .is_some_and(|old| old.serial == slot.serial)
                            });
                            if let Some(slot) = held {
                                paint_image(ui, &slot.texture, slot.original, slot.fit, rect, 1.0);
                            }
                        }
                        paint_bars(ui, rect, original, fit, base.gamma_multiply(progress));
                        paint_image(ui, &texture, original, fit, rect, progress);
                        // Only a fade that has not run out asks for another pass.
                        ui.ctx().request_repaint();
                        ("image", error)
                    }
                    (Some((texture, original, fit)), None) => {
                        paint(ui, rect, base);
                        paint_image(ui, &texture, original, fit, rect, 1.0);
                        ("image", error)
                    }
                }
            }
        };
        self.shows = shows;
        self.error = error;
        store_slots(&ctx, slots);
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
        let new = budget
            .reserve(100)
            .expect("the released bytes are free again");
        assert!(budget.reserve(51).is_none(), "the limit still holds");
        assert!(budget.reserve(50).is_some(), "exactly the limit is allowed");
        drop(new);
    }

    #[test]
    fn transparent_pixels_are_composited_once_before_crossfading() {
        let image = Decoded {
            size: [3, 1],
            original_size: [3, 1],
            rgba: image::RgbaImage::from_raw(
                3,
                1,
                vec![255, 0, 0, 0, 255, 0, 0, 128, 255, 0, 0, 255],
            )
            .unwrap(),
        };
        let base = Color32::from_rgb(0, 0, 255);
        let pixels = upload(&image, base).pixels;
        assert!(pixels.iter().all(|p| p.a() == 255));
        assert_eq!(pixels[0], base);
        assert!(pixels[1].r() > 0 && pixels[1].b() > 0);
        assert_eq!(pixels[2], Color32::from_rgb(255, 0, 0));
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

    #[test]
    fn a_release_is_reported_once_and_only_when_bytes_are_freed() {
        // A 512 MiB-shaped budget at 100 bytes: a starved output needs a release.
        let budget = Budget::new(100);
        let held = budget.reserve(60).expect("60 of 100 fits");
        assert!(budget.reserve(60).is_none(), "a second 60 is starved");
        assert!(
            !budget.take_freed(),
            "nothing released, so nothing to wake for"
        );
        drop(held);
        assert!(budget.take_freed(), "the release is reported");
        assert!(!budget.take_freed(), "and reported once");
        assert!(budget.reserve(60).is_some(), "the starved output now fits");
    }

    const SIDE: u32 = 4;

    fn wallpaper_image(path: &str) -> Wallpaper {
        Wallpaper::Image {
            path: PathBuf::from(path),
            fallback: None,
            fit: Fit::Fill,
        }
    }

    /// A square of `SIDE` pixels: 4 × 4 × 4 bytes.
    fn resident(serial: u64) -> Arc<Resident> {
        let rgba = image::RgbaImage::from_pixel(SIDE, SIDE, image::Rgba([200, 30, 30, 255]));
        Arc::new(Resident {
            serial,
            _resident_bytes: None,
            image: Decoded {
                size: [SIDE, SIDE],
                original_size: [SIDE, SIDE],
                rgba,
            },
        })
    }

    /// A view showing `path`, its image at `serial`.
    fn view(revision: u64, path: &str, serial: u64) -> View {
        View {
            revision,
            config: Config {
                default: wallpaper_image(path),
                outputs: BTreeMap::new(),
            },
            images: BTreeMap::from([(PathBuf::from(path), resident(serial))]),
            ..Default::default()
        }
    }

    /// A view fading `path` in over `old_path`, which fell out at `started`.
    fn fade(
        revision: u64,
        path: &str,
        serial: u64,
        old_path: &str,
        old_serial: u64,
        started: Instant,
    ) -> View {
        View {
            previous: BTreeMap::from([(PathBuf::from(old_path), resident(old_serial))]),
            previous_config: Some(Config {
                default: wallpaper_image(old_path),
                outputs: BTreeMap::new(),
            }),
            fade_started: Some(started),
            ..view(revision, path, serial)
        }
    }

    fn surface() -> SurfaceInfo {
        SurfaceInfo {
            output: Some("DP-1".to_owned()),
            width: 64,
            height: 64,
            scale: 1.0,
        }
    }

    fn fixture() -> (egui::Context, Backdrop) {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let program = Backdrop::new(toolkit::Theme::embedded(), shared, Wakers::default());
        (egui::Context::default(), program)
    }

    fn publish(program: &Backdrop, view: View) {
        lock(&program.shared).view = Arc::new(view);
    }

    /// Headless tests intentionally discard GPU deltas after inspecting them.
    struct Headless(egui::FullOutput);

    impl std::ops::Deref for Headless {
        type Target = egui::FullOutput;
        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }

    impl Drop for Headless {
        fn drop(&mut self) {
            self.0.textures_delta.clear();
        }
    }

    /// One headless pass over the output.
    fn pass(ctx: &egui::Context, program: &mut Backdrop) -> Headless {
        let surface = surface();
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(64.0, 64.0))),
            ..Default::default()
        };
        Headless(ctx.run_ui(input, |ui| program.show_pass(ui, &surface)))
    }

    /// Uploads in a pass, not counting egui's own font atlas.
    fn uploads(out: &egui::FullOutput) -> usize {
        out.textures_delta
            .set
            .iter()
            .filter(|(id, _)| **id != egui::TextureId::default())
            .count()
    }

    #[test]
    fn a_settled_image_is_uploaded_once_and_then_never_again() {
        let (ctx, mut program) = fixture();
        publish(&program, view(1, "a.png", 1));
        let first = pass(&ctx, &mut program);
        assert_eq!(uploads(&first), 1, "the first pass uploads the image");
        assert_eq!(program.budget.used(), 64);
        let steady = pass(&ctx, &mut program);
        assert_eq!(uploads(&steady), 0, "a steady pass uploads nothing");
        assert_eq!(program.budget.used(), 64);
    }

    #[test]
    fn a_fade_holds_two_images_and_its_end_frees_the_outgoing_one() {
        let (ctx, mut program) = fixture();
        publish(&program, view(1, "a.png", 1));
        pass(&ctx, &mut program);
        // B fades in over A, which becomes the outgoing image.
        publish(&program, fade(2, "b.png", 2, "a.png", 1, Instant::now()));
        let mid = pass(&ctx, &mut program);
        assert_eq!(uploads(&mid), 1, "the new image uploads once");
        assert_eq!(program.budget.used(), 128, "two images while the fade runs");
        // The time is up: the outgoing image goes, even without controller cleanup.
        publish(
            &program,
            fade(2, "b.png", 2, "a.png", 1, Instant::now() - FADE),
        );
        let done = pass(&ctx, &mut program);
        assert_eq!(uploads(&done), 0, "a finished fade uploads nothing");
        assert_eq!(program.budget.used(), 64, "the completed fade frees A");
    }

    #[test]
    fn an_interrupted_fade_still_holds_only_two_images() {
        let (ctx, mut program) = fixture();
        publish(&program, view(1, "a.png", 1));
        pass(&ctx, &mut program);
        publish(&program, fade(2, "b.png", 2, "a.png", 1, Instant::now()));
        pass(&ctx, &mut program);
        // C arrives before B's fade ends: A is obsolete, B becomes the outgoing image.
        publish(&program, fade(3, "c.png", 3, "b.png", 2, Instant::now()));
        pass(&ctx, &mut program);
        assert_eq!(program.budget.used(), 128, "never a third image");
    }

    #[test]
    fn a_budget_refusal_releases_old_slots_so_a_retry_can_make_progress() {
        let (ctx, mut program) = fixture();
        program.budget = Budget::new(100);
        publish(&program, view(1, "a.png", 1));
        pass(&ctx, &mut program);
        // A second 64-byte image cannot be reserved beside the first.
        publish(&program, view(2, "b.png", 2));
        let out = pass(&ctx, &mut program);
        assert_eq!(program.budget.used(), 0, "the old slot is released");
        assert!(program.error.is_some(), "the reason is recorded");
        assert_eq!(program.shows, "fallback");
        assert_eq!(uploads(&out), 0, "a refused texture uploads nothing");
        drop(out);
        let retry = pass(&ctx, &mut program);
        assert_eq!(uploads(&retry), 1);
        assert_eq!(program.budget.used(), 64);
        assert!(program.error.is_none());
    }

    #[test]
    fn a_texture_over_the_device_limit_uses_fallback_without_uploading() {
        let (ctx, mut program) = fixture();
        let mut large = view(1, "a.png", 1);
        large.images.insert(
            "a.png".into(),
            Arc::new(Resident {
                serial: 1,
                image: Decoded {
                    size: [3840, 1],
                    original_size: [3840, 1],
                    rgba: image::RgbaImage::new(3840, 1),
                },
                _resident_bytes: None,
            }),
        );
        publish(&program, large);
        let frame = Headless(ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(64.0, 64.0))),
                max_texture_side: Some(2048),
                ..Default::default()
            },
            |ui| program.show_pass(ui, &surface()),
        ));
        assert_eq!(program.shows, "fallback");
        assert!(program.error.as_deref().unwrap().contains("2048"));
        assert_eq!(program.budget.used(), 0);
        assert_eq!(uploads(&frame), 0);
    }

    #[test]
    fn a_solid_colour_and_removal_free_the_textures() {
        let (ctx, mut program) = fixture();
        publish(&program, view(1, "a.png", 1));
        pass(&ctx, &mut program);
        assert_eq!(program.budget.used(), 64);
        let mut solid = view(2, "a.png", 1);
        solid.config = Config {
            default: Wallpaper::Colour(Rgb([0, 0, 0])),
            outputs: BTreeMap::new(),
        };
        publish(&program, solid);
        pass(&ctx, &mut program);
        assert_eq!(program.budget.used(), 0, "a solid colour holds no texture");
        publish(&program, view(3, "a.png", 1));
        pass(&ctx, &mut program);
        assert_eq!(program.budget.used(), 64);
        program.removed(&ctx, &surface());
        assert_eq!(program.budget.used(), 0, "removal frees the texture");
    }
}
