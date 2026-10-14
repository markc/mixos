// SPDX-License-Identifier: MIT OR Apache-2.0
// Term's grid: one background quad sampling a cols x rows colour texture,
// instanced glyph quads reading the atlas texel-exactly, and cursor rects.
// Every position is in physical framebuffer pixels; the viewport is the
// whole framebuffer and `origin` is the pane's top-left, a whole pixel.

struct Pane {
    origin: vec2<f32>,
    screen: vec2<f32>,
    cell: vec2<f32>,
    grid: vec2<f32>,
};

@group(0) @binding(0) var<uniform> pane: Pane;
@group(0) @binding(1) var backgrounds: texture_2d<f32>;
@group(1) @binding(0) var atlas: texture_2d<f32>;

fn clip_position(px: vec2<f32>) -> vec4<f32> {
    let p = (pane.origin + px) / pane.screen;
    return vec4<f32>(p.x * 2.0 - 1.0, 1.0 - p.y * 2.0, 0.0, 1.0);
}

// Two triangles: 0 1 2 / 2 1 3 as unit-square corners.
fn corner(i: u32) -> vec2<f32> {
    let x = select(0.0, 1.0, i == 1u || i == 4u || i == 5u);
    let y = select(0.0, 1.0, i == 2u || i == 3u || i == 5u);
    return vec2<f32>(x, y);
}

// The pane-local pixel a fragment covers.
fn local_pixel(position: vec4<f32>) -> vec2<i32> {
    return vec2<i32>(floor(position.xy - pane.origin));
}

@vertex
fn vs_background(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    return clip_position(corner(i) * pane.cell * pane.grid);
}

@fragment
fn fs_background(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let cell = local_pixel(position) / vec2<i32>(pane.cell);
    return vec4<f32>(textureLoad(backgrounds, cell, 0).rgb, 1.0);
}

struct GlyphOut {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) source: vec2<i32>,
    @location(1) @interpolate(flat) at: vec2<i32>,
    @location(2) @interpolate(flat) fg: vec4<f32>,
};

@vertex
fn vs_glyph(
    @builtin(vertex_index) i: u32,
    @location(0) at: vec2<f32>,
    @location(1) source: vec4<u32>,
    @location(2) fg: vec4<f32>,
) -> GlyphOut {
    var out: GlyphOut;
    out.position = clip_position(at + corner(i) * vec2<f32>(source.zw));
    out.source = vec2<i32>(source.xy);
    out.at = vec2<i32>(at);
    out.fg = fg;
    return out;
}

// fg.a is the layer kind: 1 = coverage mask in the foreground colour,
// 0 = premultiplied colour. Both blend One / OneMinusSrcAlpha.
@fragment
fn fs_glyph(in: GlyphOut) -> @location(0) vec4<f32> {
    let texel = textureLoad(atlas, in.source + local_pixel(in.position) - in.at, 0);
    if in.fg.a > 0.5 {
        return vec4<f32>(in.fg.rgb * texel.a, texel.a);
    }
    return texel;
}

struct RectOut {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) colour: vec4<f32>,
};

@vertex
fn vs_rect(
    @builtin(vertex_index) i: u32,
    @location(0) at: vec2<f32>,
    @location(1) size: vec2<f32>,
    @location(2) colour: vec4<f32>,
) -> RectOut {
    var out: RectOut;
    out.position = clip_position(at + corner(i) * size);
    out.colour = colour;
    return out;
}

@fragment
fn fs_rect(in: RectOut) -> @location(0) vec4<f32> {
    return in.colour;
}
