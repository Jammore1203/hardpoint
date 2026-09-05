//! Brush geometry and the collision world.
//!
//! Levels are built from axis-aligned boxes plus axis-aligned ramps. That
//! restriction is what makes the whole thing fast: collision is swept-AABB,
//! the broadphase is a flat uniform grid, and a full level's collision data is
//! a few tens of kilobytes that fit comfortably in cache.

use crate::assets::materials::Mat;
use crate::math::{sweep_aabb, Aabb, TraceHit};
use glam::Vec3;

bitflags_lite! {
    /// Behavioural flags on a brush.
    pub struct BrushFlags: u16 {
        /// Blocks player movement.
        const SOLID        = 1 << 0;
        /// Never rendered; used for invisible clip volumes and playspace bounds.
        const NODRAW       = 1 << 1;
        /// Blocks bullets and grenades as well as players.
        const OPAQUE       = 1 << 2;
        /// Climbable; the mover switches to ladder physics inside the volume.
        const LADDER       = 1 << 3;
        /// Rendered with alpha testing.
        const CUTOUT       = 1 << 4;
        /// Excluded from lightmap occlusion (thin detail, railings).
        const NOSHADOW     = 1 << 5;
        /// Blocks bullets but not players (window bars, grates you shoot through).
        const BULLET_CLIP  = 1 << 6;
        /// Do not generate navigation on top of this brush (hazard, decoration).
        const NONAV        = 1 << 7;
        /// Bright surface; skips ambient occlusion darkening.
        const FULLBRIGHT   = 1 << 8;
    }
}

impl BrushFlags {
    /// The common case: a normal solid, visible, bullet-blocking wall.
    pub const WALL: BrushFlags = BrushFlags(Self::SOLID.0 | Self::OPAQUE.0);
}

/// Which of the six faces of a box to emit geometry for. Interior faces are
/// culled at build time, which typically removes a third of a level's
/// triangles for free.
#[derive(Copy, Clone, PartialEq, Eq)]
pub struct FaceMask(pub u8);

impl FaceMask {
    pub const ALL: FaceMask = FaceMask(0b111111);
    pub const NONE: FaceMask = FaceMask(0);
    pub const NEG_X: u8 = 1 << 0;
    pub const POS_X: u8 = 1 << 1;
    pub const NEG_Y: u8 = 1 << 2;
    pub const POS_Y: u8 = 1 << 3;
    pub const NEG_Z: u8 = 1 << 4;
    pub const POS_Z: u8 = 1 << 5;
    /// Everything except the underside, for floors sitting on the void.
    pub const NO_BOTTOM: FaceMask = FaceMask(0b111111 & !Self::NEG_Y);
    #[inline] pub fn has(self, f: u8) -> bool { self.0 & f != 0 }
}

#[derive(Copy, Clone, PartialEq, Eq)]
pub enum RampAxis {
    /// Surface rises along +X.
    PosX,
    NegX,
    PosZ,
    NegZ,
}

#[derive(Copy, Clone)]
pub enum BrushKind {
    Box,
    /// The top surface slopes from the box's min height to its max height
    /// along the given axis. Sides and underside behave like a box.
    Ramp(RampAxis),
}

#[derive(Clone)]
pub struct Brush {
    pub aabb: Aabb,
    pub kind: BrushKind,
    /// Material used for the four vertical sides.
    pub mat: Mat,
    /// Material for the top face; usually differs (floor vs wall).
    pub top: Mat,
    pub flags: BrushFlags,
    pub faces: FaceMask,
    /// Per-brush texture scale in world units per texture repeat.
    pub tex_scale: f32,
    /// Multiplied into baked vertex lighting; lets authors darken interiors.
    pub light_scale: f32,
}

impl Brush {
    pub fn new(aabb: Aabb, mat: Mat) -> Brush {
        Brush {
            aabb,
            kind: BrushKind::Box,
            mat,
            top: mat,
            flags: BrushFlags::WALL,
            faces: FaceMask::ALL,
            tex_scale: 2.0,
            light_scale: 1.0,
        }
    }

    #[inline]
    pub fn is_solid(&self) -> bool { self.flags.contains(BrushFlags::SOLID) }
    #[inline]
    pub fn blocks_bullets(&self) -> bool {
        self.flags.contains(BrushFlags::OPAQUE) || self.flags.contains(BrushFlags::BULLET_CLIP)
    }

    /// Height of the walkable surface at a world XZ position. For ramps this
    /// interpolates; for boxes it is simply the top.
    #[inline]
    pub fn surface_height(&self, x: f32, z: f32) -> f32 {
        match self.kind {
            BrushKind::Box => self.aabb.max.y,
            BrushKind::Ramp(axis) => {
                let (lo, hi, v) = match axis {
                    RampAxis::PosX => (self.aabb.min.x, self.aabb.max.x, x),
                    RampAxis::NegX => (self.aabb.max.x, self.aabb.min.x, x),
                    RampAxis::PosZ => (self.aabb.min.z, self.aabb.max.z, z),
                    RampAxis::NegZ => (self.aabb.max.z, self.aabb.min.z, z),
                };
                let t = if (hi - lo).abs() < 1e-5 { 1.0 } else { ((v - lo) / (hi - lo)).clamp(0.0, 1.0) };
                self.aabb.min.y + (self.aabb.max.y - self.aabb.min.y) * t
            }
        }
    }

    /// Collision box used for the sweep pass. Ramps are swept against only
    /// their lower portion so a mover walks onto them instead of into them;
    /// the ramp surface itself is resolved separately as a height field.
    #[inline]
    pub fn sweep_box(&self) -> Aabb {
        match self.kind {
            BrushKind::Box => self.aabb,
            BrushKind::Ramp(_) => Aabb::new(self.aabb.min, Vec3::new(self.aabb.max.x, self.aabb.min.y, self.aabb.max.z)),
        }
    }
}

/// Uniform grid broadphase over the XZ plane.
///
/// Levels are small (roughly 100 m square), so a flat grid of index lists beats
/// any tree: build is linear, lookup is a couple of array reads, and there are
/// no pointer chases in the trace inner loop.
pub struct BrushGrid {
    origin: glam::Vec2,
    cell_size: f32,
    cells_x: i32,
    cells_z: i32,
    /// CSR layout: `starts[i]..starts[i+1]` indexes into `indices`.
    starts: Vec<u32>,
    indices: Vec<u32>,
}

const GRID_CELL: f32 = 6.0;

impl BrushGrid {
    pub fn build(brushes: &[Brush], bounds: Aabb) -> BrushGrid {
        let origin = glam::Vec2::new(bounds.min.x - GRID_CELL, bounds.min.z - GRID_CELL);
        let span_x = (bounds.max.x - bounds.min.x + GRID_CELL * 2.0).max(GRID_CELL);
        let span_z = (bounds.max.z - bounds.min.z + GRID_CELL * 2.0).max(GRID_CELL);
        let cells_x = (span_x / GRID_CELL).ceil() as i32 + 1;
        let cells_z = (span_z / GRID_CELL).ceil() as i32 + 1;
        let cell_count = (cells_x * cells_z) as usize;

        // Two-pass counting sort into CSR; no per-cell Vec allocations.
        fn cells_for(b: &Brush, origin: glam::Vec2, cells_x: i32, cells_z: i32, mut f: impl FnMut(usize)) {
            let x0 = (((b.aabb.min.x - origin.x) / GRID_CELL).floor() as i32).clamp(0, cells_x - 1);
            let x1 = (((b.aabb.max.x - origin.x) / GRID_CELL).floor() as i32).clamp(0, cells_x - 1);
            let z0 = (((b.aabb.min.z - origin.y) / GRID_CELL).floor() as i32).clamp(0, cells_z - 1);
            let z1 = (((b.aabb.max.z - origin.y) / GRID_CELL).floor() as i32).clamp(0, cells_z - 1);
            for z in z0..=z1 {
                for x in x0..=x1 {
                    f((z * cells_x + x) as usize);
                }
            }
        }

        let mut counts = vec![0u32; cell_count + 1];
        for b in brushes {
            cells_for(b, origin, cells_x, cells_z, |c| counts[c + 1] += 1);
        }
        for i in 0..cell_count { counts[i + 1] += counts[i]; }
        let total = counts[cell_count] as usize;
        let mut indices = vec![0u32; total];
        let mut cursor = counts.clone();
        for (bi, b) in brushes.iter().enumerate() {
            cells_for(b, origin, cells_x, cells_z, |c| {
                indices[cursor[c] as usize] = bi as u32;
                cursor[c] += 1;
            });
        }

        BrushGrid { origin, cell_size: GRID_CELL, cells_x, cells_z, starts: counts, indices }
    }

    #[inline]
    fn cell_of(&self, x: f32, z: f32) -> (i32, i32) {
        (
            (((x - self.origin.x) / self.cell_size).floor() as i32).clamp(0, self.cells_x - 1),
            (((z - self.origin.y) / self.cell_size).floor() as i32).clamp(0, self.cells_z - 1),
        )
    }

    #[inline]
    fn cell_slice(&self, cx: i32, cz: i32) -> &[u32] {
        if cx < 0 || cz < 0 || cx >= self.cells_x || cz >= self.cells_z { return &[]; }
        let i = (cz * self.cells_x + cx) as usize;
        let a = self.starts[i] as usize;
        let b = self.starts[i + 1] as usize;
        &self.indices[a..b]
    }

    /// Calls `f` for every brush index whose cell overlaps the box. A brush
    /// spanning several cells is reported more than once, so callers must
    /// tolerate duplicates (all of ours do: they take a minimum or a max).
    #[inline]
    pub fn query_aabb(&self, b: &Aabb, mut f: impl FnMut(u32)) {
        let (x0, z0) = self.cell_of(b.min.x, b.min.z);
        let (x1, z1) = self.cell_of(b.max.x, b.max.z);
        for z in z0..=z1 {
            for x in x0..=x1 {
                for &i in self.cell_slice(x, z) { f(i); }
            }
        }
    }

    /// Walks the grid along a ray with a 2D DDA, visiting cells in order so a
    /// trace can stop at the first hit instead of testing the whole level.
    pub fn query_ray(&self, origin: Vec3, dir: Vec3, max_t: f32, mut f: impl FnMut(u32) -> bool) {
        let (mut cx, mut cz) = self.cell_of(origin.x, origin.z);
        let step_x = if dir.x > 0.0 { 1 } else { -1 };
        let step_z = if dir.z > 0.0 { 1 } else { -1 };

        let inv_x = if dir.x.abs() < 1e-8 { f32::MAX } else { self.cell_size / dir.x.abs() };
        let inv_z = if dir.z.abs() < 1e-8 { f32::MAX } else { self.cell_size / dir.z.abs() };

        let cell_min_x = self.origin.x + cx as f32 * self.cell_size;
        let cell_min_z = self.origin.y + cz as f32 * self.cell_size;
        let mut next_x = if dir.x.abs() < 1e-8 {
            f32::MAX
        } else if dir.x > 0.0 {
            (cell_min_x + self.cell_size - origin.x) / dir.x
        } else {
            (cell_min_x - origin.x) / dir.x
        };
        let mut next_z = if dir.z.abs() < 1e-8 {
            f32::MAX
        } else if dir.z > 0.0 {
            (cell_min_z + self.cell_size - origin.z) / dir.z
        } else {
            (cell_min_z - origin.z) / dir.z
        };

        // Guard against pathological loops on degenerate input.
        let mut guard = (self.cells_x + self.cells_z) * 2 + 8;
        loop {
            for &i in self.cell_slice(cx, cz) {
                if !f(i) { return; }
            }
            guard -= 1;
            if guard <= 0 { return; }
            if next_x < next_z {
                if next_x > max_t { return; }
                next_x += inv_x;
                cx += step_x;
                if cx < 0 || cx >= self.cells_x { return; }
            } else {
                if next_z > max_t { return; }
                next_z += inv_z;
                cz += step_z;
                if cz < 0 || cz >= self.cells_z { return; }
            }
        }
    }
}

/// Everything the simulation needs to collide against a level.
pub struct CollisionWorld {
    pub brushes: Vec<Brush>,
    pub grid: BrushGrid,
    pub bounds: Aabb,
}

/// What a trace is allowed to collide with.
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum TraceMask {
    /// Player movement: solid brushes only.
    Solid,
    /// Bullets and line of sight: solids plus bullet clips.
    Shot,
    /// Grenades: same as bullets, they bounce off the same things.
    Projectile,
}

impl CollisionWorld {
    pub fn new(brushes: Vec<Brush>) -> CollisionWorld {
        let mut bounds = Aabb::EMPTY;
        for b in &brushes {
            bounds = bounds.union(&b.aabb);
        }
        if brushes.is_empty() {
            bounds = Aabb::new(Vec3::splat(-1.0), Vec3::splat(1.0));
        }
        let grid = BrushGrid::build(&brushes, bounds);
        CollisionWorld { brushes, grid, bounds }
    }

    #[inline]
    fn passes(&self, b: &Brush, mask: TraceMask) -> bool {
        match mask {
            TraceMask::Solid => b.is_solid(),
            TraceMask::Shot | TraceMask::Projectile => b.blocks_bullets(),
        }
    }

    /// Ray trace against level geometry. Returns the nearest hit.
    pub fn trace_ray(&self, origin: Vec3, dir: Vec3, max_t: f32, mask: TraceMask) -> TraceHit {
        let inv = Vec3::new(
            if dir.x.abs() < 1e-8 { f32::MAX } else { 1.0 / dir.x },
            if dir.y.abs() < 1e-8 { f32::MAX } else { 1.0 / dir.y },
            if dir.z.abs() < 1e-8 { f32::MAX } else { 1.0 / dir.z },
        );
        let mut best = TraceHit::miss(origin + dir * max_t);
        let mut best_t = max_t;

        self.grid.query_ray(origin, dir, max_t, |bi| {
            let b = &self.brushes[bi as usize];
            if !self.passes(b, mask) { return true; }
            // Ramps use their bounding box for shots; the small error is
            // invisible at these polygon sizes and keeps the trace branchless.
            if let Some((t, axis)) = b.aabb.ray_hit(origin, inv, best_t) {
                if t < best_t {
                    best_t = t;
                    let mut n = Vec3::ZERO;
                    n[axis] = if dir[axis] > 0.0 { -1.0 } else { 1.0 };
                    best = TraceHit {
                        fraction: t / max_t.max(1e-6),
                        normal: n,
                        point: origin + dir * t,
                        brush: bi,
                        hit: true,
                    };
                }
            }
            true
        });
        best
    }

    /// True if nothing blocks the segment between two points.
    #[inline]
    pub fn line_of_sight(&self, from: Vec3, to: Vec3) -> bool {
        let delta = to - from;
        let len = delta.length();
        if len < 1e-4 { return true; }
        !self.trace_ray(from, delta / len, len, TraceMask::Shot).hit
    }

    /// Sweep a box through the world and return the first contact.
    pub fn trace_box(&self, box_at_origin: &Aabb, delta: Vec3, mask: TraceMask) -> TraceHit {
        let swept = box_at_origin.union(&box_at_origin.translated(delta)).expanded_uniform(0.02);
        let mut best_t = 1.0f32;
        let mut best_n = Vec3::ZERO;
        let mut best_b = u32::MAX;

        self.grid.query_aabb(&swept, |bi| {
            let b = &self.brushes[bi as usize];
            if !self.passes(b, mask) { return; }
            if let Some((t, n)) = sweep_aabb(box_at_origin, delta, &b.sweep_box()) {
                if t < best_t {
                    best_t = t;
                    best_n = n;
                    best_b = bi;
                }
            }
        });

        if best_b == u32::MAX {
            TraceHit::miss(box_at_origin.center() + delta)
        } else {
            TraceHit {
                fraction: best_t,
                normal: best_n,
                point: box_at_origin.center() + delta * best_t,
                brush: best_b,
                hit: true,
            }
        }
    }

    /// True if the box overlaps any brush matching the mask.
    pub fn box_blocked(&self, b: &Aabb, mask: TraceMask) -> bool {
        let mut hit = false;
        self.grid.query_aabb(b, |bi| {
            if hit { return; }
            let br = &self.brushes[bi as usize];
            if self.passes(br, mask) && br.sweep_box().overlaps(b) { hit = true; }
        });
        hit
    }

    /// Can an agent stand with its feet at `feet`?
    ///
    /// Unlike a plain box-overlap test this ignores geometry the agent could
    /// simply step onto (anything whose top is within `step` of the feet).
    /// That distinction matters enormously: an agent box is wider than a stair
    /// tread, so a rigid overlap test reports every step of every staircase as
    /// blocked and silently removes all navigation from them.
    pub fn standable(&self, feet: Vec3, radius: f32, height: f32, step: f32) -> bool {
        let body = Aabb::from_base(feet, radius, height);
        let mut ok = true;
        self.grid.query_aabb(&body, |bi| {
            if !ok { return; }
            let b = &self.brushes[bi as usize];
            if !b.is_solid() { return; }
            // Ramps are height fields, not obstacles; their walkable surface is
            // resolved by `ramp_surface`/`ground_below` instead.
            if matches!(b.kind, BrushKind::Ramp(_)) { return; }
            if b.aabb.max.y <= feet.y + step { return; }
            if b.aabb.overlaps(&body) { ok = false; }
        });
        ok
    }

    /// Highest ramp surface underneath a point, within `reach` below the
    /// point's feet. Returns `None` when standing over no ramp.
    pub fn ramp_surface(&self, feet: Vec3, radius: f32, reach: f32) -> Option<(f32, u32)> {
        let query = Aabb::new(
            Vec3::new(feet.x - radius, feet.y - reach, feet.z - radius),
            Vec3::new(feet.x + radius, feet.y + 1.0, feet.z + radius),
        );
        let mut best: Option<(f32, u32)> = None;
        self.grid.query_aabb(&query, |bi| {
            let b = &self.brushes[bi as usize];
            if !b.is_solid() { return; }
            if !matches!(b.kind, BrushKind::Ramp(_)) { return; }
            // Only if we are horizontally over the ramp footprint.
            if feet.x < b.aabb.min.x - radius || feet.x > b.aabb.max.x + radius { return; }
            if feet.z < b.aabb.min.z - radius || feet.z > b.aabb.max.z + radius { return; }
            let h = b.surface_height(
                feet.x.clamp(b.aabb.min.x, b.aabb.max.x),
                feet.z.clamp(b.aabb.min.z, b.aabb.max.z),
            );
            if h <= feet.y + 1.0 && h >= feet.y - reach {
                if best.map_or(true, |(bh, _)| h > bh) { best = Some((h, bi)); }
            }
        });
        best
    }

    /// Distance from `feet` down to the nearest solid surface, capped at
    /// `max_drop`. Used for blob shadows and bot ledge avoidance.
    pub fn ground_below(&self, feet: Vec3, radius: f32, max_drop: f32) -> Option<(f32, u32)> {
        let query = Aabb::new(
            Vec3::new(feet.x - radius, feet.y - max_drop, feet.z - radius),
            Vec3::new(feet.x + radius, feet.y + 0.05, feet.z + radius),
        );
        let mut best: Option<(f32, u32)> = None;
        self.grid.query_aabb(&query, |bi| {
            let b = &self.brushes[bi as usize];
            if !b.is_solid() { return; }
            if feet.x < b.aabb.min.x - radius || feet.x > b.aabb.max.x + radius { return; }
            if feet.z < b.aabb.min.z - radius || feet.z > b.aabb.max.z + radius { return; }
            let h = b.surface_height(
                feet.x.clamp(b.aabb.min.x, b.aabb.max.x),
                feet.z.clamp(b.aabb.min.z, b.aabb.max.z),
            );
            if h <= feet.y + 0.05 && h >= feet.y - max_drop {
                if best.map_or(true, |(bh, _)| h > bh) { best = Some((h, bi)); }
            }
        });
        best
    }

    pub fn material_at(&self, brush: u32, normal: Vec3) -> Mat {
        match self.brushes.get(brush as usize) {
            Some(b) => if normal.y > 0.5 { b.top } else { b.mat },
            None => Mat::Concrete,
        }
    }
}
