// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement;
use smithay::backend::renderer::element::surface::render_elements_from_surface_tree;
use smithay::backend::renderer::element::{AsRenderElements, Kind};
use smithay::backend::renderer::{ImportAll, ImportMem, Texture};
use smithay::utils::{Logical, Physical, Point, Rectangle, Scale, Size};
use std::os::linux::raw::stat;
use world::seat::pointer::element::element::PointerRenderElement;
use world::state::state::CoordinateTrait;
use world::state::{Loop, Transform};

pub fn element<R>(
    state: &mut Loop,
    renderer: &mut R,
    size: Size<i32, Physical>,
) -> Vec<PointerRenderElement<R>>
where
    R: smithay::backend::renderer::Renderer + ImportAll + ImportMem,
    R::TextureId: Texture + Clone + Send + 'static,
{
    let mut elements: Vec<PointerRenderElement<R>> = Vec::new();

    // While a touch sequence owns the active output the shared cursor "disappears"
    // — it reappears where it was once a pointer device is used again.
    if state.inner.saved_cursor.is_some() {
        elements.extend(agent_arrow(state, renderer));
        return elements;
    }

    let Some(pointer) = state.state.seat.seat.get_pointer() else {
        elements.extend(agent_arrow(state, renderer));
        return elements;
    };
    // The sprite belongs where the user's HAND is — which, under a displacing
    // bundle, is NOT where `current_location()` points.
    //
    // That location is warp-corrected: it names the content the cursor is over,
    // in un-displaced world space. Projecting it back to the screen therefore
    // lands the sprite at the PRE-warp position of that content, which is exactly
    // where the content is not drawn.
    //
    // The canvas solid-box cursor needs exactly the same correction, and for the
    // same reason — it does NOT get it for free by being a world-band element.
    // Two independent reasons: the pipeline's output pass samples only the
    // pipeline's own targets, so an engine op drawn after it is never displaced;
    // and `split_at`'s predicate (`renderer/submit.rs`) ignores `DrawOp::Solid`
    // entirely, so a cursor solid falls into the SCREEN band regardless of band.
    // Both cursors are therefore told, from the same value.
    //
    // That value carries the gate as well as the position — `None` means no warp
    // is in effect — so neither reader re-derives it. A frame where one warps and
    // the other does not is a cursor that renders away from where it clicks.
    let true_screen = world::seat::pointer::publish::publish::true_screen();
    let cursor_world = match true_screen {
        Some((x, y)) => {
            let ctx = state.focus_pane_context();
            let t: Transform = (Point::<f64, Physical>::from((x, y)), ctx).into();
            t.into_storage_point_f64()
        }
        None => pointer.current_location(),
    };
    // let hotspot = state.inner.pointer_mut().element.get_current_hotspot();

    // Project the world pointer location through the pane the cursor is in (not
    // the full output), so the rendered cursor lands at the physical hardware
    // position even when a split pane's camera is panned/zoomed.
    let size_context = state.focus_pane_context();

    // A compositor-forced cursor (e.g. the canvas hand tool's grab icon) overrides
    // whatever the focused client set, so the affordance survives moving over windows.
    // A region selection's crosshair overrides both, only while the run holds the seat.
    // Chosen before the hotspot is read, so a change of shape is placed by its own
    // hotspot on its first frame (the crosshair's is its centre, the arrow's its tip).
    let forced = state
        .inner
        .comp
        .region
        .cursor()
        .or(state.state.seat.force_cursor);
    state.inner.pointer_mut().element.status = match forced {
        Some(icon) => smithay::input::pointer::CursorImageStatus::Named(icon),
        None => state.state.seat.pointer_status.clone(),
    };

    let hotspot_logical = state.inner.pointer_mut().element.get_current_hotspot();
    // hotspot is in surface logical buffer space; convert to physical:
    let hotspot_phys = Point::<i32, Physical>::from((
        (hotspot_logical.x as f64 * size_context.scale).round() as i32,
        (hotspot_logical.y as f64 * size_context.scale).round() as i32,
    ));

    // let render_at = cursor_phys - hotspot_phys;

    // Build Transform from world position. Hotspot is in *physical* pixels
    // per the smithay convention (hotspot is on the buffer, buffers are
    // physical-density).
    let cursor_rect_transform: Transform =
        ((cursor_world.x, cursor_world.y, 0.0, 0.0), size_context).into();

    let cursor_phys: Point<i32, Physical> = cursor_rect_transform.into();
    let render_at = cursor_phys - hotspot_phys; // subtract hotspot

    let pointer_elements: Vec<PointerRenderElement<_>> =
        state.inner.pointer_mut().element.render_elements(
            renderer,
            render_at,
            Scale::from(size_context.scale), // not .round() — exact fractional
            1.0,
        );

    if let Some(icon_surface) = &state.state.dnd.icon {
        let dnd_element = render_elements_from_surface_tree::<_, PointerRenderElement<R>>(
            renderer,
            icon_surface,
            render_at,
            Scale::from(size_context.scale),
            1.0,
            Kind::Unspecified,
        );

        elements.extend(dnd_element);
    }

    // let pointer_elements: Vec<PointerRenderElement<_>> = state.inner.pointer_mut().element.render_elements(
    //     renderer,
    //     cursor_rect.loc,
    //     Scale::from(size_ctx.scale.round()),
    //     1.0, // Alpha/Opacity
    // );

    // if pointer_elements.is_empty()
    //     println!("Cursor is empty");
    // }

    // POinter is in window space. SHould it be scaled? Yes. it is expected to be in render space which is different from world space. However it must remain unscaled because mouse is uniform across zoom.
    // also- pointer.current_location may already be in world coordinates. in this case it should not be transformed at all.
    // It should be same as canvas ( which does transform )

    // Legacy which is transformed over panning and zooming.
    // let cursor_offset_x = cursor_logical.x - state.inner.camera_mut().transform.position().x;
    // let cursor_offset_y = cursor_logical.y - state.inner.camera_mut().transform.position().y;
    //
    // let cursor_scaled_x = cursor_offset_x * state.inner.camera_mut().transform.zoom();
    // let cursor_scaled_y = cursor_offset_y * state.inner.camera_mut().transform.zoom();
    //
    // let cursor_screen_x = cursor_scaled_x + (size.w as f64 / 2.0);
    // let cursor_screen_y = cursor_scaled_y + (size.h as f64 / 2.0);
    // let cursor_hotspot = state.inner.pointer_mut().element.get_current_hotspot();
    //
    // // 3. Subtract the hotspot (scaled by your camera zoom!) from the ACTUAL screen position
    // let scaled_hotspot_x = cursor_hotspot.x as f64;
    // let scaled_hotspot_y = cursor_hotspot.y as f64;
    // // let scaled_hotspot_x = cursor_hotspot.x as f64 * state.camera.zoom;
    // // let scaled_hotspot_y = cursor_hotspot.y as f64 * state.camera.zoom;
    //
    // let draw_x = cursor_screen_x - scaled_hotspot_x;
    // let draw_y = cursor_screen_y - scaled_hotspot_y;

    // let draw_location = cursor_logical;
    // let draw_location = smithay::utils::Point::<i32, smithay::utils::Physical>::from((
    //     draw_x.round() as i32,
    //     draw_y.round() as i32,
    // ));

    // 4. Generate the pointer elements
    // Note: We pass your `state.camera.zoom` as the scale factor so the cursor scales with the world
    // Legacy - Scaled version ( should also scale by output scale.
    // let pointer_elements: Vec<PointerRenderElement<_>> = state.inner.pointer_mut().element.render_elements(
    //     renderer,
    //     draw_location,
    //     smithay::utils::Scale::from(*state.inner.camera_mut().transform.zoom()),
    //     1.0, // Alpha/Opacity
    // );

    // CHECK:  use damage output in redraw and vblank
    // 5. Push the generated pointer elements into your master canvas list
    for el in pointer_elements {
        // Wrap it in the CanvasElement enum variant we just created
        elements.push(el);
    }

    elements.extend(agent_arrow(state, renderer));
    elements
}

/// The agent seat's pointer, so a human sees where an agent points: its own
/// amber arrow at the agent's location (the same space as the human's
/// `current_location()`). The caller pushes it after the human cursor, so it
/// draws behind it (first-pushed is front-most); `Kind::Unspecified` keeps it
/// off the one hardware cursor plane. Gone once the agent seat is cleared.
/// Drawn during a touch sequence too: touch hides the human sprite only.
///
/// Limit: projected through the pane the HUMAN cursor is in, like the whole
/// cursor layer, which is also drawn only on the human cursor's output. Right
/// on one output with one pane; with split panes or a second output an agent
/// elsewhere is drawn through the wrong camera, or not at all. A per-point
/// pane lookup is the fix (motion plan follow-up).
fn agent_arrow<R>(state: &mut Loop, renderer: &mut R) -> Option<PointerRenderElement<R>>
where
    R: smithay::backend::renderer::Renderer + ImportAll + ImportMem,
    R::TextureId: Texture + Clone + Send + 'static,
{
    let (x, y) = state.inner.comp.injection.agent_pointer?;
    let size_context = state.focus_pane_context();
    let theme = std::sync::Arc::clone(&state.inner.pointer_mut().element.theme);
    let cursor = theme.agent_arrow(size_context.scale);
    let at: Point<i32, Physical> = Transform::from(((x, y, 0.0, 0.0), size_context)).into();
    let ratio = f64::from(theme.base_size()) / f64::from(cursor.nominal.max(1));
    let pixels = (f64::from(cursor.size.w), f64::from(cursor.size.h));
    let src = Rectangle::<f64, Logical>::from_size(pixels.into());
    let logical = Size::<i32, Logical>::from((
        (pixels.0 * ratio).round().max(1.0) as i32,
        (pixels.1 * ratio).round().max(1.0) as i32,
    ));
    MemoryRenderBufferRenderElement::from_buffer(
        renderer,
        at.to_f64(),
        &cursor.frames[0].buffer,
        None,
        Some(src),
        Some(logical),
        Kind::Unspecified,
    )
    .ok()
    .map(PointerRenderElement::from)
}
