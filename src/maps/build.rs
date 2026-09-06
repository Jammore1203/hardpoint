//! Level authoring helpers.
//!
//! Maps are written as sequences of calls into `MapBuilder`. The helpers here
//! are the vocabulary a level takes shape in: rooms, buildings, catwalks,
//! stairs, containers, cover clumps. Keeping them here means the map files
//! stay readable and every level shares the same construction rules (wall
//! thickness, doorway height, step size), which is a large part of why the
//! set feels like one game.

use super::brush::{Brush, BrushFlags, BrushKind, Clips, CollisionWorld, FaceMask, RampAxis, TraceMask};
use crate::core::Rng;
use super::nav::NavGrid;
use super::{Env, MapData, MapId};
use crate::assets::materials::Mat;
use crate::game::types::{Objective, PickupKind, PickupSpot, SpawnPoint, Team};
use crate::math::Aabb;
use glam::Vec3;

/// Standard architectural constants. Every map obeys them, so movement,
/// vaulting and sightlines behave identically everywhere.
/// One kind of clutter the dressing pass can drop on open ground. Each map
/// picks a palette that suits it, so a desert airfield gets revetments and
/// pallets where a village gets planters and stacked timber.
#[derive(Copy, Clone, Debug)]
pub enum CoverPiece {
    Container(Mat),
    /// Material and cube size.
    Crates(Mat, f32),
    Barrels(Mat),
    Sandbags(Mat),
    /// Material, length and height.
    Block(Mat, f32, f32),
    /// Body material and top material.
    Planter(Mat, Mat),
    Pipes(Mat),
}

pub const WALL: f32 = 0.30;
/// Doorways are deliberately wide. Beyond suiting the game's pace, a doorway
/// narrower than about two metres cannot reliably hold a navigation node, and
/// an interior whose doors have no navigation is an interior bots never enter.
pub const DOOR_W: f32 = 2.1;
pub const DOOR_H: f32 = 2.3;
pub const STOREY: f32 = 3.4;
pub const STEP: f32 = 0.22;
/// Width of a railing opening, and of every access staircase.
pub const RAIL_GAP: f32 = 2.6;
pub const STAIR_W: f32 = 2.2;

impl Brush {
    pub fn with_top(&mut self, m: Mat) -> &mut Self { self.top = m; self }
    pub fn with_flags(&mut self, f: BrushFlags) -> &mut Self { self.flags = f; self }
    pub fn add_flags(&mut self, f: BrushFlags) -> &mut Self { self.flags.insert(f); self }
    pub fn with_scale(&mut self, s: f32) -> &mut Self { self.tex_scale = s; self }
    pub fn with_light(&mut self, l: f32) -> &mut Self { self.light_scale = l; self }
    pub fn with_faces(&mut self, f: FaceMask) -> &mut Self { self.faces = f; self }
    pub fn no_bottom(&mut self) -> &mut Self { self.faces = FaceMask::NO_BOTTOM; self }
}

pub struct MapBuilder {
    pub id: MapId,
    pub brushes: Vec<Brush>,
    pub decor: Vec<Brush>,
    pub spawns: Vec<SpawnPoint>,
    pub domination: Vec<Objective>,
    pub bomb_sites: Vec<Objective>,
    pub pickups: Vec<PickupSpot>,
    pub env: Env,
    /// Explicit playspace; if left `None` it is derived from the geometry.
    pub playspace: Option<Aabb>,
    default_mat: Mat,
    default_floor: Mat,
    /// Clutter plan, applied in `finish` once the ground is known.
    dressing: Option<(usize, u32, &'static [CoverPiece])>,
}

impl MapBuilder {
    pub fn new(id: MapId) -> MapBuilder {
        MapBuilder {
            id,
            brushes: Vec::with_capacity(512),
            decor: Vec::with_capacity(128),
            spawns: Vec::with_capacity(32),
            domination: Vec::new(),
            bomb_sites: Vec::new(),
            pickups: Vec::new(),
            env: Env::default(),
            playspace: None,
            default_mat: Mat::Concrete,
            default_floor: Mat::ConcreteFloor,
            dressing: None,
        }
    }

    pub fn set_env(&mut self, env: Env) -> &mut Self { self.env = env; self }
    pub fn defaults(&mut self, wall: Mat, floor: Mat) -> &mut Self {
        self.default_mat = wall;
        self.default_floor = floor;
        self
    }

    // ---------------------------------------------------------------- brushes

    pub fn push(&mut self, b: Brush) -> &mut Brush {
        self.brushes.push(b);
        self.brushes.last_mut().unwrap()
    }

    /// Axis-aligned box given its minimum corner and size.
    pub fn boxx(&mut self, x: f32, y: f32, z: f32, sx: f32, sy: f32, sz: f32, mat: Mat) -> &mut Brush {
        let a = Aabb::new(Vec3::new(x, y, z), Vec3::new(x + sx, y + sy, z + sz));
        self.push(Brush::new(a, mat))
    }

    /// Box given its centre on the XZ plane; convenient for props.
    pub fn boxc(&mut self, cx: f32, y: f32, cz: f32, sx: f32, sy: f32, sz: f32, mat: Mat) -> &mut Brush {
        self.boxx(cx - sx * 0.5, y, cz - sz * 0.5, sx, sy, sz, mat)
    }

    /// Non-colliding decoration. Drawn, never traced.
    pub fn decor(&mut self, x: f32, y: f32, z: f32, sx: f32, sy: f32, sz: f32, mat: Mat) -> &mut Brush {
        let a = Aabb::new(Vec3::new(x, y, z), Vec3::new(x + sx, y + sy, z + sz));
        let mut b = Brush::new(a, mat);
        b.flags = BrushFlags::NOSHADOW | BrushFlags::NONAV;
        self.decor.push(b);
        self.decor.last_mut().unwrap()
    }

    /// Invisible collision volume: playspace limits, ledge blockers.
    pub fn clip(&mut self, x: f32, y: f32, z: f32, sx: f32, sy: f32, sz: f32) -> &mut Brush {
        let b = self.boxx(x, y, z, sx, sy, sz, Mat::Concrete);
        b.flags = BrushFlags::SOLID | BrushFlags::NODRAW | BrushFlags::NONAV;
        b
    }

    /// A ground slab. Never emits its underside.
    pub fn floor(&mut self, x: f32, z: f32, sx: f32, sz: f32, y: f32, mat: Mat) -> &mut Brush {
        let b = self.boxx(x, y - 0.6, z, sx, 0.6, sz, mat);
        b.top = mat;
        b.faces = FaceMask::NO_BOTTOM;
        b.tex_scale = 3.0;
        b
    }

    /// A ceiling / roof slab.
    pub fn ceiling(&mut self, x: f32, z: f32, sx: f32, sz: f32, y: f32, mat: Mat) -> &mut Brush {
        let b = self.boxx(x, y, z, sx, 0.35, sz, mat);
        // Nobody walks on the outside of a ceiling; keep it out of the bake.
        b.flags.insert(BrushFlags::NONAV);
        b
    }

    /// Floor slab with rectangular openings cut out of it, for stairwells and
    /// hatches. The region is split into bands at every hole edge and the
    /// covered bands are simply not emitted, so the opening is real geometry
    /// rather than a hole painted on a texture.
    ///
    /// Holes are `(x, z, size_x, size_z)`.
    pub fn floor_with_holes(&mut self, x: f32, z: f32, sx: f32, sz: f32, y: f32, mat: Mat, holes: &[(f32, f32, f32, f32)]) {
        fn cuts(lo: f32, hi: f32, edges: impl Iterator<Item = f32>) -> Vec<f32> {
            let mut v: Vec<f32> = std::iter::once(lo).chain(std::iter::once(hi))
                .chain(edges.filter(|e| *e > lo + 0.01 && *e < hi - 0.01))
                .collect();
            v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            v.dedup_by(|a, b| (*a - *b).abs() < 0.01);
            v
        }

        let live: Vec<(f32, f32, f32, f32)> = holes.iter().copied()
            .filter(|h| h.2 > 0.01 && h.3 > 0.01)
            .collect();
        if live.is_empty() {
            self.floor(x, z, sx, sz, y, mat);
            return;
        }

        let z_cuts = cuts(z, z + sz, live.iter().flat_map(|h| [h.1, h.1 + h.3]));
        for zi in 0..z_cuts.len().saturating_sub(1) {
            let (z0, z1) = (z_cuts[zi], z_cuts[zi + 1]);
            let zc = (z0 + z1) * 0.5;
            // Only holes spanning this band can cut it in X.
            let band: Vec<&(f32, f32, f32, f32)> = live.iter()
                .filter(|h| zc > h.1 && zc < h.1 + h.3)
                .collect();
            if band.is_empty() {
                self.floor(x, z0, sx, z1 - z0, y, mat);
                continue;
            }
            let x_cuts = cuts(x, x + sx, band.iter().flat_map(|h| [h.0, h.0 + h.2]));
            for xi in 0..x_cuts.len().saturating_sub(1) {
                let (x0, x1) = (x_cuts[xi], x_cuts[xi + 1]);
                let xc = (x0 + x1) * 0.5;
                if band.iter().any(|h| xc > h.0 && xc < h.0 + h.2) { continue; }
                self.floor(x0, z0, x1 - x0, z1 - z0, y, mat);
            }
        }
    }

    /// Convenience wrapper for the single-opening case.
    #[allow(clippy::too_many_arguments)]
    pub fn floor_with_hole(&mut self, x: f32, z: f32, sx: f32, sz: f32, y: f32, mat: Mat,
                           hx: f32, hz: f32, hsx: f32, hsz: f32) {
        self.floor_with_holes(x, z, sx, sz, y, mat, &[(hx, hz, hsx, hsz)]);
    }

    /// Wall running along X.
    pub fn wall_x(&mut self, x: f32, z: f32, len: f32, y: f32, h: f32, mat: Mat) -> &mut Brush {
        self.boxx(x, y, z - WALL * 0.5, len, h, WALL, mat)
    }

    /// Wall running along Z.
    pub fn wall_z(&mut self, x: f32, z: f32, len: f32, y: f32, h: f32, mat: Mat) -> &mut Brush {
        self.boxx(x - WALL * 0.5, y, z, WALL, h, len, mat)
    }

    /// Wall along X with a doorway centred at `door_at` (an offset from `x`).
    pub fn wall_x_door(&mut self, x: f32, z: f32, len: f32, y: f32, h: f32, door_at: f32, mat: Mat) {
        let d0 = (door_at - DOOR_W * 0.5).clamp(0.0, len);
        let d1 = (door_at + DOOR_W * 0.5).clamp(0.0, len);
        if d0 > 0.02 { self.wall_x(x, z, d0, y, h, mat); }
        if len - d1 > 0.02 { self.wall_x(x + d1, z, len - d1, y, h, mat); }
        if h > DOOR_H {
            self.wall_x(x + d0, z, d1 - d0, y + DOOR_H, h - DOOR_H, mat);
        }
    }

    /// Wall along Z with a doorway centred at `door_at` (an offset from `z`).
    pub fn wall_z_door(&mut self, x: f32, z: f32, len: f32, y: f32, h: f32, door_at: f32, mat: Mat) {
        let d0 = (door_at - DOOR_W * 0.5).clamp(0.0, len);
        let d1 = (door_at + DOOR_W * 0.5).clamp(0.0, len);
        if d0 > 0.02 { self.wall_z(x, z, d0, y, h, mat); }
        if len - d1 > 0.02 { self.wall_z(x, z + d1, len - d1, y, h, mat); }
        if h > DOOR_H {
            self.wall_z(x, z + d0, d1 - d0, y + DOOR_H, h - DOOR_H, mat);
        }
    }

    /// Wall along X with a waist-height window band: cover you can shoot over.
    pub fn wall_x_window(&mut self, x: f32, z: f32, len: f32, y: f32, h: f32, sill: f32, head: f32, mat: Mat) {
        self.wall_x(x, z, len, y, sill, mat);
        if h > head { self.wall_x(x, z, len, y + head, h - head, mat); }
    }

    pub fn wall_z_window(&mut self, x: f32, z: f32, len: f32, y: f32, h: f32, sill: f32, head: f32, mat: Mat) {
        self.wall_z(x, z, len, y, sill, mat);
        if h > head { self.wall_z(x, z, len, y + head, h - head, mat); }
    }

    /// A ramp. `axis` is the direction the surface rises in.
    pub fn ramp(&mut self, x: f32, y: f32, z: f32, sx: f32, sy: f32, sz: f32, axis: RampAxis, mat: Mat) -> &mut Brush {
        let a = Aabb::new(Vec3::new(x, y, z), Vec3::new(x + sx, y + sy, z + sz));
        let mut b = Brush::new(a, mat);
        b.kind = BrushKind::Ramp(axis);
        b.top = mat;
        self.brushes.push(b);
        self.brushes.last_mut().unwrap()
    }

    /// A staircase built from discrete steps. Cheaper to collide than a ramp
    /// and reads better with the era's chunky geometry.
    pub fn stairs(&mut self, x: f32, y: f32, z: f32, width: f32, run: f32, rise: f32, axis: RampAxis, mat: Mat) {
        let steps = (rise / STEP).ceil().max(1.0) as i32;
        let step_rise = rise / steps as f32;
        let step_run = run / steps as f32;
        for i in 0..steps {
            let h = step_rise * (i + 1) as f32;
            let t = i as f32 * step_run;
            let (bx, bz, sx, sz) = match axis {
                RampAxis::PosX => (x + t, z, step_run + 0.02, width),
                RampAxis::NegX => (x + run - t - step_run, z, step_run + 0.02, width),
                RampAxis::PosZ => (x, z + t, width, step_run + 0.02),
                RampAxis::NegZ => (x, z + run - t - step_run, width, step_run + 0.02),
            };
            self.boxx(bx, y, bz, sx, h, sz, mat).with_scale(1.5);
        }
    }

    /// A rectangular room: floor, four walls with optional doorways, optional
    /// ceiling. `doors` is a bitmask of `DOOR_NX | DOOR_PX | DOOR_NZ | DOOR_PZ`.
    #[allow(clippy::too_many_arguments)]
    pub fn room(&mut self, x: f32, z: f32, sx: f32, sz: f32, y: f32, h: f32, doors: u8, wall_mat: Mat, floor_mat: Mat, ceil: bool) {
        self.floor(x, z, sx, sz, y, floor_mat);
        if doors & DOOR_NZ != 0 {
            self.wall_x_door(x, z, sx, y, h, sx * 0.5, wall_mat);
        } else {
            self.wall_x(x, z, sx, y, h, wall_mat);
        }
        if doors & DOOR_PZ != 0 {
            self.wall_x_door(x, z + sz, sx, y, h, sx * 0.5, wall_mat);
        } else {
            self.wall_x(x, z + sz, sx, y, h, wall_mat);
        }
        if doors & DOOR_NX != 0 {
            self.wall_z_door(x, z, sz, y, h, sz * 0.5, wall_mat);
        } else {
            self.wall_z(x, z, sz, y, h, wall_mat);
        }
        if doors & DOOR_PX != 0 {
            self.wall_z_door(x + sx, z, sz, y, h, sz * 0.5, wall_mat);
        } else {
            self.wall_z(x + sx, z, sz, y, h, wall_mat);
        }
        if ceil { self.ceiling(x, z, sx, sz, y + h, wall_mat); }
    }

    /// Elevated walkway with railings along its two long sides.
    ///
    /// `openings` lists positions along the walk axis where both railings are
    /// broken, leaving a gap wide enough to walk through. Every catwalk that a
    /// staircase arrives at partway along its length needs one: a continuous
    /// railing seals the walkway off completely, which is invisible when
    /// looking at the level and obvious the moment a bot tries to use it.
    #[allow(clippy::too_many_arguments)]
    pub fn catwalk(&mut self, x: f32, y: f32, z: f32, sx: f32, sz: f32, mat: Mat, rail_x: bool, openings: &[f32]) {
        self.boxx(x, y - 0.25, z, sx, 0.25, sz, mat).with_scale(2.0).no_bottom();
        let rail_h = 1.05;
        let rail_flags = BrushFlags::SOLID | BrushFlags::NOSHADOW | BrushFlags::NONAV;

        // Railing segments between the openings, along whichever axis runs.
        let (lo, hi) = if rail_x { (x, x + sx) } else { (z, z + sz) };
        let mut cuts: Vec<(f32, f32)> = openings.iter()
            .map(|o| (o - RAIL_GAP * 0.5, o + RAIL_GAP * 0.5))
            .filter(|(a, b)| *b > lo && *a < hi)
            .collect();
        cuts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

        let mut segments: Vec<(f32, f32)> = Vec::with_capacity(cuts.len() + 1);
        let mut cursor = lo;
        for (a, b) in cuts {
            if a > cursor + 0.05 { segments.push((cursor, a.min(hi))); }
            cursor = cursor.max(b);
        }
        if hi > cursor + 0.05 { segments.push((cursor, hi)); }

        for (a, b) in segments {
            let len = b - a;
            if rail_x {
                for zz in [z - 0.05, z + sz - 0.05] {
                    self.boxx(a, y, zz, len, rail_h, 0.10, Mat::PipeMetal).with_flags(rail_flags);
                }
            } else {
                for xx in [x - 0.05, x + sx - 0.05] {
                    self.boxx(xx, y, a, 0.10, rail_h, len, Mat::PipeMetal).with_flags(rail_flags);
                }
            }
        }
    }

    /// A staircase that climbs to `y_top` and arrives at the plane `edge`,
    /// centred on `at`. `along_x` picks the axis it climbs along;
    /// `from_negative` means it approaches `edge` from the low side.
    ///
    /// The run is derived from the rise so every staircase in the game has the
    /// same, comfortably navigable pitch.
    #[allow(clippy::too_many_arguments)]
    pub fn access_stair(&mut self, at: f32, edge: f32, y_base: f32, y_top: f32, along_x: bool, from_negative: bool, mat: Mat) {
        let rise = y_top - y_base;
        if rise <= 0.01 { return; }
        let run = (rise * 1.5).max(1.6);
        let w = STAIR_W;
        match (along_x, from_negative) {
            (true, true) => self.stairs(edge - run, y_base, at - w * 0.5, w, run, rise, RampAxis::PosX, mat),
            (true, false) => self.stairs(edge, y_base, at - w * 0.5, w, run, rise, RampAxis::NegX, mat),
            (false, true) => self.stairs(at - w * 0.5, y_base, edge - run, w, run, rise, RampAxis::PosZ, mat),
            (false, false) => self.stairs(at - w * 0.5, y_base, edge, w, run, rise, RampAxis::NegZ, mat),
        }
    }

    /// A shipping container, the workhorse prop of the whole game.
    pub fn container(&mut self, cx: f32, y: f32, cz: f32, along_x: bool, mat: Mat) -> &mut Brush {
        let (sx, sz) = if along_x { (6.0, 2.44) } else { (2.44, 6.0) };
        let b = self.boxc(cx, y, cz, sx, 2.6, sz, mat);
        b.top = mat;
        b.tex_scale = 2.6;
        b
    }

    /// Stacked crates of a given base size; a reliable piece of chest-high cover.
    pub fn crates(&mut self, cx: f32, y: f32, cz: f32, size: f32, count: u32, mat: Mat) {
        let mut yy = y;
        for i in 0..count {
            let jitter = (i as f32 * 0.37).sin() * size * 0.12;
            self.boxc(cx + jitter, yy, cz - jitter * 0.6, size, size, size, mat).with_scale(size);
            yy += size;
        }
    }

    /// A low sandbag emplacement: cover you can shoot over when standing.
    ///
    /// Built as a stepped stack rather than a single course. A metre-high
    /// block is above the step height, so navigation offers it as a climb and
    /// a player walking that link wedges against the face of it; a half-height
    /// course on each long side makes the same emplacement something you can
    /// actually walk up, which is also how sandbags are stacked.
    pub fn sandbags(&mut self, x: f32, y: f32, z: f32, sx: f32, sz: f32) {
        self.boxx(x, y, z, sx, 1.05, sz, Mat::Sandbag).with_scale(1.2).with_top(Mat::Sandbag);
        let step = 0.62f32;
        if sx >= sz {
            self.boxx(x, y, z - step, sx, 0.52, step, Mat::Sandbag).with_scale(1.2).with_top(Mat::Sandbag);
            self.boxx(x, y, z + sz, sx, 0.52, step, Mat::Sandbag).with_scale(1.2).with_top(Mat::Sandbag);
        } else {
            self.boxx(x - step, y, z, step, 0.52, sz, Mat::Sandbag).with_scale(1.2).with_top(Mat::Sandbag);
            self.boxx(x + sx, y, z, step, 0.52, sz, Mat::Sandbag).with_scale(1.2).with_top(Mat::Sandbag);
        }
    }

    /// Concrete barrier line, the standard chest-high cover unit.
    pub fn barrier(&mut self, x: f32, y: f32, z: f32, sx: f32, sz: f32) {
        self.boxx(x, y, z, sx, 1.10, sz, Mat::Concrete).with_scale(2.0);
    }

    /// A fuel drum. Small, round-ish cover; also a grenade landmark.
    pub fn barrel(&mut self, cx: f32, y: f32, cz: f32, mat: Mat) {
        self.boxc(cx, y, cz, 0.66, 0.92, 0.66, mat).with_scale(0.9);
    }

    /// A vertical pillar or support column.
    pub fn pillar(&mut self, cx: f32, y: f32, cz: f32, w: f32, h: f32, mat: Mat) -> &mut Brush {
        self.boxc(cx, y, cz, w, h, w, mat)
    }

    /// Distant scenery outside the playspace.
    ///
    /// A level that stops at its own walls reads as a diorama: a few buildings
    /// on a plane with nothing beyond them. Real competitive maps are almost
    /// entirely surrounded by geometry you can never reach, and that is most
    /// of why they feel like a place rather than an arena. This is that
    /// geometry: never collided with, never navigated, never lit properly,
    /// just a silhouette in the haze past the edge of play.
    ///
    /// `seed` decides the skyline, so a map's backdrop is stable between runs
    /// and between clients.
    #[allow(clippy::too_many_arguments)]
    pub fn backdrop(&mut self, seed: u32, ring: f32, count: u32,
                    min_h: f32, max_h: f32, mats: &[Mat]) {
        let Some(space) = self.playspace else { return };
        let mut rng = Rng::seeded(seed ^ 0xBD_0000);
        let cx = (space.min.x + space.max.x) * 0.5;
        let cz = (space.min.z + space.max.z) * 0.5;
        let half_x = (space.max.x - space.min.x) * 0.5;
        let half_z = (space.max.z - space.min.z) * 0.5;

        for i in 0..count {
            // Spread around the ring rather than at random angles, so the
            // horizon has no gaps for the void to show through. Two depths:
            // a near row that reads as the next street, and a far row that
            // reads as the rest of the town.
            let a = (i as f32 + rng.range(0.15, 0.85)) / count as f32 * std::f32::consts::TAU;
            let far = i % 3 == 0;
            let depth = if far { rng.range(ring * 0.7, ring * 1.6) } else { rng.range(4.0, ring * 0.55) };
            let (sx, sz) = (a.cos(), a.sin());
            // Project out to the edge of the playspace along this angle, then
            // step further out by `depth`.
            let edge = (half_x / sx.abs().max(0.15)).min(half_z / sz.abs().max(0.15));
            let d = edge + 8.0 + depth;
            let x = cx + sx * d;
            let z = cz + sz * d;

            // The far row is taller, so the skyline has a profile instead of
            // being a single band of equal blocks.
            let h = if far { rng.range(max_h * 0.8, max_h * 1.7) } else { rng.range(min_h, max_h) };
            let w = rng.range(9.0, 22.0);
            let dpt = rng.range(9.0, 22.0);
            let mat = mats[(rng.next_u32() as usize) % mats.len().max(1)];
            let b = self.decor(x - w * 0.5, space.min.y, z - dpt * 0.5, w, h, dpt, mat);
            b.tex_scale = 3.5;
            // Backdrop is lit flat and brightly: it sits in the haze, and
            // shading it like playable geometry only makes it look near.
            b.light_scale = 1.25;

            // A roofline detail or two, because a skyline of plain boxes reads
            // as a wall of boxes.
            if rng.chance(0.55) {
                let tw = w * rng.range(0.25, 0.5);
                let th = rng.range(2.0, 7.0);
                self.decor(x - tw * 0.5 + rng.range(-w * 0.2, w * 0.2), space.min.y + h,
                           z - tw * 0.5, tw, th, tw, mat).light_scale = 1.25;
            }
        }
    }

    /// A ridge of low scenery just outside the playspace: treelines, spoil
    /// heaps, a harbour wall. Fills the gap between the play area and the
    /// backdrop so the horizon has no seam at ground level.
    pub fn skirt(&mut self, seed: u32, count: u32, height: f32, mat: Mat) {
        let Some(space) = self.playspace else { return };
        let mut rng = Rng::seeded(seed ^ 0x5417);
        let cx = (space.min.x + space.max.x) * 0.5;
        let cz = (space.min.z + space.max.z) * 0.5;
        let half_x = (space.max.x - space.min.x) * 0.5;
        let half_z = (space.max.z - space.min.z) * 0.5;
        for i in 0..count {
            let a = (i as f32 + 0.5) / count as f32 * std::f32::consts::TAU;
            let (sx, sz) = (a.cos(), a.sin());
            let edge = (half_x / sx.abs().max(0.12)).min(half_z / sz.abs().max(0.12));
            let d = edge + rng.range(2.0, 7.0);
            let (x, z) = (cx + sx * d, cz + sz * d);
            let h = height * rng.range(0.7, 1.5);
            // Wide and overlapping: the skirt has to be continuous, because a
            // gap in it is a hole through to nothing at ground level, which is
            // exactly the seam the backdrop exists to hide.
            let w = rng.range(14.0, 26.0);
            let d = rng.range(8.0, 16.0);
            self.decor(x - w * 0.5, space.min.y - 2.0, z - d * 0.5, w, h + 2.0, d, mat)
                .light_scale = 1.1;
        }
    }

    /// A stack of floors joined by staircases that alternate ends, with a
    /// balcony overlooking whatever is outside it.
    ///
    /// The point is vertical play that is actually reachable: a route up that
    /// is fought over on the way, rather than a roof you can see and not get
    /// to. Alternating the flights matters - stacking them leaves a climber's
    /// head in the flight above and quietly seals the building.
    #[allow(clippy::too_many_arguments)]
    pub fn stack_house(&mut self, x: f32, z: f32, sx: f32, sz: f32, y: f32, floors: u32,
                       doors: u8, wall: Mat, floor_mat: Mat) {
        let well = STAIR_W + 0.6;
        for f in 0..floors {
            let fy = y + f as f32 * STOREY;
            if f == 0 {
                self.floor(x, z, sx, sz, fy, floor_mat);
            }
            // Walls with openings on the requested sides.
            let d = if f == 0 { doors } else { doors | DOOR_PX };
            if d & DOOR_NZ != 0 { self.wall_x_door(x, z, sx, fy, STOREY, sx * 0.5, wall); }
            else { self.wall_x(x, z, sx, fy, STOREY, wall); }
            if d & DOOR_PZ != 0 { self.wall_x_door(x, z + sz, sx, fy, STOREY, sx * 0.5, wall); }
            else { self.wall_x(x, z + sz, sx, fy, STOREY, wall); }
            if d & DOOR_NX != 0 { self.wall_z_door(x, z, sz, fy, STOREY, sz * 0.5, wall); }
            else { self.wall_z(x, z, sz, fy, STOREY, wall); }
            // The +X face is a balcony rail above the ground floor, so the
            // upper storeys can shoot out and be shot at.
            if f == 0 {
                if d & DOOR_PX != 0 { self.wall_z_door(x + sx, z, sz, fy, STOREY, sz * 0.5, wall); }
                else { self.wall_z(x + sx, z, sz, fy, STOREY, wall); }
            } else {
                self.wall_z(x + sx, z, sz, fy, 1.3, wall);
            }

            // The floor above, with the stairwell cut out of it, and the
            // flight that reaches it. Flights alternate ends.
            let top = fy + STOREY;
            let near_z = f % 2 == 0;
            let well_z = if near_z { z + 0.4 } else { z + sz - well - 0.4 };
            self.floor_with_hole(x, z, sx, sz, top, floor_mat,
                                 x + sx - well - 0.4, well_z, well, well);
            let axis = if near_z { RampAxis::PosZ } else { RampAxis::NegZ };
            let run = (STOREY * 1.5).max(1.6);
            let stair_z = if near_z { well_z + well } else { well_z - run };
            self.stairs(x + sx - well - 0.2, fy, stair_z, STAIR_W, run, STOREY, axis, floor_mat);
        }
    }

    /// A wall panel that can be shot away.
    ///
    /// The point is not spectacle: it is that a position defended by cover can
    /// be attacked by removing the cover, so holding an angle is a decision
    /// with a counter rather than a fact about the level.
    #[allow(clippy::too_many_arguments)]
    pub fn breakable(&mut self, x: f32, y: f32, z: f32, sx: f32, sy: f32, sz: f32,
                     health: f32, mat: Mat) -> &mut Brush {
        let b = self.boxx(x, y, z, sx, sy, sz, mat);
        b.flags.insert(BrushFlags::BREAKABLE);
        b.health = health.max(1.0);
        b.tex_scale = 1.6;
        b
    }

    /// A breakable panel filling a doorway-sized hole in a wall: a shortcut
    /// that has to be opened before it exists.
    pub fn breakable_wall_x(&mut self, x: f32, y: f32, z: f32, len: f32, h: f32, health: f32, mat: Mat) {
        self.breakable(x, y, z - WALL * 0.5, len, h, WALL, health, mat);
    }

    pub fn breakable_wall_z(&mut self, x: f32, y: f32, z: f32, len: f32, h: f32, health: f32, mat: Mat) {
        self.breakable(x - WALL * 0.5, y, z, WALL, h, len, health, mat);
    }

    /// A block with one vertical corner cut off at 45 degrees.
    ///
    /// The cheapest way to stop a building being a box: chamfered corners read
    /// as architecture, and they stop a player rounding a corner from being
    /// briefly inside the wall's silhouette.
    #[allow(clippy::too_many_arguments)]
    pub fn chamfer(&mut self, x: f32, y: f32, z: f32, sx: f32, sy: f32, sz: f32,
                   cut: f32, corner: Corner, mat: Mat) -> &mut Brush {
        let (x1, z1) = (x + sx, z + sz);
        let cut = cut.min(sx * 0.98).min(sz * 0.98);
        let mid = glam::Vec2::new(x + sx * 0.5, z + sz * 0.5);
        let clips = match corner {
            Corner::NegXNegZ => Clips::new().cut(x + cut, z, x, z + cut, mid),
            Corner::PosXNegZ => Clips::new().cut(x1 - cut, z, x1, z + cut, mid),
            Corner::PosXPosZ => Clips::new().cut(x1 - cut, z1, x1, z1 - cut, mid),
            Corner::NegXPosZ => Clips::new().cut(x + cut, z1, x, z1 - cut, mid),
        };
        let b = self.boxx(x, y, z, sx, sy, sz, mat);
        b.kind = BrushKind::Clipped(clips);
        b
    }

    /// An octagonal column: a box with all four corners cut.
    ///
    /// Eight sides is as round as anything gets here, and it is exact for both
    /// collision and rendering rather than a box pretending to be a cylinder.
    pub fn column(&mut self, cx: f32, y: f32, cz: f32, r: f32, h: f32, mat: Mat) -> &mut Brush {
        let c = r * 0.586; // 45-degree chamfer that makes a regular octagon
        let mid = glam::Vec2::new(cx, cz);
        let (x0, z0, x1, z1) = (cx - r, cz - r, cx + r, cz + r);
        let clips = Clips::new()
            .cut(x0 + c, z0, x0, z0 + c, mid)
            .cut(x1 - c, z0, x1, z0 + c, mid)
            .cut(x1 - c, z1, x1, z1 - c, mid)
            .cut(x0 + c, z1, x0, z1 - c, mid);
        let b = self.boxx(x0, y, z0, r * 2.0, h, r * 2.0, mat);
        b.kind = BrushKind::Clipped(clips);
        b.tex_scale = 1.8;
        b
    }

    /// A wall running along an arbitrary line rather than an axis.
    ///
    /// Four clip planes turn the bounding box into the rotated box the wall
    /// actually is, so a diagonal wall collides exactly where it is drawn.
    /// Every street in the game ran at right angles before this existed.
    #[allow(clippy::too_many_arguments)]
    pub fn wall_diag(&mut self, x0: f32, z0: f32, x1: f32, z1: f32,
                     y: f32, h: f32, thick: f32, mat: Mat) -> &mut Brush {
        let (dx, dz) = (x1 - x0, z1 - z0);
        let len = (dx * dx + dz * dz).sqrt().max(1e-4);
        let (ux, uz) = (dx / len, dz / len);
        // Perpendicular offset for the two long faces.
        let (px, pz) = (-uz * thick * 0.5, ux * thick * 0.5);
        let mid = glam::Vec2::new((x0 + x1) * 0.5, (z0 + z1) * 0.5);
        let clips = Clips::new()
            .cut(x0 + px, z0 + pz, x1 + px, z1 + pz, mid)
            .cut(x0 - px, z0 - pz, x1 - px, z1 - pz, mid)
            .cut(x0 + px, z0 + pz, x0 - px, z0 - pz, mid)
            .cut(x1 + px, z1 + pz, x1 - px, z1 - pz, mid);
        let (lo_x, hi_x) = (x0.min(x1) - thick, x0.max(x1) + thick);
        let (lo_z, hi_z) = (z0.min(z1) - thick, z0.max(z1) + thick);
        let b = self.boxx(lo_x, y, lo_z, hi_x - lo_x, h, hi_z - lo_z, mat);
        b.kind = BrushKind::Clipped(clips);
        b
    }

    /// A room whose corners are cut, so the interior reads as a shape rather
    /// than a cube. Doors follow the same bitmask as `room`.
    #[allow(clippy::too_many_arguments)]
    pub fn chamfered_room(&mut self, x: f32, z: f32, sx: f32, sz: f32, y: f32, h: f32,
                          doors: u8, cut: f32, wall_mat: Mat, floor_mat: Mat, ceil: bool) {
        self.room(x, z, sx, sz, y, h, doors, wall_mat, floor_mat, ceil);
        // Fill each corner with a chamfered block, which reads as a pillar
        // inside and a cut corner outside.
        let c = cut.max(0.6);
        for (corner, cx, cz) in [
            (Corner::PosXPosZ, x, z),
            (Corner::NegXPosZ, x + sx - c, z),
            (Corner::PosXNegZ, x, z + sz - c),
            (Corner::NegXNegZ, x + sx - c, z + sz - c),
        ] {
            self.chamfer(cx, y, cz, c, h, c, c, corner, wall_mat);
        }
    }

    /// Chest-high freestanding cover: the single most useful piece in the set.
    ///
    /// Waist height means a standing player can shoot over it and a crouching
    /// one cannot be shot, which is the whole basis of positional play. A map
    /// made of full-height walls and open ground has only two states - seen
    /// and unseen - and plays like a corridor shooter in a field.
    pub fn half_wall(&mut self, x: f32, y: f32, z: f32, len: f32, along_x: bool, mat: Mat) -> &mut Brush {
        let (sx, sz) = if along_x { (len, WALL * 1.4) } else { (WALL * 1.4, len) };
        self.boxx(x, y, z, sx, 1.15, sz, mat).with_scale(2.0)
    }

    /// A full-height wall that chops a sightline, with a single doorway so the
    /// space behind it stays connected.
    ///
    /// This is the tool for the "too open" problem. Dropping cover onto a big
    /// field lowers the number of angles slightly; cutting the field into
    /// rooms with two ways between them changes what the space is.
    #[allow(clippy::too_many_arguments)]
    pub fn divider(&mut self, x: f32, y: f32, z: f32, len: f32, h: f32, along_x: bool, door_at: f32, mat: Mat) {
        if along_x {
            self.wall_x_door(x, z, len, y, h, door_at, mat);
        } else {
            self.wall_z_door(x, z, len, y, h, door_at, mat);
        }
    }

    /// A roofed connector between two areas: two walls, a floor and a lid,
    /// open at both ends.
    ///
    /// Every good competitive map is mostly connectors. They are what make a
    /// rotation cost time, give a defender something to hold that is not a
    /// sightline across the whole level, and let a team move without being
    /// seen from three positions at once.
    #[allow(clippy::too_many_arguments)]
    pub fn connector(&mut self, x: f32, z: f32, sx: f32, sz: f32, y: f32, h: f32, along_x: bool, wall_mat: Mat, floor_mat: Mat) {
        self.floor(x, z, sx, sz, y, floor_mat);
        if along_x {
            self.wall_x(x, z, sx, y, h, wall_mat);
            self.wall_x(x, z + sz, sx, y, h, wall_mat);
        } else {
            self.wall_z(x, z, sz, y, h, wall_mat);
            self.wall_z(x + sx, z, sz, y, h, wall_mat);
        }
        self.ceiling(x, z, sx, sz, y + h, wall_mat);
    }

    /// A boxed-in objective site: three walls, one open face, a doorway on one
    /// of the closed sides, and crates inside to plant behind.
    ///
    /// The open face and the doorway deliberately face different directions,
    /// so attackers arriving by the two routes arrive on different timings and
    /// a defender cannot watch both from one position.
    #[allow(clippy::too_many_arguments)]
    pub fn site_box(&mut self, x: f32, z: f32, sx: f32, sz: f32, y: f32, h: f32,
                    open: u8, door: u8, wall_mat: Mat, floor_mat: Mat, crate_mat: Mat) {
        self.floor(x, z, sx, sz, y, floor_mat);
        let mut face = |bit: u8, along_x: bool, at_far: bool| {
            if open & bit != 0 { return; }
            let (wx, wz, len, mid) = if along_x {
                (x, if at_far { z + sz } else { z }, sx, sx * 0.5)
            } else {
                (if at_far { x + sx } else { x }, z, sz, sz * 0.5)
            };
            if door & bit != 0 {
                if along_x { self.wall_x_door(wx, wz, len, y, h, mid, wall_mat); }
                else { self.wall_z_door(wx, wz, len, y, h, mid, wall_mat); }
            } else if along_x {
                self.wall_x(wx, wz, len, y, h, wall_mat);
            } else {
                self.wall_z(wx, wz, len, y, h, wall_mat);
            }
        };
        face(DOOR_NZ, true, false);
        face(DOOR_PZ, true, true);
        face(DOOR_NX, false, false);
        face(DOOR_PX, false, true);

        // Cover inside: one stack to plant behind, one to climb.
        self.crates(x + sx * 0.24, y, z + sz * 0.28, 1.35, 2, crate_mat);
        self.crates(x + sx * 0.74, y, z + sz * 0.70, 1.25, 1, crate_mat);
        self.half_wall(x + sx * 0.35, y, z + sz * 0.62, sx * 0.34, true, wall_mat);
    }

    /// A staggered line of chest-high cover along an axis.
    ///
    /// Staggered rather than aligned: a straight row of identical blocks is a
    /// wall with holes in it, and players read it as one. Offsetting alternate
    /// pieces gives each one two approaches and a flank.
    #[allow(clippy::too_many_arguments)]
    pub fn cover_line(&mut self, x: f32, y: f32, z: f32, len: f32, along_x: bool, count: u32, mat: Mat) {
        if count == 0 { return; }
        let step = len / count as f32;
        let piece = (step * 0.55).clamp(1.4, 4.2);
        for i in 0..count {
            let t = (i as f32 + 0.5) * step - piece * 0.5;
            let off = if i % 2 == 0 { -0.9 } else { 0.9 };
            if along_x {
                self.half_wall(x + t, y, z + off, piece, true, mat);
            } else {
                self.half_wall(x + off, y, z + t, piece, false, mat);
            }
        }
    }

    /// A see-through chain fence: blocks movement, not bullets or sight.
    pub fn fence_x(&mut self, x: f32, y: f32, z: f32, len: f32, h: f32) {
        let b = self.boxx(x, y, z - 0.04, len, h, 0.08, Mat::Mesh);
        b.flags = BrushFlags::SOLID | BrushFlags::CUTOUT | BrushFlags::NOSHADOW | BrushFlags::NONAV;
        b.tex_scale = 2.0;
    }

    pub fn fence_z(&mut self, x: f32, y: f32, z: f32, len: f32, h: f32) {
        let b = self.boxx(x - 0.04, y, z, 0.08, h, len, Mat::Mesh);
        b.flags = BrushFlags::SOLID | BrushFlags::CUTOUT | BrushFlags::NOSHADOW | BrushFlags::NONAV;
        b.tex_scale = 2.0;
    }

    /// A multi-storey building shell with a doorway or window band on every
    /// face and an internal staircase serving every floor including the roof.
    ///
    /// Successive flights alternate between the two ends of the -Z wall. That
    /// is not decoration: stacking flights directly above one another leaves a
    /// climber's head inside the flight above, which quietly makes the whole
    /// building unreachable.
    #[allow(clippy::too_many_arguments)]
    pub fn tower(&mut self, x: f32, z: f32, sx: f32, sz: f32, y: f32, storeys: u32, wall_mat: Mat, floor_mat: Mat, roof_mat: Mat) {
        let run = (STOREY * 1.5).max(1.6);
        let z0 = z + 0.6;
        // Two alternating stair bays, side by side against the -Z wall.
        let bay = |s: u32| -> f32 {
            if s % 2 == 0 { x + 0.5 } else { (x + sx - 0.5 - STAIR_W).max(x + 0.5) }
        };
        let hole_of = |s: u32| -> (f32, f32, f32, f32) {
            (bay(s) - 0.3, z0 - 0.3, STAIR_W + 0.6, run + 0.4)
        };

        for s in 0..storeys {
            let fy = y + s as f32 * STOREY;
            if s == 0 {
                self.floor(x, z, sx, sz, fy, floor_mat);
            } else {
                let h = hole_of(s - 1);
                self.floor_with_hole(x, z, sx, sz, fy, floor_mat, h.0, h.1, h.2, h.3);
            }
            // Alternating openings so the building has flow rather than one
            // obvious way in.
            let openings = if s % 2 == 0 { DOOR_NZ | DOOR_PX } else { DOOR_PZ | DOOR_NX };
            if openings & DOOR_NZ != 0 { self.wall_x_door(x, z, sx, fy, STOREY, sx * 0.5, wall_mat); }
            else { self.wall_x_window(x, z, sx, fy, STOREY, 1.0, 2.4, wall_mat); }
            if openings & DOOR_PZ != 0 { self.wall_x_door(x, z + sz, sx, fy, STOREY, sx * 0.5, wall_mat); }
            else { self.wall_x_window(x, z + sz, sx, fy, STOREY, 1.0, 2.4, wall_mat); }
            if openings & DOOR_NX != 0 { self.wall_z_door(x, z, sz, fy, STOREY, sz * 0.5, wall_mat); }
            else { self.wall_z_window(x, z, sz, fy, STOREY, 1.0, 2.4, wall_mat); }
            if openings & DOOR_PX != 0 { self.wall_z_door(x + sx, z, sz, fy, STOREY, sz * 0.5, wall_mat); }
            else { self.wall_z_window(x + sx, z, sz, fy, STOREY, 1.0, 2.4, wall_mat); }

            self.stairs(bay(s), fy, z0, STAIR_W, run, STOREY, RampAxis::PosZ, floor_mat);
        }

        let top = y + storeys as f32 * STOREY;
        // Roof with a parapet: a power position that still has counterplay.
        //
        // The parapet is chest high rather than waist high on purpose. At a
        // metre it stops a crouching player and nothing else, so a roof is a
        // place you are seen from everywhere on the map; at 1.3 it is cover
        // you stand behind and lean out of, which is what makes height worth
        // taking rather than worth avoiding.
        let h = hole_of(storeys - 1);
        self.floor_with_hole(x, z, sx, sz, top, roof_mat, h.0, h.1, h.2, h.3);
        self.wall_x(x, z, sx, top, 1.3, roof_mat);
        self.wall_x(x, z + sz, sx, top, 1.3, roof_mat);
        self.wall_z(x, z, sz, top, 1.3, roof_mat);
        self.wall_z(x + sx, z, sz, top, 1.3, roof_mat);

        // A head-house over the stairwell and a plant box, so the roof has
        // something to fight around instead of being an empty rectangle.
        self.boxx(h.0, top, h.1 - 0.9, h.2, 2.3, 0.6, roof_mat);
        self.boxc(x + sx * 0.35, top, z + sz * 0.62,
                  (sx * 0.28).min(3.2), 1.5, (sz * 0.24).min(2.6), roof_mat)
            .with_scale(1.8);
    }

    // ------------------------------------------------------------- gameplay

    pub fn spawn(&mut self, x: f32, y: f32, z: f32, yaw_deg: f32, team: Team) -> &mut Self {
        self.spawns.push(SpawnPoint::new(Vec3::new(x, y, z), yaw_deg).team(team));
        self
    }

    pub fn spawn_initial(&mut self, x: f32, y: f32, z: f32, yaw_deg: f32, team: Team) -> &mut Self {
        self.spawns.push(SpawnPoint::new(Vec3::new(x, y, z), yaw_deg).team(team).initial());
        self
    }

    /// A cluster of spawns spread around a point, which prevents the classic
    /// "everyone materialises on one tile" problem.
    pub fn spawn_cluster(&mut self, cx: f32, y: f32, cz: f32, yaw_deg: f32, team: Team, count: u32, spread: f32, initial: bool) {
        for i in 0..count {
            let a = i as f32 * 2.399_963; // golden angle: even, non-repeating
            let r = spread * ((i as f32 + 0.5) / count as f32).sqrt();
            let p = Vec3::new(cx + a.cos() * r, y, cz + a.sin() * r);
            let mut s = SpawnPoint::new(p, yaw_deg + (i as f32 * 11.0) % 30.0 - 15.0).team(team);
            if initial { s = s.initial(); }
            self.spawns.push(s);
        }
    }

    pub fn dom(&mut self, label: &'static str, x: f32, y: f32, z: f32, r: f32) -> &mut Self {
        self.domination.push(Objective::new(label, Vec3::new(x, y, z), r));
        self
    }

    pub fn site(&mut self, label: &'static str, x: f32, y: f32, z: f32, r: f32) -> &mut Self {
        self.bomb_sites.push(Objective::new(label, Vec3::new(x, y, z), r));
        self
    }

    pub fn pickup(&mut self, x: f32, y: f32, z: f32, kind: PickupKind) -> &mut Self {
        self.pickups.push(PickupSpot { pos: Vec3::new(x, y, z), kind, respawn: match kind {
            PickupKind::Ammo => 14.0,
            PickupKind::Armor => 26.0,
            PickupKind::Health => 20.0,
            PickupKind::Grenade => 22.0,
            PickupKind::Weapon => 24.0,
        }});
        self
    }

    /// A large decorative ground plane beyond the playspace.
    ///
    /// Without it the world simply stops at the clip wall and the player sees
    /// a hard line where the sand meets the sky. It is non-colliding, carries
    /// no navigation, and costs two triangles.
    pub fn apron(&mut self, mat: Mat, y: f32) {
        let b = self.playspace.unwrap_or(Aabb::new(Vec3::splat(-50.0), Vec3::splat(50.0)));
        let pad = 240.0;
        let brush = self.decor(
            b.min.x - pad, y - 0.9, b.min.z - pad,
            (b.max.x - b.min.x) + pad * 2.0, 0.9, (b.max.z - b.min.z) + pad * 2.0,
            mat,
        );
        brush.faces = FaceMask(FaceMask::POS_Y);
        brush.tex_scale = 4.0;
        brush.top = mat;
    }

    /// Boundary: a floor slab plus invisible walls, so nobody escapes the map.
    pub fn playspace(&mut self, x0: f32, z0: f32, x1: f32, z1: f32, y_floor: f32, y_ceiling: f32) {
        self.playspace = Some(Aabb::new(
            Vec3::new(x0, y_floor - 4.0, z0),
            Vec3::new(x1, y_ceiling, z1),
        ));
        let t = 3.0;
        let h = y_ceiling - y_floor + 8.0;
        // The boundary stops bullets as well as bodies. It is a wall; rounds
        // leaving the world through it were both wrong and a way to shoot
        // someone standing behind the map edge.
        for (bx, bz, sx, sz) in [
            (x0 - t, z0 - t, t, z1 - z0 + t * 2.0),
            (x1, z0 - t, t, z1 - z0 + t * 2.0),
            (x0 - t, z0 - t, x1 - x0 + t * 2.0, t),
            (x0 - t, z1, x1 - x0 + t * 2.0, t),
        ] {
            let b = self.clip(bx, y_floor - 2.0, bz, sx, h, sz);
            b.flags |= BrushFlags::BULLET_CLIP;
        }
    }

    // --------------------------------------------------------------- finish

    /// Removes box faces that are completely covered by another opaque box.
    /// Buildings sitting on the ground, stacked crates and abutting walls all
    /// shed geometry here; on the larger maps this removes roughly a third of
    /// the triangles at zero visual cost.
    fn cull_hidden_faces(&mut self) {
        use super::brush::BrushGrid;
        let mut bounds = Aabb::EMPTY;
        for b in &self.brushes { bounds = bounds.union(&b.aabb); }
        if self.brushes.is_empty() { return; }
        let grid = BrushGrid::build(&self.brushes, bounds);

        // Faces are indexed 0..6 as (-X, +X, -Y, +Y, -Z, +Z).
        const AXES: [(usize, bool, u8); 6] = [
            (0, false, FaceMask::NEG_X), (0, true, FaceMask::POS_X),
            (1, false, FaceMask::NEG_Y), (1, true, FaceMask::POS_Y),
            (2, false, FaceMask::NEG_Z), (2, true, FaceMask::POS_Z),
        ];

        let snapshot: Vec<(Aabb, bool, BrushKind)> = self.brushes.iter()
            .map(|b| (b.aabb, b.flags.contains(BrushFlags::OPAQUE) && !b.flags.contains(BrushFlags::CUTOUT), b.kind))
            .collect();

        let mut new_masks: Vec<FaceMask> = self.brushes.iter().map(|b| b.faces).collect();

        for (i, b) in self.brushes.iter().enumerate() {
            if b.flags.contains(BrushFlags::NODRAW) { continue; }
            if !matches!(b.kind, BrushKind::Box) { continue; }
            let mut mask = b.faces;
            for &(axis, positive, bit) in &AXES {
                if !mask.has(bit) { continue; }
                let plane = if positive { b.aabb.max[axis] } else { b.aabb.min[axis] };
                // Probe just outside the face.
                let mut probe = b.aabb;
                if positive {
                    probe.min[axis] = plane + 0.001;
                    probe.max[axis] = plane + 0.02;
                } else {
                    probe.max[axis] = plane - 0.001;
                    probe.min[axis] = plane - 0.02;
                }
                let mut covered = false;
                grid.query_aabb(&probe.expanded_uniform(0.05), |j| {
                    if covered || j as usize == i { return; }
                    let (other, opaque, kind) = snapshot[j as usize];
                    if !opaque || !matches!(kind, BrushKind::Box) { return; }
                    // Coplanar and fully covering in the other two axes?
                    let touch = if positive { (other.min[axis] - plane).abs() < 0.02 } else { (other.max[axis] - plane).abs() < 0.02 };
                    if !touch { return; }
                    let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
                    if other.min[u] <= b.aabb.min[u] + 0.01 && other.max[u] >= b.aabb.max[u] - 0.01
                        && other.min[v] <= b.aabb.min[v] + 0.01 && other.max[v] >= b.aabb.max[v] - 0.01
                    {
                        covered = true;
                    }
                });
                if covered { mask.0 &= !bit; }
            }
            new_masks[i] = mask;
        }
        for (b, m) in self.brushes.iter_mut().zip(new_masks) { b.faces = m; }
    }

    /// Fills the most exposed ground with era-appropriate clutter.
    ///
    /// Hand-placing cover across twelve maps by eye is how you end up with
    /// half of them still reading as an empty field, which is exactly the
    /// complaint this answers. Instead the builder measures its own openness,
    /// takes the worst cells and dresses them from the map's own palette,
    /// aligned to a grid and rotated in right angles so it reads as a yard
    /// somebody stacked rather than as scattered noise.
    ///
    /// Nothing is placed near a spawn, an objective or a pickup, nothing goes
    /// next to another piece, and every piece is short enough to shoot over or
    /// vault, so this adds cover without closing a single route. The map audit
    /// and the traversal test both run afterwards and would catch it if it did.
    pub fn dress_open_ground(&mut self, budget: usize, seed: u32, palette: &'static [CoverPiece]) {
        self.dressing = Some((budget, seed, palette));
    }

    /// Places the clutter, using a preliminary navigation bake to know where
    /// the ground players actually walk on is.
    ///
    /// Probing for the highest surface in a column instead put half of it on
    /// roofs, where it changed nothing: the nav node count did not move and
    /// neither did the openness measurement.
    fn apply_dressing(&mut self, nav: &NavGrid) {
        let Some((budget, seed, palette)) = self.dressing else { return };
        if palette.is_empty() || nav.nodes.is_empty() { return; }
        let Some(play) = self.playspace else { return };
        const CELL: f32 = 3.0;
        const CLEAR: f32 = 5.5;

        // Somewhere a piece must not go: spawns, objectives and pickups.
        let mut keep_out: Vec<Vec3> = Vec::new();
        keep_out.extend(self.spawns.iter().map(|s| s.pos));
        keep_out.extend(self.domination.iter().map(|o| o.pos));
        keep_out.extend(self.bomb_sites.iter().map(|o| o.pos));
        keep_out.extend(self.pickups.iter().map(|p| p.pos));

        let collision = CollisionWorld::new(self.brushes.clone());
        let nx = (((play.max.x - play.min.x) / CELL).floor() as i32).max(1);
        let nz = (((play.max.z - play.min.z) / CELL).floor() as i32).max(1);

        // One candidate per coarse cell, taken from the navigation node in it
        // that sits lowest: the ground floor is where the fighting happens.
        let mut best_in_cell: Vec<Option<Vec3>> = vec![None; (nx * nz) as usize];
        for n in nav.nodes.iter() {
            let ix = ((n.pos.x - play.min.x) / CELL).floor() as i32;
            let iz = ((n.pos.z - play.min.z) / CELL).floor() as i32;
            if ix < 0 || iz < 0 || ix >= nx || iz >= nz { continue; }
            let slot = &mut best_in_cell[(iz * nx + ix) as usize];
            match slot {
                Some(p) if p.y <= n.pos.y => {}
                _ => *slot = Some(n.pos),
            }
        }

        let mut candidates: Vec<(f32, i32, i32, Vec3)> = Vec::new();
        let mut rejected_close = 0u32;
        let mut rejected_stand = 0u32;
        let mut rejected_keepout = 0u32;
        let mut rejected_ground = 0u32;
        for iz in 0..nz {
            for ix in 0..nx {
                let Some(node) = best_in_cell[(iz * nx + ix) as usize] else {
                    rejected_ground += 1;
                    continue;
                };
                let feet = node + Vec3::Y * 0.05;
                // A prop needs more room than a player, or it seals the gap.
                if !collision.standable(feet, 1.15, 2.4, 0.3) { rejected_stand += 1; continue; }
                if keep_out.iter().any(|k| (*k - feet).length() < CLEAR) { rejected_keepout += 1; continue; }

                // How exposed is it? Nearest obstruction on eight bearings.
                let eye = feet + Vec3::Y * 1.05;
                let mut nearest = f32::MAX;
                for i in 0..8 {
                    let a = i as f32 / 8.0 * std::f32::consts::TAU;
                    let dir = Vec3::new(a.cos(), 0.0, a.sin());
                    let hit = collision.trace_ray(eye, dir, 30.0, TraceMask::Shot);
                    let d = if hit.hit { (hit.point - eye).length() } else { 30.0 };
                    nearest = nearest.min(d);
                }
                if nearest < 6.0 { rejected_close += 1; continue; }
                candidates.push((nearest, ix, iz, feet));
            }
        }

        // Worst first, but never two pieces in adjacent cells: cover you can
        // move between is cover, a wall is not.
        candidates.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut used: Vec<(i32, i32)> = Vec::new();
        let mut rng = Rng::seeded(seed ^ 0x9E37_79B9);
        let mut placed = 0usize;
        for (_, ix, iz, feet) in candidates {
            if placed >= budget { break; }
            if used.iter().any(|(ux, uz)| (ux - ix).abs() <= 1 && (uz - iz).abs() <= 1) { continue; }
            let piece = palette[rng.below(palette.len() as u32) as usize];
            let jx = rng.range(-0.6, 0.6);
            let jz = rng.range(-0.6, 0.6);
            let turned = rng.chance(0.5);
            self.place_cover(piece, feet.x + jx, feet.y, feet.z + jz, turned, &mut rng);
            used.push((ix, iz));
            placed += 1;
        }
        if std::env::var_os("HARDPOINT_TRACE").is_some() {
            eprintln!("[dress] {:?}: placed {}/{}  candidates {}  rejected: ground {} stand {} keepout {} close {}",
                      self.id, placed, budget, used.len(), rejected_ground, rejected_stand,
                      rejected_keepout, rejected_close);
        }
    }

    fn place_cover(&mut self, piece: CoverPiece, x: f32, y: f32, z: f32, turned: bool, rng: &mut Rng) {
        match piece {
            CoverPiece::Container(mat) => { self.container(x, y, z, turned, mat); }
            CoverPiece::Crates(mat, size) => {
                self.crates(x, y, z, size, 1 + rng.below(2) as u32, mat);
            }
            CoverPiece::Barrels(mat) => {
                let n = 2 + rng.below(3);
                for i in 0..n {
                    let a = i as f32 / n as f32 * std::f32::consts::TAU;
                    self.barrel(x + a.cos() * 0.55, y, z + a.sin() * 0.55, mat);
                }
            }
            CoverPiece::Sandbags(mat) => {
                let (sx, sz) = if turned { (1.0, 3.2) } else { (3.2, 1.0) };
                self.boxc(x, y, z, sx, 1.05, sz, mat).with_scale(1.2);
            }
            CoverPiece::Block(mat, w, h) => {
                let (sx, sz) = if turned { (1.1, w) } else { (w, 1.1) };
                self.boxc(x, y, z, sx, h, sz, mat).with_scale(1.6);
            }
            CoverPiece::Planter(mat, top) => {
                let b = self.boxc(x, y, z, 2.2, 0.95, 2.2, mat);
                b.top = top;
                b.tex_scale = 1.4;
            }
            CoverPiece::Pipes(mat) => {
                for i in 0..3 {
                    let o = (i as f32 - 1.0) * 0.75;
                    let (px, pz) = if turned { (x + o, z) } else { (x, z + o) };
                    let (sx, sz) = if turned { (0.7, 3.4) } else { (3.4, 0.7) };
                    self.boxc(px, y + i as f32 * 0.02, pz, sx, 0.7, sz, mat).with_scale(1.4);
                }
            }
        }
    }

    pub fn finish(mut self) -> MapData {
        self.cull_hidden_faces();

        let mut bounds = Aabb::EMPTY;
        for b in self.brushes.iter().chain(self.decor.iter()) {
            bounds = bounds.union(&b.aabb);
        }
        let play = self.playspace.unwrap_or(bounds);

        // Navigation is baked twice: once to find the ground the dressing pass
        // should use, then again over the geometry it added. A map without
        // dressing pays nothing for this.
        let mut collision = CollisionWorld::new(self.brushes.clone());
        let mut nav = NavGrid::bake(&collision, play);
        if self.dressing.is_some() {
            self.apply_dressing(&nav);
            self.cull_hidden_faces();
            collision = CollisionWorld::new(self.brushes.clone());
            nav = NavGrid::bake(&collision, play);
        }

        let mut map = MapData {
            id: self.id,
            brushes: self.brushes,
            collision,
            spawns: self.spawns,
            domination: self.domination,
            bomb_sites: self.bomb_sites,
            pickups: self.pickups,
            env: self.env,
            nav,
            bounds: play,
            decor: self.decor,
            repairs: (0, 0),
        };

        map.snap_markers();

        // Trim navigation down to what a player can actually reach from a
        // spawn. Everything else is roofs and rack tops. The escape hatch
        // exists so `--nav` can show the raw bake when diagnosing a map.
        if std::env::var_os("HARDPOINT_NAV_RAW").is_some() { return map; }
        let seeds: Vec<Vec3> = map.spawns.iter()
            .filter_map(|s| map.resolve_spawn(s.pos).map(|(p, _)| p))
            .collect();
        map.nav.prune_to_reachable(&seeds);
        let (dropped, moved) = map.bind_markers_to_navigation();
        map.repairs = (dropped, moved);
        map
    }
}

pub const DOOR_NX: u8 = 1 << 0;
pub const DOOR_PX: u8 = 1 << 1;
pub const DOOR_NZ: u8 = 1 << 2;
pub const DOOR_PZ: u8 = 1 << 3;
pub const DOOR_ALL: u8 = 0b1111;
pub const DOOR_NONE: u8 = 0;

/// Which horizontal corner of a block a chamfer removes.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Corner { NegXNegZ, PosXNegZ, PosXPosZ, NegXPosZ }
