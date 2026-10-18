// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use crate::state::Loop;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::utils::{Logical, Physical, Point, Rectangle, Size};
use std::sync::{Arc, mpsc};
use ui::{UiHandle, UiRegistry};

use ui::{UiProgram, UiSnapshot};

pub use ui::UiSpace;

/// [`load`] for a UI the compositor reads back synchronously via
/// `UiRegistry::snapshot`. Identical placement and registration; the only
/// difference is that the off-thread worker publishes a copy of the UI's state
/// each tick, which the compositor reads instead of borrowing the runtime.
pub fn load_snapshot<T: UiSnapshot>(
    state: &mut Loop,
    gles: &mut GlesRenderer,
    t: T,
    rect: Rectangle<i32, Physical>,
    space: UiSpace,
    layer: u64,
) -> UiHandle<T> {
    let gpu = state.inner.environment.GPU.clone();
    let handle = {
        let registry = state
            .inner
            .surface_mut()
            .registry
            .as_mut()
            .unwrap_or_else(|| abort!("registry to be created"));
        registry
            .create_in_space_snapshot(gpu.as_str(), t, gles, rect.loc, rect.size, space, layer)
            .unwrap()
    };
    register(state, handle, space, layer);
    handle
}

/// The draw-order registration both `load` variants share.
///
/// WORLD-space UI surfaces are registered in the renderer-agnostic draw order
/// so they interleave with windows by DrawOrder ("everything" interleaves);
/// screen-space UI is a screen-locked overlay and keeps its own band, so it is
/// not registered at all. The drawable id is derived from the UI `HandleId`
/// and is reversible: `HandleId(uuid.as_u128() as u64)`.
///
/// The layer mask picks the tier: a group frame sits beneath the windows it
/// contains, everything else is CONTENT.
fn register<T: UiProgram>(state: &mut Loop, handle: UiHandle<T>, space: UiSpace, layer: u64) {
    if let UiSpace::World = space {
        let tier = if layer & crate::scene::layer::base::Layer::SCENE_SURFACE_GROUP.bits() != 0 {
            crate::order::track::base::DrawLayer::GROUP
        } else {
            crate::order::track::base::DrawLayer::CONTENT
        };
        state
            .inner
            .register_drawable(uuid::Uuid::from_u128(handle.id.0 as u128), tier);
    }
}

pub fn load<T: UiProgram>(
    state: &mut Loop,
    gles: &mut GlesRenderer,
    t: T,
    rect: Rectangle<i32, Physical>,
    space: UiSpace,
    layer: u64,
) -> UiHandle<T> {
    // Hoist GPU before the surface-registry borrow (surface_mut borrows all of inner).
    let gpu = state.inner.environment.GPU.clone();
    let registry = state
        .inner
        .surface_mut()
        .registry
        .as_mut()
        .unwrap_or_else(|| abort!("registry to be created"));

    let handle = match space {
        UiSpace::World => registry
            .create(gpu.as_str(), t, gles, rect.loc, rect.size, layer)
            .unwrap(),
        UiSpace::Screen => registry
            .create_screen(gpu.as_str(), t, gles, rect.loc, rect.size, layer)
            .unwrap(),
    };

    register(state, handle, space, layer);
    handle
}
