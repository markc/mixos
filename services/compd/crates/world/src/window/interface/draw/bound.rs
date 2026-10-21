// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use crate::camera::transform::translate::slot;
use crate::camera::transform::translate::transform::Transform;
use crate::state::CoordinateTrait;
use crate::state::Loop;
use smithay::backend::renderer::{ImportAll, ImportMem, Texture};
use smithay::desktop::Window;
use smithay::utils::{Logical, Physical, Point, Rectangle, Size};

#[derive(Debug)]
pub struct Bound {
    // pub top: Rectangle<f64, Logical>,
    // pub left: Rectangle<f64, Logical>,
    // pub right: Rectangle<f64, Logical>,
    // pub bottom: Rectangle<f64, Logical>,
    pub top: f64,
    pub left: f64,
    pub right: f64,
    pub width: f64,
    pub height: f64,
    pub bottom: f64,
}

pub struct CalculateBoundResult {
    pub screen: Bound,

    // CHECK: world is wrong here
    pub world: Bound,
    pub bounds: Rectangle<i32, Logical>,
}

pub fn calculate<R>(
    state: &mut Loop,
    _renderer: &mut R,
    _size: Size<i32, Physical>,
    window: &Window,
    _context: &crate::canvas::draw::context::Context,
) -> CalculateBoundResult
where
    R: smithay::backend::renderer::Renderer + ImportAll + ImportMem,
    R::TextureId: Texture + Clone + Send + 'static,
{
    let ctx = state.viewport_context();

    // Recognized bounds = the compositor-decided **slot** (the size the window is enforced at,
    // and the rect content is letterboxed into), so decoration borders frame the slot rather
    // than the client's (possibly misbehaving) committed geometry. Mirrors the render/input
    // slot logic. Falls back to element_geometry when no size is decided (0x0 defer).
    let elem_loc = state
        .inner
        .space_state()
        .state
        .element_location(window)
        .unwrap_or_default();
    let slot_size = slot::expected_size(window);
    let geom = match slot_size.filter(|s| s.w > 0 && s.h > 0) {
        Some(size) => smithay::utils::Rectangle::new(elem_loc, size),
        None => state
            .inner
            .space_state()
            .state
            .element_geometry(window)
            .unwrap_or_default(),
    };

    // world bounds: directly from the geometry, no projection needed.
    let world = Bound {
        top: geom.loc.y as f64,
        left: geom.loc.x as f64,
        right: (geom.loc.x + geom.size.w) as f64,
        bottom: (geom.loc.y + geom.size.h) as f64,
        width: geom.size.w as f64,
        height: geom.size.h as f64,
    };

    // screen bounds: project the slot by its **corners** (each rounded once), identical to the
    // content crop in `window.draw.frame::scene`, so the border tracks the content exactly and an
    // edge at a fixed world coordinate (e.g. the right edge during a resize-from-left) stays put
    // instead of wobbling ±1px — which happens if you project the loc and add a separately-rounded
    // scaled width.
    let tl_t: Transform = (geom.loc, ctx).into();
    let tl: Point<i32, Physical> = tl_t.into();
    let br_t: Transform = (geom.loc + Point::from((geom.size.w, geom.size.h)), ctx).into();
    let br: Point<i32, Physical> = br_t.into();

    let screen = Bound {
        top: tl.y as f64,
        left: tl.x as f64,
        right: br.x as f64,
        bottom: br.y as f64,
        width: (br.x - tl.x) as f64,
        height: (br.y - tl.y) as f64,
    };

    CalculateBoundResult {
        bounds: geom,
        world: world,
        screen: screen,
    }
}

// pub fn calculate<R>(
//     state: &mut Loop,
//     renderer: &mut R,
//     size: Size<i32, Physical>,
//     window: &Window,
//     context: &crate::canvas::draw::context::Context,
// ) -> (CalculateBoundResult)
// where
//     R: smithay::backend::renderer::Renderer + ImportAll + ImportMem,
//     R::TextureId: Texture + Clone + Send + 'static,
// {
//     let loc = state
//         .state
//         .space
//         .state
//         .element_location(window)
//         .unwrap_or_default();
//     let window_bbox = state
//         .state
//         .space
//         .state
//         .element_bbox(window)
//         .unwrap_or_default();

//     let win_w = window_bbox.size.w as f64;
//     let win_h = window_bbox.size.h as f64;

//     let win_top_left = global_to_canvas(
//         &state.inner.camera_mut().transform,
//         Size::new(size.w as f64, size.h as f64),
//         Point::new(loc.x as f64, loc.y as f64),
//         state.inner.space_state().default_scale(),
//     );

//     let win_bot_right = global_to_canvas(
//         &state.inner.camera_mut().transform,
//         Size::new(size.w as f64, size.h as f64),
//         Point::new(loc.x as f64 + win_w, loc.y as f64 + win_h),
//         state.inner.space_state().default_scale(),
//     );

//     // let world_width = scale(&state.inner.camera_mut().transform, win_w);
//     // let world_height = scale(&state.inner.camera_mut().transform, win_h);
//     // let world_offset_x = loc.x as f64;
//     // let world_offset_y = loc.y as f64;

//     let world: Transform = (window_bbox, state.viewport_context()).into();
//     // let world_transform = world_to_screen(&state.camera, Size::new(size.w as f64, size.h as f64), Point::new(
//     //     world_offset_x,
//     //     world_offset_y,
//     // ));

//     let world: Rectangle<f64, Logical> = world.into();
//     let world = Bound {
//         top: world.loc.y,
//         left: world.loc.x,
//         right: world.loc.x + world.size.w,
//         bottom: world.loc.y + world.size.h,
//         width: world.size.w,
//         height: world.size.h,
//         // top: world_transform.y,
//         // left: world_transform.x,
//         // right: world_transform.x + world_width,
//         // bottom: world_transform.y + world_height,
//     };
//     // let world_offset_x = loc.x as f64 - state.camera.position.x;
//     // let world_offset_y = loc.y as f64 - state.camera.position.y;
//     //
//     // let scaled_x = world_offset_x * state.camera.zoom;
//     // let scaled_y = world_offset_y * state.camera.zoom;
//     //
//     // let screen_x = scaled_x + (size.w as f64 / 2.0);
//     // let screen_y = scaled_y + (size.h as f64 / 2.0);

//     // let physical_loc = smithay::utils::Point::from((screen_x as i32, screen_y as i32));

//     return CalculateBoundResult {
//         bounds: window_bbox,
//         world: world,
//         screen: Bound {
//             // top: screen_rect.y() as f64,
//             // left: screen_rect.x() as f64,
//             // right: screen_rect.right() as f64,
//             // bottom: screen_rect.bottom() as f64,
//             // width: screen_rect.w() as f64,
//             // height: screen_rect.h() as f64,
//             top: win_top_left.y,
//             left: win_top_left.x,
//             right: win_bot_right.x,
//             bottom: win_bot_right.y,
//             width: win_bot_right.x - win_top_left.x,
//             height: win_bot_right.y - win_top_left.y,
//         },
//     };
// }
