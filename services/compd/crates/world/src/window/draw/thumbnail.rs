// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Live window thumbnails: a window's own surfaces, drawn scaled into a
//! rectangle of the screen (a hover tip's preview box, as Plasma's task
//! tooltips show one).
//!
//! The scene host asks for a thumbnail by [`set`]ting a request while its
//! tip card shows (and clears it when the card hides); the scene builder
//! reads [`requests`] each frame, resolves the window by its `{id,
//! generation}` fence and draws [`elements`] above the screen UI. The
//! window is rendered exactly as a capture renders it, then rescaled to fit
//! the box, centred, and cropped to it. Every element gets its own id
//! ([`Rekeyed`]): the same surface may also be drawn in place this frame,
//! and two elements with one id would break damage tracking.

use std::cell::RefCell;
use std::collections::HashMap;

use smithay::backend::renderer::element::utils::{
    CropRenderElement, Relocate, RelocateRenderElement, RescaleRenderElement,
};
use smithay::backend::renderer::element::{
    Element as ElementSmithay, Id, RenderElement, UnderlyingStorage,
};
use smithay::backend::renderer::utils::CommitCounter;
use smithay::backend::renderer::{ImportAll, ImportMem, Renderer, Texture};
use smithay::desktop::Window;
use smithay::utils::user_data::UserDataMap;
use smithay::utils::{Buffer, Physical, Point, Rectangle, Scale, Size};

use crate::window::draw::element::element::Element as WindowElement;

/// One thumbnail to draw: window `{id, generation}` fitted into `rect`
/// (output physical px) on output `output` (its key).
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub id: u64,
    pub generation: u64,
    pub rect: Rectangle<i32, Physical>,
    pub output: String,
}

thread_local! {
    /// The thumbnails asked for now (the scene host owns the list).
    static REQUESTS: RefCell<Vec<Request>> = const { RefCell::new(Vec::new()) };
    /// Stable ids per (window id, inner element id), kept while drawn.
    static IDS: RefCell<HashMap<(u64, Id), Id>> = RefCell::new(HashMap::new());
    /// Who sends each previewed window's frame callbacks.
    static OWNERS: RefCell<Owners<Window>> = RefCell::new(Owners::default());
    /// Every window drawn as a thumbnail since tips began asking: their
    /// captured-pass rasters are released when the asking stops.
    static PREVIEWED: RefCell<Vec<Window>> = const { RefCell::new(Vec::new()) };
}

/// Who sends a previewed window's frame callbacks, from what each output's
/// last scene drew. An output sends them for the windows it draws in place;
/// a window it draws only as a thumbnail is not its to pace, and one drawn
/// in place nowhere is the thumbnail pacer's. Generic over the window type
/// so the rules are testable without a compositor.
pub struct Owners<T> {
    drawn: HashMap<String, Drawn<T>>,
}

struct Drawn<T> {
    in_place: Vec<T>,
    preview_only: Vec<T>,
}

impl<T> Default for Owners<T> {
    fn default() -> Self {
        Owners {
            drawn: HashMap::new(),
        }
    }
}

impl<T: PartialEq + Clone> Owners<T> {
    /// Output `output` drew `in_place` (its presented set before
    /// thumbnails) and `preview_only` (thumbnailed there, not in place).
    pub fn note(&mut self, output: &str, in_place: &[T], preview_only: Vec<T>) {
        self.drawn.insert(
            output.to_owned(),
            Drawn {
                in_place: in_place.to_vec(),
                preview_only,
            },
        );
    }

    pub fn clear(&mut self) {
        self.drawn.clear();
    }

    /// Forget outputs that are gone (`live` are the current keys).
    pub fn retain_outputs(&mut self, live: &[String]) {
        self.drawn.retain(|key, _| live.contains(key));
    }

    /// Whether `output` drew `window` only as a thumbnail.
    pub fn preview_only_on(&self, output: &str, window: &T) -> bool {
        self.drawn
            .get(output)
            .is_some_and(|d| d.preview_only.contains(window))
    }

    /// The windows the pacer owns, each with an output a tip shows it on:
    /// previewed somewhere and drawn in place nowhere.
    pub fn paced(&self) -> Vec<(T, String)> {
        let mut out: Vec<(T, String)> = Vec::new();
        for (key, d) in &self.drawn {
            for window in &d.preview_only {
                let in_place = self.drawn.values().any(|o| o.in_place.contains(window));
                if !in_place && !out.iter().any(|(w, _)| w == window) {
                    out.push((window.clone(), key.clone()));
                }
            }
        }
        out
    }
}

/// Whether some tip asks for a thumbnail now (when not, nothing here does
/// any work).
pub fn asked() -> bool {
    REQUESTS.with_borrow(|r| !r.is_empty())
}

/// Record what output `output`'s scene just drew ([`Owners::note`]), and
/// `previewed`, every window thumbnailed there. Kept only while some tip
/// asks for a thumbnail, so no window outlives its preview here. Once the
/// asking has stopped, the captured-pass rasters of the windows previewed
/// meanwhile are released.
pub fn note_drawn(
    output: &str,
    in_place: &[Window],
    preview_only: Vec<Window>,
    previewed: &[Window],
) {
    if !asked() {
        OWNERS.with_borrow_mut(Owners::clear);
        for window in PREVIEWED.take() {
            decor::render::release_captured(&window);
        }
        return;
    }
    OWNERS.with_borrow_mut(|owners| owners.note(output, in_place, preview_only));
    PREVIEWED.with_borrow_mut(|all| {
        for window in previewed {
            if !all.contains(window) {
                all.push(window.clone());
            }
        }
    });
}

/// [`Owners::retain_outputs`].
pub fn retain_outputs(live: &[String]) {
    OWNERS.with_borrow_mut(|owners| owners.retain_outputs(live));
}

/// [`Owners::preview_only_on`], while some tip asks.
pub fn preview_only_on(output: &str, window: &Window) -> bool {
    asked() && OWNERS.with_borrow(|owners| owners.preview_only_on(output, window))
}

/// [`Owners::paced`], while some tip asks.
pub fn paced() -> Vec<(Window, String)> {
    if !asked() {
        return Vec::new();
    }
    OWNERS.with_borrow(Owners::paced)
}

/// Replace the requested thumbnails (empty: none).
pub fn set(requests: Vec<Request>) {
    REQUESTS.with_borrow_mut(|current| *current = requests);
}

/// The thumbnails asked for now.
pub fn requests() -> Vec<Request> {
    REQUESTS.with_borrow(Clone::clone)
}

/// The fitted box: `content` scaled to fit inside `rect`, centred. `None`
/// for an empty box or empty content.
pub fn fit(
    content: Size<i32, Physical>,
    rect: Rectangle<i32, Physical>,
) -> Option<(f64, Point<i32, Physical>)> {
    if content.w <= 0 || content.h <= 0 || rect.size.w <= 0 || rect.size.h <= 0 {
        return None;
    }
    let s = (rect.size.w as f64 / content.w as f64).min(rect.size.h as f64 / content.h as f64);
    let w = (content.w as f64 * s).round() as i32;
    let h = (content.h as f64 * s).round() as i32;
    Some((
        s,
        Point::from((
            rect.loc.x + (rect.size.w - w) / 2,
            rect.loc.y + (rect.size.h - h) / 2,
        )),
    ))
}

/// Where a capture goes in `rect` when its `frame` (content and chrome, in
/// capture px) is what fills the box, as the box was sized from it: the
/// scale, where the capture's origin lands, and the fitted frame to crop to.
/// The shadow and any open popup fall outside the crop, so neither shrinks
/// the preview.
pub fn place(
    frame: Rectangle<i32, Physical>,
    rect: Rectangle<i32, Physical>,
) -> Option<(f64, Point<i32, Physical>, Rectangle<i32, Physical>)> {
    let (s, frame_at) = fit(frame.size, rect)?;
    // Each corner rounded once, as RescaleRenderElement (and the rounded
    // clip) place the frame: the crop is exactly the frame drawn.
    let corner = |v: i32| (v as f64 * s).round() as i32;
    let at = Point::from((
        frame_at.x - corner(frame.loc.x),
        frame_at.y - corner(frame.loc.y),
    ));
    let x0 = at.x + corner(frame.loc.x);
    let y0 = at.y + corner(frame.loc.y);
    let x1 = at.x + corner(frame.loc.x + frame.size.w);
    let y1 = at.y + corner(frame.loc.y + frame.size.h);
    let shown = Rectangle::new((x0, y0).into(), (x1 - x0, y1 - y0).into());
    Some((s, at, shown.intersection(rect)?))
}

/// The element a thumbnail draws: a window element rescaled, moved into its
/// box, cropped to it, under its own id.
pub type Thumb<R> =
    Rekeyed<CropRenderElement<RelocateRenderElement<RescaleRenderElement<WindowElement<R>>>>>;

/// `window` drawn into `rect` at output `scale`; the elements front-most
/// first. Empty when the window has nothing to draw.
pub fn elements<R>(
    renderer: &mut R,
    window: &Window,
    window_id: u64,
    rect: Rectangle<i32, Physical>,
    scale: f64,
) -> Vec<Thumb<R>>
where
    R: Renderer + ImportAll + ImportMem + dispatcher::frame::frame::SceneDispatch,
    R::TextureId: Texture + Clone + Send + 'static,
{
    let Ok((inner, _, frame)) =
        crate::window::draw::frame::scene::capture_framed(renderer, window, scale)
    else {
        return Vec::new();
    };
    let Some((s, at, rect)) = place(frame, rect) else {
        return Vec::new();
    };
    inner
        .into_iter()
        .filter_map(|element| {
            let key = (window_id, element.id().clone());
            // A window cut to its rounded frame carries the cut as output-space
            // shader uniforms: move them with the thumbnail too.
            let element = match element {
                WindowElement::WindowFitClipped(clipped) => {
                    WindowElement::WindowFitClipped(clipped.rescaled(s, at))
                }
                other => other,
            };
            let r =
                RescaleRenderElement::from_element(element, Point::from((0, 0)), Scale::from(s));
            let l = RelocateRenderElement::from_element(r, at, Relocate::Relative);
            let c = CropRenderElement::from_element(l, Scale::from(scale), rect)?;
            let id = IDS.with_borrow_mut(|ids| ids.entry(key).or_insert_with(Id::new).clone());
            Some(Rekeyed { inner: c, id })
        })
        .collect()
}

/// Forget the ids of thumbnails no longer drawn (called once per frame
/// with the window ids drawn), so the cache stays as small as what shows.
pub fn retain_ids(drawn: &[u64]) {
    IDS.with_borrow_mut(|ids| ids.retain(|(window, _), _| drawn.contains(window)));
}

/// An element under a different id; everything else delegated. A thumbnail
/// is always composited: it never offers its buffer for direct scanout.
pub struct Rekeyed<E> {
    pub inner: E,
    pub id: Id,
}

impl<E: ElementSmithay> ElementSmithay for Rekeyed<E> {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.inner.current_commit()
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.inner.src()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.inner.geometry(scale)
    }

    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        self.inner.location(scale)
    }

    fn transform(&self) -> smithay::utils::Transform {
        self.inner.transform()
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> smithay::backend::renderer::utils::DamageSet<i32, Physical> {
        self.inner.damage_since(scale, commit)
    }

    fn opaque_regions(
        &self,
        scale: Scale<f64>,
    ) -> smithay::backend::renderer::utils::OpaqueRegions<i32, Physical> {
        self.inner.opaque_regions(scale)
    }

    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }

    fn kind(&self) -> smithay::backend::renderer::element::Kind {
        self.inner.kind()
    }

    fn is_framebuffer_effect(&self) -> bool {
        self.inner.is_framebuffer_effect()
    }
}

impl<R, E> RenderElement<R> for Rekeyed<E>
where
    R: Renderer,
    E: RenderElement<R>,
{
    fn draw(
        &self,
        frame: &mut R::Frame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), R::Error> {
        self.inner
            .draw(frame, src, dst, damage, opaque_regions, cache)
    }

    fn underlying_storage(&self, _renderer: &mut R) -> Option<UnderlyingStorage> {
        None
    }

    fn capture_framebuffer(
        &self,
        frame: &mut R::Frame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        cache: &UserDataMap,
    ) -> Result<(), R::Error> {
        self.inner.capture_framebuffer(frame, src, dst, cache)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_fits_its_box_centred() {
        // A 1600x900 window into a 240x150 box: width-bound, centred vertically.
        let rect = Rectangle::new(Point::from((100, 200)), Size::from((240, 150)));
        let (s, at) = fit(Size::from((1600, 900)), rect).unwrap();
        assert!((s - 0.15).abs() < 1e-9);
        assert_eq!(at, Point::from((100, 200 + (150 - 135) / 2)));
        // A tall window: height-bound, centred horizontally.
        let (s, at) = fit(Size::from((400, 1000)), rect).unwrap();
        assert!((s - 0.15).abs() < 1e-9);
        assert_eq!(at, Point::from((100 + (240 - 60) / 2, 200)));
        assert!(fit(Size::from((0, 10)), rect).is_none());
        assert!(
            fit(
                Size::from((10, 10)),
                Rectangle::new(Point::from((0, 0)), Size::from((0, 5)))
            )
            .is_none()
        );
    }

    /// The frame fills the box; the capture's shadow margin lands outside it
    /// and is cropped.
    #[test]
    fn the_frame_fills_the_box_and_the_shadow_is_cropped() {
        // A 1000x700 frame 40px into its capture (shadow all round).
        let frame = Rectangle::new(Point::from((40, 40)), Size::from((1000, 700)));
        let rect = Rectangle::new(Point::from((500, 300)), Size::from((200, 140)));
        let (s, at, crop) = place(frame, rect).unwrap();
        assert!((s - 0.2).abs() < 1e-9);
        // The frame's corner is the box's corner; the capture origin is 8px
        // (40 * 0.2) up and left of it.
        assert_eq!(at, Point::from((492, 292)));
        assert_eq!(crop, rect);
        // A frame of another aspect is centred, and the crop is the frame.
        let wide = Rectangle::new(Point::from((0, 0)), Size::from((1000, 500)));
        let (_, at, crop) = place(wide, rect).unwrap();
        assert_eq!(
            crop,
            Rectangle::new(Point::from((500, 320)), Size::from((200, 100)))
        );
        assert_eq!(at, crop.loc);
    }

    /// The crop is the frame as the rescale draws it, each corner rounded
    /// once: a 792x634 frame at (22,16) into 187x150 draws 149 rows, and a
    /// crop of round(634 * s) = 150 would admit a row of shadow.
    #[test]
    fn the_crop_follows_the_rounded_corners() {
        let frame = Rectangle::new(Point::from((22, 16)), Size::from((792, 634)));
        let rect = Rectangle::new(Point::from((0, 0)), Size::from((187, 150)));
        let (s, at, crop) = place(frame, rect).unwrap();
        let corner = |v: i32| (v as f64 * s).round() as i32;
        assert_eq!(crop.loc.y, at.y + corner(16));
        assert_eq!(crop.loc.y + crop.size.h, at.y + corner(16 + 634));
        assert_eq!(crop.size, Size::from((187, 149)));
    }

    /// One owner per window: in place wins; preview-only on every output
    /// it shows is the pacer's; a gone output stops counting.
    #[test]
    fn each_previewed_window_has_one_callback_owner() {
        let mut owners: Owners<u32> = Owners::default();
        // Window 1 in place on B and previewed on A; window 2 only previewed.
        owners.note("A", &[], vec![1, 2]);
        owners.note("B", &[1], Vec::new());
        assert!(owners.preview_only_on("A", &1));
        assert!(!owners.preview_only_on("B", &1));
        assert_eq!(owners.paced(), vec![(2, "A".to_owned())]);
        // B unplugged: window 1 is drawn in place nowhere, so the pacer has it.
        owners.retain_outputs(&["A".to_owned()]);
        let mut paced = owners.paced();
        paced.sort();
        assert_eq!(paced, vec![(1, "A".to_owned()), (2, "A".to_owned())]);
        owners.retain_outputs(&[]);
        assert!(owners.paced().is_empty());
        owners.note("A", &[], vec![3]);
        owners.clear();
        assert!(owners.paced().is_empty());
    }

    #[test]
    fn requests_replace_and_clear() {
        let r = Request {
            id: 7,
            generation: 2,
            rect: Rectangle::new(Point::from((1, 2)), Size::from((3, 4))),
            output: "o".into(),
        };
        set(vec![r.clone()]);
        assert_eq!(requests(), vec![r]);
        set(Vec::new());
        assert!(requests().is_empty());
    }
}
