// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use crate::window::draw::element::element::Element as WindowElement;
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::element::{
    Element as SmithayElement, Id, RenderElement, UnderlyingStorage,
};
use smithay::backend::renderer::{ImportAll, ImportMem, Texture};

use dispatcher::frame::frame::SceneDispatch;

smithay::render_elements! {
    // SceneDispatch: the window element's clipped content needs it.
    pub Element<R> where R: ImportAll + ImportMem + SceneDispatch;
    Window = WindowElement<R>,
    SolidBox = SolidColorRenderElement,
}
