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

/// Tiles clockwise from top. Offsets are physical pixel extents in that order
/// (left, top, right, bottom); buffers contain native-endian premultiplied ARGB.
struct Tiles {
    images: Vec<(i32, i32, Vec<u8>)>,
    extents: [i32; 4],
}

fn tiles(shadow: egui::epaint::Shadow, scale: f32) -> Tiles {
    let scale = scale.clamp(0.5, 8.0);
    let radius = (f32::from(shadow.blur) * scale).ceil().max(1.0);
    let spread = f32::from(shadow.spread) * scale;
    let [ox, oy] = shadow.offset.map(|v| f32::from(v) * scale);
    let extent = |offset: f32| (radius + spread + offset).ceil().clamp(1.0, 4096.0) as i32;
    let [left, top, right, bottom] = [extent(-ox), extent(-oy), extent(ox), extent(oy)];
    let extents = [left, top, right, bottom];
    // The short side overlaps the window by the offset. Without that overlap,
    // its corner ends before the Gaussian reaches the edge tile's peak and
    // leaves a visible seam (especially with a vertical shadow offset).
    let lw = left + ox.max(0.0).ceil() as i32;
    let th = top + oy.max(0.0).ceil() as i32;
    let rw = right + (-ox).max(0.0).ceil() as i32;
    let bh = bottom + (-oy).max(0.0).ceil() as i32;
    let specs = [
        (1, th, 0, -top, 0, -1),
        (rw, th, right - rw, -top, 1, -1),
        (rw, 1, right - rw, 0, 1, 0),
        (rw, bh, right - rw, bottom - bh, 1, 1),
        (1, bh, 0, bottom - bh, 0, 1),
        (lw, bh, -left, bottom - bh, -1, 1),
        (lw, 1, -left, 0, -1, 0),
        (lw, th, -left, -top, -1, -1),
    ];
    let colour = shadow.color.to_srgba_unmultiplied();
    let images = specs
        .into_iter()
        .map(|(w, h, x0, y0, horizontal, vertical)| {
            let mut bytes = Vec::with_capacity((w * h * 4) as usize);
            for y in y0..y0 + h {
                for x in x0..x0 + w {
                    let distance = |coordinate: i32, offset: f32, direction: i32| {
                        if direction < 0 {
                            (-coordinate as f32 - 1.0 + offset - spread).max(0.0)
                        } else {
                            (coordinate as f32 - offset - spread).max(0.0)
                        }
                    };
                    let dx = if horizontal != 0 {
                        distance(x, ox, horizontal)
                    } else {
                        0.0
                    };
                    let dy = if vertical != 0 {
                        distance(y, oy, vertical)
                    } else {
                        0.0
                    };
                    let sigma = radius / 3.0;
                    let a = (f32::from(colour[3])
                        * (-(dx * dx + dy * dy) / (2.0 * sigma * sigma)).exp())
                    .round() as u32;
                    let channel = |c: u8| u32::from(c) * a / 255;
                    let argb = (a << 24)
                        | (channel(colour[0]) << 16)
                        | (channel(colour[1]) << 8)
                        | channel(colour[2]);
                    bytes.extend_from_slice(&argb.to_ne_bytes());
                }
            }
            (w, h, bytes)
        })
        .collect();
    Tiles { images, extents }
}

struct Attached {
    shadow: OrgKdeKwinShadow,
    buffers: Vec<wl_buffer::WlBuffer>,
    settings: (egui::epaint::Shadow, f32),
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

    fn attach(&mut self, settings: (egui::epaint::Shadow, f32)) -> std::io::Result<()> {
        let Some(manager) = &self.globals.manager else {
            return Ok(());
        };
        let Some(shm) = &self.globals.shm else {
            return Ok(());
        };
        let tiles = tiles(settings.0, settings.1);
        let path = std::env::temp_dir().join(format!("mixos-shadow-{}", uuid::Uuid::new_v4()));
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)?;
        std::fs::remove_file(path)?;
        for (_, _, pixels) in &tiles.images {
            file.write_all(pixels)?;
        }
        let size = tiles.images.iter().map(|(_, _, p)| p.len() as i32).sum();
        let qh = self.queue.handle();
        let pool = shm.create_pool(file.as_fd(), size, &qh, ());
        let mut offset = 0;
        let buffers: Vec<_> = tiles
            .images
            .iter()
            .map(|(w, h, pixels)| {
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
        let [left, top, right, bottom] =
            tiles.extents.map(|v| f64::from(v) / f64::from(settings.1));
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
        let settings = (
            ui.style().visuals.window_shadow,
            ui.input(|i| i.viewport().native_pixels_per_point.unwrap_or(1.0))
                .clamp(0.5, 8.0),
        );
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gaussian_tiles_are_scaled_premultiplied_and_seamless() {
        let shadow = egui::epaint::Shadow {
            offset: [0, 4],
            blur: 24,
            spread: 0,
            color: egui::Color32::from_black_alpha(90),
        };
        let a = tiles(shadow, 1.0);
        let b = tiles(shadow, 2.0);
        assert_eq!(a.extents, [24, 20, 24, 28]);
        assert_eq!(b.extents, [48, 40, 48, 56]);
        assert_eq!(a.images.len(), 8);
        for (w, h, bytes) in &a.images {
            assert_eq!(bytes.len(), (w * h * 4) as usize);
            for pixel in bytes.chunks_exact(4) {
                let argb = u32::from_ne_bytes(pixel.try_into().unwrap());
                assert_eq!(argb & 0x00ffffff, 0);
                assert!(argb >> 24 <= 90);
            }
        }
        let alpha = |i: usize, x: i32, y: i32| {
            let (w, _, bytes) = &a.images[i];
            u32::from_ne_bytes(bytes[((y * w + x) * 4) as usize..][..4].try_into().unwrap()) >> 24
        };
        assert!(alpha(0, 0, 0) < alpha(0, 0, 19));
        assert!(alpha(4, 0, 27) < alpha(4, 0, 0));
        // At each corner/edge seam the extra axis is at the window boundary.
        assert!(alpha(0, 0, 10).abs_diff(alpha(1, 0, 10)) <= 1);
        assert!(alpha(4, 0, 10).abs_diff(alpha(3, 0, 10)) <= 1);
        for (corner, edge, row) in [
            (1, 2, a.images[1].1 - 1),
            (3, 2, 0),
            (5, 6, 0),
            (7, 6, a.images[7].1 - 1),
        ] {
            for x in 0..a.images[corner].0 {
                assert_eq!(alpha(corner, x, row), alpha(edge, x, 0));
            }
        }
        let clear = tiles(egui::epaint::Shadow::NONE, 1.0);
        assert!(
            clear
                .images
                .iter()
                .all(|(_, _, bytes)| bytes.iter().all(|b| *b == 0))
        );
    }
}
