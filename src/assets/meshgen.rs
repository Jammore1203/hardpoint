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

/// Target edge length of one baked-lighting cell, in metres. Small enough that
/// a shadow edge lands where it belongs, large enough that a level is still a
/// few tens of thousands of vertices.
const LIGHT_CELL: f32 = 2.2;
/// Ceiling on the subdivision of any one face, so a stray enormous brush
/// cannot turn a map load into a minute of ray tracing.
const MAX_LIGHT_STEPS: usize = 48;

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
    //
    // Faces are subdivided before they are lit. Lighting is baked per vertex,
    // so an unsubdivided face carries exactly four light samples however large
    // it is: a ninety-metre apron gets one value at each corner and a linear
    // ramp between them, which is why every open ground plane in the game read
    // as a flat wash with a gradient across it. At this cell size the same
    // apron carries a couple of thousand samples, and the sun shadows and
    // corner occlusion that the bake already computes actually land somewhere.
    let env = &map.env;
    let mut vertices: Vec<WorldVertex> = Vec::with_capacity(faces.len() * 9);
    // Which face each vertex came from, so the bake can be run over the flat
    // vertex array rather than nested inside the face loop.
    let mut vertex_face: Vec<u32> = Vec::with_capacity(faces.len() * 9);
    // (face, base vertex, columns, rows)
    let mut grids: Vec<(usize, u32, usize, usize)> = Vec::with_capacity(faces.len());

    let mut bounds = Aabb::EMPTY;
    for (fi, f) in faces.iter().enumerate() {
        let c = &f.corners;
        // Corners run round the quad, so 0->1 and 3->2 are one edge pair.
        let span_u = ((c[1] - c[0]).length()).max((c[2] - c[3]).length());
        let span_v = ((c[3] - c[0]).length()).max((c[2] - c[1]).length());
        let steps = |span: f32| -> usize {
            ((span / LIGHT_CELL).ceil() as usize).clamp(1, MAX_LIGHT_STEPS)
        };
        let (nu, nv) = (steps(span_u), steps(span_v));

        let base = vertices.len() as u32;
        for iv in 0..=nv {
            let tv = iv as f32 / nv as f32;
            for iu in 0..=nu {
                let tu = iu as f32 / nu as f32;
                let top = c[0].lerp(c[1], tu);
                let bottom = c[3].lerp(c[2], tu);
                let p = top.lerp(bottom, tv);
                let uv_top = [
                    f.uvs[0][0] + (f.uvs[1][0] - f.uvs[0][0]) * tu,
                    f.uvs[0][1] + (f.uvs[1][1] - f.uvs[0][1]) * tu,
                ];
                let uv_bottom = [
                    f.uvs[3][0] + (f.uvs[2][0] - f.uvs[3][0]) * tu,
                    f.uvs[3][1] + (f.uvs[2][1] - f.uvs[3][1]) * tu,
                ];
                bounds.union_point(p);
                vertices.push(WorldVertex {
                    pos: [p.x, p.y, p.z],
                    uv: [
                        uv_top[0] + (uv_bottom[0] - uv_top[0]) * tv,
                        uv_top[1] + (uv_bottom[1] - uv_top[1]) * tv,
                    ],
                    color: [128, 128, 128, 255],
                    layer: f.mat.layer(),
                });
                vertex_face.push(fi as u32);
            }
        }
        grids.push((fi, base, nu, nv));
    }

    // The bake is the expensive half of loading a map - up to five collision
    // traces a vertex - and every vertex is independent, so it goes wide.
    {
        let workers = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .clamp(1, 16);
        let chunk = vertices.len().div_ceil(workers).max(1);
        let faces = &faces;
        let vertex_face = &vertex_face;
        std::thread::scope(|s| {
            for (ci, slice) in vertices.chunks_mut(chunk).enumerate() {
                let first = ci * chunk;
                s.spawn(move || {
                    for (k, v) in slice.iter_mut().enumerate() {
                        let f = &faces[vertex_face[first + k] as usize];
                        let p = Vec3::from(v.pos);
                        v.color = bake_light(map, env, p, f.normal, f.mat, f.light_scale, f.no_shadow, quality);
                    }
                });
            }
        });
    }

    // ------------------------------------------------------------ clusters
    let origin = Vec3::new(bounds.min.x, 0.0, bounds.min.z);
    let cells_x = (((bounds.max.x - bounds.min.x) / CLUSTER_SIZE).ceil() as usize).max(1);
    let cells_z = (((bounds.max.z - bounds.min.z) / CLUSTER_SIZE).ceil() as usize).max(1);
    let cell_count = cells_x * cells_z;

    let mut opaque_by_cell: Vec<Vec<u32>> = vec![Vec::new(); cell_count];
    let mut cutout_by_cell: Vec<Vec<u32>> = vec![Vec::new(); cell_count];
    let mut cell_bounds: Vec<Aabb> = vec![Aabb::EMPTY; cell_count];

    for (fi, base, nu, nv) in grids {
        let f = &faces[fi];
        let stride = (nu + 1) as u32;
        for iv in 0..nv {
            for iu in 0..nu {
                let v00 = base + iv as u32 * stride + iu as u32;
                let v10 = v00 + 1;
                let v01 = v00 + stride;
                let v11 = v01 + 1;
                // Each subdivided cell is placed in the cluster it actually
                // sits in rather than the one its parent face's centre does,
                // which is what lets a floor spanning the level still be
                // culled a piece at a time.
                let p00 = Vec3::from(vertices[v00 as usize].pos);
                let p11 = Vec3::from(vertices[v11 as usize].pos);
                let c = (p00 + p11) * 0.5;
                let cx = (((c.x - origin.x) / CLUSTER_SIZE) as usize).min(cells_x - 1);
                let cz = (((c.z - origin.z) / CLUSTER_SIZE) as usize).min(cells_z - 1);
                let ci = cz * cells_x + cx;
                for v in [v00, v10, v01, v11] {
                    cell_bounds[ci].union_point(Vec3::from(vertices[v as usize].pos));
                }
                let list = if f.cutout { &mut cutout_by_cell[ci] } else { &mut opaque_by_cell[ci] };
                // Two triangles, wound counter-clockwise seen from the front.
                list.extend_from_slice(&[v00, v10, v11, v00, v11, v01]);
            }
        }
    }

    let mut indices: Vec<u32> = Vec::with_capacity(vertices.len() * 3);
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

/// The parts a soldier is built from.
///
/// Chunky on purpose - a strong silhouette is what makes a target readable at
/// speed - but chunky is not the same as shapeless. Boots, gloves, a vest and
/// shoulder pads cost four boxes each and are what separate a soldier from a
/// stack of crates wearing a helmet.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Part {
    Hips = 0,
    Torso,
    Vest,
    Head,
    Helmet,
    Pack,
    ShoulderL,
    ShoulderR,
    ArmUpperL,
    ArmLowerL,
    GloveL,
    ArmUpperR,
    ArmLowerR,
    GloveR,
    LegUpperL,
    LegLowerL,
    BootL,
    LegUpperR,
    LegLowerR,
    BootR,
    Holster,
}

pub const PART_COUNT: usize = 21;

pub const ALL_PARTS: [Part; PART_COUNT] = [
    Part::Hips, Part::Torso, Part::Vest, Part::Head, Part::Helmet, Part::Pack,
    Part::ShoulderL, Part::ShoulderR,
    Part::ArmUpperL, Part::ArmLowerL, Part::GloveL,
    Part::ArmUpperR, Part::ArmLowerR, Part::GloveR,
    Part::LegUpperL, Part::LegLowerL, Part::BootL,
    Part::LegUpperR, Part::LegLowerR, Part::BootR,
    Part::Holster,
];

/// Size of the parts that are boxes rather than bones, in metres at the
/// standing pose. Limbs are absent: they are sized from their two endpoints,
/// which is the only way a limb can be guaranteed to reach what it is holding.
pub const PART_SIZE: [[f32; 3]; PART_COUNT] = [
    [0.38, 0.22, 0.25], // hips
    [0.43, 0.50, 0.27], // torso
    [0.455, 0.30, 0.30], // vest
    [0.185, 0.215, 0.20], // head
    [0.225, 0.135, 0.235], // helmet
    [0.30, 0.29, 0.15], // pack
    [0.15, 0.13, 0.24], // shoulder L
    [0.15, 0.13, 0.24], // shoulder R
    [0.13, 0.30, 0.14], // upper arm L
    [0.11, 0.28, 0.12], // lower arm L
    [0.10, 0.11, 0.13], // glove L
    [0.13, 0.30, 0.14], // upper arm R
    [0.11, 0.28, 0.12], // lower arm R
    [0.10, 0.11, 0.13], // glove R
    [0.17, 0.42, 0.19], // upper leg L
    [0.14, 0.40, 0.16], // lower leg L
    [0.15, 0.11, 0.26], // boot L
    [0.17, 0.42, 0.19], // upper leg R
    [0.14, 0.40, 0.16], // lower leg R
    [0.15, 0.11, 0.26], // boot R
    [0.11, 0.20, 0.09], // holster
];

/// What a part is made of and how it should be tinted. Kit stays neutral so
/// the team colour reads from the fatigues alone rather than turning the whole
/// model into a colour swatch.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PartLook {
    Fatigues,
    Skin,
    Webbing,
    Hard,
    Boots,
}

pub fn part_look(part: Part) -> PartLook {
    match part {
        Part::Head => PartLook::Skin,
        Part::Helmet | Part::ShoulderL | Part::ShoulderR => PartLook::Hard,
        Part::Vest | Part::Pack | Part::Holster => PartLook::Webbing,
        Part::GloveL | Part::GloveR | Part::BootL | Part::BootR => PartLook::Boots,
        _ => PartLook::Fatigues,
    }
}

/// Which texture layer a look uses, by team.
pub fn look_material(look: PartLook, team_index: usize) -> Mat {
    match look {
        PartLook::Fatigues => match team_index {
            1 => Mat::Camo,
            2 => Mat::CamoDesert,
            _ => Mat::CamoWinter,
        },
        PartLook::Skin => Mat::Fabric,
        PartLook::Webbing => Mat::Canvas,
        PartLook::Hard => Mat::MetalPanel,
        PartLook::Boots => Mat::Rubber,
    }
}

/// Kept for callers that still ask by part.
pub fn part_material(part: Part, team_index: usize) -> Mat {
    look_material(part_look(part), team_index)
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
    /// Where the two hands sit on the weapon this character is carrying, in
    /// weapon-local metres after the per-weapon proportion scale. The rig
    /// solves the arms to these, so a revolver and a machine gun are held
    /// differently without either being a special case here.
    pub grip: Vec3,
    pub fore: Vec3,
}

/// A finished pose: every part's world transform, plus where the weapon goes.
pub struct Pose {
    pub parts: [Mat4; PART_COUNT],
    /// Frame the weapon model is drawn in: origin at the grip, -Z down the
    /// barrel, scaled to metres.
    pub weapon: Mat4,
    /// Scale factor the rig was built at, from the stance blend.
    pub scale: f32,
}

/// A box spanning two points, `thick_x` wide and `thick_z` deep.
///
/// Limbs are built this way rather than from a chain of rotations because the
/// thing that matters about an arm is where its two ends are: a rotation chain
/// that is a few degrees off leaves the hand somewhere near the weapon, and
/// "near" is exactly what reads as broken.
fn bone(from: Vec3, to: Vec3, thick_x: f32, thick_z: f32, twist_ref: Vec3) -> Mat4 {
    let d = to - from;
    let len = d.length();
    if len < 1e-5 {
        return Mat4::from_translation(from) * Mat4::from_scale(Vec3::new(thick_x, 1e-4, thick_z));
    }
    let y = d / len;
    let mut x = twist_ref.cross(y);
    if x.length_squared() < 1e-8 {
        x = if y.x.abs() < 0.9 { Vec3::X.cross(y) } else { Vec3::Z.cross(y) };
    }
    let x = x.normalize();
    let z = x.cross(y);
    Mat4::from_cols(
        (x * thick_x).extend(0.0),
        (y * len).extend(0.0),
        (z * thick_z).extend(0.0),
        (from + d * 0.5).extend(1.0),
    )
}

/// Where the joint between two bones of length `upper` and `lower` sits, given
/// the two ends and which way the joint should break.
///
/// Plain two-bone inverse kinematics. The target is pulled inside reach first,
/// so an arm asked for something it cannot touch straightens toward it instead
/// of collapsing or producing a NaN.
fn joint(root: Vec3, target: Vec3, upper: f32, lower: f32, pole: Vec3) -> Vec3 {
    let to = target - root;
    let dist = to.length().clamp((upper - lower).abs() + 1e-3, upper + lower - 1e-3);
    if dist < 1e-4 { return root + pole.normalize_or_zero() * upper; }
    let dir = to.normalize_or_zero();
    // Distance along the root-to-target line at which the joint sits.
    let along = (dist * dist + upper * upper - lower * lower) / (2.0 * dist);
    let out = (upper * upper - along * along).max(0.0).sqrt();
    // The bend direction: the pole, with anything along the limb removed.
    let mut side = pole - dir * pole.dot(dir);
    if side.length_squared() < 1e-8 {
        side = if dir.y.abs() < 0.9 { Vec3::Y.cross(dir) } else { Vec3::X.cross(dir) };
    }
    root + dir * along + side.normalize_or_zero() * out
}

/// Builds the world transform of every part, and of the weapon in their hands.
///
/// This is a procedural animator rather than a set of clips: a walk cycle
/// driven by stride phase, a lean driven by speed, a weapon carried on the aim
/// line and arms solved to reach it. It costs a few dozen multiplications per
/// character and never needs an artist.
pub fn pose_character(input: &PoseInput, origin: Vec3) -> Pose {
    let mut out = [Mat4::IDENTITY; PART_COUNT];

    // Scale the whole rig to the current collision height so crouching and
    // going prone shrink the model exactly as much as the hitbox.
    let s = (input.height / 1.78).clamp(0.34, 1.05);
    let crouch = (1.0 - s).clamp(0.0, 1.0);

    let run = (input.speed / 8.0).clamp(0.0, 1.0);
    let bob = (input.phase * 2.0).sin() * 0.030 * run;
    // A running soldier leans into it. Negative X rotation tips the model's
    // forward axis - which is -Z - downward.
    let lean = -run * 0.17;

    let (death_sink, death_tilt) = if input.dead {
        let t = (input.death_time / 0.6).clamp(0.0, 1.0);
        (t * 0.52 * s, t * std::f32::consts::FRAC_PI_2 * 0.92)
    } else {
        (0.0, 0.0)
    };

    let root = Mat4::from_translation(origin + Vec3::Y * (bob - death_sink))
        * Mat4::from_rotation_y(input.yaw)
        * Mat4::from_rotation_x(lean - death_tilt);

    // ------------------------------------------------------------ skeleton
    // Heights are in rig-local metres before the stance scale.
    let hip_y = 0.90;
    let chest_y = hip_y + 0.30;
    let shoulder_y = hip_y + 0.46;
    let neck_y = hip_y + 0.58;

    let at = |x: f32, y: f32, z: f32| -> Vec3 {
        root.transform_point3(Vec3::new(x * s, y * s, z * s))
    };

    // The torso pitches with the aim, but only partly: the rest is taken up by
    // the arms, which is both how a person does it and what keeps the head
    // from swinging through the chest at extreme angles.
    let spine = input.pitch * 0.30;
    let torso_m = root
        * Mat4::from_translation(Vec3::new(0.0, chest_y * s, 0.0))
        * Mat4::from_rotation_x(spine);
    let torso_pt = |x: f32, y: f32, z: f32| -> Vec3 {
        torso_m.transform_point3(Vec3::new(x * s, y * s, z * s))
    };

    out[Part::Hips as usize] = Mat4::from_translation(at(0.0, hip_y, 0.0))
        * Mat4::from_rotation_y(input.yaw)
        * Mat4::from_scale(Vec3::from(PART_SIZE[Part::Hips as usize]) * s);
    out[Part::Torso as usize] = torso_m
        * Mat4::from_translation(Vec3::new(0.0, 0.05 * s, 0.0))
        * Mat4::from_scale(Vec3::from(PART_SIZE[Part::Torso as usize]) * s);
    // The vest sits proud of the chest, slightly forward and a little short,
    // so it reads as worn rather than as a wider torso.
    out[Part::Vest as usize] = torso_m
        * Mat4::from_translation(Vec3::new(0.0, 0.02 * s, -0.012 * s))
        * Mat4::from_scale(Vec3::from(PART_SIZE[Part::Vest as usize]) * s);
    out[Part::Pack as usize] = torso_m
        * Mat4::from_translation(Vec3::new(0.0, 0.02 * s, 0.20 * s))
        * Mat4::from_scale(Vec3::from(PART_SIZE[Part::Pack as usize]) * s);

    // Head: the remainder of the aim, so looking up raises the face.
    let head_m = root
        * Mat4::from_translation(Vec3::new(0.0, neck_y * s, 0.0))
        * Mat4::from_rotation_x(input.pitch * 0.62);
    out[Part::Head as usize] = head_m
        * Mat4::from_translation(Vec3::new(0.0, 0.045 * s, 0.0))
        * Mat4::from_scale(Vec3::from(PART_SIZE[Part::Head as usize]) * s);
    out[Part::Helmet as usize] = head_m
        * Mat4::from_translation(Vec3::new(0.0, 0.145 * s, 0.008 * s))
        * Mat4::from_scale(Vec3::from(PART_SIZE[Part::Helmet as usize]) * s);

    for (side, part) in [(-1.0f32, Part::ShoulderL), (1.0, Part::ShoulderR)] {
        out[part as usize] = torso_m
            * Mat4::from_translation(Vec3::new(side * 0.235 * s, 0.155 * s, 0.0))
            * Mat4::from_rotation_z(side * 0.24)
            * Mat4::from_scale(Vec3::from(PART_SIZE[part as usize]) * s);
    }

    out[Part::Holster as usize] = root
        * Mat4::from_translation(Vec3::new(0.20 * s, (hip_y - 0.14) * s, 0.03 * s))
        * Mat4::from_rotation_z(0.18)
        * Mat4::from_scale(Vec3::from(PART_SIZE[Part::Holster as usize]) * s);

    // ---------------------------------------------------------------- legs
    // Opposed swing, with the knee breaking forward on the return stroke and
    // both knees bent by the crouch blend.
    let stride = input.phase.sin() * 0.62 * run;
    let air = if input.grounded { 0.0 } else { 0.35 };
    let up = root.transform_vector3(Vec3::Y);
    let fwd = root.transform_vector3(-Vec3::Z);

    let thigh = 0.44 * s;
    let shin = 0.44 * s;
    for (side, swing, upper, lower, boot) in [
        (-1.0f32, stride, Part::LegUpperL, Part::LegLowerL, Part::BootL),
        (1.0, -stride, Part::LegUpperR, Part::LegLowerR, Part::BootR),
    ] {
        let hip = at(side * 0.11, hip_y - 0.06, 0.0);
        // Foot placement drives the leg, so feet land where they look like
        // they land instead of floating a hand's width above the ground.
        let lift = (swing.max(0.0)) * 0.20 * s + air * 0.12 * s;
        let reach = swing * 0.34 * s;
        let squat = (crouch * 0.34 + air * 0.10) * s;
        let ankle = hip + fwd * reach - up * ((thigh + shin) * 0.94 - lift - squat);
        // Knees break forward, and outward a little so they never cross.
        let pole = (fwd + up * 0.10 + root.transform_vector3(Vec3::X) * (side * 0.20)).normalize();
        let knee = joint(hip, ankle, thigh, shin, pole);

        out[upper as usize] = bone(hip, knee, PART_SIZE[upper as usize][0] * s, PART_SIZE[upper as usize][2] * s, fwd);
        out[lower as usize] = bone(knee, ankle, PART_SIZE[lower as usize][0] * s, PART_SIZE[lower as usize][2] * s, fwd);
        // The boot is level with the ground, not with the shin.
        let toe = (ankle - knee).normalize_or_zero() * 0.0;
        let _ = toe;
        out[boot as usize] = Mat4::from_translation(ankle - up * (0.04 * s) + fwd * (0.05 * s))
            * Mat4::from_rotation_y(input.yaw)
            * Mat4::from_scale(Vec3::from(PART_SIZE[boot as usize]) * s);
    }

    // -------------------------------------------------------------- weapon
    // The weapon is placed first and the arms are solved to it. Doing it the
    // other way - rotating a shoulder, then an elbow, then hanging a gun off
    // the end - is what left the model holding a rifle sideways across its
    // chest with both elbows out.
    let aim = root
        * Mat4::from_translation(Vec3::new(0.0, shoulder_y * s, 0.0))
        * Mat4::from_rotation_x(input.pitch - spine * 0.0);

    let recoil = input.firing;
    let reload = if input.reloading { 1.0 } else { 0.0 };
    // Carried across the body at the ready: right of centre, just under the
    // eye line, muzzle forward.
    let carry = Vec3::new(0.085, -0.155 - reload * 0.12 - run * 0.05, -0.20 + recoil * 0.045);
    let weapon = aim
        * Mat4::from_translation(carry * s)
        * Mat4::from_rotation_x(recoil * 0.22 + reload * 0.45 + run * 0.10)
        * Mat4::from_rotation_z(reload * 0.30 - 0.04)
        * Mat4::from_scale(Vec3::splat(s));

    // ---------------------------------------------------------------- arms
    let upper_arm = 0.31 * s;
    let fore_arm = 0.29 * s;
    // Grip and handguard in weapon-local metres; the weapon models put the
    // grip at the origin and run forward along -Z.
    let grip = weapon.transform_point3(input.grip);
    let fore = weapon.transform_point3(input.fore);

    let right = root.transform_vector3(Vec3::X);
    for (side, hand, upper, lower, glove) in [
        (1.0f32, grip, Part::ArmUpperR, Part::ArmLowerR, Part::GloveR),
        (-1.0, fore, Part::ArmUpperL, Part::ArmLowerL, Part::GloveL),
    ] {
        let shoulder = torso_pt(side * 0.225, 0.145, 0.0);
        // Elbows break down and away from the body.
        let pole = (-up * 1.0 + right * (side * 0.75) - fwd * 0.30).normalize();
        let elbow = joint(shoulder, hand, upper_arm, fore_arm, pole);
        out[upper as usize] = bone(shoulder, elbow, PART_SIZE[upper as usize][0] * s, PART_SIZE[upper as usize][2] * s, fwd);
        out[lower as usize] = bone(elbow, hand, PART_SIZE[lower as usize][0] * s, PART_SIZE[lower as usize][2] * s, fwd);
        out[glove as usize] = Mat4::from_translation(hand)
            * Mat4::from_rotation_y(input.yaw)
            * Mat4::from_rotation_x(input.pitch * 0.5)
            * Mat4::from_scale(Vec3::from(PART_SIZE[glove as usize]) * s);
    }

    Pose { parts: out, weapon, scale: s }
}

/// Approximate world position of a character's muzzle, for effects.
pub fn muzzle_position(pose: &Pose) -> Vec3 {
    pose.weapon.transform_point3(Vec3::new(0.0, 0.0, -0.62))
}

// ============================================================ weapon models

/// One box of a weapon model, in weapon-local space where -Z is forward and
/// the origin sits between the hands.
///
/// The rotation is what buys the silhouette: a magazine canted forward, a
/// pistol grip raked back, a scope ring standing proud. Axis-aligned boxes
/// alone can only ever describe a brick with smaller bricks stuck to it, and
/// that is what these models used to look like.
#[derive(Copy, Clone, Debug)]
pub struct WeaponPart {
    pub offset: Vec3,
    pub size: Vec3,
    /// Euler XYZ in radians, applied about the part's own centre.
    pub rot: Vec3,
    pub mat: Mat,
}

const fn wp(x: f32, y: f32, z: f32, sx: f32, sy: f32, sz: f32, mat: Mat) -> WeaponPart {
    WeaponPart { offset: Vec3::new(x, y, z), size: Vec3::new(sx, sy, sz), rot: Vec3::ZERO, mat }
}

/// The same, tilted about X (the usual case: rake and cant).
const fn wpx(x: f32, y: f32, z: f32, sx: f32, sy: f32, sz: f32, rx: f32, mat: Mat) -> WeaponPart {
    WeaponPart { offset: Vec3::new(x, y, z), size: Vec3::new(sx, sy, sz), rot: Vec3::new(rx, 0.0, 0.0), mat }
}

/// A weapon silhouette, plus the three points anything else needs from it:
/// where each hand goes and where the flash comes out.
///
/// These used to be inferred - firing hand on the lowest box, support hand on
/// the most forward one - which is a reasonable guess for a rifle and quite
/// wrong for a revolver, whose lowest box is the grip's bottom edge and whose
/// most forward box is the barrel.
pub struct WeaponModel {
    pub parts: &'static [WeaponPart],
    pub grip: Vec3,
    pub fore: Vec3,
    pub muzzle: Vec3,
}

use crate::game::weapons::ModelShape;
use Mat::*;

// Assault rifle: separate upper and lower, free-floating handguard, flat-top
// rail, collapsible stock on a buffer tube.
const RIFLE: [WeaponPart; 18] = [
    wp(0.0, 0.014, -0.07, 0.050, 0.064, 0.34, MetalPanel),      // upper receiver
    wp(0.0, -0.040, 0.015, 0.046, 0.058, 0.15, MetalPanel),     // lower receiver
    wp(0.0, 0.006, -0.31, 0.054, 0.058, 0.23, MetalPanel),      // handguard
    wp(0.0, 0.006, -0.49, 0.019, 0.019, 0.20, PipeMetal),       // barrel
    wp(0.0, 0.006, -0.615, 0.029, 0.029, 0.055, MetalRust),     // muzzle brake
    wp(0.0, 0.032, -0.43, 0.029, 0.030, 0.048, PipeMetal),      // gas block
    wp(0.0, 0.051, -0.17, 0.030, 0.014, 0.40, MetalPlateDiamond), // top rail
    wp(0.0, 0.076, 0.02, 0.026, 0.030, 0.032, MetalPanel),      // rear sight
    wp(0.0, 0.072, -0.45, 0.014, 0.036, 0.020, MetalPanel),     // front post
    wp(0.026, 0.052, 0.115, 0.022, 0.015, 0.085, MetalPanel),   // charging handle
    wp(0.029, 0.020, -0.02, 0.008, 0.030, 0.095, MetalRust),    // ejection port
    wpx(0.0, -0.145, 0.010, 0.038, 0.185, 0.072, -0.16, MetalRust), // magazine
    wpx(0.0, -0.112, 0.088, 0.040, 0.150, 0.052, 0.34, Rubber), // pistol grip
    wp(0.0, -0.056, 0.048, 0.030, 0.010, 0.052, MetalPanel),    // trigger guard
    wp(0.0, -0.038, 0.052, 0.012, 0.026, 0.012, MetalPlateDiamond), // trigger
    wp(0.0, 0.014, 0.155, 0.031, 0.031, 0.13, PipeMetal),       // buffer tube
    wp(0.0, 0.006, 0.235, 0.042, 0.082, 0.13, MetalPanel),      // stock
    wp(0.0, 0.002, 0.305, 0.046, 0.094, 0.022, Tire),           // butt pad
];

// Bullpup: the whole action sits behind the grip, so the same barrel length
// comes in a much shorter weapon.
const BULLPUP: [WeaponPart; 15] = [
    wp(0.0, 0.012, 0.04, 0.056, 0.090, 0.40, MetalPanel),       // body shell
    wp(0.0, 0.010, -0.24, 0.048, 0.056, 0.20, MetalPanel),      // handguard
    wp(0.0, 0.010, -0.41, 0.019, 0.019, 0.17, PipeMetal),       // barrel
    wp(0.0, 0.010, -0.515, 0.027, 0.027, 0.05, MetalRust),      // flash hider
    wp(0.0, 0.058, -0.10, 0.028, 0.014, 0.44, MetalPlateDiamond), // rail
    wp(0.0, 0.086, -0.06, 0.048, 0.044, 0.16, MetalPanel),      // optic body
    wp(0.0, 0.086, -0.145, 0.052, 0.052, 0.022, ControlPanel),  // objective
    wp(0.0, 0.086, 0.028, 0.046, 0.046, 0.020, Glass),          // eyepiece
    wpx(0.0, -0.125, 0.145, 0.040, 0.150, 0.070, -0.12, MetalRust), // magazine
    wpx(0.0, -0.100, -0.055, 0.038, 0.135, 0.050, 0.30, Rubber), // grip
    wp(0.0, -0.048, -0.095, 0.028, 0.010, 0.050, MetalPanel),   // trigger guard
    wp(0.0, -0.030, 0.10, 0.052, 0.030, 0.12, MetalPanel),      // magwell shoulder
    wp(0.0, 0.006, 0.255, 0.052, 0.095, 0.03, Tire),            // butt pad
    wp(0.026, 0.040, 0.10, 0.010, 0.026, 0.07, MetalRust),      // ejection port
    wp(0.0, -0.028, -0.34, 0.024, 0.026, 0.05, MetalPanel),     // sling loop
];

// Submachine gun: short, blocky, folding stock, magazine through the grip.
const SMG: [WeaponPart; 14] = [
    wp(0.0, 0.012, -0.06, 0.048, 0.070, 0.28, MetalPanel),      // receiver
    wp(0.0, 0.012, -0.235, 0.036, 0.040, 0.10, MetalPanel),     // barrel shroud
    wp(0.0, 0.012, -0.315, 0.017, 0.017, 0.09, PipeMetal),      // barrel
    wp(0.0, 0.048, -0.10, 0.026, 0.013, 0.28, MetalPlateDiamond), // rail
    wp(0.0, 0.070, -0.005, 0.024, 0.028, 0.028, MetalPanel),    // rear sight
    wp(0.0, 0.066, -0.245, 0.013, 0.030, 0.018, MetalPanel),    // front sight
    wpx(0.0, -0.135, 0.005, 0.034, 0.185, 0.056, -0.10, MetalRust), // magazine
    wpx(0.0, -0.098, 0.062, 0.038, 0.130, 0.048, 0.28, Rubber), // grip
    wp(0.0, -0.050, 0.030, 0.028, 0.010, 0.048, MetalPanel),    // trigger guard
    wpx(0.0, -0.030, -0.175, 0.030, 0.090, 0.042, -0.22, Rubber), // foregrip
    wp(0.024, 0.030, 0.02, 0.010, 0.026, 0.075, MetalRust),     // ejection port
    wp(0.0, 0.014, 0.115, 0.026, 0.026, 0.09, PipeMetal),       // stock strut
    wp(0.0, 0.012, 0.185, 0.044, 0.070, 0.024, Tire),           // butt plate
    wp(0.0, 0.040, 0.09, 0.018, 0.014, 0.06, MetalPanel),       // charging handle
];

// Pump shotgun: wooden furniture, a magazine tube slung under the barrel.
const SHOTGUN: [WeaponPart; 13] = [
    wp(0.0, 0.000, -0.05, 0.052, 0.078, 0.26, MetalPanel),      // receiver
    wp(0.0, 0.022, -0.36, 0.032, 0.032, 0.38, PipeMetal),       // barrel
    wp(0.0, -0.026, -0.32, 0.028, 0.028, 0.30, MetalPanel),     // magazine tube
    wp(0.0, -0.024, -0.235, 0.058, 0.052, 0.13, WoodPlank),     // pump
    wp(0.0, 0.048, -0.05, 0.022, 0.014, 0.10, MetalPlateDiamond), // rib
    wp(0.0, 0.046, -0.535, 0.012, 0.020, 0.014, MetalRust),     // bead sight
    wp(0.0, -0.052, 0.045, 0.024, 0.010, 0.050, MetalPanel),    // trigger guard
    wpx(0.0, -0.086, 0.085, 0.044, 0.115, 0.055, 0.38, WoodPlank), // grip
    wp(0.0, 0.006, 0.175, 0.048, 0.092, 0.16, WoodPlank),       // stock
    wp(0.0, -0.008, 0.262, 0.050, 0.100, 0.024, Tire),          // butt pad
    wp(0.026, 0.000, -0.005, 0.010, 0.030, 0.09, MetalRust),    // ejection port
    wp(0.0, -0.046, -0.06, 0.030, 0.020, 0.08, MetalPanel),     // loading gate
    wp(0.0, -0.006, -0.50, 0.030, 0.026, 0.05, MetalRust),      // choke
];

// Bolt rifle: long heavy barrel, big glass, bipod, cheek riser.
const SNIPER: [WeaponPart; 17] = [
    wp(0.0, 0.006, -0.04, 0.048, 0.070, 0.34, MetalPanel),      // action
    wp(0.0, 0.006, -0.40, 0.023, 0.023, 0.40, PipeMetal),       // barrel
    wp(0.0, 0.006, -0.635, 0.032, 0.032, 0.07, MetalRust),      // muzzle brake
    wp(0.0, 0.048, -0.10, 0.030, 0.014, 0.34, MetalPlateDiamond), // rail
    wp(0.0, 0.098, -0.10, 0.052, 0.052, 0.30, MetalPanel),      // scope tube
    wp(0.0, 0.098, -0.265, 0.062, 0.062, 0.045, ControlPanel),  // objective bell
    wp(0.0, 0.098, 0.058, 0.056, 0.056, 0.030, Glass),          // eyepiece
    wp(0.0, 0.098, -0.13, 0.058, 0.058, 0.028, MetalRust),      // elevation turret
    wp(0.0, 0.074, -0.16, 0.026, 0.036, 0.026, MetalPanel),     // front ring
    wp(0.0, 0.074, -0.03, 0.026, 0.036, 0.026, MetalPanel),     // rear ring
    wp(0.030, 0.014, 0.05, 0.030, 0.016, 0.016, MetalPlateDiamond), // bolt handle
    wpx(0.0, -0.118, 0.005, 0.036, 0.130, 0.062, -0.14, MetalRust), // magazine
    wpx(0.0, -0.106, 0.095, 0.042, 0.140, 0.052, 0.32, Rubber), // grip
    wp(0.0, -0.052, 0.055, 0.028, 0.010, 0.050, MetalPanel),    // trigger guard
    wp(0.0, 0.030, 0.215, 0.048, 0.062, 0.16, WoodPlank),       // cheek riser
    wp(0.0, -0.020, 0.240, 0.046, 0.090, 0.12, WoodPlank),      // butt stock
    wpx(0.0, -0.075, -0.48, 0.090, 0.075, 0.016, 0.0, PipeMetal), // folded bipod
];

// Light machine gun: box magazine, carry handle, heavy barrel, bipod.
const LMG: [WeaponPart; 16] = [
    wp(0.0, 0.010, -0.05, 0.062, 0.098, 0.36, MetalPanel),      // receiver
    wp(0.0, 0.010, -0.40, 0.026, 0.026, 0.36, PipeMetal),       // barrel
    wp(0.0, 0.010, -0.605, 0.036, 0.036, 0.06, MetalRust),      // flash hider
    wp(0.0, 0.048, -0.34, 0.034, 0.048, 0.20, MetalPlateDiamond), // heat shield
    wp(0.0, 0.078, 0.02, 0.028, 0.030, 0.032, MetalPanel),      // rear sight
    wp(0.0, 0.074, -0.44, 0.014, 0.040, 0.020, MetalPanel),     // front post
    wp(0.0, 0.084, -0.16, 0.028, 0.026, 0.16, PipeMetal),       // carry handle
    wp(0.0, -0.128, -0.02, 0.098, 0.150, 0.170, MetalRust),     // ammunition box
    wp(0.0, -0.128, -0.108, 0.086, 0.120, 0.010, HazardStripe), // box latch
    wpx(0.0, -0.112, 0.105, 0.044, 0.150, 0.055, 0.32, Rubber), // grip
    wp(0.0, -0.056, 0.062, 0.032, 0.011, 0.055, MetalPanel),    // trigger guard
    wp(0.0, 0.010, 0.205, 0.050, 0.096, 0.16, MetalPanel),      // stock
    wp(0.0, 0.004, 0.292, 0.052, 0.104, 0.024, Tire),           // butt pad
    wp(0.0, -0.070, -0.46, 0.110, 0.086, 0.018, PipeMetal),     // bipod legs
    wp(0.0, -0.030, -0.46, 0.030, 0.040, 0.030, MetalPanel),    // bipod mount
    wp(0.028, 0.024, -0.02, 0.012, 0.034, 0.10, MetalRust),     // feed cover
];

// Service pistol.
const PISTOL_SMALL: [WeaponPart; 9] = [
    wp(0.0, 0.028, -0.075, 0.032, 0.046, 0.185, MetalPanel),    // slide
    wp(0.0, 0.052, -0.075, 0.014, 0.008, 0.175, MetalPlateDiamond), // slide serration rib
    wp(0.0, 0.044, -0.155, 0.012, 0.016, 0.014, MetalRust),     // front sight
    wp(0.0, 0.046, 0.005, 0.024, 0.018, 0.016, MetalRust),      // rear sight
    wp(0.0, 0.008, -0.170, 0.014, 0.014, 0.030, PipeMetal),     // muzzle
    wp(0.0, -0.005, -0.030, 0.030, 0.030, 0.110, MetalPanel),   // frame
    wpx(0.0, -0.088, 0.030, 0.032, 0.140, 0.048, 0.24, Rubber), // grip
    wp(0.0, -0.038, -0.010, 0.022, 0.010, 0.044, MetalPanel),   // trigger guard
    wpx(0.0, -0.088, 0.030, 0.020, 0.130, 0.030, 0.24, MetalRust), // magazine floorplate
];

// Heavy revolver.
const PISTOL_HEAVY: [WeaponPart; 9] = [
    wp(0.0, 0.024, -0.150, 0.024, 0.026, 0.170, PipeMetal),     // barrel
    wp(0.0, 0.046, -0.150, 0.020, 0.016, 0.165, MetalPlateDiamond), // top rib
    wp(0.0, -0.002, -0.150, 0.022, 0.024, 0.150, MetalPanel),   // ejector shroud
    wp(0.0, 0.020, -0.030, 0.058, 0.058, 0.070, MetalRust),     // cylinder
    wp(0.0, 0.022, 0.030, 0.030, 0.052, 0.075, MetalPanel),     // frame
    wp(0.0, 0.056, 0.052, 0.020, 0.024, 0.026, MetalRust),      // hammer
    wpx(0.0, -0.078, 0.070, 0.036, 0.145, 0.058, 0.30, WoodPlank), // grip
    wp(0.0, -0.030, 0.020, 0.024, 0.010, 0.050, MetalPanel),    // trigger guard
    wp(0.0, 0.050, -0.225, 0.012, 0.018, 0.014, MetalRust),     // front sight
];

// Combat knife.
const KNIFE: [WeaponPart; 5] = [
    wp(0.0, 0.006, -0.150, 0.010, 0.036, 0.170, MetalPlateDiamond), // blade
    wp(0.0, 0.020, -0.235, 0.008, 0.020, 0.045, MetalPanel),    // point taper
    wp(0.0, -0.010, -0.140, 0.011, 0.014, 0.110, MetalRust),    // serrated spine
    wp(0.0, 0.000, -0.052, 0.038, 0.030, 0.016, MetalPanel),    // guard
    wp(0.0, 0.000, 0.010, 0.026, 0.030, 0.110, Rubber),         // handle
];

// Entrenching tool.
const SPADE: [WeaponPart; 5] = [
    wp(0.0, 0.000, -0.235, 0.120, 0.018, 0.130, MetalRust),     // blade
    wp(0.0, 0.000, -0.300, 0.090, 0.014, 0.045, MetalPlateDiamond), // blade edge
    wp(0.0, 0.000, -0.165, 0.040, 0.030, 0.055, MetalPanel),    // socket
    wp(0.0, 0.000, -0.040, 0.024, 0.024, 0.210, WoodPlank),     // shaft
    wp(0.0, 0.000, 0.075, 0.048, 0.026, 0.030, Rubber),         // grip
];

const M_RIFLE: WeaponModel = WeaponModel {
    parts: &RIFLE,
    grip: Vec3::new(0.0, -0.105, 0.085),
    fore: Vec3::new(0.0, -0.035, -0.31),
    muzzle: Vec3::new(0.0, 0.006, -0.645),
};
const M_BULLPUP: WeaponModel = WeaponModel {
    parts: &BULLPUP,
    grip: Vec3::new(0.0, -0.095, -0.055),
    fore: Vec3::new(0.0, -0.030, -0.26),
    muzzle: Vec3::new(0.0, 0.010, -0.545),
};
const M_SMG: WeaponModel = WeaponModel {
    parts: &SMG,
    grip: Vec3::new(0.0, -0.092, 0.062),
    fore: Vec3::new(0.0, -0.055, -0.185),
    muzzle: Vec3::new(0.0, 0.012, -0.365),
};
const M_SHOTGUN: WeaponModel = WeaponModel {
    parts: &SHOTGUN,
    grip: Vec3::new(0.0, -0.082, 0.085),
    fore: Vec3::new(0.0, -0.036, -0.235),
    muzzle: Vec3::new(0.0, 0.022, -0.555),
};
const M_SNIPER: WeaponModel = WeaponModel {
    parts: &SNIPER,
    grip: Vec3::new(0.0, -0.100, 0.095),
    fore: Vec3::new(0.0, -0.040, -0.28),
    muzzle: Vec3::new(0.0, 0.006, -0.675),
};
const M_LMG: WeaponModel = WeaponModel {
    parts: &LMG,
    grip: Vec3::new(0.0, -0.106, 0.105),
    fore: Vec3::new(0.0, -0.040, -0.34),
    muzzle: Vec3::new(0.0, 0.010, -0.640),
};
const M_PISTOL_SMALL: WeaponModel = WeaponModel {
    parts: &PISTOL_SMALL,
    grip: Vec3::new(0.0, -0.080, 0.030),
    fore: Vec3::new(0.0, -0.045, -0.060),
    muzzle: Vec3::new(0.0, 0.020, -0.190),
};
const M_PISTOL_HEAVY: WeaponModel = WeaponModel {
    parts: &PISTOL_HEAVY,
    grip: Vec3::new(0.0, -0.072, 0.070),
    fore: Vec3::new(0.0, -0.040, -0.020),
    muzzle: Vec3::new(0.0, 0.024, -0.240),
};
const M_KNIFE: WeaponModel = WeaponModel {
    parts: &KNIFE,
    grip: Vec3::new(0.0, 0.000, 0.010),
    fore: Vec3::new(0.0, 0.000, 0.010),
    muzzle: Vec3::new(0.0, 0.010, -0.250),
};
const M_SPADE: WeaponModel = WeaponModel {
    parts: &SPADE,
    grip: Vec3::new(0.0, 0.000, 0.060),
    fore: Vec3::new(0.0, 0.000, -0.080),
    muzzle: Vec3::new(0.0, 0.000, -0.300),
};

pub fn weapon_model(shape: ModelShape) -> &'static WeaponModel {
    use ModelShape::*;
    match shape {
        Rifle => &M_RIFLE,
        Bullpup => &M_BULLPUP,
        Smg => &M_SMG,
        Shotgun => &M_SHOTGUN,
        SniperLong => &M_SNIPER,
        Lmg => &M_LMG,
        PistolSmall => &M_PISTOL_SMALL,
        PistolHeavy => &M_PISTOL_HEAVY,
        Knife => &M_KNIFE,
        Spade => &M_SPADE,
    }
}

pub fn weapon_parts(shape: ModelShape) -> &'static [WeaponPart] {
    weapon_model(shape).parts
}

/// The transform for one box of a weapon, given the frame the weapon is drawn
/// in and the per-weapon proportions.
///
/// The proportion scale multiplies offsets and box dimensions separately
/// rather than wrapping the whole thing, so a canted magazine on a long
/// weapon comes out longer rather than sheared.
pub fn weapon_part_matrix(base: Mat4, model_scale: Vec3, part: &WeaponPart) -> Mat4 {
    base * Mat4::from_translation(part.offset * model_scale)
        * Mat4::from_euler(glam::EulerRot::XYZ, part.rot.x, part.rot.y, part.rot.z)
        * Mat4::from_scale(part.size * model_scale)
}

/// A scale applied to a weapon's model so heavier weapons look heavier.
pub fn weapon_model_scale(def: &crate::game::weapons::WeaponDef) -> Vec3 {
    // Longer-ranged weapons get longer barrels; higher-capacity ones get
    // bulkier bodies. Both are derived, so a new weapon needs no art.
    let length = 0.88 + (def.range_far / 200.0).clamp(0.0, 1.0) * 0.30;
    let bulk = 0.92 + (def.mag as f32 / 100.0).clamp(0.0, 1.0) * 0.26;
    Vec3::new(bulk, bulk, length)
}
