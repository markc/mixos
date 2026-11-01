// SPDX-License-Identifier: MIT OR Apache-2.0
struct Vertex { @builtin(position) position:vec4<f32>, @location(0) colour:vec3<f32> };
@vertex fn vs(@location(0) corner:vec3<f32>, @location(1) shade:f32,
    @location(2) centre:vec3<f32>, @location(3) size:vec3<f32>, @location(4) colour:vec3<f32>) -> Vertex {
    let world=centre+corner*size;
    var out:Vertex;
    out.position=vec4<f32>(world.x*0.085+world.z*0.035,
        -0.1+world.y*0.14-world.z*0.065,0.5-world.z*0.02-world.y*0.01-world.x*0.002,1.0);
    out.colour=colour*shade;
    return out;
}
@fragment fn fs(in: Vertex) -> @location(0) vec4<f32> { return vec4<f32>(in.colour, 1.0); }
struct Quad { @builtin(position) position:vec4<f32>, @location(0) uv:vec2<f32> };
@vertex fn blit_vs(@builtin(vertex_index) i:u32)->Quad {
    let uv=vec2<f32>(f32((i<<1u)&2u),f32(i&2u));
    var out:Quad;
    out.position=vec4<f32>(uv*2.0-1.0,0.0,1.0);
    out.uv=vec2<f32>(uv.x,1.0-uv.y);
    return out;
}
@group(0) @binding(0) var scene:texture_2d<f32>;
@group(0) @binding(1) var scene_sampler:sampler;
@fragment fn blit_fs(in:Quad)->@location(0) vec4<f32> { return textureSample(scene,scene_sampler,in.uv); }
