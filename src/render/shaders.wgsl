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
    grade: vec4<f32>,          // detail strength, detail layer, exposure, saturation
    sun: vec4<f32>,            // direction toward the sun (xyz), cloud cover (w)
    warm: vec4<f32>,           // per-map warm tint (rgb), fog height falloff (w)
    cool: vec4<f32>,           // per-map cool tint (rgb), fog floor height (w)
    gloss: array<vec4<f32>, 17>,   // per-material gloss, four to a row
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
    var t = clamp((d - start) / max(end - start, 0.001), 0.0, 1.0);
    // Squaring this - which is what it used to do - keeps the haze at a
    // quarter strength through the whole middle of the range, so a building
    // eighty metres away came back as crisp and as saturated as the wall in
    // front of you and the ground ran to a hard line against the sky. This
    // curve still eases in from nothing, so near geometry is untouched, but
    // it is two thirds of the way to solid by three quarters of the distance,
    // which is what aerial perspective actually looks like.
    t = t * t * (2.0 - t);

    // Height fog: haze pools in the low ground and thins with altitude, which
    // is what separates a distant rooftop from the street it stands over. The
    // falloff is per map, so an enclosed foundry can switch it off entirely
    // while a coastal fort sits in it.
    let falloff = G.warm.w;
    if (falloff > 0.0001) {
        let floor_y = G.cool.w;
        let h = max(world_pos.y - floor_y, 0.0);
        t = t * clamp(exp(-h * falloff), 0.0, 1.0);
    }
    return t;
}

// The sun's own colour is not in the uniform block; the sky's top colour is a
// good enough stand-in, warmed a little.
fn G_sun_color_or_white() -> vec3<f32> {
    return mix(vec3<f32>(1.0, 0.97, 0.90), vec3<f32>(1.0), 0.35);
}

// Haze is not one colour. Looking into the sun through it, the light that
// reaches you is the light it scattered out of the beam, so it glows; looking
// away, it is the flat map colour. One power term buys the whole effect, and
// it is the difference between fog that reads as atmosphere and fog that
// reads as a grey card someone put in front of the far half of the map.
//
// `dir` points from the eye out toward whatever is being fogged.
fn fog_color_along(dir: vec3<f32>) -> vec3<f32> {
    let sun_dot = clamp(dot(dir, G.sun.xyz), 0.0, 1.0);
    let clear = 1.0 - G.sun.w * 0.65;
    let glow = (pow(sun_dot, 8.0) * 0.30 + pow(sun_dot, 2.0) * 0.07) * clear;
    return G.fog_color.rgb + G_sun_color_or_white() * glow;
}

fn fog_color_at(world_pos: vec3<f32>) -> vec3<f32> {
    return fog_color_along(normalize(world_pos - G.camera_pos.xyz));
}

// Gloss lives four to a row because a uniform array of scalars is padded to
// sixteen bytes an element. WGSL will not index a vector by a runtime value,
// hence the select chain.
fn gloss_of(layer: u32) -> f32 {
    let row = G.gloss[layer >> 2u];
    let i = layer & 3u;
    return select(select(select(row.w, row.z, i == 2u), row.y, i == 1u), row.x, i == 0u);
}

// One specular lobe, shared by the world and by props.
//
// `lit` is the surface's own baked or vertex light, which the highlight is
// multiplied by so a face standing in shadow does not sparkle; `fade` is the
// fog amount, because a highlight seen through two hundred metres of haze is
// not there. The exponent runs with gloss so that dull surfaces get a wide,
// weak sheen and polished ones get a small hard point, and the strength runs
// with it too -- one parameter, both ends.
fn specular(n: vec3<f32>, world_pos: vec3<f32>, g: f32, lit: vec3<f32>, fade: f32) -> vec3<f32> {
    if (g <= 0.005) { return vec3<f32>(0.0); }
    let v = normalize(G.camera_pos.xyz - world_pos);
    let h = normalize(v + G.sun.xyz);
    let ndh = max(dot(n, h), 0.0);
    let power = 6.0 + g * g * 220.0;
    // Grazing angles return more light off every real material. Kept mild:
    // a full Fresnel curve on flat brush faces reads as a bug, not a sheen.
    let fres = 0.35 + 0.65 * pow(1.0 - clamp(dot(n, v), 0.0, 1.0), 4.0);
    let amount = pow(ndh, power) * g * fres * (1.0 - fade);
    // Overcast skies have no sun to catch.
    let clouds = 1.0 - G.sun.w * 0.7;
    let key = dot(lit, vec3<f32>(0.299, 0.587, 0.114));
    return G_sun_color_or_white() * amount * clouds * clamp(key, 0.0, 1.6);
}

fn grade(c: vec3<f32>) -> vec3<f32> {
    // A warm/cool split-tone plus a gentle contrast curve: the whole colour
    // treatment of the era in three instructions. The two tints come from the
    // map, so a desert airfield and an arctic radar site do not resolve to the
    // same picture once the fog and the sky have had their say.
    let lum = dot(c, vec3<f32>(0.299, 0.587, 0.114));
    let warm = G.warm.rgb;
    let cool = G.cool.rgb;
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
    // Fullscreen triangle; no vertex buffer needed. Depth zero is the far
    // plane under the reversed-depth projection, so the sky is rejected
    // wherever the world already drew and only shades pixels that are
    // genuinely sky. It is drawn after the world for that reason.
    var out: SkyOut;
    let x = f32((vi << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(vi & 2u) * 2.0 - 1.0;
    out.clip = vec4<f32>(x, y, 0.0, 1.0);
    out.uv = vec2<f32>(x, y);
    return out;
}

// Value noise on a hashed lattice. The sky is the one place in this renderer
// that generates a pattern at run time rather than baking it, because a cloud
// layer has to move.
fn sky_hash(p: vec2<f32>) -> f32 {
    let q = fract(p * vec2<f32>(0.3183099, 0.3678794));
    let r = q + dot(q, q + 33.33);
    return fract((r.x + r.y) * r.x * r.y);
}

fn sky_noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = sky_hash(i);
    let b = sky_hash(i + vec2<f32>(1.0, 0.0));
    let c = sky_hash(i + vec2<f32>(0.0, 1.0));
    let d = sky_hash(i + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
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
    var c = mix(G.sky_horizon.rgb, G.sky_top.rgb, pow(up_amt, 0.55));

    let sun_dir = G.sun.xyz;
    let sun_dot = clamp(dot(dir, sun_dir), 0.0, 1.0);

    // Sun: a small hard disc inside a wide halo. The halo does most of the
    // work - it is what tells you which way the light is coming from when the
    // disc itself is behind you.
    let halo = pow(sun_dot, 40.0) * 0.55 + pow(sun_dot, 6.0) * 0.16;
    let disc = smoothstep(0.9975, 0.9990, sun_dot);
    c += G_sun_color_or_white() * (halo + disc * 2.4);

    // Clouds, on a plane above the world.
    //
    // Projected along the view ray rather than drawn on the sky dome, so they
    // flatten and crowd toward the horizon the way real cloud cover does, and
    // the player can tell they are a layer at a height rather than a texture
    // on a hemisphere.
    let cover = G.sun.w;
    if (cover > 0.001 && dir.y > 0.015) {
        let t = 260.0 / dir.y;
        var p = (G.camera_pos.xz + dir.xz * t) * 0.0060;
        p += vec2<f32>(G.time.x * 0.012, G.time.x * 0.006);

        // Four octaves. The fourth is worth its cost overhead, where the
        // projection is not stretching the noise and a cloud has enough size
        // on screen to show an edge; it is faded out toward the horizon with
        // everything else.
        var n = sky_noise(p) * 0.52;
        n += sky_noise(p * 2.17 + 3.1) * 0.27;
        n += sky_noise(p * 4.31 + 7.7) * 0.15;
        n += sky_noise(p * 8.90 + 19.3) * 0.06 * smoothstep(0.10, 0.45, dir.y);

        // Thin the cover toward the horizon, where the projection stretches
        // the noise into streaks that read as smearing rather than as cloud.
        let band = smoothstep(0.015, 0.30, dir.y);
        let lo = mix(0.66, 0.24, cover);

        // Coverage and depth, separately. One threshold gives a flat stencil
        // of one grey, which is what made these read as airbrushed smudges:
        // the edge of a cumulus is thin and lets the sky through, and its
        // middle is opaque, bright on the sun's side and grey underneath.
        let edge = smoothstep(lo, lo + 0.17, n);
        let core = smoothstep(lo + 0.10, lo + 0.44, n);
        let amount = edge * band;

        let base = mix(vec3<f32>(0.58, 0.60, 0.67), vec3<f32>(0.84, 0.84, 0.87), core);
        let sunny = mix(vec3<f32>(0.90, 0.90, 0.92), vec3<f32>(1.12, 1.07, 0.98),
                        pow(sun_dot, 2.0));
        let lit = mix(base, sunny, 0.32 + core * 0.52);
        c = mix(c, lit * mix(0.72, 1.0, up_amt), amount * 0.94);
    }

    // Fully fogged geometry is fog_color, so the sky must be exactly that at
    // the horizon or the world ends on a visible seam.
    let haze = 1.0 - smoothstep(0.0, 0.10, dir.y);
    c = mix(c, fog_color_along(dir), haze);
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

// A material tiles every couple of metres, so by the time you are standing
// against a wall its highest frequency is several centimetres across and the
// surface reads as flat colour. The detail layer is the same trick every
// engine of this era used: a shared noise tile sampled far finer than the
// material, modulating brightness only, faded out with distance so it never
// becomes the thing that aliases.
// The world has no normal attribute -- brush faces are flat, and the baked
// light is already in the vertex colour -- so the face normal is recovered
// from how the world position changes across the triangle. It costs two
// derivatives and is exact for flat geometry, which is all the world is.
fn face_normal(world_pos: vec3<f32>) -> vec3<f32> {
    let n = normalize(cross(dpdx(world_pos), dpdy(world_pos)));
    // Winding and screen orientation decide the sign; the viewer decides it
    // better.
    let v = G.camera_pos.xyz - world_pos;
    return select(-n, n, dot(n, v) >= 0.0);
}

fn detail_modulation(uv: vec2<f32>, world_pos: vec3<f32>) -> f32 {
    let strength = G.grade.x;
    if (strength <= 0.001) { return 1.0; }
    let d = length(world_pos - G.camera_pos.xyz);
    // Out to thirty-four metres rather than twenty-two. The layer costs one
    // sample of a texture already resident and it is what keeps a floor from
    // going flat as it recedes; twenty-two metres is close enough that on an
    // open map most of the ground was past it.
    let near = 1.0 - smoothstep(6.0, 34.0, d);
    if (near <= 0.001) { return 1.0; }
    let n = textureSample(world_tex, world_smp, uv * 6.0, i32(G.grade.y)).r;
    return 1.0 + (n - 0.5) * strength * near;
}

fn world_shade(uv_a: vec2<f32>, uv_c: vec2<f32>, color: vec4<f32>, layer: u32, world_pos: vec3<f32>) -> vec4<f32> {
    var uv = mix(uv_c, uv_a, G.retro.y);
    // Water drifts. Two layers at different speeds and directions, summed by
    // sampling twice, which is the cheapest thing that stops a lake looking
    // like a photograph of one. `time.y` carries the water layer index; a
    // negative value means this map has none.
    let water_layer = G.time.y;
    if (water_layer >= 0.0 && layer == u32(water_layer)) {
        let t = G.time.x;
        let a = textureSample(world_tex, world_smp, uv + vec2<f32>(t * 0.014, t * 0.009), i32(layer));
        let b = textureSample(world_tex, world_smp, uv * 0.73 + vec2<f32>(t * -0.010, t * 0.017), i32(layer));
        var wc = (a.rgb * 0.58 + b.rgb * 0.46) * color.rgb * detail_modulation(uv, world_pos);
        let wf = fog_amount(world_pos);
        // Water is flat geometry, so its highlight has to come from somewhere
        // else: perturb the face normal by the same two scrolling layers that
        // move the colour, and the sun's reflection travels with the swell.
        let wobble = vec3<f32>((a.r - 0.5) * 0.45, 1.0, (b.r - 0.5) * 0.45);
        wc = wc + specular(normalize(wobble), world_pos, gloss_of(layer), color.rgb, wf);
        wc = mix(wc, fog_color_at(world_pos), wf);
        return vec4<f32>(grade(wc), 1.0);
    }
    var tex = textureSample(world_tex, world_smp, uv, i32(layer));
    var c = tex.rgb * color.rgb * detail_modulation(uv, world_pos);
    let f = fog_amount(world_pos);
    c = c + specular(face_normal(world_pos), world_pos, gloss_of(layer), color.rgb, f);
    c = mix(c, fog_color_at(world_pos), f);
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
    @location(4) normal: vec3<f32>,
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
    out.normal = n;
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
    // View space, not world space: the fragment stage uses it to work out
    // where the eye is relative to the surface, and fs_viewmodel is the only
    // consumer.
    out.world_pos = wp;
    out.normal = n;
    return out;
}

@fragment
fn fs_part(in: PartOut) -> @location(0) vec4<f32> {
    var tex = textureSample(world_tex, world_smp, in.uv, i32(in.layer));
    var c = tex.rgb * in.color.rgb;
    let f = fog_amount(in.world_pos);
    c = c + specular(normalize(in.normal), in.world_pos, gloss_of(in.layer), in.color.rgb, f);
    c = mix(c, fog_color_at(in.world_pos), f);
    if (tex.a * in.color.a < 0.5) { discard; }
    return vec4<f32>(grade(c), 1.0);
}

@fragment
fn fs_viewmodel(in: PartOut) -> @location(0) vec4<f32> {
    var tex = textureSample(world_tex, world_smp, in.uv, i32(in.layer));
    var c = tex.rgb * in.color.rgb;
    // The viewmodel is drawn in view space with its own projection, so its
    // world position is meaningless and the eye is at the origin. Light it
    // with a fixed key over the shoulder instead: the gun is the thing the
    // player looks at for the whole match and it is worth the four lines.
    let n = normalize(in.normal);
    let v = normalize(-in.world_pos);
    let h = normalize(v + normalize(vec3<f32>(-0.35, 0.78, -0.52)));
    let g = gloss_of(in.layer);
    if (g > 0.005) {
        let power = 6.0 + g * g * 220.0;
        c = c + G_sun_color_or_white() * pow(max(dot(n, h), 0.0), power) * g * 0.9;
    }
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
        // A vignette, not a wash. At full strength the old version mixed
        // eighty-five per cent of a saturated red over the corners and most of
        // that over the middle, which hides the thing you were about to shoot
        // at exactly the moment you most need to see it. The curve is steeper
        // now, so the centre of the screen stays readable and the edges carry
        // the message.
        let d = length((in.uv - vec2<f32>(0.5, 0.5)) * vec2<f32>(1.1, 1.0));
        let edge = smoothstep(0.18, 0.72, d);
        c = mix(c, vec3<f32>(0.42, 0.05, 0.04), clamp(damage * edge, 0.0, 0.62));
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
