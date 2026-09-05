//! Geometry generation.
//!
//! Levels are converted from brushes into one vertex buffer with baked
//! per-vertex lighting: a sun term with real cast shadows, a hemisphere
//! ambient, and short-range ambient occlusion. Baking is the whole reason the
//! game can look lit while running a shader with no lighting in it at all.
//!
//! Geometry is grouped into spatial clusters so the renderer can frustum-cull
//! whole regions with a handful of comparisons.

use crate::assets::materials::Mat;
use crate::game::movement;
use crate::maps::brush::{BrushFlags, BrushKind, FaceMask, RampAxis, TraceMask};
use crate::maps::{Env, MapData};
use crate::math::Aabb;
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};

/// A world-geometry vertex. Twenty-eight bytes; a whole level is typically
/// under a megabyte, which fits in cache-friendly territory.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct WorldVertex {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
    /// Baked light, stored at half scale so values above 1 survive.
    pub color: [u8; 4],
    /// Texture array layer.
    pub layer: u32,
}

/// A spatially local group of triangles, culled as a unit.
#[derive(Clone, Debug)]
pub struct Cluster {
    pub bounds: Aabb,
    pub opaque: std::ops::Range<u32>,
    pub cutout: std::ops::Range<u32>,
}

pub struct MapMesh {
    pub vertices: Vec<WorldVertex>,
    pub indices: Vec<u32>,
    pub clusters: Vec<Cluster>,
    pub bounds: Aabb,
    pub triangle_count: usize,
}

/// How much work the lighting bake does.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BakeQuality {
    /// Ambient only; instant.
    Flat,
    /// Sun with cast shadows.
    Shadows,
    /// Sun, shadows and ambient occlusion.
    Full,
}

const CLUSTER_SIZE: f32 = 18.0;

/// The six face directions of a box, as (axis, positive, mask bit).
const FACES: [(usize, bool, u8); 6] = [
    (0, false, FaceMask::NEG_X),
    (0, true, FaceMask::POS_X),
    (1, false, FaceMask::NEG_Y),
    (1, true, FaceMask::POS_Y),
    (2, false, FaceMask::NEG_Z),
    (2, true, FaceMask::POS_Z),
];

struct Face {
    corners: [Vec3; 4],
    normal: Vec3,
    uvs: [[f32; 2]; 4],
    mat: Mat,
    light_scale: f32,
    cutout: bool,
    no_shadow: bool,
}

pub fn build_map_mesh(map: &MapData, quality: BakeQuality) -> MapMesh {
    let mut faces: Vec<Face> = Vec::with_capacity(map.brushes.len() * 4);

    for list in [&map.brushes, &map.decor] {
        for b in list.iter() {
            if b.flags.contains(BrushFlags::NODRAW) { continue; }
            let cutout = b.flags.contains(BrushFlags::CUTOUT) || b.mat.is_cutout();
            let no_shadow = b.flags.contains(BrushFlags::NOSHADOW);
            match b.kind {
                BrushKind::Box => {
                    for &(axis, positive, bit) in &FACES {
                        if !b.faces.has(bit) { continue; }
                        let (corners, normal) = box_face(&b.aabb, axis, positive);
                        let mat = if axis == 1 && positive { b.top } else { b.mat };
                        let uvs = plane_uvs(&corners, axis, b.tex_scale);
                        faces.push(Face { corners, normal, uvs, mat, light_scale: b.light_scale, cutout, no_shadow });
                    }
                }
                BrushKind::Ramp(ax) => {
                    // Sloped top plus the four sides beneath it.
                    let (c, n) = ramp_top(&b.aabb, ax);
                    let uvs = plane_uvs(&c, 1, b.tex_scale);
                    faces.push(Face { corners: c, normal: n, uvs, mat: b.top, light_scale: b.light_scale, cutout, no_shadow });
                    for &(axis, positive, bit) in &FACES {
                        if axis == 1 && positive { continue; }
                        if !b.faces.has(bit) { continue; }
                        let (mut corners, normal) = box_face(&b.aabb, axis, positive);
                        // Pull the top edge of each side down onto the slope.
                        for c in corners.iter_mut() {
                            if (c.y - b.aabb.max.y).abs() < 1e-4 {
                                c.y = b.surface_height(c.x, c.z);
                            }
                        }
                        let uvs = plane_uvs(&corners, axis, b.tex_scale);
                        faces.push(Face { corners, normal, uvs, mat: b.mat, light_scale: b.light_scale, cutout, no_shadow });
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------ lighting
    let env = &map.env;
    let mut vertices: Vec<WorldVertex> = Vec::with_capacity(faces.len() * 4);
    let mut face_cluster: Vec<(usize, u32, bool)> = Vec::with_capacity(faces.len());

    let mut bounds = Aabb::EMPTY;
    for (fi, f) in faces.iter().enumerate() {
        let base = vertices.len() as u32;
        for i in 0..4 {
            let p = f.corners[i];
            bounds.union_point(p);
            let light = bake_light(map, env, p, f.normal, f.mat, f.light_scale, f.no_shadow, quality);
            vertices.push(WorldVertex {
                pos: [p.x, p.y, p.z],
                uv: f.uvs[i],
                color: light,
                layer: f.mat.layer(),
            });
        }
        face_cluster.push((fi, base, f.cutout));
    }

    // ------------------------------------------------------------ clusters
    let origin = Vec3::new(bounds.min.x, 0.0, bounds.min.z);
    let cells_x = (((bounds.max.x - bounds.min.x) / CLUSTER_SIZE).ceil() as usize).max(1);
    let cells_z = (((bounds.max.z - bounds.min.z) / CLUSTER_SIZE).ceil() as usize).max(1);
    let cell_count = cells_x * cells_z;

    let mut opaque_by_cell: Vec<Vec<u32>> = vec![Vec::new(); cell_count];
    let mut cutout_by_cell: Vec<Vec<u32>> = vec![Vec::new(); cell_count];
    let mut cell_bounds: Vec<Aabb> = vec![Aabb::EMPTY; cell_count];

    for (fi, base, cutout) in face_cluster {
        let f = &faces[fi];
        let c = (f.corners[0] + f.corners[2]) * 0.5;
        let cx = (((c.x - origin.x) / CLUSTER_SIZE) as usize).min(cells_x - 1);
        let cz = (((c.z - origin.z) / CLUSTER_SIZE) as usize).min(cells_z - 1);
        let ci = cz * cells_x + cx;
        for corner in f.corners.iter() { cell_bounds[ci].union_point(*corner); }
        let list = if cutout { &mut cutout_by_cell[ci] } else { &mut opaque_by_cell[ci] };
        // Two triangles, wound counter-clockwise when seen from the front.
        list.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    let mut indices: Vec<u32> = Vec::with_capacity(vertices.len() * 3 / 2);
    let mut clusters = Vec::with_capacity(cell_count);
    for ci in 0..cell_count {
        if opaque_by_cell[ci].is_empty() && cutout_by_cell[ci].is_empty() { continue; }
        let o_start = indices.len() as u32;
        indices.extend_from_slice(&opaque_by_cell[ci]);
        let o_end = indices.len() as u32;
        indices.extend_from_slice(&cutout_by_cell[ci]);
        let c_end = indices.len() as u32;
        clusters.push(Cluster {
            bounds: cell_bounds[ci],
            opaque: o_start..o_end,
            cutout: o_end..c_end,
        });
    }

    let triangle_count = indices.len() / 3;
    MapMesh { vertices, indices, clusters, bounds, triangle_count }
}

/// Computes the baked colour for one vertex.
fn bake_light(
    map: &MapData,
    env: &Env,
    p: Vec3,
    n: Vec3,
    mat: Mat,
    scale: f32,
    no_shadow: bool,
    quality: BakeQuality,
) -> [u8; 4] {
    // Hemisphere ambient: sky above, bounced ground light below.
    let t = (n.y * 0.5 + 0.5).clamp(0.0, 1.0);
    let mut r = env.ambient_ground[0] + (env.ambient_sky[0] - env.ambient_ground[0]) * t;
    let mut g = env.ambient_ground[1] + (env.ambient_sky[1] - env.ambient_ground[1]) * t;
    let mut b = env.ambient_ground[2] + (env.ambient_sky[2] - env.ambient_ground[2]) * t;

    let ndotl = (-env.sun_dir).dot(n).max(0.0);
    if ndotl > 0.0 && quality != BakeQuality::Flat && !no_shadow {
        // Offset along the normal so a surface never shadows itself.
        let origin = p + n * 0.06;
        let hit = map.collision.trace_ray(origin, -env.sun_dir, 60.0, TraceMask::Shot);
        if !hit.hit {
            r += env.sun_color[0] * ndotl;
            g += env.sun_color[1] * ndotl;
            b += env.sun_color[2] * ndotl;
        }
    } else if ndotl > 0.0 {
        r += env.sun_color[0] * ndotl;
        g += env.sun_color[1] * ndotl;
        b += env.sun_color[2] * ndotl;
    }

    if quality == BakeQuality::Full {
        let ao = ambient_occlusion(map, p, n);
        r *= ao;
        g *= ao;
        b *= ao;
    }

    let e = mat.emissive();
    if e > 0.0 {
        r = r.max(e);
        g = g.max(e);
        b = b.max(e);
    }

    r *= scale;
    g *= scale;
    b *= scale;

    // Stored at half scale so the shader can reach 2.0 without an HDR target.
    [
        (r * 0.5 * 255.0).clamp(0.0, 255.0) as u8,
        (g * 0.5 * 255.0).clamp(0.0, 255.0) as u8,
        (b * 0.5 * 255.0).clamp(0.0, 255.0) as u8,
        255,
    ]
}

/// Short-range occlusion from four rays in the normal's hemisphere. Cheap,
/// and enough to seat geometry into corners instead of letting it float.
fn ambient_occlusion(map: &MapData, p: Vec3, n: Vec3) -> f32 {
    const DIRS: [Vec3; 4] = [
        Vec3::new(0.5, 0.75, 0.43),
        Vec3::new(-0.5, 0.75, 0.43),
        Vec3::new(0.0, 0.75, -0.66),
        Vec3::new(0.0, 1.0, 0.0),
    ];
    // Build a basis so the sample directions follow the surface.
    let up = if n.y.abs() > 0.9 { Vec3::Z } else { Vec3::Y };
    let tangent = n.cross(up).normalize_or_zero();
    let bitangent = tangent.cross(n);
    let origin = p + n * 0.05;

    let mut open = 0.0f32;
    for d in DIRS {
        let dir = (tangent * d.x + n * d.y + bitangent * d.z).normalize_or_zero();
        let hit = map.collision.trace_ray(origin, dir, 1.1, TraceMask::Shot);
        if !hit.hit { open += 1.0; } else { open += (hit.point - origin).length() / 1.1 * 0.6; }
    }
    (0.35 + 0.65 * (open / 4.0)).clamp(0.0, 1.0)
}

/// Corners of one face of a box, wound counter-clockwise seen from outside.
///
/// The winding is derived from a tangent pair chosen so that `t1 x t2` equals
/// the face normal, rather than from per-axis special cases. Getting this
/// wrong is invisible in the data and very visible on screen: back-face
/// culling silently removes every floor in the level.
fn box_face(b: &Aabb, axis: usize, positive: bool) -> ([Vec3; 4], Vec3) {
    let mut n = Vec3::ZERO;
    n[axis] = if positive { 1.0 } else { -1.0 };
    let plane = if positive { b.max[axis] } else { b.min[axis] };

    // (first tangent axis, second tangent axis) with t1 x t2 == n.
    let (a1, a2) = match (axis, positive) {
        (0, true) => (1usize, 2usize),   // +X: Y x Z = X
        (0, false) => (2, 1),            // -X: Z x Y = -X
        (1, true) => (2, 0),             // +Y: Z x X = Y
        (1, false) => (0, 2),            // -Y: X x Z = -Y
        (2, true) => (0, 1),             // +Z: X x Y = Z
        _ => (1, 0),                     // -Z: Y x X = -Z
    };

    let mut origin = Vec3::ZERO;
    origin[axis] = plane;
    origin[a1] = b.min[a1];
    origin[a2] = b.min[a2];

    let mut e1 = Vec3::ZERO;
    e1[a1] = b.max[a1] - b.min[a1];
    let mut e2 = Vec3::ZERO;
    e2[a2] = b.max[a2] - b.min[a2];

    ([origin, origin + e1, origin + e1 + e2, origin + e2], n)
}

/// The sloped top face of a ramp.
fn ramp_top(b: &Aabb, axis: RampAxis) -> ([Vec3; 4], Vec3) {
    let h = |x: f32, z: f32| -> f32 {
        let (lo, hi, val) = match axis {
            RampAxis::PosX => (b.min.x, b.max.x, x),
            RampAxis::NegX => (b.max.x, b.min.x, x),
            RampAxis::PosZ => (b.min.z, b.max.z, z),
            RampAxis::NegZ => (b.max.z, b.min.z, z),
        };
        let t = if (hi - lo).abs() < 1e-5 { 1.0 } else { ((val - lo) / (hi - lo)).clamp(0.0, 1.0) };
        b.min.y + (b.max.y - b.min.y) * t
    };
    let c = [
        Vec3::new(b.min.x, h(b.min.x, b.min.z), b.min.z),
        Vec3::new(b.min.x, h(b.min.x, b.max.z), b.max.z),
        Vec3::new(b.max.x, h(b.max.x, b.max.z), b.max.z),
        Vec3::new(b.max.x, h(b.max.x, b.min.z), b.min.z),
    ];
    let n = (c[1] - c[0]).cross(c[2] - c[0]).normalize_or_zero();
    let n = if n.y < 0.0 { -n } else { n };
    ([c[0], c[1], c[2], c[3]], n)
}

/// World-space planar UVs, so textures line up across adjacent brushes.
fn plane_uvs(corners: &[Vec3; 4], axis: usize, scale: f32) -> [[f32; 2]; 4] {
    let s = if scale <= 0.001 { 1.0 } else { scale };
    let mut out = [[0.0f32; 2]; 4];
    for (i, c) in corners.iter().enumerate() {
        let (u, v) = match axis {
            0 => (c.z, -c.y),
            1 => (c.x, c.z),
            _ => (c.x, -c.y),
        };
        out[i] = [u / s, v / s];
    }
    out
}

// ============================================================ part geometry

/// A vertex for the instanced box models: characters, weapons and props.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct PartVertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

/// Per-instance data for one box.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct PartInstance {
    /// Rows of a 3x4 transform.
    pub row0: [f32; 4],
    pub row1: [f32; 4],
    pub row2: [f32; 4],
    pub color: [f32; 4],
    /// Texture layer, plus three spare parameters used by the shaders.
    pub params: [f32; 4],
}

impl PartInstance {
    pub fn from_matrix(m: Mat4, color: [f32; 4], layer: u32, extra: [f32; 3]) -> PartInstance {
        let c = m.to_cols_array_2d();
        PartInstance {
            row0: [c[0][0], c[1][0], c[2][0], c[3][0]],
            row1: [c[0][1], c[1][1], c[2][1], c[3][1]],
            row2: [c[0][2], c[1][2], c[2][2], c[3][2]],
            color,
            params: [layer as f32, extra[0], extra[1], extra[2]],
        }
    }
}

/// A unit cube centred on the origin, one square metre, with per-face UVs.
pub fn unit_cube() -> (Vec<PartVertex>, Vec<u16>) {
    let mut verts = Vec::with_capacity(24);
    let mut idx = Vec::with_capacity(36);
    let b = Aabb::new(Vec3::splat(-0.5), Vec3::splat(0.5));
    for &(axis, positive, _) in &FACES {
        let (corners, n) = box_face(&b, axis, positive);
        let base = verts.len() as u16;
        let uvs = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        for i in 0..4 {
            verts.push(PartVertex {
                pos: [corners[i].x, corners[i].y, corners[i].z],
                normal: [n.x, n.y, n.z],
                uv: uvs[i],
            });
        }
        idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    (verts, idx)
}

/// A flat quad on the XY plane, used for sprites and the blob shadow.
pub fn unit_quad() -> (Vec<PartVertex>, Vec<u16>) {
    let verts = vec![
        PartVertex { pos: [-0.5, -0.5, 0.0], normal: [0.0, 0.0, 1.0], uv: [0.0, 1.0] },
        PartVertex { pos: [0.5, -0.5, 0.0], normal: [0.0, 0.0, 1.0], uv: [1.0, 1.0] },
        PartVertex { pos: [0.5, 0.5, 0.0], normal: [0.0, 0.0, 1.0], uv: [1.0, 0.0] },
        PartVertex { pos: [-0.5, 0.5, 0.0], normal: [0.0, 0.0, 1.0], uv: [0.0, 0.0] },
    ];
    let idx = vec![0u16, 1, 2, 0, 2, 3];
    (verts, idx)
}

// ============================================================ character rig

/// The parts a soldier is built from. Deliberately chunky: the era's models
/// were boxes with a silhouette, and a strong silhouette is what makes a
/// target readable at speed.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Part {
    Hips = 0,
    Torso,
    Head,
    Helmet,
    ArmUpperL,
    ArmLowerL,
    ArmUpperR,
    ArmLowerR,
    LegUpperL,
    LegLowerL,
    LegUpperR,
    LegLowerR,
    Pack,
    Weapon,
}

pub const PART_COUNT: usize = 14;

/// Size of each part in metres, at the standing pose.
pub const PART_SIZE: [[f32; 3]; PART_COUNT] = [
    [0.40, 0.20, 0.26], // hips
    [0.46, 0.52, 0.28], // torso
    [0.20, 0.20, 0.21], // head
    [0.25, 0.13, 0.26], // helmet
    [0.14, 0.30, 0.15], // upper arm L
    [0.13, 0.30, 0.14], // lower arm L
    [0.14, 0.30, 0.15], // upper arm R
    [0.13, 0.30, 0.14], // lower arm R
    [0.17, 0.40, 0.19], // upper leg L
    [0.15, 0.40, 0.16], // lower leg L
    [0.17, 0.40, 0.19], // upper leg R
    [0.15, 0.40, 0.16], // lower leg R
    [0.34, 0.30, 0.16], // pack
    [0.08, 0.14, 0.62], // weapon
];

/// Which texture layer each part uses, by team.
pub fn part_material(part: Part, team_index: usize) -> Mat {
    let fatigues = match team_index {
        1 => Mat::Camo,
        2 => Mat::CamoDesert,
        _ => Mat::CamoWinter,
    };
    match part {
        Part::Head => Mat::Fabric,
        Part::Helmet => Mat::MetalPanel,
        Part::Pack => Mat::Canvas,
        Part::Weapon => Mat::MetalPanel,
        _ => fatigues,
    }
}

/// Everything the animator needs to know about a character this frame.
#[derive(Copy, Clone, Debug)]
pub struct PoseInput {
    pub yaw: f32,
    pub pitch: f32,
    /// Horizontal speed in metres per second.
    pub speed: f32,
    /// Accumulated stride phase, in radians.
    pub phase: f32,
    /// Collision height, which encodes the stance blend.
    pub height: f32,
    pub grounded: bool,
    pub dead: bool,
    /// Seconds since death, for the ragdoll-free collapse.
    pub death_time: f32,
    pub firing: f32,
    pub reloading: bool,
}

/// Builds the world transform of every part.
///
/// This is a procedural animator rather than a set of clips: a walk cycle
/// driven by stride phase, a lean driven by speed, an aim driven by pitch. It
/// costs a few dozen multiplications per character and never needs an artist.
pub fn pose_character(input: &PoseInput, origin: Vec3) -> [Mat4; PART_COUNT] {
    let mut out = [Mat4::IDENTITY; PART_COUNT];

    let stand = movement::tune::RADIUS;
    let _ = stand;
    // Scale the whole rig to the current collision height so crouching and
    // going prone shrink the model exactly as much as the hitbox.
    let scale = (input.height / 1.78).clamp(0.34, 1.05);
    let crouch = 1.0 - scale;

    let body_yaw = input.yaw;
    let run = (input.speed / 8.0).clamp(0.0, 1.0);
    let bob = (input.phase * 2.0).sin() * 0.035 * run;
    let lean = run * 0.16;

    // Collapse on death: sink and topple rather than animate a ragdoll.
    let (death_sink, death_tilt) = if input.dead {
        let t = (input.death_time / 0.6).clamp(0.0, 1.0);
        (t * 0.55 * scale, t * std::f32::consts::FRAC_PI_2 * 0.92)
    } else {
        (0.0, 0.0)
    };

    let root = Mat4::from_translation(origin + Vec3::Y * (bob - death_sink))
        * Mat4::from_rotation_y(body_yaw)
        * Mat4::from_rotation_x(-lean + death_tilt);

    let place = |m: &Mat4, part: Part, offset: Vec3, rot: Vec3| -> Mat4 {
        let size = PART_SIZE[part as usize];
        *m * Mat4::from_translation(offset * scale)
            * Mat4::from_euler(glam::EulerRot::XYZ, rot.x, rot.y, rot.z)
            * Mat4::from_scale(Vec3::new(size[0], size[1], size[2]) * scale)
    };

    // Hips sit at the top of the legs; everything hangs from them.
    let hip_y = 0.86;
    out[Part::Hips as usize] = place(&root, Part::Hips, Vec3::new(0.0, hip_y, 0.0), Vec3::ZERO);

    // The torso leans with the aim, which sells looking up and down.
    let spine_pitch = -input.pitch * 0.35;
    let torso_m = root
        * Mat4::from_translation(Vec3::new(0.0, (hip_y + 0.34) * scale, 0.0))
        * Mat4::from_rotation_x(spine_pitch);
    out[Part::Torso as usize] = torso_m * Mat4::from_scale(Vec3::from(PART_SIZE[Part::Torso as usize]) * scale);

    let head_m = torso_m
        * Mat4::from_translation(Vec3::new(0.0, 0.36 * scale, 0.0))
        * Mat4::from_rotation_x(-input.pitch * 0.55);
    out[Part::Head as usize] = head_m * Mat4::from_scale(Vec3::from(PART_SIZE[Part::Head as usize]) * scale);
    out[Part::Helmet as usize] = head_m
        * Mat4::from_translation(Vec3::new(0.0, 0.10 * scale, -0.01 * scale))
        * Mat4::from_scale(Vec3::from(PART_SIZE[Part::Helmet as usize]) * scale);
    out[Part::Pack as usize] = torso_m
        * Mat4::from_translation(Vec3::new(0.0, 0.02 * scale, 0.20 * scale))
        * Mat4::from_scale(Vec3::from(PART_SIZE[Part::Pack as usize]) * scale);

    // Legs: opposed swing, with the knee bending on the return stroke.
    let swing = input.phase.sin() * 0.85 * run;
    let swing_b = (input.phase + std::f32::consts::PI).sin() * 0.85 * run;
    let knee = |s: f32| (s.max(0.0)) * 0.9;
    let air = if input.grounded { 0.0 } else { 0.5 };

    for (side, upper, lower, s) in [
        (-1.0f32, Part::LegUpperL, Part::LegLowerL, swing),
        (1.0, Part::LegUpperR, Part::LegLowerR, swing_b),
    ] {
        let hip = root
            * Mat4::from_translation(Vec3::new(side * 0.12 * scale, (hip_y - 0.10) * scale, 0.0))
            * Mat4::from_rotation_x(s * 0.5 + air + crouch * 0.9);
        out[upper as usize] = hip
            * Mat4::from_translation(Vec3::new(0.0, -0.20 * scale, 0.0))
            * Mat4::from_scale(Vec3::from(PART_SIZE[upper as usize]) * scale);
        let knee_m = hip
            * Mat4::from_translation(Vec3::new(0.0, -0.40 * scale, 0.0))
            * Mat4::from_rotation_x(-knee(s) - air * 0.8 - crouch * 1.5);
        out[lower as usize] = knee_m
            * Mat4::from_translation(Vec3::new(0.0, -0.20 * scale, 0.0))
            * Mat4::from_scale(Vec3::from(PART_SIZE[lower as usize]) * scale);
    }

    // Arms: the right hand holds the weapon and follows the aim; the left
    // supports it. Both add a little counter-swing while running.
    let arm_swing = input.phase.sin() * 0.4 * run;
    let recoil = input.firing * 0.35;
    let reload_dip = if input.reloading { 0.55 } else { 0.0 };

    let shoulder_r = torso_m
        * Mat4::from_translation(Vec3::new(0.28 * scale, 0.16 * scale, 0.0))
        * Mat4::from_euler(glam::EulerRot::XYZ, -1.15 + recoil + reload_dip - arm_swing * 0.3, 0.25, -0.15);
    out[Part::ArmUpperR as usize] = shoulder_r
        * Mat4::from_translation(Vec3::new(0.0, -0.15 * scale, 0.0))
        * Mat4::from_scale(Vec3::from(PART_SIZE[Part::ArmUpperR as usize]) * scale);
    let elbow_r = shoulder_r
        * Mat4::from_translation(Vec3::new(0.0, -0.30 * scale, 0.0))
        * Mat4::from_rotation_x(-0.55 - reload_dip * 0.5);
    out[Part::ArmLowerR as usize] = elbow_r
        * Mat4::from_translation(Vec3::new(0.0, -0.15 * scale, 0.0))
        * Mat4::from_scale(Vec3::from(PART_SIZE[Part::ArmLowerR as usize]) * scale);

    let shoulder_l = torso_m
        * Mat4::from_translation(Vec3::new(-0.28 * scale, 0.16 * scale, 0.0))
        * Mat4::from_euler(glam::EulerRot::XYZ, -1.35 + recoil * 0.6 + reload_dip * 1.4 + arm_swing * 0.3, -0.35, 0.20);
    out[Part::ArmUpperL as usize] = shoulder_l
        * Mat4::from_translation(Vec3::new(0.0, -0.15 * scale, 0.0))
        * Mat4::from_scale(Vec3::from(PART_SIZE[Part::ArmUpperL as usize]) * scale);
    let elbow_l = shoulder_l
        * Mat4::from_translation(Vec3::new(0.0, -0.30 * scale, 0.0))
        * Mat4::from_rotation_x(-0.75);
    out[Part::ArmLowerL as usize] = elbow_l
        * Mat4::from_translation(Vec3::new(0.0, -0.15 * scale, 0.0))
        * Mat4::from_scale(Vec3::from(PART_SIZE[Part::ArmLowerL as usize]) * scale);

    // The weapon hangs off the right hand.
    out[Part::Weapon as usize] = elbow_r
        * Mat4::from_translation(Vec3::new(0.0, -0.30 * scale, -0.16 * scale))
        * Mat4::from_rotation_x(1.15)
        * Mat4::from_scale(Vec3::from(PART_SIZE[Part::Weapon as usize]) * scale);

    out
}

/// Approximate world position of a character's muzzle, for effects.
pub fn muzzle_position(pose: &[Mat4; PART_COUNT]) -> Vec3 {
    let m = pose[Part::Weapon as usize];
    (m * glam::Vec4::new(0.0, 0.0, -0.55, 1.0)).truncate()
}

// ============================================================ weapon models

/// One box of a weapon model, in weapon-local space where -Z is forward and
/// the origin sits at the grip.
#[derive(Copy, Clone, Debug)]
pub struct WeaponPart {
    pub offset: Vec3,
    pub size: Vec3,
    pub mat: Mat,
}

const fn wp(x: f32, y: f32, z: f32, sx: f32, sy: f32, sz: f32, mat: Mat) -> WeaponPart {
    WeaponPart { offset: Vec3::new(x, y, z), size: Vec3::new(sx, sy, sz), mat }
}

/// The boxes that make up each weapon silhouette.
///
/// Ten shapes cover twenty-five weapons; proportions are then varied per
/// weapon from its own stats, which is exactly how the era got a full armoury
/// out of a handful of meshes.
use crate::game::weapons::ModelShape;
use Mat::*;

const RIFLE: [WeaponPart; 7] = [
            wp(0.0, 0.0, -0.10, 0.05, 0.09, 0.52, MetalPanel),   // receiver
            wp(0.0, -0.01, -0.44, 0.032, 0.032, 0.30, PipeMetal), // barrel
            wp(0.0, 0.055, -0.16, 0.028, 0.030, 0.22, MetalPanel), // rail
            wp(0.0, 0.085, -0.02, 0.022, 0.035, 0.07, MetalPanel), // rear sight
            wp(0.0, -0.09, -0.02, 0.045, 0.13, 0.06, Rubber),      // grip
            wp(0.0, -0.075, -0.10, 0.042, 0.10, 0.10, MetalRust),  // magazine
            wp(0.0, -0.005, 0.19, 0.05, 0.085, 0.20, Rubber),      // stock
];
const BULLPUP: [WeaponPart; 5] = [
            wp(0.0, 0.0, -0.02, 0.055, 0.10, 0.48, MetalPanel),
            wp(0.0, -0.005, -0.40, 0.030, 0.030, 0.26, PipeMetal),
            wp(0.0, 0.07, -0.06, 0.026, 0.035, 0.30, MetalPanel),
            wp(0.0, -0.085, -0.16, 0.042, 0.12, 0.055, Rubber),
            wp(0.0, -0.06, 0.10, 0.042, 0.09, 0.11, MetalRust),
];
const SMG: [WeaponPart; 6] = [
            wp(0.0, 0.0, -0.06, 0.048, 0.085, 0.34, MetalPanel),
            wp(0.0, -0.005, -0.28, 0.026, 0.026, 0.16, PipeMetal),
            wp(0.0, 0.055, -0.06, 0.022, 0.026, 0.16, MetalPanel),
            wp(0.0, -0.085, -0.02, 0.040, 0.12, 0.05, Rubber),
            wp(0.0, -0.10, -0.09, 0.036, 0.16, 0.06, MetalRust),
            wp(0.0, 0.0, 0.14, 0.03, 0.05, 0.12, PipeMetal),
];
const SHOTGUN: [WeaponPart; 6] = [
            wp(0.0, 0.0, -0.10, 0.055, 0.075, 0.50, WoodPlank),
            wp(0.0, 0.012, -0.46, 0.040, 0.040, 0.32, PipeMetal),
            wp(0.0, -0.032, -0.40, 0.036, 0.036, 0.28, MetalPanel),  // tube
            wp(0.0, -0.035, -0.28, 0.062, 0.055, 0.14, WoodPlank),   // pump
            wp(0.0, -0.085, 0.0, 0.045, 0.12, 0.06, WoodPlank),
            wp(0.0, -0.01, 0.21, 0.05, 0.10, 0.22, WoodPlank),
];
const SNIPER: [WeaponPart; 7] = [
            wp(0.0, 0.0, -0.06, 0.05, 0.085, 0.62, MetalPanel),
            wp(0.0, -0.005, -0.56, 0.030, 0.030, 0.44, PipeMetal),
            wp(0.0, 0.095, -0.14, 0.05, 0.05, 0.30, MetalPanel),     // scope
            wp(0.0, 0.095, -0.30, 0.062, 0.062, 0.05, ControlPanel), // objective
            wp(0.0, -0.09, 0.02, 0.045, 0.13, 0.06, Rubber),
            wp(0.0, -0.06, -0.06, 0.040, 0.08, 0.09, MetalRust),
            wp(0.0, -0.01, 0.28, 0.055, 0.10, 0.26, WoodPlank),
];
const LMG: [WeaponPart; 7] = [
            wp(0.0, 0.0, -0.08, 0.07, 0.11, 0.58, MetalPanel),
            wp(0.0, 0.0, -0.50, 0.038, 0.038, 0.36, PipeMetal),
            wp(0.0, 0.075, -0.12, 0.030, 0.030, 0.34, MetalPanel),
            wp(0.0, -0.10, -0.14, 0.10, 0.14, 0.16, MetalRust),      // drum
            wp(0.0, -0.095, 0.04, 0.048, 0.13, 0.06, Rubber),
            wp(0.0, -0.01, 0.26, 0.055, 0.10, 0.22, Rubber),
            wp(0.0, -0.075, -0.42, 0.09, 0.09, 0.05, PipeMetal),     // bipod
];
const PISTOL_SMALL: [WeaponPart; 3] = [
            wp(0.0, 0.0, -0.06, 0.034, 0.070, 0.20, MetalPanel),
            wp(0.0, -0.09, 0.02, 0.036, 0.13, 0.05, Rubber),
            wp(0.0, -0.05, 0.01, 0.030, 0.08, 0.035, MetalRust),
];
const PISTOL_HEAVY: [WeaponPart; 4] = [
            wp(0.0, 0.0, -0.08, 0.042, 0.085, 0.26, MetalPanel),
            wp(0.0, -0.02, -0.20, 0.030, 0.030, 0.12, PipeMetal),
            wp(0.0, -0.10, 0.02, 0.042, 0.15, 0.055, WoodPlank),
            wp(0.0, -0.035, -0.04, 0.058, 0.058, 0.07, MetalRust),   // cylinder
];
const KNIFE: [WeaponPart; 2] = [
            wp(0.0, 0.0, -0.14, 0.012, 0.038, 0.20, MetalPlateDiamond),
            wp(0.0, 0.0, 0.02, 0.028, 0.032, 0.11, Rubber),
];
const SPADE: [WeaponPart; 2] = [
            wp(0.0, 0.0, -0.20, 0.11, 0.02, 0.15, MetalRust),
            wp(0.0, 0.0, -0.02, 0.022, 0.022, 0.30, WoodPlank),
];

pub fn weapon_parts(shape: ModelShape) -> &'static [WeaponPart] {
    use ModelShape::*;
    match shape {
        Rifle => &RIFLE,
        Bullpup => &BULLPUP,
        Smg => &SMG,
        Shotgun => &SHOTGUN,
        SniperLong => &SNIPER,
        Lmg => &LMG,
        PistolSmall => &PISTOL_SMALL,
        PistolHeavy => &PISTOL_HEAVY,
        Knife => &KNIFE,
        Spade => &SPADE,
    }
}

/// A scale applied to a weapon's model so heavier weapons look heavier.
pub fn weapon_model_scale(def: &crate::game::weapons::WeaponDef) -> Vec3 {
    // Longer-ranged weapons get longer barrels; higher-capacity ones get
    // bulkier bodies. Both are derived, so a new weapon needs no art.
    let length = 0.85 + (def.range_far / 200.0).clamp(0.0, 1.0) * 0.4;
    let bulk = 0.9 + (def.mag as f32 / 100.0).clamp(0.0, 1.0) * 0.35;
    Vec3::new(bulk, bulk, length)
}
