// Point cloud sprites: one instance per point, drawn as a square of
// POINTCLOUDPOINTSIZE pixels around its projected position, depth-tested.

// ── Bind group 0: shared projection uniforms ─────────────────────────────────
// Must match the leading fields of `Uniforms` (scene::pipeline::uniforms).
struct Uniforms {
    viewport_size:      vec2<f32>,
    world_per_pixel:    f32,
    lwdisplay_enable:   f32,
    flat_shade:         f32,
    transparency_enable: f32,
    _pad:               vec2<f32>,
    // Relative-to-eye (double-single): see wire.wgsl.
    view_rot:           mat4x4<f32>,
    eye_high:           vec3<f32>,
    _pad_eh:            f32,
    eye_low:            vec3<f32>,
    _pad_el:            f32,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

struct PointParams {
    size_px: f32,
    // POINTCLOUDLIGHTING (0 off; 1 lit from both sides; 2 back faces dark)
    // and POINTCLOUDLIGHTSOURCE (0 headlight; 1 the default lights).
    lighting: f32,
    source: f32,
    _pad0: f32,
    light_dir: vec3<f32>,
    _pad1: f32,
};
@group(1) @binding(0) var<uniform> params: PointParams;

struct PointIn {
    @location(0) pos:     vec3<f32>,
    @location(1) pos_low: vec3<f32>,
    @location(2) color:   vec4<f32>,
    @location(3) normal:  vec4<f32>,
};

struct VertOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0)       color:    vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) corner: u32, in: PointIn) -> VertOut {
    var out: VertOut;
    let rel = (in.pos - u.eye_high) + (in.pos_low - u.eye_low);
    let clip = u.view_rot * vec4<f32>(rel, 1.0);
    // Two triangles covering the square: corners in -1..1.
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    // Half the size in pixels → NDC (2 / viewport), scaled by w to stay in
    // clip space.
    let half_ndc = params.size_px / max(u.viewport_size, vec2<f32>(1.0, 1.0));
    out.clip_pos = vec4<f32>(clip.xy + corners[corner] * half_ndc * clip.w, clip.z, clip.w);
    out.color = in.color;
    let n = in.normal.xyz;
    if params.lighting > 0.5 && dot(n, n) > 0.01 {
        // Measured from the reference: headlight ambient 0.2 + 0.576 diffuse,
        // the default lights 0.3 + 0.9 (clamped).
        let d = dot(normalize(n), params.light_dir);
        var shade: f32;
        if params.source > 0.5 {
            shade = min(0.3 + 0.9 * max(d, 0.0), 1.0);
        } else if params.lighting > 1.5 {
            shade = 0.2 + 0.576 * max(d, 0.0);
        } else {
            shade = 0.2 + 0.576 * abs(d);
        }
        out.color = vec4<f32>(in.color.rgb * shade, in.color.a);
    }
    return out;
}

@fragment
fn fs_main(in: VertOut) -> @location(0) vec4<f32> {
    return in.color;
}
