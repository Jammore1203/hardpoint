//! Thrown equipment, and the volumes it leaves behind.
//!
//! Grenade physics are deliberately simple and predictable: a point mass with
//! gravity, swept against the same brush world everything else uses, bouncing
//! with fixed restitution. Players need to be able to learn a bounce, and a
//! simulation cheap enough to run dozens of at once leaves headroom for the
//! things that matter more.

use super::loadout::Equipment;
use super::types::Team;
use crate::maps::brush::{CollisionWorld, TraceMask};
use crate::math::{bounce_velocity, Aabb};
use glam::Vec3;

/// Radius of the grenade's collision sphere, approximated as a box.
const GRENADE_RADIUS: f32 = 0.09;
/// Speed below which a grenade stops rolling.
const REST_SPEED: f32 = 0.55;
const GRAVITY: f32 = 21.0;

#[derive(Clone, Debug)]
pub struct Grenade {
    pub id: u16,
    pub owner: u8,
    pub team: Team,
    pub kind: Equipment,
    pub pos: Vec3,
    pub vel: Vec3,
    /// Seconds until detonation.
    pub fuse: f32,
    /// Stuck grenades stop simulating.
    pub stuck: bool,
    pub resting: bool,
    /// Set when the fuse reaches zero; the world removes it next tick.
    pub spent: bool,
}

impl Grenade {
    pub fn new(id: u16, owner: u8, team: Team, kind: Equipment, pos: Vec3, vel: Vec3, cooked: f32) -> Grenade {
        Grenade {
            id, owner, team, kind, pos, vel,
            fuse: (kind.fuse() - cooked).max(0.05),
            stuck: false,
            resting: false,
            spent: false,
        }
    }

    #[inline]
    fn body(&self) -> Aabb {
        Aabb::from_center_size(self.pos, Vec3::splat(GRENADE_RADIUS * 2.0))
    }

    /// Advances the grenade. Returns the surface it bounced off this step, if
    /// any, so the caller can play the right impact sound.
    pub fn step(&mut self, world: &CollisionWorld, dt: f32) -> Option<(Vec3, u32)> {
        if self.spent { return None; }
        self.fuse -= dt;
        if self.fuse <= 0.0 { self.spent = true; }
        if self.stuck || self.resting { return None; }

        let (restitution, friction) = self.kind.physics();
        self.vel.y -= GRAVITY * dt;

        let mut remaining = self.vel * dt;
        let mut bounce: Option<(Vec3, u32)> = None;

        for _ in 0..3 {
            if remaining.length_squared() < 1e-10 { break; }
            let body = self.body();
            let hit = world.trace_box(&body, remaining, TraceMask::Projectile);
            if !hit.hit {
                self.pos += remaining;
                break;
            }
            let advance = remaining * hit.fraction;
            self.pos += advance + hit.normal * 0.002;
            remaining -= advance;

            if self.kind == Equipment::Sticky {
                // Sticky charges adhere on first contact and stop entirely.
                self.stuck = true;
                self.vel = Vec3::ZERO;
                return Some((self.pos, hit.brush));
            }

            let speed_before = self.vel.length();
            self.vel = bounce_velocity(self.vel, hit.normal, restitution, friction);
            remaining = bounce_velocity(remaining, hit.normal, restitution, friction);
            if speed_before > 2.0 && bounce.is_none() {
                bounce = Some((self.pos, hit.brush));
            }

            // Come to rest once it is barely moving and sitting on something.
            if self.vel.length() < REST_SPEED && hit.normal.y > 0.6 {
                self.vel = Vec3::ZERO;
                self.resting = true;
                break;
            }
        }
        bounce
    }
}

/// A smoke cloud. Grows to full size, holds, then dissipates.
#[derive(Clone, Debug)]
pub struct Smoke {
    pub id: u16,
    pub pos: Vec3,
    pub age: f32,
    pub life: f32,
    pub radius: f32,
    pub team: Team,
}

impl Smoke {
    pub const GROW_TIME: f32 = 1.4;
    pub const FADE_TIME: f32 = 2.5;

    pub fn new(id: u16, pos: Vec3, team: Team) -> Smoke {
        Smoke { id, pos, age: 0.0, life: 13.0, radius: 4.6, team }
    }

    /// Current radius, accounting for the grow-in and fade-out.
    pub fn current_radius(&self) -> f32 {
        if self.age < Self::GROW_TIME {
            self.radius * (self.age / Self::GROW_TIME).clamp(0.0, 1.0)
        } else if self.age > self.life - Self::FADE_TIME {
            let t = ((self.life - self.age) / Self::FADE_TIME).clamp(0.0, 1.0);
            self.radius * t
        } else {
            self.radius
        }
    }

    /// Opacity in [0, 1], used for rendering and for bot vision.
    pub fn density(&self) -> f32 {
        if self.age < Self::GROW_TIME { (self.age / Self::GROW_TIME).clamp(0.0, 1.0) }
        else if self.age > self.life - Self::FADE_TIME { ((self.life - self.age) / Self::FADE_TIME).clamp(0.0, 1.0) }
        else { 1.0 }
    }

    pub fn expired(&self) -> bool { self.age >= self.life }
}

/// A burning patch left by an incendiary.
#[derive(Clone, Debug)]
pub struct Fire {
    pub id: u16,
    pub owner: u8,
    pub team: Team,
    pub pos: Vec3,
    pub radius: f32,
    pub age: f32,
    pub life: f32,
    /// Accumulator so damage is applied in discrete ticks rather than smeared.
    pub tick_accum: f32,
}

impl Fire {
    pub fn new(id: u16, owner: u8, team: Team, pos: Vec3) -> Fire {
        Fire { id, owner, team, pos, radius: 3.6, age: 0.0, life: 9.5, tick_accum: 0.0 }
    }
    pub fn expired(&self) -> bool { self.age >= self.life }
    pub fn intensity(&self) -> f32 {
        let t = (self.age / self.life).clamp(0.0, 1.0);
        // Flares up, then dies away.
        (1.0 - (t - 0.15).max(0.0) / 0.85).clamp(0.0, 1.0) * (t / 0.15).min(1.0)
    }
    #[inline]
    pub fn contains(&self, p: Vec3) -> bool {
        let dy = p.y - self.pos.y;
        if dy < -1.0 || dy > 2.4 { return false; }
        let dx = p.x - self.pos.x;
        let dz = p.z - self.pos.z;
        dx * dx + dz * dz <= self.radius * self.radius
    }
}

/// How much of the segment from `a` to `b` passes through smoke, in [0, 1].
/// Bots use this to decide whether they can actually see a target.
pub fn smoke_occlusion(smokes: &[Smoke], a: Vec3, b: Vec3) -> f32 {
    if smokes.is_empty() { return 0.0; }
    let seg = b - a;
    let len = seg.length();
    if len < 1e-4 { return 0.0; }
    let dir = seg / len;
    let mut worst = 0.0f32;
    for s in smokes {
        let r = s.current_radius();
        if r <= 0.1 { continue; }
        // Closest approach of the segment to the cloud centre.
        let to = s.pos - a;
        let t = to.dot(dir).clamp(0.0, len);
        let closest = a + dir * t;
        let d = (closest - s.pos).length();
        if d >= r { continue; }
        // How much of the segment is inside the sphere, normalised.
        let half_chord = (r * r - d * d).max(0.0).sqrt();
        let inside = (2.0 * half_chord).min(len) / len.max(0.001);
        worst = worst.max((inside * 2.0).min(1.0) * s.density());
    }
    worst.clamp(0.0, 1.0)
}

/// Initial velocity for a throw. Overhand for lethals, underhand-ish for
/// tacticals so smoke lands closer to where you are looking down.
pub fn throw_velocity(aim: Vec3, player_vel: Vec3, kind: Equipment) -> Vec3 {
    let power = match kind {
        Equipment::Sticky => 21.0,
        Equipment::Frag => 18.0,
        Equipment::Incendiary => 17.0,
        Equipment::Flashbang | Equipment::Concussion => 19.0,
        Equipment::Smoke => 15.0,
    };
    // A little lift so a flat throw still arcs, plus the thrower's own motion.
    let lift = if aim.y < 0.35 { 0.16 } else { 0.0 };
    ((aim + Vec3::Y * lift).normalize_or_zero() * power) + player_vel * 0.5
}

/// Explosive damage falloff. Linear from full at the centre to zero at the
/// edge, with a floor so a direct hit is decisive and a graze is not.
#[inline]
pub fn blast_falloff(distance: f32, radius: f32) -> f32 {
    if radius <= 0.0 { return 0.0; }
    let t = (distance / radius).clamp(0.0, 1.0);
    // Squared falloff keeps the lethal zone tight and the nuisance zone wide.
    (1.0 - t) * (1.0 - t * 0.55)
}

/// Flash intensity for a target, given geometry. Returns seconds of blindness.
pub fn flash_strength(flash_pos: Vec3, eye: Vec3, look: Vec3, radius: f32, occluded: bool) -> f32 {
    if occluded { return 0.0; }
    let to = flash_pos - eye;
    let dist = to.length();
    if dist > radius { return 0.0; }
    let dir = to / dist.max(0.001);
    // Facing it is far worse than catching it in the corner of your eye.
    let facing = dir.dot(look).clamp(-1.0, 1.0);
    let angle_term = ((facing + 0.35) / 1.35).clamp(0.0, 1.0);
    let dist_term = 1.0 - (dist / radius).clamp(0.0, 1.0);
    let s = angle_term.powf(0.7) * dist_term;
    if s < 0.06 { 0.0 } else { 0.6 + s * 4.4 }
}
