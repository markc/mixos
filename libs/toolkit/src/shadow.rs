// SPDX-License-Identifier: MIT OR Apache-2.0
//! KWin shadows on the root window's own connection. Winit owns socket reads;
//! we dispatch only our guest backend's dedicated queue, never winit's queue.
use std::io::Write;
use std::os::fd::AsFd;
use std::sync::Arc;
use std::time::{Duration, Instant};

use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, delegate_noop};

#[allow(dead_code, non_camel_case_types, unused_imports, clippy::all)]
mod protocol {
    use wayland_client;
    use wayland_client::protocol::*;
    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("protocols/shadow.xml");
    }
    use self::__interfaces::*;
    wayland_scanner::generate_client_code!("protocols/shadow.xml");
}
use protocol::org_kde_kwin_shadow::OrgKdeKwinShadow;
use protocol::org_kde_kwin_shadow_manager::OrgKdeKwinShadowManager;

#[derive(Default)]
struct Globals {
    manager: Option<OrgKdeKwinShadowManager>,
    shm: Option<wl_shm::WlShm>,
    done: bool,
}

impl Dispatch<wl_registry::WlRegistry, ()> for Globals {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "org_kde_kwin_shadow_manager" => {
                    state.manager = Some(registry.bind(name, version.min(2), qh, ()))
                }
                "wl_shm" => state.shm = Some(registry.bind(name, 1, qh, ())),
                _ => {}
            }
        }
    }
}
impl Dispatch<wl_callback::WlCallback, ()> for Globals {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        _: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        state.done = true;
    }
}
delegate_noop!(Globals: ignore OrgKdeKwinShadowManager);
delegate_noop!(Globals: ignore OrgKdeKwinShadow);
delegate_noop!(Globals: ignore wl_shm::WlShm);
delegate_noop!(Globals: ignore wl_shm_pool::WlShmPool);
delegate_noop!(Globals: ignore wl_buffer::WlBuffer);

#[derive(Clone, Copy, PartialEq)]
struct ShadowSettings {
    frame: design::frame::Frame,
    radius: f32,
    scale: f32,
    focused: bool,
    size: [u32; 2],
}
struct Attached {
    shadow: OrgKdeKwinShadow,
    buffers: Vec<wl_buffer::WlBuffer>,
    settings: ShadowSettings,
}
impl Drop for Attached {
    fn drop(&mut self) {
        // Version 1 has no destroy request; sending one is a protocol error,
        // so a v1 shadow object is left to die with the connection.
        if self.shadow.version() >= 2 {
            self.shadow.destroy();
        }
        for buffer in &self.buffers {
            buffer.destroy();
        }
    }
}

struct NativeShadow {
    attached: Option<Attached>,
    globals: Globals,
    surface: wl_surface::WlSurface,
    registry: wl_registry::WlRegistry,
    queue: EventQueue<Globals>,
    connection: Connection,
    deadline: Instant,
    // Drop last: the foreign display and surface stay live through cleanup,
    // including when an egui Context outlives the eframe integration.
    _owner: Arc<winit::window::Window>,
}

pub(crate) fn install(ctx: &egui::Context, window: Arc<winit::window::Window>) {
    let Some(shadow) = NativeShadow::new(window) else {
        return;
    };
    ctx.add_plugin(shadow);
}

impl NativeShadow {
    fn new(window: Arc<winit::window::Window>) -> Option<Self> {
        let RawDisplayHandle::Wayland(display) = window.display_handle().ok()?.as_raw() else {
            return None;
        };
        let RawWindowHandle::Wayland(handle) = window.window_handle().ok()?.as_raw() else {
            return None;
        };
        // SAFETY: the retained window owns both pointers, and is dropped after
        // every proxy, queue and connection. Never destroy/reassign its surface.
        let backend = unsafe {
            wayland_backend::client::Backend::from_foreign_display(display.display.as_ptr().cast())
        };
        let connection = Connection::from_backend(backend);
        let id = unsafe {
            wayland_backend::client::ObjectId::from_ptr(
                wl_surface::WlSurface::interface(),
                handle.surface.as_ptr().cast(),
            )
            .ok()?
        };
        let surface = wl_surface::WlSurface::from_id(&connection, id).ok()?;
        let queue = connection.new_event_queue();
        let qh = queue.handle();
        let registry = connection.display().get_registry(&qh, ());
        connection.display().sync(&qh, ());
        connection.flush().ok()?;
        Some(Self {
            attached: None,
            globals: Globals::default(),
            surface,
            registry,
            queue,
            connection,
            deadline: Instant::now() + Duration::from_millis(250),
            _owner: window,
        })
    }

    fn detach(&mut self) {
        if self.attached.is_some() {
            if let Some(manager) = &self.globals.manager {
                manager.unset(&self.surface);
            }
            self.attached = None;
        }
    }

    fn attach(&mut self, settings: ShadowSettings) -> std::io::Result<()> {
        let Some(manager) = &self.globals.manager else {
            return Ok(());
        };
        let Some(shm) = &self.globals.shm else {
            return Ok(());
        };
        let tiles = design::frame::shadow_pieces(
            settings.frame.shadow,
            settings.radius,
            settings.scale,
            settings.focused,
            settings.size,
        );
        let path = std::env::temp_dir().join(format!("mixos-shadow-{}", uuid::Uuid::new_v4()));
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)?;
        std::fs::remove_file(path)?;
        for piece in &tiles.pieces {
            file.write_all(&piece.argb)?;
        }
        let size = tiles.pieces.iter().map(|p| p.argb.len() as i32).sum();
        let qh = self.queue.handle();
        let pool = shm.create_pool(file.as_fd(), size, &qh, ());
        let mut offset = 0;
        let buffers: Vec<_> = tiles
            .pieces
            .iter()
            .map(|piece| {
                let (w, h, pixels) = (&piece.width, &piece.height, &piece.argb);
                let buffer =
                    pool.create_buffer(offset, *w, *h, w * 4, wl_shm::Format::Argb8888, &qh, ());
                offset += pixels.len() as i32;
                buffer
            })
            .collect();
        pool.destroy();
        let shadow = manager.create(&self.surface, &qh, ());
        shadow.attach_top(&buffers[0]);
        shadow.attach_top_right(&buffers[1]);
        shadow.attach_right(&buffers[2]);
        shadow.attach_bottom_right(&buffers[3]);
        shadow.attach_bottom(&buffers[4]);
        shadow.attach_bottom_left(&buffers[5]);
        shadow.attach_left(&buffers[6]);
        shadow.attach_top_left(&buffers[7]);
        let [left, top, right, bottom] = tiles
            .extents
            .map(|v| f64::from(v) / f64::from(settings.scale));
        shadow.set_left_offset(left);
        shadow.set_top_offset(top);
        shadow.set_right_offset(right);
        shadow.set_bottom_offset(bottom);
        shadow.commit();
        // Winit's next surface commit applies this state. Do not commit its
        // surface here: doing so could consume frame callbacks or pending state.
        self.attached = Some(Attached {
            shadow,
            buffers,
            settings,
        });
        Ok(())
    }
}

impl egui::Plugin for NativeShadow {
    fn debug_name(&self) -> &'static str {
        "KWin shadow"
    }

    fn on_end_pass(&mut self, ui: &mut egui::Ui) {
        if ui.ctx().viewport_id() != egui::ViewportId::ROOT {
            return;
        }
        if self.queue.dispatch_pending(&mut self.globals).is_err() {
            return;
        }
        if !self.globals.done {
            if Instant::now() < self.deadline {
                ui.ctx().request_repaint_after(Duration::from_millis(16));
            }
            return;
        }
        let visible = ui.input(|i| {
            !i.viewport().maximized.unwrap_or(false) && !i.viewport().fullscreen.unwrap_or(false)
        }) && crate::chrome::Chrome::of(ui.ctx()).decorations
            == design::Decorations::Client;
        let frame = crate::chrome::Chrome::of(ui.ctx()).frame;
        let scale = ui
            .input(|i| i.viewport().native_pixels_per_point.unwrap_or(1.0))
            .clamp(0.5, 8.0);
        let size = (ui.ctx().content_rect().size() * ui.ctx().pixels_per_point()).round();
        let settings = ShadowSettings {
            frame,
            scale,
            radius: crate::window_frame::radius(ui.ctx(), frame),
            focused: ui.input(|i| i.viewport().focused.unwrap_or(i.focused)),
            size: [size.x.max(1.0) as u32, size.y.max(1.0) as u32],
        };
        if !visible
            || self
                .attached
                .as_ref()
                .is_some_and(|a| a.settings != settings)
        {
            self.detach();
        }
        if visible
            && self.attached.is_none()
            && let Err(error) = self.attach(settings)
        {
            logging::debug!("KWin shadow: {error}");
        }
        let _ = self.connection.flush();
    }
    fn on_exit(&mut self, _: &egui::Context) {
        self.detach();
        let _ = self.connection.flush();
    }
}

impl Drop for NativeShadow {
    fn drop(&mut self) {
        self.detach();
        if let Some(manager) = &self.globals.manager
            && manager.version() >= 2
        {
            manager.destroy();
        }
        let _ = self
            .connection
            .backend()
            .destroy_object(&self.registry.id());
        let _ = self.connection.flush();
    }
}
