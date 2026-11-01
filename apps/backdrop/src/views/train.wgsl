// SPDX-License-Identifier: MIT OR Apache-2.0
struct Vertex { @builtin(position) position: vec4<f32>, @location(0) colour: vec3<f32> };
@vertex fn vs(@location(0) position: vec2<f32>, @location(1) colour: vec3<f32>) -> Vertex {
    var out: Vertex;
    out.position = vec4<f32>(position, 0.0, 1.0);
    out.colour = colour;
    return out;
}
@fragment fn fs(in: Vertex) -> @location(0) vec4<f32> { return vec4<f32>(in.colour, 1.0); }
