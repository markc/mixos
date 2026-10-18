// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The presenter's storage: its own UI registry, the message on screen, and
//! one pill per output showing it.
//!
//! Owned by the kernel-hosted `NotifySystem`, by no world, so a world switch
//! neither strands nor rebuilds the pills; nothing hit-tests this registry, so
//! they are click-through by construction. Per OUTPUT because each monitor's
//! pass sizes and places its own pill — one surface moved between passes would
//! be re-laid-out every frame and could only ever be right for one of them.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use crate::notify::view::NotifyUi;
use crate::scene::layer::base::Layer;
use crate::surface::system::base::UI_CONTEXT;
use slots::storage::slot::base::Storage;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::utils::{Physical, Point, Size};
use ui::{UiHandle, UiRegistry};

/// Pill height. Width is estimated, not measured: UI lays out INSIDE the
/// surface, so it only has to avoid clipping — overshoot is invisible behind
/// the shrink-centred pill.
pub const HEIGHT: i32 = 44;
const CHAR_W: i32 = 8;
const PAD_W: i32 = 48;
const MIN_W: i32 = 240;

pub struct Pill {
    pub handle: UiHandle<NotifyUi>,
    pub width: i32,
}

#[derive(Default)]
pub struct Presenter {
    pub registry: Option<UiRegistry>,
    /// The message on screen and when it was raised. One at a time.
    pub current: Option<(String, Instant)>,
    /// Its pill on each output that has drawn since it was raised, by output key.
    pub pills: HashMap<Arc<str>, Pill>,
}

impl Presenter {
    /// Take the message down: every output's pill, and the message itself.
    pub fn retire(&mut self) {
        if let Some(reg) = self.registry.as_mut() {
            for (_, pill) in self.pills.drain() {
                reg.destroy(pill.handle);
            }
        }
        self.current = None;
    }
}

/// This output's pill for the current message, created on its first pass —
/// bound to the output so its element carries the key the passes gate on.
pub fn pill<'p>(
    p: &'p mut Presenter,
    message: &str,
    output: &Arc<str>,
    size: Size<i32, Physical>,
    renderer: &mut GlesRenderer,
    node: &str,
) -> Option<&'p Pill> {
    let Presenter {
        registry, pills, ..
    } = p;
    if !pills.contains_key(output) {
        let width =
            ((message.len() as i32 * CHAR_W) + PAD_W).clamp(MIN_W, (size.w - 40).max(MIN_W));
        // Fully below the edge, so frame one starts off-screen not mid-slide.
        let start = Point::from(((size.w - width) / 2, size.h));
        let reg = registry.as_mut()?;
        let handle = reg
            .create_screen(
                node,
                NotifyUi::new(message.to_owned()),
                renderer,
                start,
                Size::from((width, HEIGHT)),
                Layer::GLOBAL_SCREEN.bits(),
            )
            .ok()?;
        reg.set_output_affinity_by_id(handle.id, Some(output.to_string()));
        pills.insert(Arc::clone(output), Pill { handle, width });
    }
    pills.get(output)
}

/// The presenter's registry, built on first use from the shared GL context,
/// exactly as every world's registry is. `false` while the context has not
/// landed yet.
pub fn ensure_registry(p: &mut Presenter, kernel: &Storage) -> bool {
    if p.registry.is_some() {
        return true;
    }
    let Some(wgpu) = kernel.try_get(&UI_CONTEXT).and_then(Clone::clone) else {
        return false;
    };
    p.registry = Some(UiRegistry::new(wgpu));
    true
}
