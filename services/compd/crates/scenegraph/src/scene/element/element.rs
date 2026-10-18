// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use dispatcher::frame::frame::SceneDispatch;
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::{ImportAll, ImportMem};
use ui::UiRenderElement;
use world::seat::pointer::element::element::PointerRenderElement;

pub use crate::scene::preimported::preimported::PreImported;

smithay::render_elements! {
    pub SceneElement<R> where R: ImportAll + ImportMem + SceneDispatch;
    Canvas = world::canvas::draw::element::element::Element<R>,
    Layershell = WaylandSurfaceRenderElement<R>,
    Surface = UiRenderElement,
    /// World-space UI surface clipped to a viewport pane (GLES path).
    SurfaceCropped = smithay::backend::renderer::element::utils::CropRenderElement<UiRenderElement>,
    /// World-space UI surface (dmabuf-imported) clipped to a pane (native path).
    TextureCropped = smithay::backend::renderer::element::utils::CropRenderElement<PreImported<R>>,
    Pointer = PointerRenderElement<R>,
    Texture = PreImported<R>,
    Sentinel = SolidColorRenderElement,
    /// A CPU-rasterised image (the region-select hint).
    Memory = smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement<R>,
    /// A live window thumbnail (a hover tip's preview), under its own ids.
    Thumbnail = world::window::draw::thumbnail::Thumb<R>,
}

impl<R> SceneElement<R>
where
    R: ImportAll + ImportMem + SceneDispatch,
{
    /// A cursor representation: the pointer sprite and drag icon (`Pointer`),
    /// or the canvas cursor, the translucent box drawn in the world band at
    /// the same point (`world::canvas::cursor`, the only `SolidBox` a canvas
    /// element is). What a screencopy with `overlay_cursor = 0` leaves out; the
    /// one place that classifies it, so every backend leaves out the same set.
    pub fn is_cursor(&self) -> bool {
        matches!(
            self,
            SceneElement::Pointer(_)
                | SceneElement::Canvas(world::canvas::draw::element::element::Element::SolidBox(_))
        )
    }
}
