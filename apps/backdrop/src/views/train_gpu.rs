// SPDX-License-Identifier: MIT OR Apache-2.0
//! Small orthographic 3D scene, three shaded faces per carriage/station.
//! No renderer dependency beyond the layer's existing wgpu device.
use super::train::Block;
use wgpu::util::DeviceExt;

fn project([x,y,z]: [f32;3]) -> [f32;2] { [x * 0.085 + z * 0.035, -0.1 + y * 0.14 - z * 0.065] }
struct Callback { bytes: Vec<u8>, count:u32 }
struct Gpu { pipeline: wgpu::RenderPipeline, vertices: wgpu::Buffer }

pub fn draw(ui: &egui::Ui, blocks: &[Block], progress: f32) {
    let rect = ui.max_rect();
    let palette = [ui.visuals().selection.bg_fill, ui.visuals().warn_fg_color,
        ui.visuals().hyperlink_color, ui.visuals().error_fg_color];
    let mut bytes = Vec::new();
    for b in blocks {
        let [mut x,y,z] = b.position;
        if !b.label.is_empty() && b.size[1] < 1. { x -= (1.-progress)*0.4; }
        let [w,h,d] = b.size.map(|v| v*0.5);
        let corners = [[x-w,y-h,z-d],[x+w,y-h,z-d],[x+w,y+h,z-d],[x-w,y+h,z-d],
            [x-w,y-h,z+d],[x+w,y-h,z+d],[x+w,y+h,z+d],[x-w,y+h,z+d]];
        let base = palette[b.tone].to_normalized_gamma_f32();
        for (face, shade) in [([4,5,6,7], 0.65), ([1,5,6,2], 0.8), ([3,2,6,7], 1.)] {
            for i in [0,1,2,0,2,3] {
                let p = project(corners[face[i]]);
                for v in [p[0],p[1],base[0]*(shade+b.glow*0.25),base[1]*(shade+b.glow*0.25),base[2]*(shade+b.glow*0.25)] { bytes.extend_from_slice(&v.to_ne_bytes()); }
            }
        }
    }
    let count = (bytes.len()/20) as u32;
    ui.painter().add(egui_wgpu::Callback::new_paint_callback(rect, Callback {bytes, count}));
    for b in blocks.iter().filter(|b| !b.label.is_empty()) {
        let p = project([b.position[0],b.position[1]+b.size[1],b.position[2]]);
        let pos = egui::pos2(rect.center().x+p[0]*rect.width()*0.5,rect.center().y-p[1]*rect.height()*0.5);
        ui.painter().text(pos, egui::Align2::CENTER_BOTTOM, &b.label, egui::TextStyle::Small.resolve(ui.style()), ui.visuals().text_color());
    }
}
impl egui_wgpu::CallbackTrait for Callback {
    fn prepare(&self, device:&wgpu::Device, _queue:&wgpu::Queue, _screen:&egui_wgpu::ScreenDescriptor,
        _encoder:&mut wgpu::CommandEncoder, resources:&mut egui_wgpu::CallbackResources) -> Vec<wgpu::CommandBuffer> {
        if resources.get::<Gpu>().is_none() {
            let Some(format) = resources.get::<wgpu::TextureFormat>().copied() else { return vec![] };
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label:Some("backdrop train"), source:wgpu::ShaderSource::Wgsl(include_str!("train.wgsl").into()) });
            let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label:Some("backdrop train"), layout:None,
                vertex:wgpu::VertexState {module:&shader, entry_point:Some("vs"), compilation_options:Default::default(),
                    buffers:&[Some(wgpu::VertexBufferLayout {array_stride:20,step_mode:wgpu::VertexStepMode::Vertex,
                        attributes:&wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x3]})]},
                fragment:Some(wgpu::FragmentState {module:&shader, entry_point:Some("fs"),compilation_options:Default::default(),
                    targets:&[Some(wgpu::ColorTargetState {format,blend:None,write_mask:wgpu::ColorWrites::ALL})]}),
                primitive:Default::default(),depth_stencil:None,multisample:Default::default(),multiview_mask:None,cache:None,
            });
            resources.insert(Gpu {pipeline, vertices:device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label:Some("train vertices"),contents:&self.bytes,usage:wgpu::BufferUsages::VERTEX})});
        } else if let Some(gpu) = resources.get_mut::<Gpu>() {
            gpu.vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label:Some("train vertices"), contents:&self.bytes,usage:wgpu::BufferUsages::VERTEX });
        }
        vec![]
    }
    fn paint(&self, _info:egui::PaintCallbackInfo, pass:&mut wgpu::RenderPass<'static>, resources:&egui_wgpu::CallbackResources) {
        if let Some(gpu) = resources.get::<Gpu>() { pass.set_pipeline(&gpu.pipeline); pass.set_vertex_buffer(0,gpu.vertices.slice(..)); pass.draw(0..self.count,0..1); }
    }
}
