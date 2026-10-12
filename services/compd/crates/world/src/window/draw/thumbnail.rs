// SPDX-License-Identifier: MIT OR Apache-2.0
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
    let Ok((inner, size)) = crate::window::draw::frame::scene::capture(renderer, window, scale)
    else {
        return Vec::new();
    };
    let Some((s, at)) = fit(size, rect) else {
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
