//! Level authoring helpers.
//!
//! Maps are written as sequences of calls into `MapBuilder`. The helpers here
//! are the vocabulary a level takes shape in: rooms, buildings, catwalks,
//! stairs, containers, cover clumps. Keeping them here means the map files
//! stay readable and every level shares the same construction rules (wall
//! thickness, doorway height, step size), which is a large part of why the
//! set feels like one game.

use super::brush::{Brush, BrushFlags, BrushKind, CollisionWorld, FaceMask, RampAxis};
use super::nav::NavGrid;
use super::{Env, MapData, MapId};
use crate::assets::materials::Mat;
use crate::game::types::{Objective, PickupKind, PickupSpot, SpawnPoint, Team};
use crate::math::Aabb;
use glam::Vec3;

/// Standard architectural constants. Every map obeys them, so movement,
/// vaulting and sightlines behave identically everywhere.
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
    pub fn sandbags(&mut self, x: f32, y: f32, z: f32, sx: f32, sz: f32) {
        self.boxx(x, y, z, sx, 1.05, sz, Mat::Sandbag).with_scale(1.2).with_top(Mat::Sandbag);
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
        let h = hole_of(storeys - 1);
        self.floor_with_hole(x, z, sx, sz, top, roof_mat, h.0, h.1, h.2, h.3);
        self.wall_x(x, z, sx, top, 1.0, roof_mat);
        self.wall_x(x, z + sz, sx, top, 1.0, roof_mat);
        self.wall_z(x, z, sz, top, 1.0, roof_mat);
        self.wall_z(x + sx, z, sz, top, 1.0, roof_mat);
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
        self.clip(x0 - t, y_floor - 2.0, z0 - t, t, h, z1 - z0 + t * 2.0);
        self.clip(x1, y_floor - 2.0, z0 - t, t, h, z1 - z0 + t * 2.0);
        self.clip(x0 - t, y_floor - 2.0, z0 - t, x1 - x0 + t * 2.0, h, t);
        self.clip(x0 - t, y_floor - 2.0, z1, x1 - x0 + t * 2.0, h, t);
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

    pub fn finish(mut self) -> MapData {
        self.cull_hidden_faces();

        let mut bounds = Aabb::EMPTY;
        for b in self.brushes.iter().chain(self.decor.iter()) {
            bounds = bounds.union(&b.aabb);
        }
        let play = self.playspace.unwrap_or(bounds);

        let collision = CollisionWorld::new(self.brushes.clone());
        let nav = NavGrid::bake(&collision, play);

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
