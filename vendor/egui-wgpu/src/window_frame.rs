// SPDX-License-Identifier: MIT OR Apache-2.0
//! Final border/corner pass on the resolved target, after all egui panels.
pub(crate) struct WindowFrame {
    pub format: wgpu::TextureFormat,
    uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    mask: wgpu::RenderPipeline,
    border: wgpu::RenderPipeline,
}

impl WindowFrame {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("window-frame"),
            source: wgpu::ShaderSource::Wgsl(include_str!("window_frame.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("window-frame"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("window-frame"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("window-frame"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("window-frame"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry, blend| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(entry),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(blend),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let multiply = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Zero,
            dst_factor: wgpu::BlendFactor::SrcAlpha,
            operation: wgpu::BlendOperation::Add,
        };
        let mask = pipeline(
            "mask",
            wgpu::BlendState {
                color: multiply,
                alpha: multiply,
            },
        );
        let border = pipeline(
            if format.is_srgb() {
                "border_linear"
            } else {
                "border_gamma"
            },
            wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING,
        );
        Self {
            format,
            uniform,
            bind_group,
            mask,
            border,
        }
    }

    pub fn paint(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        size: [u32; 2],
        request: (f32, f32, [f32; 4]),
    ) {
        let [w, h] = size;
        if w == 0 || h == 0 {
            return;
        }
        let (radius, width, color) = request;
        let radius = radius.max(0.0).min(w as f32 / 2.0).min(h as f32 / 2.0);
        let data = [
            w as f32, h as f32, radius, width, color[0], color[1], color[2], color[3],
        ];
        queue.write_buffer(&self.uniform, 0, bytemuck::cast_slice(&data));
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("window-frame"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_bind_group(0, &self.bind_group, &[]);
        if width > 0.0 && color[3] > 0.0 {
            pass.set_pipeline(&self.border);
            let strip = (radius.max(width) + 1.0).ceil() as u32;
            let y = strip.min(h / 2);
            let x = (width + 1.0).ceil() as u32;
            for (left, top, wide, high) in [
                (0, 0, w, y),
                (0, h - y, w, y),
                (0, y, x.min(w), h - 2 * y),
                (w - x.min(w), y, x.min(w), h - 2 * y),
            ] {
                if wide > 0 && high > 0 {
                    pass.set_scissor_rect(left, top, wide, high);
                    pass.draw(0..3, 0..1);
                }
            }
        }
        if radius > 0.0 {
            pass.set_pipeline(&self.mask);
            let c = (radius.ceil() as u32).max(1).min(w).min(h);
            // Only the four corner squares touch destination pixels. The
            // rest of the frame never passes through a full-screen filter.
            for (x, y) in [(0, 0), (w - c, 0), (w - c, h - c), (0, h - c)] {
                pass.set_scissor_rect(x, y, c, c);
                pass.draw(0..3, 0..1);
            }
        }
    }
}
