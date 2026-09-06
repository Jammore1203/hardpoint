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
use glam::{Mat4, Vec2, Vec3};

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
    /// `(brush index, index range, bounds)` for every breakable brush, drawn
    /// separately from the clusters so one can be dropped from the frame
    /// without touching the buffers.
    pub breakables: Vec<(u32, std::ops::Range<u32>, Aabb)>,
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
const LIGHT_CELL: f32 = 1.7;
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
    /// Index into `map.brushes`, or `u32::MAX` for decor. Breakable brushes
    /// need their triangles addressable at draw time.
    brush: u32,
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

    for (list_index, list) in [&map.brushes, &map.decor].into_iter().enumerate() {
        for (brush_index, b) in list.iter().enumerate() {
            let current_brush = if list_index == 0 { brush_index as u32 } else { u32::MAX };
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
                        faces.push(Face { brush: current_brush, corners, normal, uvs, mat, light_scale: b.light_scale, cutout, no_shadow });
                    }
                }
                BrushKind::Clipped(clips) => {
                    // A box cut by vertical planes is a convex polygon in XZ
                    // extruded between two heights, so the geometry is that
                    // polygon on top, the same polygon underneath, and one
                    // quad per edge. Clipping the footprint once and walking
                    // its edges is both simpler and more robust than trying to
                    // clip six box faces individually.
                    let poly = clip_footprint(&b.aabb, clips.slice());
                    if poly.len() < 3 { continue; }
                    let (y0, y1) = (b.aabb.min.y, b.aabb.max.y);

                    if b.faces.has(FaceMask::POS_Y) {
                        let c: Vec<Vec3> = poly.iter().map(|p| Vec3::new(p.x, y1, p.y)).collect();
                        push_fan(&mut faces, current_brush, &c, Vec3::Y, b, b.top, cutout, no_shadow);
                    }
                    if b.faces.has(FaceMask::NEG_Y) {
                        let c: Vec<Vec3> = poly.iter().rev().map(|p| Vec3::new(p.x, y0, p.y)).collect();
                        push_fan(&mut faces, current_brush, &c, Vec3::NEG_Y, b, b.mat, cutout, no_shadow);
                    }
                    for i in 0..poly.len() {
                        let a = poly[i];
                        let c = poly[(i + 1) % poly.len()];
                        let edge = Vec2::new(c.x - a.x, c.y - a.y);
                        if edge.length_squared() < 1e-6 { continue; }
                        let n = Vec3::new(edge.y, 0.0, -edge.x).normalize();
                        // Wound counter-clockwise seen from outside.
                        let corners = [
                            Vec3::new(a.x, y0, a.y),
                            Vec3::new(c.x, y0, c.y),
                            Vec3::new(c.x, y1, c.y),
                            Vec3::new(a.x, y1, a.y),
                        ];
                        let uvs = side_uvs(&corners, b.tex_scale);
                        faces.push(Face { brush: current_brush, corners, normal: n, uvs, mat: b.mat,
                                          light_scale: b.light_scale, cutout, no_shadow });
                    }
                }
                BrushKind::Ramp(ax) => {
                    // Sloped top plus the four sides beneath it.
                    let (c, n) = ramp_top(&b.aabb, ax);
                    let uvs = plane_uvs(&c, 1, b.tex_scale);
                    faces.push(Face { brush: current_brush, corners: c, normal: n, uvs, mat: b.top, light_scale: b.light_scale, cutout, no_shadow });
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
                        faces.push(Face { brush: current_brush, corners, normal, uvs, mat: b.mat, light_scale: b.light_scale, cutout, no_shadow });
                    }
                }
            }
        }
    }

    // ------------------------------------------------- hidden face removal
    //
    // Map brushes overlap constantly - a wall sunk into a floor, a shed
    // dropped on an apron, two blocks sharing a corner - and every face buried
    // inside another solid was still being built and drawn. Coplanar or nearly
    // coplanar, those faces fight the surface in front of them and show
    // through it as a patch of the wrong texture. Dropping them fixes that at
    // the source and takes a chunk of geometry out with it.
    //
    // A face is buried when every corner, nudged just inside its own surface,
    // is inside some other opaque brush. Sampling the corners rather than only
    // the centre keeps a face that is only partly covered.
    {
        let solids: Vec<&crate::maps::brush::Brush> = map.brushes.iter()
            .filter(|b| b.is_solid() && b.flags.contains(BrushFlags::OPAQUE)
                        && !b.flags.contains(BrushFlags::CUTOUT)
                        && !matches!(b.kind, BrushKind::Ramp(_)))
            .collect();
        let grid = &map.collision.grid;
        faces.retain(|f| {
            if f.cutout { return true; }
            let inset = f.normal * -0.02;
            f.corners.iter().any(|c| {
                let p = *c + inset;
                let mut covered = false;
                grid.query_aabb(&Aabb::new(p - Vec3::splat(0.01), p + Vec3::splat(0.01)), |bi| {
                    if covered { return; }
                    let b = &map.collision.brushes[bi as usize];
                    if !b.is_solid() || !b.flags.contains(BrushFlags::OPAQUE) { return; }
                    if b.flags.contains(BrushFlags::CUTOUT) { return; }
                    if matches!(b.kind, BrushKind::Ramp(_)) { return; }
                    let a = b.aabb;
                    if p.x > a.min.x + 0.005 && p.x < a.max.x - 0.005
                        && p.y > a.min.y + 0.005 && p.y < a.max.y - 0.005
                        && p.z > a.min.z + 0.005 && p.z < a.max.z - 0.005
                        && b.covers_xz(p.x, p.z)
                    { covered = true; }
                });
                !covered
            })
        });
        let _ = solids;
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
    let lights = gather_lights(map);
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
        let lights = &lights[..];
        std::thread::scope(|s| {
            for (ci, slice) in vertices.chunks_mut(chunk).enumerate() {
                let first = ci * chunk;
                s.spawn(move || {
                    for (k, v) in slice.iter_mut().enumerate() {
                        let f = &faces[vertex_face[first + k] as usize];
                        let p = Vec3::from(v.pos);
                        v.color = bake_light(map, env, p, f.normal, f.mat, f.light_scale, f.no_shadow, quality, lights);
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

    // Breakable brushes are collected separately: their triangles need to be
    // addressable by brush at draw time, and a cluster is addressable only as
    // a whole.
    let breakable_brush: Vec<bool> = map.brushes.iter().map(|b| b.is_breakable()).collect();
    let mut break_tris: std::collections::BTreeMap<u32, (Vec<u32>, Aabb)> = Default::default();

    for (fi, base, nu, nv) in grids {
        let f = &faces[fi];
        let breakable = (f.brush as usize) < breakable_brush.len()
            && breakable_brush[f.brush as usize];
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
                if breakable {
                    let e = break_tris.entry(f.brush).or_insert_with(|| (Vec::new(), Aabb::EMPTY));
                    for v in [v00, v10, v01, v11] {
                        e.1.union_point(Vec3::from(vertices[v as usize].pos));
                    }
                    e.0.extend_from_slice(&[v00, v10, v11, v00, v11, v01]);
                    continue;
                }
                let list = if f.cutout { &mut cutout_by_cell[ci] } else { &mut opaque_by_cell[ci] };
                // Two triangles, wound counter-clockwise seen from the front.
                list.extend_from_slice(&[v00, v10, v11, v00, v11, v01]);
            }
        }
    }

    // All the opaque geometry first, then all the cut-out geometry, rather
    // than interleaving the two per cluster.
    //
    // The layout is what lets the renderer merge draws. Clusters are visited
    // in order, and neighbouring ones are usually either both visible or both
    // not; with opaque and cut-out interleaved, two consecutive visible
    // clusters' opaque ranges are separated by a cut-out range and cannot be
    // submitted as one call. Separated, a run of visible clusters is a single
    // contiguous range of indices and a single draw.
    let live: Vec<usize> = (0..cell_count)
        .filter(|ci| !opaque_by_cell[*ci].is_empty() || !cutout_by_cell[*ci].is_empty())
        .collect();

    let mut indices: Vec<u32> = Vec::with_capacity(vertices.len() * 3);
    let mut opaque_ranges = Vec::with_capacity(live.len());
    for &ci in &live {
        let start = indices.len() as u32;
        indices.extend_from_slice(&opaque_by_cell[ci]);
        opaque_ranges.push(start..indices.len() as u32);
    }
    let mut clusters = Vec::with_capacity(live.len());
    for (k, &ci) in live.iter().enumerate() {
        let start = indices.len() as u32;
        indices.extend_from_slice(&cutout_by_cell[ci]);
        clusters.push(Cluster {
            bounds: cell_bounds[ci],
            opaque: opaque_ranges[k].clone(),
            cutout: start..indices.len() as u32,
        });
    }

    let mut breakables = Vec::with_capacity(break_tris.len());
    for (brush, (tris, bb)) in break_tris {
        let start = indices.len() as u32;
        indices.extend_from_slice(&tris);
        breakables.push((brush, start..indices.len() as u32, bb));
    }

    let triangle_count = indices.len() / 3;
    MapMesh { vertices, indices, clusters, bounds, triangle_count, breakables }
}

/// Clips a box's XZ footprint by a set of vertical planes.
///
/// Sutherland-Hodgman against `nx*x + nz*z <= d`. The result is convex because
/// the input is, which is what lets the rest of the pipeline treat it as one
/// polygon rather than a general mesh.
fn clip_footprint(aabb: &Aabb, planes: &[[f32; 3]]) -> Vec<Vec2> {
    let mut poly = vec![
        Vec2::new(aabb.min.x, aabb.min.z),
        Vec2::new(aabb.max.x, aabb.min.z),
        Vec2::new(aabb.max.x, aabb.max.z),
        Vec2::new(aabb.min.x, aabb.max.z),
    ];
    for p in planes {
        if poly.len() < 3 { break; }
        let dist = |v: Vec2| p[0] * v.x + p[1] * v.y - p[2];
        let mut out: Vec<Vec2> = Vec::with_capacity(poly.len() + 2);
        for i in 0..poly.len() {
            let a = poly[i];
            let b = poly[(i + 1) % poly.len()];
            let (da, db) = (dist(a), dist(b));
            if da <= 0.0 { out.push(a); }
            if (da > 0.0) != (db > 0.0) {
                let t = da / (da - db);
                out.push(a + (b - a) * t);
            }
        }
        poly = out;
    }
    // Drop points the clipper duplicated at a corner.
    poly.dedup_by(|a, b| (*a - *b).length_squared() < 1e-8);
    if poly.len() > 1 && (poly[0] - poly[poly.len() - 1]).length_squared() < 1e-8 { poly.pop(); }
    poly
}

/// Emits a convex polygon as a triangle fan of quads, so it can travel through
/// the same four-corner `Face` the rest of the builder uses.
#[allow(clippy::too_many_arguments)]
fn push_fan(faces: &mut Vec<Face>, brush: u32, poly: &[Vec3], normal: Vec3,
            b: &crate::maps::brush::Brush, mat: Mat, cutout: bool, no_shadow: bool) {
    for i in 1..poly.len().saturating_sub(1) {
        let corners = [poly[0], poly[i], poly[i + 1], poly[i + 1]];
        let uvs = plane_uvs(&corners, 1, b.tex_scale);
        faces.push(Face { brush, corners, normal, uvs, mat, light_scale: b.light_scale, cutout, no_shadow });
    }
}

/// World-space UVs for a vertical face of arbitrary orientation: `u` runs
/// along the wall, `v` down it, so an angled wall's texture is continuous with
/// the square ones either side of it.
fn side_uvs(corners: &[Vec3; 4], scale: f32) -> [[f32; 2]; 4] {
    let s = if scale <= 0.001 { 1.0 } else { scale };
    let along = Vec3::new(corners[1].x - corners[0].x, 0.0, corners[1].z - corners[0].z);
    let dir = along.normalize_or_zero();
    let base = corners[0];
    let mut out = [[0.0f32; 2]; 4];
    for (i, c) in corners.iter().enumerate() {
        let d = Vec3::new(c.x - base.x, 0.0, c.z - base.z).dot(dir);
        // Offset by the world position of the wall's start so neighbouring
        // brushes on the same line agree about where the texture begins.
        let u = d + base.x * dir.x + base.z * dir.z;
        out[i] = [u / s, -c.y / s];
    }
    out
}

/// Computes the baked colour for one vertex.

// ------------------------------------------------------------ baked lights

/// A point light recovered from an emissive surface, for the vertex bake.
///
/// Interiors in this game were lit by exactly two things: the sun, which a
/// roof stops dead, and a flat hemisphere ambient. A foundry with a roof on
/// it therefore came back as one uniform murk from wall to wall, with the row
/// of lit clerestory windows along the top contributing nothing but their own
/// brightness. Every lightmapped game of this period solved that the same
/// way, by treating the glowing surface as a light and baking what it spills.
#[derive(Clone, Copy)]
struct BakedLight {
    pos: Vec3,
    color: Vec3,
    radius: f32,
}

/// The most lights any one map will bake. Sorted by strength first, so a map
/// that overruns this loses its dimmest.
const MAX_BAKED_LIGHTS: usize = 64;

/// Emissive materials below this contribute a glow to their own surface but
/// are too weak to light anything around them. Hazard striping is the case
/// this excludes, and there are hundreds of metres of it.
const LIGHT_THRESHOLD: f32 = 0.18;

fn light_tint(mat: Mat) -> Vec3 {
    match mat {
        // Daylight through glass, slightly cool.
        Mat::WindowLit => Vec3::new(0.86, 0.91, 1.00),
        // A monitor.
        Mat::Screen => Vec3::new(0.62, 0.78, 1.00),
        // Instrument panels are amber in every industrial building ever built.
        Mat::ControlPanel => Vec3::new(1.00, 0.78, 0.44),
        _ => Vec3::ONE,
    }
}

/// Turns every emissive brush in the map into one or more point lights.
///
/// A seven-metre window is not a point, and baking it as one puts a hot spot
/// under its middle and nothing under its ends, so long panels are split
/// along their length and the flux divided between the pieces. Each light is
/// pushed off its own surface toward the middle of the map, which is where
/// the room is: a window in an outside wall faces in.
fn gather_lights(map: &MapData) -> Vec<BakedLight> {
    let centre = (map.bounds.min + map.bounds.max) * 0.5;
    let mut lights: Vec<BakedLight> = Vec::new();

    for b in map.brushes.iter().chain(map.decor.iter()) {
        if b.flags.contains(BrushFlags::NODRAW) { continue; }
        let e = b.mat.emissive().max(b.top.emissive());
        if e < LIGHT_THRESHOLD { continue; }

        let size = b.aabb.max - b.aabb.min;
        let mid = (b.aabb.min + b.aabb.max) * 0.5;
        let dims = [size.x, size.y, size.z];
        let thin = (0..3).min_by(|&i, &j| dims[i].total_cmp(&dims[j])).unwrap();
        let long = (0..3).max_by(|&i, &j| dims[i].total_cmp(&dims[j])).unwrap();
        if thin == long { continue; }

        // Out of the surface, on whichever side the map is.
        let mut out = Vec3::ZERO;
        out[thin] = if centre[thin] >= mid[thin] { 1.0 } else { -1.0 };
        let stand_off = dims[thin] * 0.5 + 0.45;

        // Radius grows with brightness and with how much surface is glowing;
        // a strip light and a shop window should not reach equally far.
        let area = dims[long] * dims[3 - thin - long];
        let radius = (5.5 + e * 13.0 + area.sqrt() * 1.6).clamp(5.0, 22.0);

        let splits = ((dims[long] / 4.0).round() as usize).clamp(1, 4);
        let tint = light_tint(if b.mat.emissive() >= b.top.emissive() { b.mat } else { b.top });
        // Total flux is fixed, so splitting a panel does not brighten it.
        let color = tint * e * 1.35 / splits as f32;

        for i in 0..splits {
            let t = (i as f32 + 0.5) / splits as f32 - 0.5;
            let mut pos = mid + out * stand_off;
            pos[long] += t * dims[long];
            lights.push(BakedLight { pos, color, radius });
        }
    }

    if lights.len() > MAX_BAKED_LIGHTS {
        lights.sort_by(|a, b| {
            let sa = a.color.max_element() * a.radius;
            let sb = b.color.max_element() * b.radius;
            sb.total_cmp(&sa)
        });
        lights.truncate(MAX_BAKED_LIGHTS);
    }
    lights
}

fn bake_light(
    map: &MapData,
    env: &Env,
    p: Vec3,
    n: Vec3,
    mat: Mat,
    scale: f32,
    no_shadow: bool,
    quality: BakeQuality,
    lights: &[BakedLight],
) -> [u8; 4] {
    // Sky occlusion: how much of the upper hemisphere this point can see.
    // Ambient used to be modulated by the same short-range occlusion term as
    // everything else, which darkens a surface uniformly; using a separate
    // upward visibility for the sky part is what makes the underside of a
    // catwalk read as shaded rather than merely dimmer.
    let sky_vis = if quality == BakeQuality::Full {
        sky_visibility(map, p, n)
    } else {
        1.0
    };

    // Hemisphere ambient: sky above, bounced ground light below.
    let t = (n.y * 0.5 + 0.5).clamp(0.0, 1.0);
    let sky_amt = t * sky_vis;
    let mut r = env.ambient_ground[0] + (env.ambient_sky[0] - env.ambient_ground[0]) * sky_amt;
    let mut g = env.ambient_ground[1] + (env.ambient_sky[1] - env.ambient_ground[1]) * sky_amt;
    let mut b = env.ambient_ground[2] + (env.ambient_sky[2] - env.ambient_ground[2]) * sky_amt;

    let sun = -env.sun_dir;
    let ndotl = sun.dot(n).max(0.0);
    if ndotl > 0.0 {
        // Soft shadows.
        //
        // One ray gives a hard edge quantised to the lighting cell size, which
        // at a couple of metres is a staircase of light and dark squares. Four
        // rays spread over a small cone cost four traces on the points that are
        // lit at all, and turn that staircase into a penumbra a metre or so
        // wide - which is about what a sun edge looks like, and hides the cell
        // grid completely.
        let visible = if quality == BakeQuality::Flat || no_shadow {
            1.0
        } else {
            // A basis around the sun direction to jitter within.
            let up = if sun.y.abs() > 0.9 { Vec3::Z } else { Vec3::Y };
            let tangent = sun.cross(up).normalize_or_zero();
            let bitangent = tangent.cross(sun);
            // The penumbra has to be wider than a lighting cell or it does
            // not exist: light is sampled every 1.7 m or so and interpolated
            // between samples, so a shadow edge that goes from full sun to
            // full shade inside half a metre lands entirely between two
            // samples and comes out as a hard diagonal across the triangles.
            // On a bright ground -- snow especially -- that reads as the
            // ground being paved in polygons. Two rings, eight rays, spread
            // wide enough that the transition takes a couple of cells and the
            // interpolation has something to interpolate.
            // Cloud cover widens the source. An overcast sky is a very large
            // light and casts almost no edge; a clear one is a point and
            // casts a hard one.
            let spread = 0.10 + env.cloud_cover * 0.22;
            const OFFSETS: [(f32, f32); 8] = [
                (0.00, 0.00), (0.50, 0.00), (-0.25, 0.43), (-0.25, -0.43),
                (1.00, 0.00), (-0.50, 0.87), (-0.50, -0.87), (0.20, 0.35),
            ];
            let origin = p + n * 0.06;
            let mut open = 0.0f32;
            for (ox, oy) in OFFSETS {
                let d = (sun + tangent * (ox * spread) + bitangent * (oy * spread)).normalize();
                if !map.collision.trace_ray(origin, d, 60.0, TraceMask::Shot).hit {
                    open += 0.125;
                }
            }
            open
        };
        // ...and it also takes most of the depth out of the shadow. Under
        // eight tenths cloud a shadow is a suggestion, not a hole; without
        // this a heavily overcast arctic map read as noon in a desert, with
        // black-bottomed shapes lying all over the snow.
        let visible = 1.0 - (1.0 - visible) * (1.0 - env.cloud_cover * 0.62);
        r += env.sun_color[0] * ndotl * visible;
        g += env.sun_color[1] * ndotl * visible;
        b += env.sun_color[2] * ndotl * visible;

        // A single bounce off the ground, tinted by it. Cheap, and the reason
        // a wall facing away from the sun in a bright map is not simply grey.
        let bounce = ndotl * 0.16 * visible;
        r += env.ambient_ground[0] * bounce;
        g += env.ambient_ground[1] * bounce;
        b += env.ambient_ground[2] * bounce;
    }

    // Emissive surfaces, as light.
    //
    // One shadow ray each, and only for the handful of lights whose radius
    // actually reaches this vertex, so an indoor map pays for its windows and
    // an open desert pays for nothing.
    for l in lights {
        let d = l.pos - p;
        let dist_sq = d.length_squared();
        if dist_sq >= l.radius * l.radius { continue; }
        let dist = dist_sq.sqrt().max(0.08);
        let dir = d / dist;
        let ndl = dir.dot(n);
        if ndl <= 0.0 { continue; }
        // Linear-squared falloff to zero at the radius. Not physical, but a
        // light that reaches its stated range and stops is a light that can
        // be culled, and inverse square never stops.
        let falloff = 1.0 - dist / l.radius;
        let att = falloff * falloff * ndl;
        if att < 0.002 { continue; }
        if quality != BakeQuality::Flat && !no_shadow {
            let origin = p + n * 0.06;
            if map.collision.trace_ray(origin, dir, dist - 0.12, TraceMask::Shot).hit {
                continue;
            }
        }
        r += l.color.x * att;
        g += l.color.y * att;
        b += l.color.z * att;
    }

    if quality == BakeQuality::Full {
        let ao = ambient_occlusion(map, p, n);
        r *= ao;
        g *= ao;
        b *= ao;
    }

    // Weathering.
    //
    // A hundred square metres of concrete lit by one sun and one sky comes
    // back as a hundred square metres of exactly the same grey, and no amount
    // of texture detail fixes it because the texture tiles every two metres
    // and the eye reads the tile, not the surface. What the games of this era
    // had that made large surfaces look large was a lightmap with grime baked
    // into it, at a scale far bigger than any tile. This is that, for free:
    // low-frequency world-space noise, keyed off position only, so adjacent
    // faces agree along their shared edge and the patch runs across the join.
    //
    // Emissive materials are exempt below - a lit window is not dirty.
    let w = weathering(p);
    r *= w;
    g *= w;
    b *= w;

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

/// Deterministic hashed lattice noise in three dimensions.
fn grime_hash(x: i32, y: i32, z: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8DA6_B343)
        ^ (y as u32).wrapping_mul(0xD8163841)
        ^ (z as u32).wrapping_mul(0xCB1A_B31F);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x2974_5C87);
    h ^= h >> 15;
    (h & 0xFF_FFFF) as f32 / 16_777_215.0
}

fn grime_noise(p: Vec3) -> f32 {
    let f = p.floor();
    let t = p - f;
    let (xi, yi, zi) = (f.x as i32, f.y as i32, f.z as i32);
    // Smoothstep weights, so the field has no lattice creases in it.
    let s = t * t * (Vec3::splat(3.0) - 2.0 * t);
    let mut acc = 0.0;
    for dz in 0..2 {
        let wz = if dz == 0 { 1.0 - s.z } else { s.z };
        for dy in 0..2 {
            let wy = if dy == 0 { 1.0 - s.y } else { s.y };
            for dx in 0..2 {
                let wx = if dx == 0 { 1.0 - s.x } else { s.x };
                acc += grime_hash(xi + dx, yi + dy, zi + dz) * wx * wy * wz;
            }
        }
    }
    acc
}

/// A slow multiplier on baked light, between roughly 0.86 and 1.07, varying
/// over metres rather than centimetres.
///
/// Two octaves is enough: the point is to break up the flatness of a large
/// surface, not to draw anything the player can identify. The vertical axis
/// is squashed so the pattern runs in bands down a wall the way water staining
/// does, rather than in isotropic blobs.
fn weathering(p: Vec3) -> f32 {
    // Both octaves have to stay well above the lighting cell size or the
    // per-vertex sampling turns them into flat triangular facets -- which is
    // exactly what happened on Whiteout, where a nine-metre octave sampled
    // every 1.7 m paved the snow with hard-edged polygons. Eighteen and ten
    // metres are safely oversampled at that spacing.
    let q = Vec3::new(p.x, p.y * 0.55, p.z);
    let n = grime_noise(q * 0.055) * 0.68 + grime_noise(q * 0.100 + Vec3::splat(11.3)) * 0.32;
    0.83 + n * 0.26
}

/// How much of the sky this point can see, over a long range.
///
/// Distinct from the short-range ambient occlusion below, which is about
/// corners and contact. This is about being under something.
fn sky_visibility(map: &MapData, p: Vec3, n: Vec3) -> f32 {
    const DIRS: [Vec3; 5] = [
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::new(0.62, 0.78, 0.0),
        Vec3::new(-0.62, 0.78, 0.0),
        Vec3::new(0.0, 0.78, 0.62),
        Vec3::new(0.0, 0.78, -0.62),
    ];
    let origin = p + n * 0.06;
    let mut open = 0.0f32;
    for d in DIRS {
        let dir = d.normalize();
        // Only count directions on this surface's own side.
        if dir.dot(n) <= 0.0 {
            open += 0.2;
            continue;
        }
        if !map.collision.trace_ray(origin, dir, 26.0, TraceMask::Shot).hit {
            open += 0.2;
        }
    }
    // Never fully black: a point that sees no sky still gets bounced light.
    0.25 + 0.75 * open
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

/// The shapes an instanced part can be drawn as.
///
/// Everything used to be a cube, which is why the guns and the soldiers read
/// as stacks of boxes however many boxes they were made of. A barrel is a
/// cylinder, a helmet is a dome, an arm is a capsule; drawing them as such
/// costs one more vertex buffer each and a separate instance stream, and no
/// extra work per instance at all.
///
/// The curved shapes carry smooth normals - radial on a cylinder wall,
/// spherical on a dome - so the fixed three-quarter key in the part shader
/// shades them as curves rather than as facets. That is the whole trick: the
/// geometry is coarse, and the lighting is what makes it read as round.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum PartShape {
    Cube = 0,
    /// Capped cylinder along Y, unit diameter and height.
    Cylinder,
    /// Cylinder with hemispherical ends: limbs, grips, sausages.
    Capsule,
    /// Cone along Y, base at the bottom.
    Cone,
    /// Sphere: heads, domes, drums.
    Sphere,
    /// A box with its twelve edges chamfered. Still a box, but one that
    /// catches a highlight on every edge instead of dying at each corner.
    Bevel,
}

pub const PART_SHAPES: usize = 6;

pub const ALL_SHAPES: [PartShape; PART_SHAPES] = [
    PartShape::Cube, PartShape::Cylinder, PartShape::Capsule,
    PartShape::Cone, PartShape::Sphere, PartShape::Bevel,
];

/// Builds the mesh for one shape, in a unit cube centred on the origin.
pub fn shape_mesh(shape: PartShape) -> (Vec<PartVertex>, Vec<u16>) {
    match shape {
        PartShape::Cube => unit_cube(),
        PartShape::Cylinder => unit_cylinder(12, false),
        PartShape::Capsule => unit_capsule(12, 3),
        PartShape::Cone => unit_cylinder(12, true),
        PartShape::Sphere => unit_sphere(12, 7),
        PartShape::Bevel => unit_bevel(0.11),
    }
}

/// A cylinder along Y, or a cone if `taper` is set. Unit diameter, unit height.
fn unit_cylinder(sides: usize, taper: bool) -> (Vec<PartVertex>, Vec<u16>) {
    let mut v = Vec::with_capacity(sides * 4 + 2);
    let mut idx = Vec::with_capacity(sides * 12);
    let top_r = if taper { 0.0 } else { 0.5 };

    // Wall: one vertex ring top and bottom, normals radial so the wall shades
    // as a curve.
    for i in 0..=sides {
        let a = i as f32 / sides as f32 * std::f32::consts::TAU;
        let (sa, ca) = a.sin_cos();
        let n = if taper {
            // A cone's surface normal leans outward by its slope.
            Vec3::new(ca, 0.5, sa).normalize()
        } else {
            Vec3::new(ca, 0.0, sa)
        };
        let u = i as f32 / sides as f32;
        v.push(PartVertex { pos: [ca * 0.5, -0.5, sa * 0.5], normal: [n.x, n.y, n.z], uv: [u, 1.0] });
        v.push(PartVertex { pos: [ca * top_r, 0.5, sa * top_r], normal: [n.x, n.y, n.z], uv: [u, 0.0] });
    }
    for i in 0..sides {
        let b = (i * 2) as u16;
        idx.extend_from_slice(&[b, b + 1, b + 3, b, b + 3, b + 2]);
    }

    // Caps as fans.
    let bottom_centre = v.len() as u16;
    v.push(PartVertex { pos: [0.0, -0.5, 0.0], normal: [0.0, -1.0, 0.0], uv: [0.5, 0.5] });
    let bottom_ring = v.len() as u16;
    for i in 0..sides {
        let a = i as f32 / sides as f32 * std::f32::consts::TAU;
        let (sa, ca) = a.sin_cos();
        v.push(PartVertex { pos: [ca * 0.5, -0.5, sa * 0.5], normal: [0.0, -1.0, 0.0],
                            uv: [ca * 0.5 + 0.5, sa * 0.5 + 0.5] });
    }
    for i in 0..sides {
        let a = bottom_ring + i as u16;
        let b = bottom_ring + ((i + 1) % sides) as u16;
        idx.extend_from_slice(&[bottom_centre, b, a]);
    }
    if !taper {
        let top_centre = v.len() as u16;
        v.push(PartVertex { pos: [0.0, 0.5, 0.0], normal: [0.0, 1.0, 0.0], uv: [0.5, 0.5] });
        let top_ring = v.len() as u16;
        for i in 0..sides {
            let a = i as f32 / sides as f32 * std::f32::consts::TAU;
            let (sa, ca) = a.sin_cos();
            v.push(PartVertex { pos: [ca * 0.5, 0.5, sa * 0.5], normal: [0.0, 1.0, 0.0],
                                uv: [ca * 0.5 + 0.5, sa * 0.5 + 0.5] });
        }
        for i in 0..sides {
            let a = top_ring + i as u16;
            let b = top_ring + ((i + 1) % sides) as u16;
            idx.extend_from_slice(&[top_centre, a, b]);
        }
    }
    (v, idx)
}

/// A sphere, built as stacked rings. Normals are the positions, so it is
/// perfectly smooth however few segments it has.
fn unit_sphere(segments: usize, rings: usize) -> (Vec<PartVertex>, Vec<u16>) {
    let mut v = Vec::with_capacity((segments + 1) * (rings + 1));
    let mut idx = Vec::with_capacity(segments * rings * 6);
    for r in 0..=rings {
        let phi = r as f32 / rings as f32 * std::f32::consts::PI;
        let (sp, cp) = phi.sin_cos();
        for sgm in 0..=segments {
            let theta = sgm as f32 / segments as f32 * std::f32::consts::TAU;
            let (st, ct) = theta.sin_cos();
            let n = Vec3::new(sp * ct, cp, sp * st);
            v.push(PartVertex {
                pos: [n.x * 0.5, n.y * 0.5, n.z * 0.5],
                normal: [n.x, n.y, n.z],
                uv: [sgm as f32 / segments as f32, r as f32 / rings as f32],
            });
        }
    }
    let stride = (segments + 1) as u16;
    for r in 0..rings as u16 {
        for sgm in 0..segments as u16 {
            let a = r * stride + sgm;
            idx.extend_from_slice(&[a, a + stride, a + stride + 1, a, a + stride + 1, a + 1]);
        }
    }
    (v, idx)
}

/// A capsule along Y: a cylindrical waist with rounded ends.
///
/// The caps take a fixed slice of the height rather than a hemisphere's worth.
/// A hemisphere at this radius would use the entire unit height and leave no
/// waist at all - which is what the first version of this did, making every
/// "capsule" a sphere, and every limb and sleeve an ellipsoid that read as a
/// flat lens once it was stretched.
fn unit_capsule(segments: usize, cap_rings: usize) -> (Vec<PartVertex>, Vec<u16>) {
    let mut v = Vec::new();
    let mut idx: Vec<u16> = Vec::new();
    let r = 0.5f32;
    let cap = 0.16f32;
    let waist = 0.5 - cap;
    let total_rings = cap_rings * 2 + 1;

    // Walk top pole to bottom pole. Rings 0..cap_rings are the top cap, the
    // next is the waist bottom, then the bottom cap.
    let ring = |phi: f32, y_centre: f32, out: &mut Vec<PartVertex>, row: usize| {
        let (sp, cp) = phi.sin_cos();
        for sgm in 0..=segments {
            let theta = sgm as f32 / segments as f32 * std::f32::consts::TAU;
            let (st, ct) = theta.sin_cos();
            // Normal accounts for the cap being squashed relative to a sphere,
            // so the shading is continuous where cap meets waist.
            let n = Vec3::new(sp * ct * cap, cp * r, sp * st * cap).normalize_or_zero();
            out.push(PartVertex {
                pos: [sp * r * ct, y_centre + cp * cap, sp * r * st],
                normal: [n.x, n.y, n.z],
                uv: [sgm as f32 / segments as f32, row as f32 / total_rings as f32],
            });
        }
    };

    let mut row = 0usize;
    for i in 0..=cap_rings {
        let phi = i as f32 / cap_rings as f32 * std::f32::consts::FRAC_PI_2;
        ring(phi, waist, &mut v, row);
        row += 1;
    }
    for i in 0..=cap_rings {
        let phi = std::f32::consts::FRAC_PI_2
            + i as f32 / cap_rings as f32 * std::f32::consts::FRAC_PI_2;
        ring(phi, -waist, &mut v, row);
        row += 1;
    }

    let stride = (segments + 1) as u16;
    let rings_built = (row - 1) as u16;
    for rr in 0..rings_built {
        for sgm in 0..segments as u16 {
            let a = rr * stride + sgm;
            idx.extend_from_slice(&[a, a + stride, a + stride + 1, a, a + stride + 1, a + 1]);
        }
    }
    (v, idx)
}

/// A box with chamfered edges and corners.
///
/// Built by taking the cube's eight corners, pulling three vertices out of
/// each along the face normals, and filling the faces, the twelve edge strips
/// and the eight corner triangles. Flat-shaded per facet, which is what a
/// bevel wants: the point is that an edge catches a different value from the
/// faces either side of it.
fn unit_bevel(cut: f32) -> (Vec<PartVertex>, Vec<u16>) {
    let mut v: Vec<PartVertex> = Vec::new();
    let mut idx: Vec<u16> = Vec::new();
    let h = 0.5f32;
    let i = h - cut.clamp(0.02, 0.45);

    let mut quad = |a: Vec3, b: Vec3, c: Vec3, d: Vec3| {
        let n = (b - a).cross(c - a).normalize_or_zero();
        let base = v.len() as u16;
        for (k, p) in [a, b, c, d].into_iter().enumerate() {
            v.push(PartVertex {
                pos: [p.x, p.y, p.z],
                normal: [n.x, n.y, n.z],
                uv: [(k == 1 || k == 2) as u8 as f32, (k >= 2) as u8 as f32],
            });
        }
        idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    };

    // Six faces, inset by the chamfer.
    for (axis, sign) in [(0usize, 1.0f32), (0, -1.0), (1, 1.0), (1, -1.0), (2, 1.0), (2, -1.0)] {
        let (a1, a2) = match (axis, sign > 0.0) {
            (0, true) => (1usize, 2usize),
            (0, false) => (2, 1),
            (1, true) => (2, 0),
            (1, false) => (0, 2),
            (2, true) => (0, 1),
            _ => (1, 0),
        };
        let corner = |s1: f32, s2: f32| {
            let mut p = Vec3::ZERO;
            p[axis] = h * sign;
            p[a1] = i * s1;
            p[a2] = i * s2;
            p
        };
        quad(corner(-1.0, -1.0), corner(1.0, -1.0), corner(1.0, 1.0), corner(-1.0, 1.0));
    }

    // Twelve edge chamfers.
    for axis in 0..3usize {
        let (b1, b2) = ((axis + 1) % 3, (axis + 2) % 3);
        for &s1 in &[-1.0f32, 1.0] {
            for &s2 in &[-1.0f32, 1.0] {
                let mut lo = Vec3::ZERO;
                lo[axis] = -i;
                let mut hi = Vec3::ZERO;
                hi[axis] = i;
                let mut e1 = Vec3::ZERO;
                e1[b1] = h * s1;
                e1[b2] = i * s2;
                let mut e2 = Vec3::ZERO;
                e2[b1] = i * s1;
                e2[b2] = h * s2;
                let (p0, p1, p2, p3) = (lo + e1, hi + e1, hi + e2, lo + e2);
                // Wind so the facet faces outward.
                if (p1 - p0).cross(p2 - p0).dot(e1 + e2) > 0.0 {
                    quad(p0, p1, p2, p3);
                } else {
                    quad(p3, p2, p1, p0);
                }
            }
        }
    }

    // Eight corner triangles.
    for &sx in &[-1.0f32, 1.0] {
        for &sy in &[-1.0f32, 1.0] {
            for &sz in &[-1.0f32, 1.0] {
                let a = Vec3::new(h * sx, i * sy, i * sz);
                let b = Vec3::new(i * sx, h * sy, i * sz);
                let c = Vec3::new(i * sx, i * sy, h * sz);
                let out = Vec3::new(sx, sy, sz);
                if (b - a).cross(c - a).dot(out) > 0.0 {
                    quad(a, b, c, c);
                } else {
                    quad(a, c, b, b);
                }
            }
        }
    }
    (v, idx)
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
    Chest,
    Neck,
    Head,
    Helmet,
    HelmetBrim,
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
    KneeL,
    LegLowerL,
    BootL,
    LegUpperR,
    KneeR,
    LegLowerR,
    BootR,
    Holster,
    PouchL,
    PouchR,
}

pub const PART_COUNT: usize = 28;

pub const ALL_PARTS: [Part; PART_COUNT] = [
    Part::Hips, Part::Torso, Part::Vest, Part::Chest, Part::Neck,
    Part::Head, Part::Helmet, Part::HelmetBrim, Part::Pack,
    Part::ShoulderL, Part::ShoulderR,
    Part::ArmUpperL, Part::ArmLowerL, Part::GloveL,
    Part::ArmUpperR, Part::ArmLowerR, Part::GloveR,
    Part::LegUpperL, Part::KneeL, Part::LegLowerL, Part::BootL,
    Part::LegUpperR, Part::KneeR, Part::LegLowerR, Part::BootR,
    Part::Holster, Part::PouchL, Part::PouchR,
];

/// Size of the parts that are boxes rather than bones, in metres at the
/// standing pose. Limbs are absent: they are sized from their two endpoints,
/// which is the only way a limb can be guaranteed to reach what it is holding.
pub const PART_SIZE: [[f32; 3]; PART_COUNT] = [
    [0.38, 0.22, 0.25],  // hips
    [0.43, 0.50, 0.27],  // torso
    [0.455, 0.30, 0.30], // vest
    [0.34, 0.13, 0.31],  // chest rig
    [0.13, 0.09, 0.13],  // neck
    [0.185, 0.225, 0.20],// head
    [0.225, 0.135, 0.235],// helmet
    [0.235, 0.045, 0.095],// helmet brim
    [0.30, 0.29, 0.15],  // pack
    [0.15, 0.13, 0.24],  // shoulder L
    [0.15, 0.13, 0.24],  // shoulder R
    [0.13, 0.30, 0.14],  // upper arm L
    [0.11, 0.28, 0.12],  // lower arm L
    [0.10, 0.11, 0.13],  // glove L
    [0.13, 0.30, 0.14],  // upper arm R
    [0.11, 0.28, 0.12],  // lower arm R
    [0.10, 0.11, 0.13],  // glove R
    [0.17, 0.42, 0.19],  // upper leg L
    [0.15, 0.10, 0.17],  // knee pad L
    [0.14, 0.40, 0.16],  // lower leg L
    [0.15, 0.11, 0.26],  // boot L
    [0.17, 0.42, 0.19],  // upper leg R
    [0.15, 0.10, 0.17],  // knee pad R
    [0.14, 0.40, 0.16],  // lower leg R
    [0.15, 0.11, 0.26],  // boot R
    [0.11, 0.20, 0.09],  // holster
    [0.10, 0.11, 0.08],  // pouch L
    [0.10, 0.11, 0.08],  // pouch R
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

/// The mesh each body part is drawn as.
///
/// Limbs and the neck are capsules, the head and helmet are spheres, the torso
/// and the armour are bevelled boxes. Nothing here is a plain cube any more
/// except the pouches, which are small enough that it would be wasted
/// geometry.
pub fn part_shape(part: Part) -> PartShape {
    match part {
        Part::ArmUpperL | Part::ArmLowerL | Part::ArmUpperR | Part::ArmLowerR
        | Part::LegUpperL | Part::LegLowerL | Part::LegUpperR | Part::LegLowerR
        | Part::Neck => PartShape::Capsule,
        Part::Head | Part::Helmet => PartShape::Sphere,
        Part::GloveL | Part::GloveR | Part::KneeL | Part::KneeR => PartShape::Capsule,
        // Wider than they are tall, so a capsule would be a squashed ball.
        Part::ShoulderL | Part::ShoulderR => PartShape::Sphere,
        Part::Torso | Part::Hips | Part::Vest | Part::Chest | Part::Pack
        | Part::BootL | Part::BootR | Part::Holster | Part::HelmetBrim => PartShape::Bevel,
        Part::PouchL | Part::PouchR => PartShape::Cube,
    }
}

pub fn part_look(part: Part) -> PartLook {
    match part {
        Part::Head | Part::Neck => PartLook::Skin,
        Part::Helmet | Part::HelmetBrim | Part::ShoulderL | Part::ShoulderR
        | Part::KneeL | Part::KneeR => PartLook::Hard,
        Part::Vest | Part::Chest | Part::Pack | Part::Holster
        | Part::PouchL | Part::PouchR => PartLook::Webbing,
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
            // Free-for-all has no teams, and winter camouflage on a desert
            // airfield made everyone a white silhouette against tan ground.
            _ => Mat::Camo,
        },
        PartLook::Skin => Mat::Fabric,
        PartLook::Webbing => Mat::Canvas,
        PartLook::Hard => Mat::GunPolymer,
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
    let neck_y = hip_y + 0.615;

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
    // A chest rig standing proud of the vest, and two magazine pouches on it.
    out[Part::Chest as usize] = torso_m
        * Mat4::from_translation(Vec3::new(0.0, 0.02 * s, -0.145 * s))
        * Mat4::from_scale(Vec3::from(PART_SIZE[Part::Chest as usize]) * s);
    for (side, part) in [(-1.0f32, Part::PouchL), (1.0, Part::PouchR)] {
        out[part as usize] = torso_m
            * Mat4::from_translation(Vec3::new(side * 0.115 * s, -0.045 * s, -0.175 * s))
            * Mat4::from_scale(Vec3::from(PART_SIZE[part as usize]) * s);
    }

    // A neck, so the head is joined to the body rather than floating above
    // the collar when the aim swings.
    out[Part::Neck as usize] = root
        * Mat4::from_translation(Vec3::new(0.0, (neck_y - 0.055) * s, 0.0))
        * Mat4::from_rotation_y(input.yaw)
        * Mat4::from_scale(Vec3::from(PART_SIZE[Part::Neck as usize]) * s);

    // Head: the remainder of the aim, so looking up raises the face.
    let head_m = root
        * Mat4::from_translation(Vec3::new(0.0, neck_y * s, 0.0))
        * Mat4::from_rotation_x(input.pitch * 0.62);
    out[Part::Head as usize] = head_m
        * Mat4::from_translation(Vec3::new(0.0, 0.045 * s, 0.0))
        * Mat4::from_scale(Vec3::from(PART_SIZE[Part::Head as usize]) * s);
    out[Part::Helmet as usize] = head_m
        * Mat4::from_translation(Vec3::new(0.0, 0.155 * s, 0.008 * s))
        * Mat4::from_scale(Vec3::from(PART_SIZE[Part::Helmet as usize]) * s);
    // A brim over the eyes: the one detail that makes a helmet read as facing
    // somewhere, which matters more than any other at the range you see it.
    out[Part::HelmetBrim as usize] = head_m
        * Mat4::from_translation(Vec3::new(0.0, 0.118 * s, -0.130 * s))
        * Mat4::from_rotation_x(-0.18)
        * Mat4::from_scale(Vec3::from(PART_SIZE[Part::HelmetBrim as usize]) * s);

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
    let stride = input.phase.sin() * 0.46 * run;
    let air = if input.grounded { 0.0 } else { 0.35 };
    let up = root.transform_vector3(Vec3::Y);
    let fwd = root.transform_vector3(-Vec3::Z);

    let thigh = 0.44 * s;
    let shin = 0.44 * s;
    for (side, swing, upper, lower, boot, knee_pad) in [
        (-1.0f32, stride, Part::LegUpperL, Part::LegLowerL, Part::BootL, Part::KneeL),
        (1.0, -stride, Part::LegUpperR, Part::LegLowerR, Part::BootR, Part::KneeR),
    ] {
        let hip = at(side * 0.11, hip_y - 0.06, 0.0);
        // Foot placement drives the leg, so feet land where they look like
        // they land instead of floating a hand's width above the ground.
        let lift = (swing.max(0.0)) * 0.13 * s + air * 0.12 * s;
        let reach = swing * 0.30 * s;
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
        // Knee pad on the joint, aligned with the shin so it follows the leg.
        let shin_dir = (ankle - knee).normalize_or_zero();
        out[knee_pad as usize] = bone(
            knee - shin_dir * (0.02 * s), knee + shin_dir * (0.10 * s),
            PART_SIZE[knee_pad as usize][0] * s, PART_SIZE[knee_pad as usize][2] * s, fwd);
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
    /// Which mesh to draw it as. Barrels, tubes and optics are round; the
    /// receiver and the furniture are bevelled boxes.
    pub shape: PartShape,
}

const fn wp(x: f32, y: f32, z: f32, sx: f32, sy: f32, sz: f32, mat: Mat) -> WeaponPart {
    WeaponPart { offset: Vec3::new(x, y, z), size: Vec3::new(sx, sy, sz), rot: Vec3::ZERO,
                 mat, shape: PartShape::Bevel }
}

/// A round part: barrels, tubes, optics, cylinders. The mesh runs along Y, so
/// anything lying down the bore is rotated a quarter turn about X and its
/// height becomes its length.
const fn wpc(x: f32, y: f32, z: f32, dia: f32, len: f32, mat: Mat) -> WeaponPart {
    WeaponPart {
        offset: Vec3::new(x, y, z),
        size: Vec3::new(dia, len, dia),
        rot: Vec3::new(std::f32::consts::FRAC_PI_2, 0.0, 0.0),
        mat,
        shape: PartShape::Cylinder,
    }
}

/// The same, tilted about X (the usual case: rake and cant).
const fn wpx(x: f32, y: f32, z: f32, sx: f32, sy: f32, sz: f32, rx: f32, mat: Mat) -> WeaponPart {
    WeaponPart { offset: Vec3::new(x, y, z), size: Vec3::new(sx, sy, sz),
                 rot: Vec3::new(rx, 0.0, 0.0), mat, shape: PartShape::Bevel }
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
    /// The rear sight aperture, in weapon-local metres.
    ///
    /// Aiming places this point exactly on the centre of the screen, so the
    /// sight picture follows from where the sight is on the model rather than
    /// from a hand-tuned offset that has to be re-tuned every time a part
    /// moves. Line the rear up with the front post and the shot goes there.
    pub sight: Vec3,
}

use crate::game::weapons::ModelShape;
use Mat::*;

// Assault rifle: separate upper and lower, free-floating handguard with
// vents, flat-top rail, collapsible stock on a buffer tube, and a magazine
// built from three segments so it curves the way a real one does.
const RIFLE: [WeaponPart; 27] = [
    wp(0.0, 0.014, -0.07, 0.050, 0.064, 0.34, GunMetal),      // upper receiver
    wp(0.0, 0.040, -0.07, 0.044, 0.020, 0.33, GunPolymer),       // upper spine
    wp(0.0, -0.040, 0.015, 0.046, 0.058, 0.15, GunMetal),     // lower receiver
    wp(0.0, 0.006, -0.31, 0.054, 0.058, 0.23, GunPolymer),    // handguard
    wp(0.026, 0.006, -0.31, 0.006, 0.030, 0.20, GunPolymer),     // vent slots, right
    wp(-0.026, 0.006, -0.31, 0.006, 0.030, 0.20, GunPolymer),    // vent slots, left
    wp(0.0, -0.020, -0.31, 0.030, 0.008, 0.20, GunPolymer),      // vent slots, under
    wpc(0.0, 0.006, -0.49, 0.019, 0.20, GunMetal),             // barrel
    wpc(0.0, 0.006, -0.615, 0.029, 0.055, GunPolymer),           // muzzle brake
    wpc(0.0, 0.006, -0.632, 0.033, 0.012, GunMetal),          // brake port ring
    wpc(0.0, 0.032, -0.43, 0.030, 0.048, GunMetal),            // gas block
    wp(0.0, 0.051, -0.17, 0.030, 0.014, 0.40, GunPolymer), // top rail
    wp(0.0, 0.076, 0.02, 0.026, 0.030, 0.032, GunMetal),      // rear sight
    wp(0.0, 0.088, 0.02, 0.030, 0.008, 0.026, GunPolymer),       // rear aperture ring
    wp(0.0, 0.072, -0.45, 0.014, 0.036, 0.020, GunMetal),     // front post
    wp(0.0, 0.086, -0.45, 0.026, 0.010, 0.016, GunMetal),     // front sight hood
    wp(0.026, 0.052, 0.115, 0.022, 0.015, 0.085, GunMetal),   // charging handle
    wp(0.029, 0.020, -0.02, 0.008, 0.030, 0.095, GunPolymer),    // ejection port
    wp(0.031, 0.020, -0.06, 0.006, 0.018, 0.030, GunPolymer), // forward assist
    wpx(0.0, -0.104, -0.002, 0.038, 0.098, 0.070, -0.10, GunPolymer), // magazine, upper
    wpx(0.0, -0.166, 0.020, 0.036, 0.080, 0.066, -0.30, GunPolymer), // magazine, curve
    wpx(0.0, -0.204, 0.038, 0.040, 0.018, 0.070, -0.30, GunPolymer),   // magazine floorplate
    wpx(0.0, -0.112, 0.088, 0.040, 0.150, 0.052, 0.34, GunPolymer), // pistol grip
    wp(0.0, -0.056, 0.048, 0.030, 0.010, 0.052, GunMetal),    // trigger guard
    wpc(0.0, 0.014, 0.145, 0.031, 0.14, GunMetal),             // buffer tube
    wp(0.0, 0.008, 0.215, 0.040, 0.070, 0.15, GunPolymer),    // stock
    wp(0.0, 0.008, 0.292, 0.044, 0.082, 0.020, GunPolymer),           // butt pad
];

// Bullpup: the whole action sits behind the grip, so the same barrel length
// comes in a much shorter weapon. Carries its optic as standard.
const BULLPUP: [WeaponPart; 20] = [
    wp(0.0, 0.012, 0.04, 0.056, 0.090, 0.40, GunMetal),       // body shell
    wp(0.0, 0.052, 0.04, 0.048, 0.014, 0.38, GunPolymer),        // shell spine
    wp(0.0, 0.010, -0.24, 0.048, 0.056, 0.20, GunMetal),      // handguard
    wp(0.024, 0.010, -0.24, 0.006, 0.028, 0.17, GunPolymer),     // vents, right
    wp(-0.024, 0.010, -0.24, 0.006, 0.028, 0.17, GunPolymer),    // vents, left
    wpc(0.0, 0.010, -0.41, 0.019, 0.17, GunMetal),             // barrel
    wpc(0.0, 0.010, -0.515, 0.027, 0.05, GunPolymer),            // flash hider
    wp(0.0, 0.058, -0.10, 0.028, 0.014, 0.44, GunPolymer), // rail
    wpc(0.0, 0.086, -0.06, 0.046, 0.16, GunMetal),            // optic body
    wpc(0.0, 0.086, -0.145, 0.052, 0.022, ControlPanel),        // objective
    wpc(0.0, 0.086, 0.028, 0.046, 0.020, Glass),                // eyepiece
    wp(0.0, 0.114, -0.06, 0.020, 0.014, 0.10, GunPolymer),       // optic mount rail
    wpx(0.0, -0.108, 0.140, 0.040, 0.100, 0.068, -0.08, GunPolymer), // magazine, upper
    wpx(0.0, -0.178, 0.168, 0.038, 0.080, 0.064, -0.26, GunPolymer), // magazine, curve
    wpx(0.0, -0.100, -0.055, 0.038, 0.135, 0.050, 0.30, GunPolymer),    // grip
    wp(0.0, -0.048, -0.095, 0.028, 0.010, 0.050, GunMetal),   // trigger guard
    wpx(0.0, -0.052, -0.235, 0.034, 0.090, 0.044, -0.20, GunPolymer), // foregrip
    wp(0.0, -0.030, 0.10, 0.052, 0.030, 0.12, GunMetal),      // magwell shoulder
    wp(0.0, 0.006, 0.255, 0.052, 0.095, 0.03, GunPolymer),            // butt pad
    wp(0.026, 0.040, 0.10, 0.010, 0.026, 0.07, GunPolymer),      // ejection port
];

// Submachine gun: short, blocky, folding stock, magazine through the grip.
const SMG: [WeaponPart; 19] = [
    wp(0.0, 0.012, -0.06, 0.048, 0.070, 0.28, GunMetal),      // receiver
    wp(0.0, 0.040, -0.06, 0.042, 0.016, 0.27, GunPolymer),       // receiver spine
    wp(0.0, 0.012, -0.235, 0.036, 0.040, 0.10, GunMetal),     // barrel shroud
    wp(0.020, 0.012, -0.235, 0.006, 0.022, 0.085, GunPolymer),   // shroud vents
    wp(-0.020, 0.012, -0.235, 0.006, 0.022, 0.085, GunPolymer),
    wpc(0.0, 0.012, -0.315, 0.017, 0.09, GunMetal),            // barrel
    wpc(0.0, 0.012, -0.360, 0.024, 0.030, GunPolymer),           // compensator
    wp(0.0, 0.048, -0.10, 0.026, 0.013, 0.28, GunPolymer), // rail
    wp(0.0, 0.070, -0.005, 0.024, 0.028, 0.028, GunMetal),    // rear sight
    wp(0.0, 0.066, -0.245, 0.013, 0.030, 0.018, GunMetal),    // front sight
    wp(0.0, 0.078, -0.245, 0.022, 0.010, 0.014, GunMetal),    // front hood
    wpx(0.0, -0.100, -0.008, 0.034, 0.110, 0.054, -0.08, GunMetal), // magazine
    wpx(0.0, -0.164, 0.006, 0.032, 0.062, 0.050, -0.20, GunMetal),  // magazine curve
    wpx(0.0, -0.098, 0.062, 0.038, 0.130, 0.048, 0.28, GunPolymer), // grip
    wp(0.0, -0.050, 0.030, 0.028, 0.010, 0.048, GunMetal),    // trigger guard
    wpx(0.0, -0.030, -0.175, 0.030, 0.090, 0.042, -0.22, GunPolymer), // foregrip
    wp(0.024, 0.030, 0.02, 0.010, 0.026, 0.075, GunPolymer),     // ejection port
    wpc(0.0, 0.014, 0.115, 0.026, 0.09, GunMetal),             // stock strut
    wp(0.0, 0.012, 0.185, 0.044, 0.070, 0.024, GunPolymer),           // butt plate
];

// Pump shotgun: wooden furniture, a magazine tube slung under the barrel.
const SHOTGUN: [WeaponPart; 17] = [
    wp(0.0, 0.000, -0.05, 0.052, 0.078, 0.26, GunMetal),      // receiver
    wp(0.0, 0.038, -0.05, 0.046, 0.014, 0.25, GunPolymer),       // receiver top
    wpc(0.0, 0.022, -0.36, 0.032, 0.38, GunMetal),             // barrel
    wpc(0.0, -0.026, -0.32, 0.028, 0.30, GunMetal),           // magazine tube
    wp(0.0, -0.002, -0.30, 0.014, 0.026, 0.26, GunPolymer),      // barrel/tube web
    wp(0.0, -0.024, -0.235, 0.058, 0.052, 0.13, WoodPlank),     // pump
    wp(0.0, -0.024, -0.235, 0.062, 0.014, 0.115, GunPolymer),    // pump grooves
    wp(0.0, 0.048, -0.05, 0.022, 0.014, 0.10, GunPolymer), // rib
    wp(0.0, 0.046, -0.535, 0.012, 0.020, 0.014, GunPolymer),     // bead sight
    wp(0.0, -0.052, 0.045, 0.024, 0.010, 0.050, GunMetal),    // trigger guard
    wp(0.0, -0.034, 0.048, 0.012, 0.024, 0.012, GunPolymer), // trigger
    wpx(0.0, -0.086, 0.085, 0.044, 0.115, 0.055, 0.38, WoodPlank), // grip
    wp(0.0, 0.006, 0.175, 0.048, 0.092, 0.16, WoodPlank),       // stock
    wp(0.0, 0.044, 0.150, 0.042, 0.028, 0.11, WoodPlank),       // comb
    wp(0.0, -0.008, 0.262, 0.050, 0.100, 0.024, GunPolymer),          // butt pad
    wp(0.026, 0.000, -0.005, 0.010, 0.030, 0.09, GunPolymer),    // ejection port
    wp(0.0, -0.046, -0.06, 0.030, 0.020, 0.08, GunMetal),     // loading gate
];

// Bolt rifle: long heavy barrel, big glass on rings, bipod, cheek riser.
const SNIPER: [WeaponPart; 22] = [
    wp(0.0, 0.006, -0.04, 0.048, 0.070, 0.34, GunMetal),      // action
    wpc(0.0, 0.006, -0.40, 0.023, 0.40, GunMetal),             // barrel
    wpc(0.0, 0.006, -0.30, 0.028, 0.18, GunMetal),            // barrel flutes
    wpc(0.0, 0.006, -0.635, 0.032, 0.07, GunPolymer),            // muzzle brake
    wpc(0.0, 0.006, -0.668, 0.036, 0.010, GunMetal),          // brake ring
    wp(0.0, 0.048, -0.10, 0.030, 0.014, 0.34, GunPolymer), // rail
    wpc(0.0, 0.098, -0.10, 0.052, 0.30, GunMetal),            // scope tube
    wpc(0.0, 0.098, -0.265, 0.062, 0.045, ControlPanel),        // objective bell
    wpc(0.0, 0.098, -0.292, 0.066, 0.012, GunPolymer),           // sunshade lip
    wpc(0.0, 0.098, 0.058, 0.056, 0.030, Glass),                // eyepiece
    wpc(0.0, 0.098, 0.078, 0.060, 0.012, GunPolymer),               // eye cup
    wpc(0.0, 0.098, -0.13, 0.058, 0.028, GunPolymer),            // elevation turret
    wp(0.030, 0.098, -0.13, 0.030, 0.040, 0.026, GunPolymer),    // windage turret
    wpc(0.0, 0.074, -0.16, 0.030, 0.030, GunMetal),           // front ring
    wpc(0.0, 0.074, -0.03, 0.030, 0.030, GunMetal),           // rear ring
    wp(0.030, 0.014, 0.05, 0.030, 0.016, 0.016, GunPolymer), // bolt handle
    wpc(0.044, 0.014, 0.05, 0.022, 0.022, GunPolymer),           // bolt knob
    wpx(0.0, -0.118, 0.005, 0.036, 0.130, 0.062, -0.14, GunPolymer), // magazine
    wpx(0.0, -0.106, 0.095, 0.042, 0.140, 0.052, 0.32, GunPolymer), // grip
    wp(0.0, -0.052, 0.055, 0.028, 0.010, 0.050, GunMetal),    // trigger guard
    wp(0.0, 0.030, 0.215, 0.048, 0.062, 0.16, WoodPlank),       // cheek riser
    wp(0.0, -0.020, 0.240, 0.046, 0.090, 0.12, WoodPlank),      // butt stock
];

// Light machine gun: box magazine, carry handle, heavy barrel, bipod.
const LMG: [WeaponPart; 20] = [
    wp(0.0, 0.010, -0.05, 0.062, 0.098, 0.36, GunMetal),      // receiver
    wp(0.0, 0.052, -0.05, 0.054, 0.016, 0.34, GunPolymer),       // receiver top
    wpc(0.0, 0.010, -0.40, 0.026, 0.36, GunMetal),             // barrel
    wpc(0.0, 0.010, -0.605, 0.036, 0.06, GunPolymer),            // flash hider
    wp(0.0, 0.048, -0.34, 0.034, 0.048, 0.20, GunPolymer), // heat shield
    wp(0.024, 0.030, -0.34, 0.008, 0.030, 0.18, GunPolymer),     // shield vents
    wp(-0.024, 0.030, -0.34, 0.008, 0.030, 0.18, GunPolymer),
    wp(0.0, 0.078, 0.02, 0.028, 0.030, 0.032, GunMetal),      // rear sight
    wp(0.0, 0.074, -0.44, 0.014, 0.040, 0.020, GunMetal),     // front post
    wp(0.0, 0.090, -0.44, 0.028, 0.010, 0.016, GunMetal),     // front hood
    wpc(0.0, 0.084, -0.16, 0.027, 0.16, GunMetal),             // carry handle
    wp(0.0, -0.128, -0.02, 0.098, 0.150, 0.170, GunPolymer),     // ammunition box
    wp(0.0, -0.128, -0.108, 0.086, 0.120, 0.010, HazardStripe), // box latch
    wp(0.0, -0.052, -0.02, 0.040, 0.030, 0.060, GunPolymer), // feed chute
    wpx(0.0, -0.112, 0.105, 0.044, 0.150, 0.055, 0.32, GunPolymer), // grip
    wp(0.0, -0.056, 0.062, 0.032, 0.011, 0.055, GunMetal),    // trigger guard
    wp(0.0, 0.010, 0.205, 0.050, 0.096, 0.16, GunMetal),      // stock
    wp(0.0, 0.004, 0.292, 0.052, 0.104, 0.024, GunPolymer),           // butt pad
    wpx(0.030, -0.078, -0.46, 0.016, 0.110, 0.016, 0.30, GunMetal),  // bipod leg, right
    wpx(-0.030, -0.078, -0.46, 0.016, 0.110, 0.016, 0.30, GunMetal), // bipod leg, left
];

// Service pistol.
const PISTOL_SMALL: [WeaponPart; 11] = [
    wp(0.0, 0.028, -0.075, 0.032, 0.046, 0.185, GunMetal),    // slide
    wp(0.0, 0.052, -0.075, 0.014, 0.008, 0.175, GunPolymer), // slide rib
    wp(0.022, 0.028, -0.010, 0.006, 0.030, 0.045, GunPolymer),   // slide serrations
    wp(-0.022, 0.028, -0.010, 0.006, 0.030, 0.045, GunPolymer),
    wp(0.0, 0.044, -0.155, 0.012, 0.016, 0.014, GunPolymer),     // front sight
    wp(0.0, 0.046, 0.005, 0.024, 0.018, 0.016, GunPolymer),      // rear sight
    wpc(0.0, 0.008, -0.170, 0.014, 0.030, GunMetal),           // muzzle
    wp(0.0, -0.005, -0.030, 0.030, 0.030, 0.110, GunMetal),   // frame
    wp(0.0, -0.022, -0.090, 0.026, 0.014, 0.060, GunPolymer), // accessory rail
    wpx(0.0, -0.088, 0.030, 0.032, 0.140, 0.048, 0.24, GunPolymer), // grip
    wp(0.0, -0.038, -0.010, 0.022, 0.010, 0.044, GunMetal),   // trigger guard
];

// Heavy revolver.
const PISTOL_HEAVY: [WeaponPart; 11] = [
    wpc(0.0, 0.024, -0.150, 0.025, 0.170, GunMetal),           // barrel
    wp(0.0, 0.046, -0.150, 0.020, 0.016, 0.165, GunPolymer), // top rib
    wp(0.0, -0.002, -0.150, 0.022, 0.024, 0.150, GunMetal),   // ejector shroud
    wpc(0.0, 0.020, -0.030, 0.058, 0.070, GunPolymer),           // cylinder
    wp(0.0, 0.020, -0.030, 0.062, 0.030, 0.060, GunMetal),    // cylinder flutes
    wp(0.0, 0.022, 0.030, 0.030, 0.052, 0.075, GunMetal),     // frame
    wp(0.0, 0.056, 0.052, 0.020, 0.024, 0.026, GunPolymer),      // hammer
    wpc(0.0, 0.020, 0.006, 0.016, 0.048, GunPolymer),    // cylinder pin
    wpx(0.0, -0.078, 0.070, 0.036, 0.145, 0.058, 0.30, WoodPlank), // grip
    wp(0.0, -0.030, 0.020, 0.024, 0.010, 0.050, GunMetal),    // trigger guard
    wp(0.0, 0.050, -0.225, 0.012, 0.018, 0.014, GunPolymer),     // front sight
];

// Combat knife.
const KNIFE: [WeaponPart; 6] = [
    wp(0.0, 0.006, -0.150, 0.010, 0.036, 0.170, GunPolymer), // blade
    wp(0.0, 0.020, -0.235, 0.008, 0.020, 0.045, GunMetal),    // point taper
    wp(0.0, -0.010, -0.140, 0.011, 0.014, 0.110, GunPolymer),    // serrated spine
    wp(0.0, 0.000, -0.052, 0.038, 0.030, 0.016, GunMetal),    // guard
    wp(0.0, 0.000, 0.010, 0.026, 0.030, 0.110, GunPolymer),         // handle
    wp(0.0, 0.000, 0.072, 0.030, 0.034, 0.014, GunPolymer),      // pommel
];

// Entrenching tool.
const SPADE: [WeaponPart; 6] = [
    wp(0.0, 0.000, -0.235, 0.120, 0.018, 0.130, GunPolymer),     // blade
    wp(0.0, 0.000, -0.300, 0.090, 0.014, 0.045, GunPolymer), // blade edge
    wp(0.0, 0.010, -0.235, 0.030, 0.014, 0.110, GunMetal),    // blade spine
    wp(0.0, 0.000, -0.165, 0.040, 0.030, 0.055, GunMetal),    // socket
    wpc(0.0, 0.000, -0.040, 0.024, 0.210, WoodPlank),           // shaft
    wp(0.0, 0.000, 0.075, 0.048, 0.026, 0.030, GunPolymer),         // grip
];

const M_RIFLE: WeaponModel = WeaponModel {
    parts: &RIFLE,
    grip: Vec3::new(0.0, -0.105, 0.085),
    fore: Vec3::new(0.0, -0.035, -0.31),
    muzzle: Vec3::new(0.0, 0.006, -0.645),
    sight: Vec3::new(0.0, 0.076, 0.02),
};
const M_BULLPUP: WeaponModel = WeaponModel {
    parts: &BULLPUP,
    grip: Vec3::new(0.0, -0.095, -0.055),
    fore: Vec3::new(0.0, -0.030, -0.26),
    muzzle: Vec3::new(0.0, 0.010, -0.545),
    sight: Vec3::new(0.0, 0.086, 0.028),
};
const M_SMG: WeaponModel = WeaponModel {
    parts: &SMG,
    grip: Vec3::new(0.0, -0.092, 0.062),
    fore: Vec3::new(0.0, -0.055, -0.185),
    muzzle: Vec3::new(0.0, 0.012, -0.365),
    sight: Vec3::new(0.0, 0.070, -0.005),
};
const M_SHOTGUN: WeaponModel = WeaponModel {
    parts: &SHOTGUN,
    grip: Vec3::new(0.0, -0.082, 0.085),
    fore: Vec3::new(0.0, -0.036, -0.235),
    muzzle: Vec3::new(0.0, 0.022, -0.555),
    sight: Vec3::new(0.0, 0.048, -0.05),
};
const M_SNIPER: WeaponModel = WeaponModel {
    parts: &SNIPER,
    grip: Vec3::new(0.0, -0.100, 0.095),
    fore: Vec3::new(0.0, -0.040, -0.28),
    muzzle: Vec3::new(0.0, 0.006, -0.675),
    sight: Vec3::new(0.0, 0.098, 0.058),
};
const M_LMG: WeaponModel = WeaponModel {
    parts: &LMG,
    grip: Vec3::new(0.0, -0.106, 0.105),
    fore: Vec3::new(0.0, -0.040, -0.34),
    muzzle: Vec3::new(0.0, 0.010, -0.640),
    sight: Vec3::new(0.0, 0.078, 0.02),
};
const M_PISTOL_SMALL: WeaponModel = WeaponModel {
    parts: &PISTOL_SMALL,
    grip: Vec3::new(0.0, -0.080, 0.030),
    fore: Vec3::new(0.0, -0.045, -0.060),
    muzzle: Vec3::new(0.0, 0.020, -0.190),
    sight: Vec3::new(0.0, 0.046, 0.005),
};
const M_PISTOL_HEAVY: WeaponModel = WeaponModel {
    parts: &PISTOL_HEAVY,
    grip: Vec3::new(0.0, -0.072, 0.070),
    fore: Vec3::new(0.0, -0.040, -0.020),
    muzzle: Vec3::new(0.0, 0.024, -0.240),
    sight: Vec3::new(0.0, 0.056, 0.052),
};
const M_KNIFE: WeaponModel = WeaponModel {
    parts: &KNIFE,
    grip: Vec3::new(0.0, 0.000, 0.010),
    fore: Vec3::new(0.0, 0.000, 0.010),
    muzzle: Vec3::new(0.0, 0.010, -0.250),
    sight: Vec3::new(0.0, 0.0, 0.0),
};
const M_SPADE: WeaponModel = WeaponModel {
    parts: &SPADE,
    grip: Vec3::new(0.0, 0.000, 0.060),
    fore: Vec3::new(0.0, 0.000, -0.080),
    muzzle: Vec3::new(0.0, 0.000, -0.300),
    sight: Vec3::new(0.0, 0.0, 0.0),
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
    // A round part's mesh runs along Y and is turned a quarter circle to lie
    // down the bore, so the axis that should follow the weapon's length scale
    // is its Y, not its Z. Scaling in the part's own frame after the rotation
    // is what keeps a long rifle's barrel long rather than merely thick.
    let size = if matches!(part.shape, PartShape::Cylinder | PartShape::Cone)
        && part.rot.x.abs() > 1.0
    {
        Vec3::new(part.size.x * model_scale.x, part.size.y * model_scale.z,
                  part.size.z * model_scale.y)
    } else {
        part.size * model_scale
    };
    base * Mat4::from_translation(part.offset * model_scale)
        * Mat4::from_euler(glam::EulerRot::XYZ, part.rot.x, part.rot.y, part.rot.z)
        * Mat4::from_scale(size)
}

/// A scale applied to a weapon's model so heavier weapons look heavier.
pub fn weapon_model_scale(def: &crate::game::weapons::WeaponDef) -> Vec3 {
    // Longer-ranged weapons get longer barrels; higher-capacity ones get
    // bulkier bodies. Both are derived, so a new weapon needs no art.
    let length = 0.88 + (def.range_far / 200.0).clamp(0.0, 1.0) * 0.30;
    let bulk = 0.92 + (def.mag as f32 / 100.0).clamp(0.0, 1.0) * 0.26;
    Vec3::new(bulk, bulk, length)
}
