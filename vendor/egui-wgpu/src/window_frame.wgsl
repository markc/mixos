// SPDX-License-Identifier: MIT OR Apache-2.0
struct Frame {
    // Physical width, height, radius, border width.
    geometry: vec4<f32>,
    color: vec4<f32>,
};
@group(0) @binding(0) var<uniform> frame: Frame;

@vertex fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let vertices = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    return vec4(vertices[index], 0.0, 1.0);
}

fn distance(p: vec2<f32>) -> f32 {
    let half_size = frame.geometry.xy * 0.5;
    let radius = min(frame.geometry.z, min(half_size.x, half_size.y));
    let q = abs(p - half_size) - half_size + vec2(radius);
    return length(max(q, vec2(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

@fragment fn mask(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let coverage = clamp(0.5 - distance(p.xy), 0.0, 1.0);
    return vec4(0.0, 0.0, 0.0, coverage);
}

fn border_alpha(p: vec2<f32>) -> f32 {
    // The outer coverage is applied once by the subsequent corner mask.
    return frame.color.a * (1.0 - clamp(0.5 - distance(p) - frame.geometry.w, 0.0, 1.0));
}
@fragment fn border_gamma(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let a = border_alpha(p.xy);
    return vec4(frame.color.rgb * a, a);
}
@fragment fn border_linear(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let a = border_alpha(p.xy);
    let rgb = select(pow((frame.color.rgb + vec3(0.055)) / 1.055, vec3(2.4)),
                     frame.color.rgb / 12.92, frame.color.rgb <= vec3(0.04045));
    return vec4(rgb * a, a);
}
