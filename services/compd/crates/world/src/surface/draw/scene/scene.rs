// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use crate::state::Loop;
use crate::state::CoordinateTrait;
use dispatcher::state::RedrawReason;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::utils::{Physical, Point, Size};
use ui::{UiRenderElement, UiSpace};

/// Returns `(world, screen, dim)` UI render-element vecs:
/// - `world`  — `UiSpace::World`, SCENE layer (drawn below windows);
/// - `screen` — `UiSpace::Screen`, SCENE layer (drawn above windows);
/// - `dim`    — the `CAPTURE_DIM` layer (the capture backdrop), drawn in its
///   own pass between windows and the world/background layers.
pub fn scene(
    _loop: &mut Loop,
    renderer: &mut GlesRenderer,
    size: Size<i32, Physical>,
) -> (
    Vec<UiRenderElement>,
    Vec<UiRenderElement>,
    Vec<UiRenderElement>,
) {
    // Right now explicit. Later on render scene when it accepts Gles only.
    // let (ui_elements) = scene_gles::scene_gles(state, gles_renderer);

    let (mut ui_elements, mut ui_elements_screen, mut ui_elements_dim): (
        Vec<UiRenderElement>,
        Vec<UiRenderElement>,
        Vec<UiRenderElement>,
    ) = (vec![], vec![], vec![]);

    let scale = _loop.size_ctx_all().scale;
    // Hoist the camera + GPU reads before the surface-registry borrow: surface_mut()
    // borrows the whole Orchestrator, so other inner fields must be read first.
    let camera_transform = _loop.inner.camera().transform.clone();
    let gpu = _loop.inner.environment.GPU.clone();
    let mut wants_frame = false;
    if let Some(ref mut UI) = _loop.inner.surface_mut().registry {
        let transform = ui::Transform {
            zoom: camera_transform.zoom,
            position: Point::new(
                camera_transform.position.x * scale,
                camera_transform.position.y * scale,
            ),
        };

        // Requires gles renderer on every frame. Temporary. should store it instead.
        let res = UI
            .render_all(
                gpu.as_str(),
                renderer,
                transform,
                size.to_f64(),
                crate::scene::layer::base::Layer::SCENE.bits(),
            )
            .unwrap_or_default();

        for item in res {
            match item.space {
                UiSpace::World => {
                    ui_elements.push(item);
                }
                UiSpace::Screen => {
                    ui_elements_screen.push(item);
                }
            }
        }

        // Capture-dim pass: rendered separately so the scene builder can place
        // it BELOW windows (its own z-slot), unlike the screen-space SCENE pass
        // which sits above them.
        let dim = UI
            .render_all(
                gpu.as_str(),
                renderer,
                transform,
                size.to_f64(),
                crate::scene::layer::base::Layer::CAPTURE_DIM.bits(),
            )
            .unwrap_or_default();
        ui_elements_dim.extend(dim);

        // Any instance still dirty or mid-animation wants another frame.
        wants_frame = UI.wants_frame();
    }

    // Keep the vblank cycle alive while UI is animating, so time-based
    // animations advance frame-to-frame (mirrors the parallax background's
    // self-scheduling). Done after the registry borrow is released.
    if wants_frame {
        _loop.schedule_redraw_post_vblank(RedrawReason::Ui);
    }

    (ui_elements, ui_elements_screen, ui_elements_dim)
}
