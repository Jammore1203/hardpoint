// HARDPOINT shaders.
//
// One module, several pipelines. The world pass is deliberately trivial:
// lighting is baked into vertex colours, so the fragment shader is a texture
// fetch, a multiply and a fog blend. Everything expensive was paid for at map
// load, which is what lets the game hold a high frame rate on weak hardware.
//
// The retro character is not an accident of low effort. Vertex snapping and
// affine (non perspective-correct) texture interpolation are the two artefacts
// that define how a PlayStation-era renderer looked, and both are here as
// deliberate, adjustable effects.

struct Globals {
    view_proj: mat4x4<f32>,
    view_proj_vm: mat4x4<f32>,
    camera_pos: vec4<f32>,
    camera_right: vec4<f32>,
    camera_up: vec4<f32>,
    fog_color: vec4<f32>,      // rgb, start
    fog_params: vec4<f32>,     // end, height_falloff, unused, unused
    sky_top: vec4<f32>,
    sky_horizon: vec4<f32>,
    screen: vec4<f32>,         // w, h, 1/w, 1/h
    time: vec4<f32>,           // seconds, dt, flash, damage
    retro: vec4<f32>,          // snap grid, affine, scanline, vignette
    grade: vec4<f32>,          // warm, cool, exposure, saturation
};

@group(0) @binding(0) var<uniform> G: Globals;

@group(1) @binding(0) var world_tex: texture_2d_array<f32>;
@group(1) @binding(1) var world_smp: sampler;

@group(2) @binding(0) var sprite_tex: texture_2d<f32>;
@group(2) @binding(1) var font_tex: texture_2d<f32>;
@group(2) @binding(2) var ui_smp: sampler;

// The finished scene, sampled only by the blit pass. It lives in its own
// group because during the scene pass it is the render target, and a texture
// cannot be bound and written at the same time.
@group(3) @binding(0) var scene_tex: texture_2d<f32>;
@group(3) @binding(1) var scene_smp: sampler;

// ---------------------------------------------------------------- helpers

// Colours chosen by hand - interface palette, particle tints, team colours -
// are authored the way they look on screen, which is sRGB. The render target
// is sRGB too, so the hardware applies the encoding on write; anything coming
// from the CPU therefore has to be decoded to linear first or it comes out
// washed out.
fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

fn apply_snap(clip: vec4<f32>) -> vec4<f32> {
    let grid = G.retro.x;
    if (grid <= 0.0) { return clip; }
    // Quantise in normalised device space, which is what the hardware of the
    // era effectively did by rasterising from fixed-point vertices.
    var c = clip;
    let ndc = c.xy / max(c.w, 0.0001);
    let snapped = floor(ndc * grid + 0.5) / grid;
    c = vec4<f32>(snapped * c.w, c.z, c.w);
    return c;
}

fn fog_amount(world_pos: vec3<f32>) -> f32 {
    let d = length(world_pos - G.camera_pos.xyz);
    let start = G.fog_color.w;
    let end = G.fog_params.x;
    let t = clamp((d - start) / max(end - start, 0.001), 0.0, 1.0);
    return t * t;
}

fn grade(c: vec3<f32>) -> vec3<f32> {
    // A warm/cool split-tone plus a gentle contrast curve: the whole colour
    // treatment of the era in three instructions.
    let lum = dot(c, vec3<f32>(0.299, 0.587, 0.114));
    let warm = vec3<f32>(1.06, 1.0, 0.92);
    let cool = vec3<f32>(0.94, 0.99, 1.08);
    let tone = mix(cool, warm, clamp(lum * 1.4, 0.0, 1.0));
    var o = c * tone * G.grade.z;
    o = mix(vec3<f32>(lum), o, G.grade.w);
    return o;
}

// ================================================================= sky

struct SkyOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_sky(@builtin(vertex_index) vi: u32) -> SkyOut {
    // Fullscreen triangle; no vertex buffer needed.
    var out: SkyOut;
    let x = f32((vi << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(vi & 2u) * 2.0 - 1.0;
    out.clip = vec4<f32>(x, y, 1.0, 1.0);
    out.uv = vec2<f32>(x, y);
    return out;
}

@fragment
fn fs_sky(in: SkyOut) -> @location(0) vec4<f32> {
    // The gradient follows the view ray rather than the screen, so the haze
    // band stays welded to the world horizon when the player looks up or down.
    let aspect = G.screen.x / max(G.screen.y, 1.0);
    let th = G.fog_params.z;
    let fwd = cross(G.camera_up.xyz, G.camera_right.xyz);
    let dir = normalize(fwd
        + G.camera_right.xyz * (in.uv.x * th * aspect)
        + G.camera_up.xyz * (in.uv.y * th));

    let up_amt = clamp(dir.y, 0.0, 1.0);
    let sky = mix(G.sky_horizon.rgb, G.sky_top.rgb, pow(up_amt, 0.55));
    // Fully fogged geometry is fog_color, so the sky must be exactly that at
    // the horizon or the world ends on a visible seam.
    let haze = 1.0 - smoothstep(0.0, 0.10, dir.y);
    var c = mix(sky, G.fog_color.rgb, haze);
    // A few gentle bands, which reads as haze rather than a flat wash.
    c += sin(up_amt * 26.0) * 0.006;
    return vec4<f32>(grade(c), 1.0);
}

// =============================================================== world

struct WorldIn {
    @location(0) pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) layer: u32,
};

struct WorldOut {
    @builtin(position) clip: vec4<f32>,
    // Affine interpolation is the signature artefact: textures swim slightly
    // across large triangles exactly as they did on the hardware.
    @location(0) @interpolate(linear) uv_affine: vec2<f32>,
    @location(1) @interpolate(perspective) uv_correct: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) @interpolate(flat) layer: u32,
    @location(4) world_pos: vec3<f32>,
};

@vertex
fn vs_world(in: WorldIn) -> WorldOut {
    var out: WorldOut;
    let clip = G.view_proj * vec4<f32>(in.pos, 1.0);
    out.clip = apply_snap(clip);
    out.uv_affine = in.uv;
    out.uv_correct = in.uv;
    // Vertex colours arrive at half scale so baked light can exceed one.
    out.color = vec4<f32>(in.color.rgb * 2.0, in.color.a);
    out.layer = in.layer;
    out.world_pos = in.pos;
    return out;
}

fn world_shade(uv_a: vec2<f32>, uv_c: vec2<f32>, color: vec4<f32>, layer: u32, world_pos: vec3<f32>) -> vec4<f32> {
    let uv = mix(uv_c, uv_a, G.retro.y);
    var tex = textureSample(world_tex, world_smp, uv, i32(layer));
    var c = tex.rgb * color.rgb;
    let f = fog_amount(world_pos);
    c = mix(c, G.fog_color.rgb, f);
    return vec4<f32>(grade(c), tex.a);
}

@fragment
fn fs_world(in: WorldOut) -> @location(0) vec4<f32> {
    return world_shade(in.uv_affine, in.uv_correct, in.color, in.layer, in.world_pos);
}

@fragment
fn fs_world_cutout(in: WorldOut) -> @location(0) vec4<f32> {
    let c = world_shade(in.uv_affine, in.uv_correct, in.color, in.layer, in.world_pos);
    if (c.a < 0.5) { discard; }
    return vec4<f32>(c.rgb, 1.0);
}

// =============================================================== parts

struct PartIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    // Instance data.
    @location(3) row0: vec4<f32>,
    @location(4) row1: vec4<f32>,
    @location(5) row2: vec4<f32>,
    @location(6) color: vec4<f32>,
    @location(7) params: vec4<f32>,
};

struct PartOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) layer: u32,
    @location(3) world_pos: vec3<f32>,
};

fn part_world(in: PartIn) -> vec3<f32> {
    let p = vec4<f32>(in.pos, 1.0);
    return vec3<f32>(dot(in.row0, p), dot(in.row1, p), dot(in.row2, p));
}

fn part_normal(in: PartIn) -> vec3<f32> {
    let n = vec4<f32>(in.normal, 0.0);
    return normalize(vec3<f32>(dot(in.row0, n), dot(in.row1, n), dot(in.row2, n)));
}

fn part_light(n: vec3<f32>) -> f32 {
    // A fixed three-quarter key with a soft fill; character lighting that
    // matches the baked world closely enough and costs nothing.
    let key = max(dot(n, normalize(vec3<f32>(-0.4, 0.82, -0.4))), 0.0);
    let fill = max(dot(n, normalize(vec3<f32>(0.5, 0.2, 0.7))), 0.0);
    return 0.42 + key * 0.72 + fill * 0.18;
}

@vertex
fn vs_part(in: PartIn) -> PartOut {
    var out: PartOut;
    let wp = part_world(in);
    out.clip = apply_snap(G.view_proj * vec4<f32>(wp, 1.0));
    out.uv = in.uv * max(in.params.y, 0.001);
    let n = part_normal(in);
    out.color = vec4<f32>(srgb_to_linear(in.color.rgb) * part_light(n), in.color.a);
    out.layer = u32(in.params.x);
    out.world_pos = wp;
    return out;
}

@vertex
fn vs_viewmodel(in: PartIn) -> PartOut {
    var out: PartOut;
    let wp = part_world(in);
    // The viewmodel lives in view space and uses its own projection, so it
    // never intersects the world no matter how close a wall is.
    out.clip = G.view_proj_vm * vec4<f32>(wp, 1.0);
    out.uv = in.uv * max(in.params.y, 0.001);
    let n = part_normal(in);
    out.color = vec4<f32>(srgb_to_linear(in.color.rgb) * part_light(n), in.color.a);
    out.layer = u32(in.params.x);
    out.world_pos = G.camera_pos.xyz;
    return out;
}

@fragment
fn fs_part(in: PartOut) -> @location(0) vec4<f32> {
    var tex = textureSample(world_tex, world_smp, in.uv, i32(in.layer));
    var c = tex.rgb * in.color.rgb;
    let f = fog_amount(in.world_pos);
    c = mix(c, G.fog_color.rgb, f);
    if (tex.a * in.color.a < 0.5) { discard; }
    return vec4<f32>(grade(c), 1.0);
}

@fragment
fn fs_viewmodel(in: PartOut) -> @location(0) vec4<f32> {
    var tex = textureSample(world_tex, world_smp, in.uv, i32(in.layer));
    let c = tex.rgb * in.color.rgb;
    if (tex.a * in.color.a < 0.5) { discard; }
    return vec4<f32>(grade(c), 1.0);
}

// ============================================================== sprites

struct SpriteIn {
    @location(0) corner: vec2<f32>,
    @location(1) pos: vec3<f32>,
    @location(2) size: vec2<f32>,
    @location(3) rot: f32,
    @location(4) uv_rect: vec4<f32>,
    @location(5) color: vec4<f32>,
    @location(6) mode: f32,
};

struct SpriteOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) world_pos: vec3<f32>,
};

@vertex
fn vs_sprite(in: SpriteIn) -> SpriteOut {
    var out: SpriteOut;
    let s = sin(in.rot);
    let c = cos(in.rot);
    let local = vec2<f32>(in.corner.x * c - in.corner.y * s, in.corner.x * s + in.corner.y * c);
    // Mode 1 lays the quad flat on the ground (decals, blob shadows).
    var world: vec3<f32>;
    if (in.mode > 0.5) {
        world = in.pos + vec3<f32>(local.x * in.size.x, 0.0, local.y * in.size.y);
    } else {
        world = in.pos
            + G.camera_right.xyz * (local.x * in.size.x)
            + G.camera_up.xyz * (local.y * in.size.y);
    }
    out.clip = G.view_proj * vec4<f32>(world, 1.0);
    out.uv = in.uv_rect.xy + (in.corner + vec2<f32>(0.5, 0.5)) * in.uv_rect.zw;
    out.color = vec4<f32>(srgb_to_linear(in.color.rgb), in.color.a);
    out.world_pos = world;
    return out;
}

@fragment
fn fs_sprite(in: SpriteOut) -> @location(0) vec4<f32> {
    let tex = textureSample(sprite_tex, ui_smp, in.uv);
    let f = fog_amount(in.world_pos);
    let rgb = mix(tex.rgb * in.color.rgb, G.fog_color.rgb, f * 0.7);
    return vec4<f32>(grade(rgb), tex.a * in.color.a);
}

// =================================================================== ui

struct UiIn {
    @location(0) pos: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) mode: f32,
};

struct UiOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) mode: f32,
};

@vertex
fn vs_ui(in: UiIn) -> UiOut {
    var out: UiOut;
    let ndc = vec2<f32>(in.pos.x * G.screen.z * 2.0 - 1.0, 1.0 - in.pos.y * G.screen.w * 2.0);
    out.clip = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = in.uv;
    out.color = vec4<f32>(srgb_to_linear(in.color.rgb), in.color.a);
    out.mode = in.mode;
    return out;
}

@fragment
fn fs_ui(in: UiOut) -> @location(0) vec4<f32> {
    if (in.mode < 0.5) {
        // Solid fill.
        return in.color;
    } else if (in.mode < 1.5) {
        // Text from the font atlas.
        let a = textureSample(font_tex, ui_smp, in.uv).r;
        return vec4<f32>(in.color.rgb, in.color.a * a);
    }
    // Sprite from the effects atlas.
    let t = textureSample(sprite_tex, ui_smp, in.uv);
    return vec4<f32>(in.color.rgb * t.rgb, in.color.a * t.a);
}

// ================================================================= blit

struct BlitOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_blit(@builtin(vertex_index) vi: u32) -> BlitOut {
    var out: BlitOut;
    let x = f32((vi << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(vi & 2u) * 2.0 - 1.0;
    out.clip = vec4<f32>(x, y, 0.0, 1.0);
    out.uv = vec2<f32>(x * 0.5 + 0.5, 0.5 - y * 0.5);
    return out;
}

@fragment
fn fs_blit(in: BlitOut) -> @location(0) vec4<f32> {
    var c = textureSample(scene_tex, scene_smp, in.uv).rgb;

    // Damage and flash are applied here so they cover everything including
    // the viewmodel, and cost one blend on an already-full-screen pass.
    let flash = G.time.z;
    let damage = G.time.w;
    if (damage > 0.0) {
        let edge = length(in.uv - vec2<f32>(0.5, 0.5)) * 1.6;
        c = mix(c, vec3<f32>(0.55, 0.04, 0.03), clamp(damage * edge, 0.0, 0.85));
    }

    let scan = G.retro.z;
    if (scan > 0.0) {
        // Scanlines in output pixels, not source pixels, so they stay a fixed
        // thickness regardless of the internal resolution.
        let line = sin(in.uv.y * G.screen.y * 3.14159);
        c *= 1.0 - scan * 0.5 * (0.5 + 0.5 * line);
    }

    let vig = G.retro.w;
    if (vig > 0.0) {
        let d = length((in.uv - vec2<f32>(0.5, 0.5)) * vec2<f32>(1.15, 1.0));
        c *= 1.0 - vig * smoothstep(0.35, 0.95, d);
    }

    if (flash > 0.0) {
        c = mix(c, vec3<f32>(1.0, 0.98, 0.94), clamp(flash, 0.0, 1.0));
    }

    return vec4<f32>(c, 1.0);
}
