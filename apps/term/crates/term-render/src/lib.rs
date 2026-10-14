// SPDX-License-Identifier: MIT OR Apache-2.0
//! Term's grid on the GPU.
//!
//! The CPU raster in `term-core` stays the authority on what a glyph looks
//! like: [`term_core::raster::Raster::glyph_layers`] rasterises each distinct
//! glyph once, and the [`atlas`] keeps it in a texture. Each frame a pane
//! draws, inside egui's own render pass:
//!
//! 1. one quad of cell backgrounds, read texel-exactly from a `cols x rows`
//!    colour texture;
//! 2. one instanced quad per glyph layer, reading the atlas texel-exactly and
//!    blending exactly as the CPU painter does (`One, OneMinusSrcAlpha` over
//!    premultiplied sources);
//! 3. the cursor: a block inverts what is under it, an underline is a solid
//!    bottom row.
//!
//! Each frame runs in two phases over all panes: every pane first reserves
//! its glyphs (the atlas may empty itself only then), then every pane builds
//! its instances, so no pane draws against an atlas emptied after it built.
//! Only rows the terminal reports dirty are rebuilt; a glyph already in the
//! atlas costs one 20-byte instance. The CPU does no pixel work after a
//! glyph's first use.
//!
//! Wiring: call [`install`] once with eframe's `RenderState`, keep one
//! [`Pane`] per terminal pane, hand it each new snapshot with
//! [`Pane::update`], and add [`Pane::callback`] to the painter where the grid
//! goes. The rect must start on a whole physical pixel.
mod atlas;

use atlas::{Atlas, Slot};
use std::sync::{Arc, Mutex};
use term_core::config::Cursor;
use term_core::raster::{GlyphKey, Raster};
use term_core::terminal::Screen;

/// What the callback needs to know about the frame it is drawn in.
const UNIFORM_SIZE: u64 = 32;
const GLYPH_STRIDE: u64 = 20;
const RECT_STRIDE: u64 = 20;
const UNDERLINE: [u8; 4] = [220, 220, 220, 255];

/// Shared GPU objects, kept in egui-wgpu's callback resources.
pub struct GridRenderer {
    pane_layout: wgpu::BindGroupLayout,
    atlas_group: wgpu::BindGroup,
    atlas: Atlas,
    background: wgpu::RenderPipeline,
    glyphs: wgpu::RenderPipeline,
    invert: wgpu::RenderPipeline,
    solid: wgpu::RenderPipeline,
}

/// Create the renderer and store it in `render_state`'s callback resources.
/// Call once, before the first frame that draws a [`Pane`].
pub fn install(render_state: &egui_wgpu::RenderState) {
    let renderer = GridRenderer::new(&render_state.device, render_state.target_format);
    render_state
        .renderer
        .write()
        .callback_resources
        .insert(renderer);
}

impl GridRenderer {
    pub fn new(device: &wgpu::Device, target: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("term grid"),
            source: wgpu::ShaderSource::Wgsl(include_str!("grid.wgsl").into()),
        });
        let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let pane_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("term pane"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(UNIFORM_SIZE),
                    },
                    count: None,
                },
                texture_entry(1),
            ],
        });
        let atlas_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("term atlas"),
            entries: &[texture_entry(0)],
        });
        let atlas = Atlas::new(device);
        let atlas_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("term atlas"),
            layout: &atlas_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&atlas.view),
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("term grid"),
            bind_group_layouts: &[Some(&pane_layout), Some(&atlas_layout)],
            immediate_size: 0,
        });
        let premultiplied = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        };
        // 1 - dst on colour, alpha untouched: the CPU's `255 - x`.
        let invert = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::OneMinusDst,
                dst_factor: wgpu::BlendFactor::Zero,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let pipeline = |label, vs, fs, buffers: &[Option<wgpu::VertexBufferLayout>], blend| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers,
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: target,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let glyph_layout = wgpu::VertexBufferLayout {
            array_stride: GLYPH_STRIDE,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Uint16x4, 2 => Unorm8x4],
        };
        let rect_layout = wgpu::VertexBufferLayout {
            array_stride: RECT_STRIDE,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Unorm8x4],
        };
        Self {
            background: pipeline(
                "term backgrounds",
                "vs_background",
                "fs_background",
                &[],
                None,
            ),
            glyphs: pipeline(
                "term glyphs",
                "vs_glyph",
                "fs_glyph",
                &[Some(glyph_layout.clone())],
                Some(premultiplied),
            ),
            invert: pipeline(
                "term cursor invert",
                "vs_rect",
                "fs_rect",
                &[Some(rect_layout.clone())],
                Some(invert),
            ),
            solid: pipeline(
                "term cursor solid",
                "vs_rect",
                "fs_rect",
                &[Some(rect_layout.clone())],
                None,
            ),
            pane_layout,
            atlas_group,
            atlas,
        }
    }
}

/// One pane's GPU objects, sized to its grid.
struct PaneGpu {
    uniform: wgpu::Buffer,
    backgrounds: wgpu::Texture,
    group: wgpu::BindGroup,
    grid: (u32, u32),
    glyphs: wgpu::Buffer,
    glyph_capacity: u64,
    cursor: wgpu::Buffer,
}

/// The CPU side of a pane: the instances it last built, per row.
#[derive(Default)]
struct PaneState {
    /// The latest snapshot not yet drawn, with the rows that changed since
    /// the one before it.
    pending: Option<(Screen, Vec<bool>)>,
    /// The snapshot the instances are built from, kept so an atlas or
    /// raster change can rebuild a pane that has no new output.
    current: Option<Screen>,
    /// Rows changed since the last build.
    owed: Vec<bool>,
    reserve_ns: u64,
    /// Atlas bytes this pane's reserve uploaded, reported with its build.
    reserve_bytes: usize,
    /// Test hook: empty the atlas at this pane's next reserve.
    reset_atlas: bool,
    gpu: Option<PaneGpu>,
    rows: Vec<Vec<u8>>,
    glyph_count: u32,
    cursor: Option<(Cursor, [f32; 4])>,
    built: Option<Built>,
    origin: [f32; 2],
    screen: [f32; 2],
    stats: Stats,
}

/// What a pane's instances were built against; any change rebuilds them all.
#[derive(Clone, PartialEq)]
struct Built {
    raster: Arc<()>,
    generation: u64,
    cols: usize,
    rows: usize,
}

/// Counters for one prepare, for the app's diagnostics and the tests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Rows whose instances were rebuilt.
    pub rows_built: usize,
    /// Glyph-layer instances drawn.
    pub glyphs: u32,
    /// Atlas bytes uploaded (new glyphs).
    pub atlas_bytes: usize,
    /// Instance and background bytes uploaded.
    pub instance_bytes: usize,
    /// CPU time of the prepare: snapshot to instances and uploads queued.
    pub prepare_ns: u64,
}

/// One terminal pane's handle: cheap to clone, shared with its paint
/// callbacks. Dropping the last handle frees its GPU objects.
#[derive(Clone, Default)]
pub struct Pane {
    state: Arc<Mutex<PaneState>>,
}

impl Pane {
    pub fn new() -> Self {
        Self::default()
    }

    /// Hand the pane a new snapshot. `dirty_rows` marks the rows that may
    /// differ from the previous snapshot (all true, or any other length,
    /// rebuilds every row). Snapshots that arrive between two frames merge.
    pub fn update(&self, screen: Screen, dirty_rows: Vec<bool>) {
        let mut state = self.state.lock().unwrap();
        let dirty = match state.pending.take() {
            Some((_, mut old)) if old.len() == dirty_rows.len() => {
                for (o, n) in old.iter_mut().zip(&dirty_rows) {
                    *o |= *n;
                }
                old
            }
            Some(_) => vec![true; dirty_rows.len()],
            None => dirty_rows,
        };
        state.pending = Some((screen, dirty));
    }

    /// Test hook: this pane's next frame empties the shared atlas during its
    /// reserve phase, as a full atlas would.
    #[doc(hidden)]
    pub fn reset_atlas_next_frame(&self) {
        self.state.lock().unwrap().reset_atlas = true;
    }

    /// The counters from this pane's last prepare.
    pub fn stats(&self) -> Stats {
        self.state.lock().unwrap().stats
    }

    /// A paint callback drawing the grid at `rect` (points). `raster` is the
    /// pane's font raster, shared by every pane of the window.
    pub fn callback(&self, rect: egui::Rect, raster: Arc<Mutex<Raster>>) -> egui::PaintCallback {
        egui_wgpu::Callback::new_paint_callback(
            rect,
            PaneCallback {
                pane: self.clone(),
                raster,
                rect,
            },
        )
    }
}

struct PaneCallback {
    pane: Pane,
    raster: Arc<Mutex<Raster>>,
    rect: egui::Rect,
}

fn grid_rows(screen: &Screen) -> usize {
    screen.rows.min(screen.cells.len() / screen.cols.max(1))
}

fn push_glyph(out: &mut Vec<u8>, at: [f32; 2], slot: &Slot, fg: [u8; 3]) {
    out.extend_from_slice(&at[0].to_le_bytes());
    out.extend_from_slice(&at[1].to_le_bytes());
    for v in slot.source {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&[fg[0], fg[1], fg[2], if slot.colour { 0 } else { 255 }]);
}

fn write_rect(at: [f32; 2], size: [f32; 2], colour: [u8; 4]) -> [u8; RECT_STRIDE as usize] {
    let mut out = [0; RECT_STRIDE as usize];
    out[0..4].copy_from_slice(&at[0].to_le_bytes());
    out[4..8].copy_from_slice(&at[1].to_le_bytes());
    out[8..12].copy_from_slice(&size[0].to_le_bytes());
    out[12..16].copy_from_slice(&size[1].to_le_bytes());
    out[16..20].copy_from_slice(&colour);
    out
}

impl PaneState {
    fn gpu(&mut self, device: &wgpu::Device, layout: &wgpu::BindGroupLayout, cols: u32, rows: u32) {
        if self.gpu.as_ref().is_some_and(|g| g.grid == (cols, rows)) {
            return;
        }
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("term pane uniform"),
            size: UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let backgrounds = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("term pane backgrounds"),
            size: wgpu::Extent3d {
                width: cols.max(1),
                height: rows.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = backgrounds.create_view(&wgpu::TextureViewDescriptor::default());
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("term pane"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
            ],
        });
        let (glyphs, glyph_capacity) = match self.gpu.take() {
            Some(old) => (old.glyphs, old.glyph_capacity),
            None => {
                let capacity = 4096 * GLYPH_STRIDE;
                (glyph_buffer(device, capacity), capacity)
            }
        };
        let cursor = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("term pane cursor"),
            size: RECT_STRIDE,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.gpu = Some(PaneGpu {
            uniform,
            backgrounds,
            group,
            grid: (cols, rows),
            glyphs,
            glyph_capacity,
            cursor,
        });
        // New textures hold nothing: every row rebuilds.
        self.built = None;
    }

    /// Phase one, egui-wgpu's `prepare`: take the newest snapshot and make
    /// sure the glyphs this pane will build are in the atlas. This is the
    /// only place the atlas may empty itself (when full); every pane builds
    /// its instances afterwards, in [`PaneState::build`], so no pane can be
    /// drawn against a generation that was reset after it built.
    fn reserve(&mut self, queue: &wgpu::Queue, renderer: &mut GridRenderer, raster: &mut Raster) {
        let started = std::time::Instant::now();
        renderer.atlas.bind(raster);
        if std::mem::take(&mut self.reset_atlas) {
            renderer.atlas.reset();
        }
        if let Some((screen, dirty)) = self.pending.take() {
            let rows = grid_rows(&screen);
            let same_shape = self
                .current
                .as_ref()
                .is_some_and(|c| c.cols == screen.cols && grid_rows(c) == rows);
            if same_shape && dirty.len() == screen.rows && self.owed.len() == rows {
                for (o, d) in self.owed.iter_mut().zip(&dirty) {
                    *o |= *d;
                }
            } else {
                self.owed = vec![true; rows];
            }
            self.current = Some(screen);
        }
        let Some(screen) = &self.current else {
            self.reserve_ns = started.elapsed().as_nanos() as u64;
            return;
        };
        let all = !self.is_current(renderer, raster);
        for attempt in 0..2 {
            let mut full = false;
            'rows: for row in 0..grid_rows(screen) {
                if !(all || attempt > 0 || self.owed.get(row).copied().unwrap_or(true)) {
                    continue;
                }
                let cells = &screen.cells[row * screen.cols..(row + 1) * screen.cols];
                for col in 0..screen.cols {
                    if let Some(key) = GlyphKey::for_cell(screen, cells, col)
                        && renderer.atlas.slots(queue, raster, &key).is_none()
                    {
                        full = true;
                        break 'rows;
                    }
                }
            }
            if !full || attempt > 0 {
                break;
            }
            // Full: empty it once and reserve this pane's whole screen. Every
            // other pane sees the new generation and rebuilds in phase two.
            renderer.atlas.reset();
        }
        self.reserve_ns = started.elapsed().as_nanos() as u64;
    }

    /// True when this pane's instances still match the atlas and raster.
    fn is_current(&self, renderer: &GridRenderer, raster: &Raster) -> bool {
        let Some(screen) = &self.current else {
            return false;
        };
        self.built.as_ref().is_some_and(|b| {
            b.generation == renderer.atlas.generation
                && raster.same_identity(&b.raster)
                && b.cols == screen.cols
                && b.rows == grid_rows(screen)
        })
    }

    /// Phase two, egui-wgpu's `finish_prepare`: rebuild owed rows (all of
    /// them when the atlas or raster changed) from the retained snapshot.
    /// The atlas only grows here; a glyph that no longer fits is skipped.
    #[allow(clippy::too_many_arguments)]
    fn build(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        renderer: &mut GridRenderer,
        raster: &mut Raster,
        origin: [f32; 2],
        screen_px: [f32; 2],
    ) {
        let started = std::time::Instant::now();
        let uploaded = renderer.atlas.uploaded;
        let mut stats = Stats::default();
        let (cell_w, cell_h) = raster.cell();
        let Some(screen) = self.current.take() else {
            self.write_uniform(queue, origin, screen_px, (cell_w, cell_h));
            stats.prepare_ns = self.reserve_ns + started.elapsed().as_nanos() as u64;
            self.stats = stats;
            return;
        };
        let cols = screen.cols;
        let rows = grid_rows(&screen);
        self.gpu(device, &renderer.pane_layout, cols as u32, rows as u32);
        self.current = Some(screen);
        let full = !self.is_current(renderer, raster);
        let screen = self.current.take().unwrap();
        let rebuild: Vec<bool> = if full || self.owed.len() != rows {
            vec![true; rows]
        } else {
            std::mem::take(&mut self.owed)
        };
        self.owed = vec![false; rows];
        self.rows.resize_with(rows, Vec::new);
        for row in (0..rows).filter(|&r| rebuild[r]) {
            let cells = &screen.cells[row * cols..(row + 1) * cols];
            let out = &mut self.rows[row];
            out.clear();
            for col in 0..cols {
                let Some(key) = GlyphKey::for_cell(&screen, cells, col) else {
                    continue;
                };
                let Some(slots) = renderer.atlas.slots(queue, raster, &key) else {
                    // More distinct glyphs on screen than the atlas holds.
                    continue;
                };
                let x = (col as u32 * cell_w) as f32;
                let y = (row as u32 * cell_h) as f32;
                for slot in slots {
                    let at = [x + f32::from(slot.offset[0]), y + f32::from(slot.offset[1])];
                    push_glyph(out, at, slot, cells[col].fg);
                }
            }
            stats.rows_built += 1;
        }
        let gpu = self.gpu.as_mut().unwrap();
        // Backgrounds: one texel per cell, rewritten for rebuilt rows.
        for row in (0..rows).filter(|&r| rebuild[r]) {
            let texels: Vec<u8> = screen.cells[row * cols..(row + 1) * cols]
                .iter()
                .flat_map(|c| [c.bg[0], c.bg[1], c.bg[2], 255])
                .collect();
            stats.instance_bytes += texels.len();
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &gpu.backgrounds,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: row as u32,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &texels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(cols as u32 * 4),
                    rows_per_image: Some(1),
                },
                wgpu::Extent3d {
                    width: cols as u32,
                    height: 1,
                    depth_or_array_layers: 1,
                },
            );
        }
        if rebuild.iter().any(|r| *r) {
            let bytes: Vec<u8> = self.rows.iter().flatten().copied().collect();
            if bytes.len() as u64 > gpu.glyph_capacity {
                gpu.glyph_capacity = (bytes.len() as u64).next_power_of_two();
                gpu.glyphs = glyph_buffer(device, gpu.glyph_capacity);
            }
            if !bytes.is_empty() {
                queue.write_buffer(&gpu.glyphs, 0, &bytes);
            }
            stats.instance_bytes += bytes.len();
            self.glyph_count = (bytes.len() as u64 / GLYPH_STRIDE) as u32;
        }
        // The cursor: the cell (or wide pair) it sits on.
        self.cursor = None;
        if screen.cursor_visible && screen.cursor.0 < cols && screen.cursor.1 < rows {
            let (mut col, row) = screen.cursor;
            let cells = &screen.cells[row * cols..(row + 1) * cols];
            use term_core::terminal::CellWidth;
            if col > 0
                && cells[col].width == CellWidth::Spacer
                && cells[col - 1].width == CellWidth::Wide
            {
                col -= 1;
            }
            let span = if cells[col].width == CellWidth::Wide
                && cells
                    .get(col + 1)
                    .is_some_and(|c| c.width == CellWidth::Spacer)
            {
                2
            } else {
                1
            };
            let x = (col as u32 * cell_w) as f32;
            let width = (span * cell_w) as f32;
            let top = (row as u32 * cell_h) as f32;
            let shape = raster.cursor();
            let rect = match shape {
                Cursor::Block => [x, top, width, cell_h as f32],
                Cursor::Underline => [x, top + cell_h as f32 - 1.0, width, 1.0],
                Cursor::Bar => [x, top, raster.bar_width() as f32, cell_h as f32],
            };
            let colour = match shape {
                Cursor::Block => [255; 4],
                Cursor::Underline | Cursor::Bar => UNDERLINE,
            };
            queue.write_buffer(
                &gpu.cursor,
                0,
                &write_rect([rect[0], rect[1]], [rect[2], rect[3]], colour),
            );
            self.cursor = Some((shape, rect));
        }
        self.built = Some(Built {
            raster: raster.identity(),
            generation: renderer.atlas.generation,
            cols,
            rows,
        });
        self.current = Some(screen);
        stats.glyphs = self.glyph_count;
        stats.atlas_bytes =
            std::mem::take(&mut self.reserve_bytes) + renderer.atlas.uploaded - uploaded;
        self.write_uniform(queue, origin, screen_px, (cell_w, cell_h));
        stats.prepare_ns = self.reserve_ns + started.elapsed().as_nanos() as u64;
        self.stats = stats;
    }

    fn write_uniform(
        &mut self,
        queue: &wgpu::Queue,
        origin: [f32; 2],
        screen_px: [f32; 2],
        cell: (u32, u32),
    ) {
        self.origin = origin;
        self.screen = screen_px;
        let Some(gpu) = &self.gpu else { return };
        let values = [
            origin[0],
            origin[1],
            screen_px[0],
            screen_px[1],
            cell.0 as f32,
            cell.1 as f32,
            gpu.grid.0 as f32,
            gpu.grid.1 as f32,
        ];
        let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        queue.write_buffer(&gpu.uniform, 0, &bytes);
    }
}

fn glyph_buffer(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("term pane glyphs"),
        size,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

impl PaneCallback {
    /// The callback rect's origin, a whole physical pixel by contract.
    fn origin(&self, ppp: f32) -> [f32; 2] {
        [
            (self.rect.min.x * ppp).round(),
            (self.rect.min.y * ppp).round(),
        ]
    }
}

impl egui_wgpu::CallbackTrait for PaneCallback {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(renderer) = resources.get_mut::<GridRenderer>() else {
            return Vec::new();
        };
        let mut state = self.pane.state.lock().unwrap();
        let mut raster = self.raster.lock().unwrap();
        let before = renderer.atlas.uploaded;
        state.reserve(queue, renderer, &mut raster);
        state.reserve_bytes = renderer.atlas.uploaded - before;
        state.origin = self.origin(screen.pixels_per_point);
        state.screen = [
            screen.size_in_pixels[0] as f32,
            screen.size_in_pixels[1] as f32,
        ];
        Vec::new()
    }

    fn finish_prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(renderer) = resources.get_mut::<GridRenderer>() else {
            return Vec::new();
        };
        let mut state = self.pane.state.lock().unwrap();
        let mut raster = self.raster.lock().unwrap();
        let (origin, screen) = (state.origin, state.screen);
        state.build(device, queue, renderer, &mut raster, origin, screen);
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(renderer) = resources.get::<GridRenderer>() else {
            return;
        };
        let state = self.pane.state.lock().unwrap();
        let screen = state.screen;
        let Some(gpu) = &state.gpu else { return };
        if state.built.is_none() {
            return;
        }
        pass.set_viewport(0.0, 0.0, screen[0], screen[1], 0.0, 1.0);
        pass.set_bind_group(0, &gpu.group, &[]);
        pass.set_bind_group(1, &renderer.atlas_group, &[]);
        pass.set_pipeline(&renderer.background);
        pass.draw(0..6, 0..1);
        if state.glyph_count > 0 {
            pass.set_pipeline(&renderer.glyphs);
            pass.set_vertex_buffer(
                0,
                gpu.glyphs
                    .slice(..u64::from(state.glyph_count) * GLYPH_STRIDE),
            );
            pass.draw(0..6, 0..state.glyph_count);
        }
        if let Some((shape, _)) = state.cursor {
            pass.set_pipeline(match shape {
                Cursor::Block => &renderer.invert,
                Cursor::Underline | Cursor::Bar => &renderer.solid,
            });
            pass.set_vertex_buffer(0, gpu.cursor.slice(..));
            pass.draw(0..6, 0..1);
        }
    }
}
