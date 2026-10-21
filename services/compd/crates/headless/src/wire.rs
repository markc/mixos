// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Render-node assembly and the virtual output's renderer contract.

use std::{cell::RefCell, fs::OpenOptions, os::fd::OwnedFd, rc::Rc};

use outputs::render_contract::contract::{DisplayBackend, RenderContract, RendererId};
use smithay::backend::allocator::{Fourcc, dmabuf::Dmabuf, format::FormatSet, gbm::GbmDevice};
use smithay::backend::drm::{DrmNode, NodeType};
use smithay::backend::egl::{EGLDevice, EGLDisplay};
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::{ImportDma, ImportEgl, Offscreen};
use smithay::output::{Mode, Output, PhysicalProperties, Scale, Subpixel};
use smithay::reexports::calloop::{EventLoop, ping::make_ping};
use smithay::reexports::wayland_server::{DisplayHandle, protocol::wl_surface::WlSurface};
use smithay::utils::{DeviceFd, Physical, Size, Transform};
use world::state::Loop;

pub struct Options<'a> {
    pub size: Size<i32, Physical>,
    pub scale: f64,
    pub render_node: &'a str,
}

struct Contract {
    output: Output,
    mode: Mode,
    renderer: GlesRenderer,
}

impl DisplayBackend for Contract {
    fn load(&mut self) -> (&Output, &Mode) {
        (&self.output, &self.mode)
    }

    fn bind_display(&mut self, handle: &DisplayHandle) -> FormatSet {
        if let Err(error) = self.renderer.bind_wl_display(handle) {
            warn!("headless: legacy EGL client bridge unavailable: {error}");
            return FormatSet::default();
        }
        self.renderer.dmabuf_formats()
    }
}

impl RenderContract for Contract {
    fn id(&self) -> RendererId {
        RendererId::Gles
    }
    fn bind_display(&mut self, handle: &DisplayHandle) -> FormatSet {
        DisplayBackend::bind_display(self, handle)
    }
    fn supported_formats(&mut self) -> FormatSet {
        self.renderer.egl_context().dmabuf_texture_formats().clone()
    }
    fn import_dmabuf(&mut self, buffer: &Dmabuf) -> bool {
        self.renderer.import_dmabuf(buffer, None).is_ok()
    }
    fn early_import(&mut self, _surface: &WlSurface) {}
    fn sync_capable(&self) -> bool {
        false
    }
    fn export_render_fence(&mut self) -> Option<OwnedFd> {
        None
    }
}

pub fn wire(
    state: &mut Loop,
    event_loop: &mut EventLoop<Loop>,
    options: Options<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    // No DrmDevice, seat, udev or input backend: GBM holds only this render fd.
    let node = DrmNode::from_path(options.render_node).map_err(|error| {
        format!(
            "headless-render-node-invalid: {}: {error}",
            options.render_node
        )
    })?;
    if node.ty() != NodeType::Render {
        return Err("headless-render-node-required: expected a DRM render node, not a primary or control node".into());
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(options.render_node)?;
    let gbm = GbmDevice::new(DeviceFd::from(OwnedFd::from(file)))?;
    let display = unsafe { EGLDisplay::new(gbm) }?;
    let reported = EGLDevice::device_for_display(&display)?
        .try_get_render_node()?
        .and_then(|node| node.dev_path())
        .ok_or("EGL device has no render node")?;
    if std::fs::canonicalize(&reported)? != std::fs::canonicalize(options.render_node)? {
        return Err("EGL device does not match the requested render node".into());
    }
    info!("headless: EGL device = {}", reported.display());
    // Published as soon as the node is validated, before anything that can
    // fail later: the dmabuf advertiser names it as the clients' main device.
    *state
        .inner
        .kernel
        .get_mut(&render_gles::format::registrar::COMPOSITE_NODE_MUT) = Some(node);
    let renderer = render_gles::context::egl::create(&display)?;
    let refresh = match std::env::var("COMPD_HEADLESS_REFRESH_HZ") {
        Ok(value) => value
            .parse::<i32>()
            .ok()
            .filter(|hz| (30..=60).contains(hz))
            .ok_or("headless-refresh-invalid: COMPD_HEADLESS_REFRESH_HZ must be 30..=60")?,
        Err(std::env::VarError::NotPresent) => 60,
        Err(_) => {
            return Err("headless-refresh-invalid: COMPD_HEADLESS_REFRESH_HZ is not UTF-8".into());
        }
    };
    let mode = Mode {
        size: options.size,
        refresh: refresh * 1000,
    };
    let output = Output::new(
        "headless-0".into(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "MixOS".into(),
            model: "Headless".into(),
            serial_number: "Unknown".into(),
        },
    );
    output.change_current_state(
        Some(mode),
        Some(Transform::Normal),
        Some(Scale::Fractional(options.scale)),
        Some((0, 0).into()),
    );
    output.set_preferred(mode);
    let mut contract = Contract {
        output: output.clone(),
        mode,
        renderer,
    };
    let handle = state.state.output.display_handle.clone();
    let damage_tracker =
        scenegraph::state::lifecycle::lifecycle::initialize(state, &output, &handle, &mut contract);
    // No physical input sources. Keep the human seat global, but advertise no
    // capabilities. This runs before the event loop serves any client.
    state.state.seat.seat.remove_pointer();
    state.state.seat.seat.remove_keyboard();
    state.state.seat.seat.remove_touch();
    state.inner.comp.injection.backend = world::comp::injection::BackendInput::Headless;
    // The loop pins the camera to identity before input or drawing. Put the
    // agent at the logical output centre, as centre_pointer_once does for
    // the human pointer. main's apply_pointer only seeds the human accumulator.
    let centre = smithay::utils::Point::from((
        f64::from(mode.size.w) / options.scale / 2.0,
        f64::from(mode.size.h) / options.scale / 2.0,
    ));
    state.inner.comp.injection.agent_pointer = Some((centre.x, centre.y));
    if let Some(pointer) = state
        .state
        .seat
        .agent
        .as_ref()
        .and_then(|seat| seat.get_pointer())
    {
        pointer.motion(
            &mut state.state,
            None,
            &smithay::input::pointer::MotionEvent {
                location: centre,
                serial: smithay::utils::SERIAL_COUNTER.next_serial(),
                time: 0,
            },
        );
        pointer.frame(&mut state.state);
    }
    let formats = state
        .inner
        .kernel
        .get(&render_gles::format::registrar::FORMATS);
    use render_gles::format::{registrar::registrar::Device, role::role::Role};
    formats.register(
        Device::UNSPECIFIED,
        Role::Sample,
        contract.renderer.dmabuf_formats(),
        "headless renderer",
    );
    formats.absent(Role::ScanoutEgl, "headless: no scanout device");
    formats.absent(Role::Render, "headless: no scanout render format");
    formats.absent(Role::Plane, "headless: no KMS plane");
    model::stats::registry::base::set_compositor_prefers_dmabuf(false);
    frames::scene::gpu::prewarm(state, &mut contract.renderer);
    let targets = [
        Offscreen::<GlesTexture>::create_buffer(
            &mut contract.renderer,
            Fourcc::Abgr8888,
            (options.size.w, options.size.h).into(),
        )?,
        Offscreen::<GlesTexture>::create_buffer(
            &mut contract.renderer,
            Fourcc::Abgr8888,
            (options.size.w, options.size.h).into(),
        )?,
    ];
    decor::window::install(decor::ChromeTheme::load(
        decor::ChromeStyle::from_name(&state.inner.preference.chrome_style).unwrap_or_default(),
    ));
    let context = Rc::new(RefCell::new(crate::frame::Context::new(
        output,
        contract.renderer,
        targets,
        damage_tracker,
    )));
    let (ping, source) = make_ping()?;
    let first = ping.clone();
    event_loop
        .handle()
        .insert_source(source, move |_, _, state| {
            crate::frame::arm(state, &context);
        })?;
    state.state.redraw.set_ping(ping);
    first.ping();
    screencopy::create_global(&state.state.output.display_handle);
    info!(
        "headless: headless-0 {}x{} at scale {} wired",
        mode.size.w, mode.size.h, options.scale
    );
    Ok(())
}
