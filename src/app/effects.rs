//! Particles, decals and tracers.
//!
//! A flat pool of particles with no per-particle allocation and no behaviour
//! trees: every effect is a burst of particles with a shared integrator. The
//! budget scales with the effects quality setting, and when the pool is full
//! the oldest particles are replaced, so a grenade in a crowded firefight
//! degrades gracefully instead of stuttering.

use crate::assets::texgen::Sprite;
use crate::assets::materials::Surface;
use crate::core::Rng;
use crate::maps::brush::{CollisionWorld, TraceMask};
use crate::maps::Weather;
use crate::render::{Renderer, SpriteInstance};
use glam::Vec3;

const MAX_PARTICLES: usize = 3000;
const MAX_DECALS: usize = 320;
const MAX_TRACERS: usize = 96;

#[derive(Clone, Copy)]
struct Particle {
    pos: Vec3,
    vel: Vec3,
    life: f32,
    max_life: f32,
    size_start: f32,
    size_end: f32,
    color_start: [f32; 4],
    color_end: [f32; 4],
    rot: f32,
    spin: f32,
    gravity: f32,
    drag: f32,
    sprite: Sprite,
    /// Lies flat on the ground rather than facing the camera.
    ground: bool,
    /// Bounces off the world instead of passing through it.
    collides: bool,
}

#[derive(Clone, Copy)]
struct Decal {
    pos: Vec3,
    size: f32,
    rot: f32,
    life: f32,
    max_life: f32,
    sprite: Sprite,
    color: [f32; 4],
}

#[derive(Clone, Copy)]
struct Tracer {
    from: Vec3,
    to: Vec3,
    life: f32,
    max_life: f32,
    width: f32,
    color: [f32; 4],
}

pub struct Effects {
    particles: Vec<Particle>,
    decals: Vec<Decal>,
    tracers: Vec<Tracer>,
    rng: Rng,
    /// Multiplier from the effects quality setting.
    pub density: f32,
    next_particle: usize,
    /// Persistent weather particles, recycled rather than respawned.
    weather: Vec<Particle>,
    weather_kind: Weather,
}

impl Effects {
    pub fn new() -> Effects {
        Effects {
            particles: Vec::with_capacity(MAX_PARTICLES),
            decals: Vec::with_capacity(MAX_DECALS),
            tracers: Vec::with_capacity(MAX_TRACERS),
            rng: Rng::from_clock(),
            density: 1.0,
            next_particle: 0,
            weather: Vec::new(),
            weather_kind: Weather::None,
        }
    }

    pub fn clear(&mut self) {
        self.particles.clear();
        self.decals.clear();
        self.tracers.clear();
    }

    pub fn particle_count(&self) -> usize { self.particles.len() + self.weather.len() }

    fn spawn(&mut self, p: Particle) {
        if self.particles.len() < MAX_PARTICLES {
            self.particles.push(p);
        } else {
            // Replace in a rotating order so a burst does not all vanish.
            self.next_particle = (self.next_particle + 1) % MAX_PARTICLES;
            self.particles[self.next_particle] = p;
        }
    }

    fn count(&self, base: usize) -> usize {
        ((base as f32 * self.density) as usize).clamp(1, 96)
    }

    // ---------------------------------------------------------- emitters

    /// A round striking the world.
    /// A piece of the level coming apart: chunks, dust and a shockwave puff.
    ///
    /// Much heavier than a bullet impact on purpose. A wall disappearing is a
    /// change to the map that everyone nearby needs to notice, and a handful
    /// of sparks would read as another ricochet.
    pub fn debris_burst(&mut self, pos: Vec3, surface: Surface) {
        let (color, _, _) = surface_look(surface);
        for _ in 0..self.count(34) {
            let dir = self.random_unit();
            let speed = self.rng.range(2.0, 11.0);
            let p = Particle {
                pos: pos + dir * self.rng.range(0.1, 0.9),
                vel: dir * speed + Vec3::Y * self.rng.range(1.0, 5.0),
                life: 0.0,
                max_life: self.rng.range(0.5, 1.4),
                size_start: self.rng.range(0.05, 0.22),
                size_end: 0.02,
                color_start: color,
                color_end: [color[0], color[1], color[2], 0.0],
                rot: self.rng.range(0.0, 6.28),
                spin: self.rng.signed() * 9.0,
                gravity: 16.0,
                drag: 0.9,
                sprite: Sprite::Dust,
                ground: false,
                collides: true,
            };
            self.spawn(p);
        }
        for _ in 0..self.count(14) {
            let dir = self.random_unit();
            let p = Particle {
                pos: pos + dir * self.rng.range(0.1, 1.2),
                vel: dir * self.rng.range(0.5, 2.5) + Vec3::Y * 0.8,
                life: 0.0,
                max_life: self.rng.range(0.9, 2.0),
                size_start: self.rng.range(0.5, 1.1),
                size_end: self.rng.range(1.8, 3.0),
                color_start: [color[0] * 1.1, color[1] * 1.1, color[2] * 1.1, 0.55],
                color_end: [color[0], color[1], color[2], 0.0],
                rot: self.rng.range(0.0, 6.28),
                spin: self.rng.signed() * 0.7,
                gravity: -0.4,
                drag: 1.6,
                sprite: Sprite::Smoke,
                ground: false,
                collides: false,
            };
            self.spawn(p);
        }
    }

    pub fn bullet_impact(&mut self, pos: Vec3, normal: Vec3, surface: Surface) {
        let (color, sparks, dust) = surface_look(surface);

        for _ in 0..self.count(6) {
            let dir = (normal + self.random_unit() * 0.7).normalize_or_zero();
            let speed = self.rng.range(2.5, 7.5);
            let particle = Particle {
                pos: pos + normal * 0.03,
                vel: dir * speed,
                life: 0.0,
                max_life: self.rng.range(0.18, 0.42),
                size_start: self.rng.range(0.02, 0.05),
                size_end: 0.005,
                color_start: color,
                color_end: [color[0], color[1], color[2], 0.0],
                rot: self.rng.range(0.0, 6.28),
                spin: self.rng.signed() * 6.0,
                gravity: 9.0,
                drag: 1.4,
                sprite: Sprite::Dust,
                ground: false,
                collides: false,
            };
            self.spawn(particle);
        }

        if sparks > 0.0 {
            for _ in 0..self.count(5) {
                let dir = (normal + self.random_unit() * 0.9).normalize_or_zero();
                let particle = Particle {
                    pos: pos + normal * 0.03,
                    vel: dir * self.rng.range(4.0, 12.0),
                    life: 0.0,
                    max_life: self.rng.range(0.12, 0.30),
                    size_start: 0.035,
                    size_end: 0.006,
                    color_start: [1.0, 0.85, 0.45, 1.0],
                    color_end: [1.0, 0.35, 0.05, 0.0],
                    rot: 0.0,
                    spin: 0.0,
                    gravity: 14.0,
                    drag: 0.6,
                    sprite: Sprite::Spark,
                    ground: false,
                    collides: false,
                };
                self.spawn(particle);
            }
        }

        if dust > 0.0 {
            let particle = Particle {
                pos: pos + normal * 0.08,
                vel: normal * 0.9,
                life: 0.0,
                max_life: 0.55,
                size_start: 0.18,
                size_end: 0.62,
                color_start: [color[0], color[1], color[2], 0.55 * dust],
                color_end: [color[0], color[1], color[2], 0.0],
                rot: self.rng.range(0.0, 6.28),
                spin: self.rng.signed() * 0.8,
                gravity: -0.6,
                drag: 2.2,
                sprite: Sprite::Smoke,
                ground: false,
                collides: false,
            };
            self.spawn(particle);
        }

        // A hole that lingers, which is most of what makes shooting feel real.
        self.add_decal(pos + normal * 0.012, normal, 0.10, Sprite::BulletHole, [0.1, 0.1, 0.1, 0.85], 22.0);
    }

    /// A round striking a player.
    pub fn blood(&mut self, pos: Vec3, dir: Vec3) {
        for _ in 0..self.count(7) {
            let d = (dir + self.random_unit() * 0.8).normalize_or_zero();
            let particle = Particle {
                pos,
                vel: d * self.rng.range(1.5, 5.0),
                life: 0.0,
                max_life: self.rng.range(0.20, 0.45),
                size_start: self.rng.range(0.04, 0.10),
                size_end: 0.01,
                color_start: [0.55, 0.05, 0.04, 0.95],
                color_end: [0.30, 0.02, 0.02, 0.0],
                rot: self.rng.range(0.0, 6.28),
                spin: self.rng.signed() * 4.0,
                gravity: 11.0,
                drag: 1.6,
                sprite: Sprite::Blood,
                ground: false,
                collides: false,
            };
            self.spawn(particle);
        }
    }

    /// The flash at the end of a barrel.
    pub fn muzzle_flash(&mut self, pos: Vec3, dir: Vec3, scale: f32) {
        if scale <= 0.01 { return; }
        let particle = Particle {
            pos: pos + dir * 0.12,
            vel: dir * 1.2,
            life: 0.0,
            max_life: 0.055,
            size_start: 0.42 * scale,
            size_end: 0.16 * scale,
            color_start: [1.0, 0.88, 0.55, 1.0],
            color_end: [1.0, 0.55, 0.15, 0.0],
            rot: self.rng.range(0.0, 6.28),
            spin: 0.0,
            gravity: 0.0,
            drag: 0.0,
            sprite: Sprite::MuzzleFlash,
            ground: false,
            collides: false,
        };
        self.spawn(particle);
        // A puff of propellant smoke that lingers a moment longer.
        for _ in 0..self.count(2) {
            let particle = Particle {
                pos: pos + dir * 0.2,
                vel: dir * self.rng.range(1.0, 3.0) + self.random_unit() * 0.4,
                life: 0.0,
                max_life: self.rng.range(0.28, 0.55),
                size_start: 0.10 * scale,
                size_end: 0.45 * scale,
                color_start: [0.72, 0.70, 0.66, 0.30],
                color_end: [0.60, 0.58, 0.55, 0.0],
                rot: self.rng.range(0.0, 6.28),
                spin: self.rng.signed() * 1.2,
                gravity: -0.8,
                drag: 2.6,
                sprite: Sprite::Smoke,
                ground: false,
                collides: false,
            };
            self.spawn(particle);
        }
    }

    /// An ejected cartridge case.
    pub fn eject_shell(&mut self, pos: Vec3, right: Vec3, up: Vec3) {
        let particle = Particle {
            pos,
            vel: right * self.rng.range(1.6, 3.0) + up * self.rng.range(0.8, 1.8),
            life: 0.0,
            max_life: 2.2,
            size_start: 0.035,
            size_end: 0.035,
            color_start: [0.95, 0.78, 0.35, 1.0],
            color_end: [0.85, 0.68, 0.30, 1.0],
            rot: self.rng.range(0.0, 6.28),
            spin: self.rng.signed() * 22.0,
            gravity: 16.0,
            drag: 0.25,
            sprite: Sprite::Casing,
            ground: false,
            collides: true,
        };
        self.spawn(particle);
    }

    /// A tracer round, drawn as a stretched streak.
    pub fn tracer(&mut self, from: Vec3, to: Vec3, color: [f32; 4]) {
        if self.tracers.len() >= MAX_TRACERS { self.tracers.remove(0); }
        self.tracers.push(Tracer {
            from,
            to,
            life: 0.0,
            max_life: 0.075,
            width: 0.035,
            color,
        });
    }

    pub fn explosion(&mut self, pos: Vec3, radius: f32) {
        let scale = (radius / 6.0).clamp(0.4, 2.0);
        // Core flash.
        let particle = Particle {
            pos,
            vel: Vec3::ZERO,
            life: 0.0,
            max_life: 0.14,
            size_start: radius * 0.7,
            size_end: radius * 1.2,
            color_start: [1.0, 0.92, 0.65, 1.0],
            color_end: [1.0, 0.45, 0.10, 0.0],
            rot: 0.0,
            spin: 0.0,
            gravity: 0.0,
            drag: 0.0,
            sprite: Sprite::Glow,
            ground: false,
            collides: false,
        };
        self.spawn(particle);
        // Expanding ring on the ground, which reads the blast radius clearly.
        let particle = Particle {
            pos: pos + Vec3::Y * 0.06,
            vel: Vec3::ZERO,
            life: 0.0,
            max_life: 0.42,
            size_start: radius * 0.4,
            size_end: radius * 2.1,
            color_start: [1.0, 0.85, 0.55, 0.55],
            color_end: [0.8, 0.4, 0.1, 0.0],
            rot: 0.0,
            spin: 0.0,
            gravity: 0.0,
            drag: 0.0,
            sprite: Sprite::Ring,
            ground: true,
            collides: false,
        };
        self.spawn(particle);

        for _ in 0..self.count(22) {
            let dir = self.random_unit();
            let particle = Particle {
                pos: pos + dir * 0.2,
                vel: dir * self.rng.range(4.0, 16.0) * scale + Vec3::Y * 2.0,
                life: 0.0,
                max_life: self.rng.range(0.5, 1.4),
                size_start: self.rng.range(0.3, 0.9) * scale,
                size_end: self.rng.range(1.2, 2.6) * scale,
                color_start: [0.42, 0.40, 0.38, 0.85],
                color_end: [0.30, 0.29, 0.28, 0.0],
                rot: self.rng.range(0.0, 6.28),
                spin: self.rng.signed() * 1.4,
                gravity: -1.2,
                drag: 1.5,
                sprite: Sprite::Smoke,
                ground: false,
                collides: false,
            };
            self.spawn(particle);
        }
        for _ in 0..self.count(16) {
            let dir = self.random_unit();
            let particle = Particle {
                pos,
                vel: dir * self.rng.range(8.0, 26.0) * scale,
                life: 0.0,
                max_life: self.rng.range(0.25, 0.7),
                size_start: 0.06,
                size_end: 0.01,
                color_start: [1.0, 0.80, 0.35, 1.0],
                color_end: [1.0, 0.25, 0.02, 0.0],
                rot: 0.0,
                spin: 0.0,
                gravity: 12.0,
                drag: 0.5,
                sprite: Sprite::Ember,
                ground: false,
                collides: false,
            };
            self.spawn(particle);
        }
        self.add_decal(pos, Vec3::Y, radius * 0.55, Sprite::Blood, [0.10, 0.09, 0.08, 0.75], 30.0);
    }

    /// A smoke cloud, emitted continuously while the grenade burns.
    pub fn smoke_puff(&mut self, pos: Vec3, radius: f32) {
        let dir = self.random_unit();
        let particle = Particle {
            pos: pos + dir * radius * 0.4,
            vel: dir * 0.6 + Vec3::Y * self.rng.range(0.3, 1.1),
            life: 0.0,
            max_life: self.rng.range(2.5, 4.5),
            size_start: radius * 0.5,
            size_end: radius * 1.5,
            color_start: [0.80, 0.80, 0.79, 0.42],
            color_end: [0.70, 0.70, 0.70, 0.0],
            rot: self.rng.range(0.0, 6.28),
            spin: self.rng.signed() * 0.35,
            gravity: -0.25,
            drag: 1.1,
            sprite: Sprite::Smoke,
            ground: false,
            collides: false,
        };
        self.spawn(particle);
    }

    /// Flames from an incendiary.
    pub fn fire_lick(&mut self, pos: Vec3, radius: f32) {
        let a = self.rng.range(0.0, 6.28);
        let r = self.rng.f32().sqrt() * radius;
        let p = pos + Vec3::new(a.cos() * r, 0.05, a.sin() * r);
        let particle = Particle {
            pos: p,
            vel: Vec3::Y * self.rng.range(1.4, 3.2),
            life: 0.0,
            max_life: self.rng.range(0.35, 0.8),
            size_start: self.rng.range(0.3, 0.7),
            size_end: 0.1,
            color_start: [1.0, 0.70, 0.20, 0.90],
            color_end: [0.9, 0.20, 0.02, 0.0],
            rot: self.rng.range(0.0, 6.28),
            spin: self.rng.signed() * 1.5,
            gravity: -3.0,
            drag: 1.0,
            sprite: Sprite::Glow,
            ground: false,
            collides: false,
        };
        self.spawn(particle);
    }

    /// Dust kicked up by a hard landing.
    pub fn landing_dust(&mut self, pos: Vec3, strength: f32) {
        for _ in 0..self.count((6.0 * strength) as usize + 2) {
            let a = self.rng.range(0.0, 6.28);
            let dir = Vec3::new(a.cos(), 0.25, a.sin());
            let particle = Particle {
                pos: pos + Vec3::Y * 0.05,
                vel: dir * self.rng.range(1.0, 3.0) * strength,
                life: 0.0,
                max_life: self.rng.range(0.3, 0.7),
                size_start: 0.12,
                size_end: 0.5,
                color_start: [0.62, 0.60, 0.56, 0.35],
                color_end: [0.55, 0.53, 0.50, 0.0],
                rot: self.rng.range(0.0, 6.28),
                spin: self.rng.signed() * 1.0,
                gravity: -0.4,
                drag: 2.4,
                sprite: Sprite::Smoke,
                ground: false,
                collides: false,
            };
            self.spawn(particle);
        }
    }

    fn add_decal(&mut self, pos: Vec3, normal: Vec3, size: f32, sprite: Sprite, color: [f32; 4], life: f32) {
        if self.decals.len() >= MAX_DECALS { self.decals.remove(0); }
        // Only ground-facing decals are laid flat; wall hits use a billboard,
        // which at this fidelity is indistinguishable and far simpler.
        let _ = normal;
        self.decals.push(Decal {
            pos,
            size,
            rot: self.rng.range(0.0, 6.28),
            life: 0.0,
            max_life: life,
            sprite,
            color,
        });
    }

    fn random_unit(&mut self) -> Vec3 {
        // Rejection-free: a normalised gaussian triple is uniform on a sphere.
        let v = Vec3::new(self.rng.gaussian(), self.rng.gaussian(), self.rng.gaussian());
        v.normalize_or_zero()
    }

    // ---------------------------------------------------------- weather

    pub fn set_weather(&mut self, kind: Weather, centre: Vec3) {
        if self.weather_kind == kind && !self.weather.is_empty() { return; }
        self.weather_kind = kind;
        self.weather.clear();
        let count = match kind {
            Weather::None => 0,
            // These were sparse enough to be invisible: four hundred flakes
            // spread through a fifty-metre cube is one flake per ninety cubic
            // metres, and at four and a half centimetres across not one of
            // them registered. Weather is cheap -- a thousand sprites against
            // the couple of hundred a firefight already draws -- and a map
            // billed as a blizzard should look like one.
            Weather::Snow => (1100.0 * self.density) as usize,
            Weather::Rain => (900.0 * self.density) as usize,
            Weather::Ash => (520.0 * self.density) as usize,
            Weather::Dust => (380.0 * self.density) as usize,
        };
        for _ in 0..count {
            let p = self.new_weather_particle(centre);
            self.weather.push(p);
        }
    }

    /// Half-width of the box weather is kept in around the listener.
    ///
    /// Tighter than it was. Weather reads by being close enough to have size
    /// on screen; spread over fifty metres the same number of flakes are all
    /// too far away to see, and the ones that are near enough are one pixel.
    const WEATHER_SPREAD: f32 = 17.0;

    fn new_weather_particle(&mut self, centre: Vec3) -> Particle {
        let spread = Self::WEATHER_SPREAD;
        let pos = centre + Vec3::new(
            self.rng.range(-spread, spread),
            self.rng.range(2.0, 16.0),
            self.rng.range(-spread, spread),
        );
        match self.weather_kind {
            Weather::Snow => Particle {
                pos,
                // Driven, not drifting: a blizzard blows sideways.
                vel: Vec3::new(self.rng.range(1.4, 3.2), -self.rng.range(1.2, 2.6), self.rng.range(-0.9, 0.9)),
                life: 0.0, max_life: 30.0,
                size_start: 0.090, size_end: 0.090,
                color_start: [1.0, 1.0, 1.0, 0.92], color_end: [1.0, 1.0, 1.0, 0.92],
                rot: 0.0, spin: 0.5, gravity: 0.0, drag: 0.0,
                sprite: Sprite::Snowflake, ground: false, collides: false,
            },
            Weather::Rain => Particle {
                pos,
                vel: Vec3::new(1.4, -14.0, 0.3),
                life: 0.0, max_life: 30.0,
                size_start: 0.16, size_end: 0.16,
                color_start: [0.72, 0.80, 0.88, 0.35], color_end: [0.72, 0.80, 0.88, 0.35],
                rot: 0.0, spin: 0.0, gravity: 0.0, drag: 0.0,
                sprite: Sprite::Raindrop, ground: false, collides: false,
            },
            Weather::Ash => Particle {
                pos,
                vel: Vec3::new(self.rng.range(-0.5, 0.5), -self.rng.range(0.3, 0.8), self.rng.range(-0.5, 0.5)),
                life: 0.0, max_life: 30.0,
                size_start: 0.075, size_end: 0.075,
                color_start: [0.55, 0.52, 0.48, 0.55], color_end: [0.55, 0.52, 0.48, 0.55],
                rot: 0.0, spin: 0.8, gravity: 0.0, drag: 0.0,
                sprite: Sprite::Dust, ground: false, collides: false,
            },
            _ => Particle {
                pos,
                vel: Vec3::new(self.rng.range(1.0, 3.0), -self.rng.range(0.1, 0.4), self.rng.range(-1.0, 1.0)),
                life: 0.0, max_life: 30.0,
                size_start: 0.09, size_end: 0.09,
                color_start: [0.72, 0.66, 0.52, 0.22], color_end: [0.72, 0.66, 0.52, 0.22],
                rot: 0.0, spin: 0.3, gravity: 0.0, drag: 0.0,
                sprite: Sprite::Dust, ground: false, collides: false,
            },
        }
    }

    // ------------------------------------------------------------ update

    pub fn update(&mut self, dt: f32, world: Option<&CollisionWorld>, listener: Vec3) {
        let mut i = 0;
        while i < self.particles.len() {
            let p = &mut self.particles[i];
            p.life += dt;
            if p.life >= p.max_life {
                self.particles.swap_remove(i);
                continue;
            }
            p.vel.y -= p.gravity * dt;
            if p.drag > 0.0 {
                let k = (-p.drag * dt).exp();
                p.vel *= k;
            }
            let step = p.vel * dt;
            if p.collides {
                if let Some(w) = world {
                    let hit = w.trace_ray(p.pos, step.normalize_or_zero(), step.length(), TraceMask::Solid);
                    if hit.hit {
                        p.pos = hit.point + hit.normal * 0.01;
                        p.vel = crate::math::bounce_velocity(p.vel, hit.normal, 0.35, 0.45);
                        p.spin *= 0.6;
                        i += 1;
                        continue;
                    }
                }
            }
            p.pos += step;
            p.rot += p.spin * dt;
            i += 1;
        }

        let mut i = 0;
        while i < self.decals.len() {
            self.decals[i].life += dt;
            if self.decals[i].life >= self.decals[i].max_life {
                self.decals.swap_remove(i);
            } else {
                i += 1;
            }
        }

        let mut i = 0;
        while i < self.tracers.len() {
            self.tracers[i].life += dt;
            if self.tracers[i].life >= self.tracers[i].max_life {
                self.tracers.swap_remove(i);
            } else {
                i += 1;
            }
        }

        // Weather follows the listener and wraps around them, so a fixed
        // number of particles covers an unbounded map.
        if self.weather_kind != Weather::None {
            let spread = Self::WEATHER_SPREAD;
            for p in self.weather.iter_mut() {
                p.pos += p.vel * dt;
                p.rot += p.spin * dt;
                let d = p.pos - listener;
                if d.y < -3.0 || d.y > 20.0 {
                    p.pos.y = listener.y + if d.y < 0.0 { 16.0 } else { 0.0 };
                }
                if d.x.abs() > spread { p.pos.x -= d.x.signum() * spread * 2.0; }
                if d.z.abs() > spread { p.pos.z -= d.z.signum() * spread * 2.0; }
            }
        }
    }

    /// Pushes everything into the renderer's sprite list.
    pub fn submit(&self, r: &mut Renderer) {
        for d in &self.decals {
            let t = (d.life / d.max_life).clamp(0.0, 1.0);
            // Fade only in the last quarter of the life, so decals persist.
            let fade = if t > 0.75 { 1.0 - (t - 0.75) / 0.25 } else { 1.0 };
            let mut c = d.color;
            c[3] *= fade;
            r.push_sprite(SpriteInstance::ground(d.pos, d.size, d.rot, d.sprite, c));
        }

        for p in self.particles.iter().chain(self.weather.iter()) {
            let t = (p.life / p.max_life).clamp(0.0, 1.0);
            let size = p.size_start + (p.size_end - p.size_start) * t;
            let color = [
                p.color_start[0] + (p.color_end[0] - p.color_start[0]) * t,
                p.color_start[1] + (p.color_end[1] - p.color_start[1]) * t,
                p.color_start[2] + (p.color_end[2] - p.color_start[2]) * t,
                p.color_start[3] + (p.color_end[3] - p.color_start[3]) * t,
            ];
            if color[3] <= 0.004 { continue; }
            let s = if p.ground {
                SpriteInstance::ground(p.pos, size, p.rot, p.sprite, color)
            } else {
                SpriteInstance::billboard(p.pos, size, p.rot, p.sprite, color)
            };
            r.push_sprite(s);
        }

        for t in &self.tracers {
            let f = 1.0 - (t.life / t.max_life).clamp(0.0, 1.0);
            let mid = (t.from + t.to) * 0.5;
            let len = (t.to - t.from).length();
            let mut c = t.color;
            c[3] *= f;
            // A single stretched quad: the classic tracer, and one instance.
            let dir = (t.to - t.from).normalize_or_zero();
            let rot = dir.y.asin() * 0.0;
            r.push_sprite(SpriteInstance::stretched(mid, len * 0.5, t.width, rot, Sprite::Tracer, c));
        }
    }
}

/// Debris colour and character per surface.
fn surface_look(s: Surface) -> ([f32; 4], f32, f32) {
    use Surface::*;
    match s {
        Concrete => ([0.72, 0.71, 0.68, 0.9], 0.0, 1.0),
        Metal => ([0.70, 0.72, 0.75, 0.9], 1.0, 0.3),
        Wood => ([0.62, 0.45, 0.26, 0.9], 0.0, 0.7),
        Dirt => ([0.48, 0.38, 0.26, 0.9], 0.0, 1.0),
        Sand => ([0.80, 0.72, 0.50, 0.9], 0.0, 1.0),
        Gravel => ([0.60, 0.58, 0.54, 0.9], 0.2, 1.0),
        Grass => ([0.36, 0.48, 0.24, 0.9], 0.0, 0.6),
        Snow => ([0.92, 0.95, 0.98, 0.9], 0.0, 1.0),
        Glass => ([0.80, 0.88, 0.92, 0.9], 0.6, 0.2),
        Soft => ([0.55, 0.50, 0.42, 0.9], 0.0, 0.5),
        Water => ([0.55, 0.70, 0.80, 0.9], 0.0, 0.8),
    }
}
