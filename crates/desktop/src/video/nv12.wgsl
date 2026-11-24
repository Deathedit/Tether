// NV12 (Y plane + interleaved UV plane) to RGBA, drawn as one full-screen
// triangle into the output texture that the UI shows.

struct Color {
    // Rows of the YUV -> RGB matrix (w unused).
    r0: vec4<f32>,
    r1: vec4<f32>,
    r2: vec4<f32>,
    // Subtracted from (Y, U, V) before the matrix (w unused).
    offset: vec4<f32>,
};

@group(0) @binding(0) var y_tex: texture_2d<f32>;
@group(0) @binding(1) var uv_tex: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;
@group(0) @binding(3) var<uniform> color: Color;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) i: u32) -> VsOut {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var out: VsOut;
    out.pos = vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
    out.uv = uv;
    return out;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let yuv = vec3<f32>(
        textureSample(y_tex, samp, in.uv).r,
        textureSample(uv_tex, samp, in.uv).rg,
    ) - color.offset.xyz;
    let rgb = vec3<f32>(dot(color.r0.xyz, yuv), dot(color.r1.xyz, yuv), dot(color.r2.xyz, yuv));
    return vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
