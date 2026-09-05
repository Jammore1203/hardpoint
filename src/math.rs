//! Geometry primitives shared by the renderer, the collision system and the
//! authoritative simulation. Everything here is deterministic and allocation
//! free so the server and the client's prediction produce identical results.

use glam::{Vec3, Vec3Swizzles};

/// Axis-aligned bounding box. The world is built almost entirely from these:
/// they make collision, raycasts and broadphase culling trivially cheap, which
/// is exactly the tradeoff an early-2000s console shooter made.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub const EMPTY: Aabb = Aabb {
        min: Vec3::new(f32::MAX, f32::MAX, f32::MAX),
        max: Vec3::new(f32::MIN, f32::MIN, f32::MIN),
    };

    #[inline]
    pub fn new(min: Vec3, max: Vec3) -> Aabb { Aabb { min, max } }

    #[inline]
    pub fn from_center_size(center: Vec3, size: Vec3) -> Aabb {
        let h = size * 0.5;
        Aabb { min: center - h, max: center + h }
    }

    /// Box standing on the ground at `base`, extending upward.
    #[inline]
    pub fn from_base(base: Vec3, half_width: f32, height: f32) -> Aabb {
        Aabb {
            min: Vec3::new(base.x - half_width, base.y, base.z - half_width),
            max: Vec3::new(base.x + half_width, base.y + height, base.z + half_width),
        }
    }

    #[inline] pub fn center(&self) -> Vec3 { (self.min + self.max) * 0.5 }
    #[inline] pub fn size(&self) -> Vec3 { self.max - self.min }
    #[inline] pub fn half(&self) -> Vec3 { (self.max - self.min) * 0.5 }

    #[inline]
    pub fn expand(&self, by: Vec3) -> Aabb {
        Aabb { min: self.min - by, max: self.max + by }
    }

    #[inline]
    pub fn expanded_uniform(&self, by: f32) -> Aabb {
        self.expand(Vec3::splat(by))
    }

    #[inline]
    pub fn translated(&self, by: Vec3) -> Aabb {
        Aabb { min: self.min + by, max: self.max + by }
    }

    #[inline]
    pub fn overlaps(&self, o: &Aabb) -> bool {
        self.min.x < o.max.x && self.max.x > o.min.x &&
        self.min.y < o.max.y && self.max.y > o.min.y &&
        self.min.z < o.max.z && self.max.z > o.min.z
    }

    #[inline]
    pub fn contains_point(&self, p: Vec3) -> bool {
        p.x >= self.min.x && p.x <= self.max.x &&
        p.y >= self.min.y && p.y <= self.max.y &&
        p.z >= self.min.z && p.z <= self.max.z
    }

    #[inline]
    pub fn union(&self, o: &Aabb) -> Aabb {
        Aabb { min: self.min.min(o.min), max: self.max.max(o.max) }
    }

    pub fn union_point(&mut self, p: Vec3) {
        self.min = self.min.min(p);
        self.max = self.max.max(p);
    }

    #[inline]
    pub fn closest_point(&self, p: Vec3) -> Vec3 { p.clamp(self.min, self.max) }

    #[inline]
    pub fn distance_sq_to(&self, p: Vec3) -> f32 { (self.closest_point(p) - p).length_squared() }

    /// Squared horizontal distance, used by 2D relevance and audio culling.
    #[inline]
    pub fn distance_sq_xz(&self, p: Vec3) -> f32 {
        let c = p.clamp(self.min, self.max);
        (c.xz() - p.xz()).length_squared()
    }

    /// Slab test. Returns entry distance along `dir` if the ray hits within
    /// `max_t`, alongside the axis index that was crossed.
    #[inline]
    pub fn ray_hit(&self, origin: Vec3, inv_dir: Vec3, max_t: f32) -> Option<(f32, usize)> {
        let t0 = (self.min - origin) * inv_dir;
        let t1 = (self.max - origin) * inv_dir;
        let tmin_v = t0.min(t1);
        let tmax_v = t0.max(t1);

        let mut tmin = tmin_v.x;
        let mut axis = 0usize;
        if tmin_v.y > tmin { tmin = tmin_v.y; axis = 1; }
        if tmin_v.z > tmin { tmin = tmin_v.z; axis = 2; }
        let tmax = tmax_v.x.min(tmax_v.y).min(tmax_v.z);

        if tmax < tmin.max(0.0) || tmin > max_t { return None; }
        Some((tmin.max(0.0), axis))
    }
}

/// Result of a swept or instantaneous trace against world geometry.
#[derive(Copy, Clone, Debug)]
pub struct TraceHit {
    /// Fraction of the requested motion completed, in `[0, 1]`.
    pub fraction: f32,
    pub normal: Vec3,
    pub point: Vec3,
    /// Index of the brush that was struck, for surface material lookup.
    pub brush: u32,
    pub hit: bool,
}

impl TraceHit {
    pub fn miss(end: Vec3) -> TraceHit {
        TraceHit { fraction: 1.0, normal: Vec3::Y, point: end, brush: u32::MAX, hit: false }
    }
}

/// Surfaces closer than this are treated as touching rather than overlapping.
/// It must exceed the gap the mover leaves after a contact.
pub const CONTACT_EPS: f32 = 1.0e-3;

/// Swept AABB against a static AABB. Returns the fraction of `delta` that can
/// be travelled before contact, and the contact normal.
///
/// This is the classic "expand the obstacle by the mover's half extents and
/// sweep a ray" reduction, which is both exact for AABB-vs-AABB and about as
/// cheap as collision gets.
#[inline]
pub fn sweep_aabb(mover: &Aabb, delta: Vec3, solid: &Aabb) -> Option<(f32, Vec3)> {
    let expanded = Aabb {
        min: solid.min - mover.half(),
        max: solid.max + mover.half(),
    };
    let origin = mover.center();

    // Already interpenetrating: the caller must depenetrate rather than
    // sweep. The box is shrunk by a hair first, because a mover resting
    // exactly flush against a wall sits on the boundary, and treating that as
    // "inside" would report no collision and let it walk straight through.
    if expanded.expanded_uniform(-CONTACT_EPS).contains_point(origin) {
        return None;
    }

    let mut t_enter = 0.0f32;
    let mut t_exit = 1.0f32;
    let mut normal = Vec3::ZERO;

    for axis in 0..3 {
        let d = delta[axis];
        let lo = expanded.min[axis];
        let hi = expanded.max[axis];
        let o = origin[axis];

        if d.abs() < 1e-8 {
            if o < lo || o > hi { return None; }
            continue;
        }
        let inv = 1.0 / d;
        let mut t0 = (lo - o) * inv;
        let mut t1 = (hi - o) * inv;
        let mut n = -1.0f32;
        if t0 > t1 {
            std::mem::swap(&mut t0, &mut t1);
            n = 1.0;
        }
        if t0 > t_enter {
            t_enter = t0;
            normal = Vec3::ZERO;
            normal[axis] = n;
        }
        if t1 < t_exit { t_exit = t1; }
        if t_enter > t_exit { return None; }
    }

    if t_enter > 1.0 || normal == Vec3::ZERO { return None; }
    Some((t_enter, normal))
}

/// Minimum translation vector pushing `a` out of `b` along the axis of least
/// penetration. Used to recover from interpenetration caused by moving
/// platforms, spawn overlap or a rejected prediction.
#[inline]
pub fn depenetrate(a: &Aabb, b: &Aabb) -> Vec3 {
    if !a.overlaps(b) { return Vec3::ZERO; }
    let mut best = f32::MAX;
    let mut out = Vec3::ZERO;
    for axis in 0..3 {
        let push_pos = b.max[axis] - a.min[axis];
        let push_neg = a.max[axis] - b.min[axis];
        let (depth, sign) = if push_pos < push_neg { (push_pos, 1.0) } else { (push_neg, -1.0) };
        if depth < best {
            best = depth;
            out = Vec3::ZERO;
            out[axis] = depth * sign;
        }
    }
    out
}

/// Slide a velocity along a surface, removing the component pushing into it.
#[inline]
pub fn clip_velocity(v: Vec3, normal: Vec3, overbounce: f32) -> Vec3 {
    let backoff = v.dot(normal) * overbounce;
    let mut out = v - normal * backoff;
    // Kill microscopic residue so players do not creep along walls.
    for i in 0..3 {
        if out[i].abs() < 1e-4 { out[i] = 0.0; }
    }
    out
}

/// Reflect a velocity off a surface with restitution and tangential friction.
/// Used by grenades, which need to feel predictable rather than realistic.
#[inline]
pub fn bounce_velocity(v: Vec3, normal: Vec3, restitution: f32, friction: f32) -> Vec3 {
    let vn = normal * v.dot(normal);
    let vt = v - vn;
    vt * (1.0 - friction) - vn * restitution
}

/// Build direction from yaw/pitch. Yaw rotates around +Y, pitch is positive
/// looking up. This convention is used everywhere: camera, aiming, netcode.
#[inline]
pub fn dir_from_angles(yaw: f32, pitch: f32) -> Vec3 {
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    Vec3::new(-sy * cp, sp, -cy * cp)
}

/// Horizontal forward/right basis, used for movement input.
#[inline]
pub fn move_basis(yaw: f32) -> (Vec3, Vec3) {
    let (sy, cy) = yaw.sin_cos();
    let forward = Vec3::new(-sy, 0.0, -cy);
    let right = Vec3::new(cy, 0.0, -sy);
    (forward, right)
}

#[inline]
pub fn angles_from_dir(d: Vec3) -> (f32, f32) {
    let horiz = (d.x * d.x + d.z * d.z).sqrt();
    let pitch = d.y.atan2(horiz.max(1e-6));
    let yaw = (-d.x).atan2(-d.z);
    (yaw, pitch)
}

/// Ray against a Y-axis-aligned cylinder capped at `[y_min, y_max]`. Player
/// hitboxes use boxes, but grenade proximity and bot vision cones use this.
#[inline]
pub fn ray_cylinder(origin: Vec3, dir: Vec3, center_xz: glam::Vec2, radius: f32, y_min: f32, y_max: f32, max_t: f32) -> Option<f32> {
    let ox = origin.x - center_xz.x;
    let oz = origin.z - center_xz.y;
    let a = dir.x * dir.x + dir.z * dir.z;
    if a < 1e-8 {
        // Vertical ray: hits only if inside the radius.
        if ox * ox + oz * oz > radius * radius { return None; }
        let t = if dir.y > 0.0 { (y_min - origin.y) / dir.y } else { (y_max - origin.y) / dir.y };
        return if t >= 0.0 && t <= max_t { Some(t) } else { None };
    }
    let b = 2.0 * (ox * dir.x + oz * dir.z);
    let c = ox * ox + oz * oz - radius * radius;
    let disc = b * b - 4.0 * a * c;
    if disc < 0.0 { return None; }
    let sq = disc.sqrt();
    for t in [(-b - sq) / (2.0 * a), (-b + sq) / (2.0 * a)] {
        if t < 0.0 || t > max_t { continue; }
        let y = origin.y + dir.y * t;
        if y >= y_min && y <= y_max { return Some(t); }
    }
    None
}

/// Six-plane frustum extracted from a view-projection matrix, for culling.
#[derive(Clone, Copy)]
pub struct Frustum {
    planes: [glam::Vec4; 6],
}

impl Frustum {
    pub fn from_view_proj(m: glam::Mat4) -> Frustum {
        let r = m.transpose();
        let rows = [r.x_axis, r.y_axis, r.z_axis, r.w_axis];
        let mut planes = [glam::Vec4::ZERO; 6];
        planes[0] = rows[3] + rows[0]; // left
        planes[1] = rows[3] - rows[0]; // right
        planes[2] = rows[3] + rows[1]; // bottom
        planes[3] = rows[3] - rows[1]; // top
        planes[4] = rows[3] + rows[2]; // near
        planes[5] = rows[3] - rows[2]; // far
        for p in planes.iter_mut() {
            let len = glam::Vec3::new(p.x, p.y, p.z).length();
            if len > 1e-6 { *p /= len; }
        }
        Frustum { planes }
    }

    #[inline]
    pub fn test_aabb(&self, b: &Aabb) -> bool {
        let center = b.center();
        let half = b.half();
        for p in &self.planes {
            let n = glam::Vec3::new(p.x, p.y, p.z);
            let dist = n.dot(center) + p.w;
            let radius = half.x * n.x.abs() + half.y * n.y.abs() + half.z * n.z.abs();
            if dist + radius < 0.0 { return false; }
        }
        true
    }

    #[inline]
    pub fn test_sphere(&self, center: Vec3, radius: f32) -> bool {
        for p in &self.planes {
            if glam::Vec3::new(p.x, p.y, p.z).dot(center) + p.w + radius < 0.0 { return false; }
        }
        true
    }
}
