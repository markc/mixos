// SPDX-License-Identifier: MIT OR Apache-2.0
//! ES3 instanced cubes: one colour and one depth target per output, no MSAA.
//! The callback records into egui's encoder; the layer submits once.
use super::train::Block;
use crate::program::{Budget, Reservation, lock};
use std::sync::{Arc, Mutex};
use wgpu::util::DeviceExt;

pub const MAX_CUBES: usize = 256;
const INSTANCE_BYTES: u64 = MAX_CUBES as u64 * 36;
const VERTEX_BYTES: u64 = 36 * 16;
pub fn allocation_bytes(size: [u32; 2]) -> u64 {
    u64::from(size[0]) * u64::from(size[1]) * 8 + INSTANCE_BYTES + VERTEX_BYTES
}
#[derive(Default)]
pub struct Cache {
    inner: Arc<Mutex<Option<Gpu>>>,
}
struct Callback {
    cache: Arc<Mutex<Option<Gpu>>>,
    bytes: Vec<u8>,
    size: [u32; 2],
    reservation: Arc<Reservation>,
}
struct Gpu {
    size: [u32; 2],
    reservation: Arc<Reservation>,
    pipeline: wgpu::RenderPipeline,
    blit: wgpu::RenderPipeline,
    vertices: wgpu::Buffer,
    instances: wgpu::Buffer,
    colour: wgpu::TextureView,
    depth: wgpu::TextureView,
    group: wgpu::BindGroup,
}
fn project([x, y, z]: [f32; 3]) -> [f32; 2] {
    [x * 0.085 + z * 0.035, -0.1 + y * 0.14 - z * 0.065]
}
fn animated_position(b: &Block, progress: f32) -> [f32; 3] {
    let mut p = b.position;
    if !b.label.is_empty() && b.size[1] > 0.2 && b.size[1] < 1. {
        p[0] -= (1. - progress) * 0.4;
    }
    p
}
impl Cache {
    pub fn draw(
        &mut self,
        ui: &egui::Ui,
        blocks: &[Block],
        progress: f32,
        budget: &Arc<Budget>,
    ) -> bool {
        let rect = ui.max_rect();
        let scale = (1280. / rect.width().max(1.))
            .min(720. / rect.height().max(1.))
            .min(ui.ctx().pixels_per_point());
        let size = [
            (rect.width() * scale).round().max(1.) as u32,
            (rect.height() * scale).round().max(1.) as u32,
        ];
        let reservation = {
            let mut gpu = lock(&self.inner);
            if let Some(gpu) = gpu.as_ref().filter(|g| g.size == size) {
                gpu.reservation.clone()
            } else if let Some(r) = budget.reserve(allocation_bytes(size)) {
                Arc::new(r)
            } else {
                *gpu = None;
                return false;
            }
        };
        let palette = [
            ui.visuals().selection.bg_fill,
            ui.visuals().warn_fg_color,
            ui.visuals().hyperlink_color,
            ui.visuals().error_fg_color,
        ];
        let mut bytes = Vec::with_capacity(blocks.len().min(MAX_CUBES) * 36);
        for b in blocks.iter().take(MAX_CUBES) {
            let p = animated_position(b, progress);
            let c = palette[b.tone].to_normalized_gamma_f32();
            for v in [
                p[0],
                p[1],
                p[2],
                b.size[0],
                b.size[1],
                b.size[2],
                c[0] * (1. + b.glow * 0.25),
                c[1] * (1. + b.glow * 0.25),
                c[2] * (1. + b.glow * 0.25),
            ] {
                bytes.extend_from_slice(&v.to_ne_bytes());
            }
        }
        ui.painter().add(egui_wgpu::Callback::new_paint_callback(
            rect,
            Callback {
                cache: self.inner.clone(),
                bytes,
                size,
                reservation,
            },
        ));
        for b in blocks
            .iter()
            .take(MAX_CUBES)
            .filter(|b| !b.label.is_empty())
        {
            let p = animated_position(b, progress);
            let p = project([p[0], p[1] + b.size[1], p[2]]);
            ui.painter().text(
                egui::pos2(
                    rect.center().x + p[0] * rect.width() * 0.5,
                    rect.center().y - p[1] * rect.height() * 0.5,
                ),
                egui::Align2::CENTER_BOTTOM,
                &b.label,
                egui::TextStyle::Small.resolve(ui.style()),
                if b.tone == 3 {
                    ui.visuals().error_fg_color
                } else {
                    ui.visuals().text_color()
                },
            );
        }
        true
    }
}
impl Gpu {
    fn new(
        device: &wgpu::Device,
        target: wgpu::TextureFormat,
        size: [u32; 2],
        reservation: Arc<Reservation>,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("train cubes"),
            source: wgpu::ShaderSource::Wgsl(include_str!("train.wgsl").into()),
        });
        let pipeline=device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label:Some("train cubes"),layout:None,
            vertex:wgpu::VertexState {module:&shader,entry_point:Some("vs"),compilation_options:Default::default(),buffers:&[
                Some(wgpu::VertexBufferLayout {array_stride:16,step_mode:wgpu::VertexStepMode::Vertex,attributes:&wgpu::vertex_attr_array![0=>Float32x3,1=>Float32]}),
                Some(wgpu::VertexBufferLayout {array_stride:36,step_mode:wgpu::VertexStepMode::Instance,attributes:&wgpu::vertex_attr_array![2=>Float32x3,3=>Float32x3,4=>Float32x3]})]},
            fragment:Some(wgpu::FragmentState {module:&shader,entry_point:Some("fs"),compilation_options:Default::default(),targets:&[Some(wgpu::ColorTargetState {format:wgpu::TextureFormat::Rgba8Unorm,blend:None,write_mask:wgpu::ColorWrites::ALL})]}),
            primitive:Default::default(),depth_stencil:Some(wgpu::DepthStencilState {format:wgpu::TextureFormat::Depth32Float,depth_write_enabled:Some(true),depth_compare:Some(wgpu::CompareFunction::Less),stencil:Default::default(),bias:Default::default()}),
            multisample:Default::default(),multiview_mask:None,cache:None,
        });
        let blit = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("train composite"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("blit_vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("blit_fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let texture = |format, usage| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("train target"),
                    size: wgpu::Extent3d {
                        width: size[0],
                        height: size[1],
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let colour = texture(
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let depth = texture(
            wgpu::TextureFormat::Depth32Float,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor::default());
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("train composite"),
            layout: &blit.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&colour),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        let corners = [
            [-0.5, -0.5, -0.5],
            [0.5, -0.5, -0.5],
            [0.5, 0.5, -0.5],
            [-0.5, 0.5, -0.5],
            [-0.5, -0.5, 0.5],
            [0.5, -0.5, 0.5],
            [0.5, 0.5, 0.5],
            [-0.5, 0.5, 0.5],
        ];
        let mut data = Vec::new();
        for (face, shade) in [
            ([0, 1, 2, 3], 0.7),
            ([4, 5, 6, 7], 0.65),
            ([1, 5, 6, 2], 0.8),
            ([0, 4, 7, 3], 0.75),
            ([3, 2, 6, 7], 1.),
            ([0, 1, 5, 4], 0.6),
        ] {
            for i in [0, 1, 2, 0, 2, 3] {
                for v in corners[face[i]].into_iter().chain([shade]) {
                    data.extend_from_slice(&f32::to_ne_bytes(v));
                }
            }
        }
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("train cube"),
            contents: &data,
            usage: wgpu::BufferUsages::VERTEX,
        });
        let instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("train instances"),
            size: INSTANCE_BYTES,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            size,
            reservation,
            pipeline,
            blit,
            vertices,
            instances,
            colour,
            depth,
            group,
        }
    }
}
impl egui_wgpu::CallbackTrait for Callback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(format) = resources.get::<wgpu::TextureFormat>().copied() else {
            return vec![];
        };
        let mut cache = lock(&self.cache);
        if cache.as_ref().is_none_or(|g| g.size != self.size) {
            *cache = Some(Gpu::new(
                device,
                format,
                self.size,
                self.reservation.clone(),
            ));
        }
        let gpu = cache.as_ref().unwrap();
        if !self.bytes.is_empty() {
            queue.write_buffer(&gpu.instances, 0, &self.bytes);
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("train depth pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &gpu.colour,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &gpu.depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        pass.set_pipeline(&gpu.pipeline);
        pass.set_vertex_buffer(0, gpu.vertices.slice(..));
        pass.set_vertex_buffer(1, gpu.instances.slice(..));
        pass.draw(0..36, 0..(self.bytes.len() / 36) as u32);
        vec![]
    }
    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        _: &egui_wgpu::CallbackResources,
    ) {
        if let Some(gpu) = lock(&self.cache).as_ref() {
            pass.set_pipeline(&gpu.blit);
            pass.set_bind_group(0, &gpu.group, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn frame(
        cache: &mut Cache,
        budget: &Arc<Budget>,
        width: f32,
        progress: f32,
    ) -> (egui::FullOutput, bool) {
        let ctx = egui::Context::default();
        let mut accepted = false;
        let block = Block {
            position: [0., 0., 0.],
            size: [0.48, 0.6, 0.75],
            tone: 0,
            glow: 0.,
            label: "work/a".into(),
        };
        let mut out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 480.),
                )),
                ..Default::default()
            },
            |ui| accepted = cache.draw(ui, std::slice::from_ref(&block), progress, budget),
        );
        out.textures_delta.clear();
        (out, accepted)
    }
    #[test]
    fn scene_targets_and_both_buffers_are_charged_to_wallpaper_budget() {
        let budget = Budget::new(32 * 1024 * 1024);
        let mut cache = Cache::default();
        let (a, accepted) = frame(&mut cache, &budget, 640., 0.);
        assert!(accepted);
        let first = budget.used();
        assert!(first > 640 * 480 * 8);
        let (b, accepted) = frame(&mut cache, &budget, 800., 1.);
        assert!(accepted);
        let both = budget.used();
        assert!(both > first);
        drop(cache);
        assert_eq!(
            budget.used(),
            both,
            "queued callbacks retain charges after release_gpu"
        );
        drop(a);
        assert_eq!(budget.used(), both - first);
        drop(b);
        assert_eq!(budget.used(), 0);
        let tiny = Budget::new(1);
        let (refused, accepted) = frame(&mut Cache::default(), &tiny, 640., 0.);
        assert!(
            !accepted,
            "the host selects 2D when Cache::draw refuses allocation"
        );
        assert_eq!(tiny.used(), 0);
        drop(refused);
    }
    #[test]
    fn painted_labels_follow_the_animated_cube_position() {
        let budget = Budget::new(32 * 1024 * 1024);
        let mut cache = Cache::default();
        let (a, _) = frame(&mut cache, &budget, 640., 0.);
        let (b, _) = frame(&mut cache, &budget, 640., 1.);
        let text_x = |out: &egui::FullOutput| {
            out.shapes
                .iter()
                .find_map(|s| match &s.shape {
                    egui::epaint::Shape::Text(t) if t.galley.job.text == "work/a" => Some(t.pos.x),
                    _ => None,
                })
                .unwrap()
        };
        assert!((text_x(&b) - text_x(&a) - 0.4 * 0.085 * 640. * 0.5).abs() < 0.01);
    }
}
