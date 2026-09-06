//! Turning client state into things to draw.
//!
//! The client never simulates the world; it receives it. This module takes
//! that received state, adds the purely cosmetic entities the server does not
//! bother replicating in detail (a grenade in flight, a smoke cloud, a shell
//! casing), and produces the renderer's instance lists.

use crate::assets::materials::Mat;
use crate::assets::meshgen::{self, Part, PartInstance, PartLook, PoseInput, PART_SIZE};
use crate::assets::texgen::Sprite;
use crate::game::loadout::Equipment;
use crate::game::types::{PFlags, PickupKind, Team, MAX_PLAYERS};
use crate::game::weapons::WeaponId;
use crate::maps::MapData;
use crate::net::client::Client;
use crate::render::{Camera, Renderer, SpriteInstance};
use glam::{Mat4, Vec3};

/// A grenade the client is drawing. Its motion is simulated locally from the
/// throw event, which is exact enough for something that lands in two seconds
/// and saves replicating a position every tick.
#[derive(Clone, Copy)]
pub struct VisGrenade {
    pub id: u16,
    pub kind: Equipment,
    pub pos: Vec3,
    pub vel: Vec3,
    pub age: f32,
    pub stuck: bool,
}

#[derive(Clone, Copy)]
pub struct VisSmoke {
    pub id: u16,
    pub pos: Vec3,
    pub age: f32,
    pub emit: f32,
}

#[derive(Clone, Copy)]
pub struct VisFire {
    pub id: u16,
    pub pos: Vec3,
    pub radius: f32,
    pub age: f32,
    pub emit: f32,
}

#[derive(Clone, Copy)]
pub struct VisPickup {
    pub pos: Vec3,
    pub kind: PickupKind,
    pub available: bool,
    pub respawn_in: f32,
}

/// Cosmetic world entities the client owns.
pub struct ClientWorld {
    pub grenades: Vec<VisGrenade>,
    pub smokes: Vec<VisSmoke>,
    pub fires: Vec<VisFire>,
    pub pickups: Vec<VisPickup>,
}

impl ClientWorld {
    pub fn new() -> ClientWorld {
        ClientWorld { grenades: Vec::new(), smokes: Vec::new(), fires: Vec::new(), pickups: Vec::new() }
    }

    pub fn reset(&mut self, map: &MapData) {
        self.grenades.clear();
        self.smokes.clear();
        self.fires.clear();
        self.pickups = map.pickups.iter().map(|p| VisPickup {
            pos: p.pos,
            kind: p.kind,
            available: true,
            respawn_in: 0.0,
        }).collect();
    }

    pub fn throw(&mut self, id: u16, kind: Equipment, pos: Vec3, vel: Vec3) {
        self.grenades.push(VisGrenade { id, kind, pos, vel, age: 0.0, stuck: false });
        if self.grenades.len() > 32 { self.grenades.remove(0); }
    }

    pub fn remove_grenade(&mut self, id: u16) {
        self.grenades.retain(|g| g.id != id);
    }

    pub fn add_smoke(&mut self, id: u16, pos: Vec3) {
        self.smokes.push(VisSmoke { id, pos, age: 0.0, emit: 0.0 });
    }

    pub fn add_fire(&mut self, id: u16, pos: Vec3, radius: f32) {
        self.fires.push(VisFire { id, pos, radius, age: 0.0, emit: 0.0 });
    }

    pub fn take_pickup(&mut self, index: u16) {
        if let Some(p) = self.pickups.get_mut(index as usize) {
            p.available = false;
            p.respawn_in = 20.0;
        }
    }

    pub fn respawn_pickup(&mut self, index: u16) {
        if let Some(p) = self.pickups.get_mut(index as usize) {
            p.available = true;
            p.respawn_in = 0.0;
        }
    }

    pub fn update(&mut self, dt: f32, map: &MapData, effects: &mut super::effects::Effects) {
        use crate::maps::brush::TraceMask;
        for g in self.grenades.iter_mut() {
            g.age += dt;
            if g.stuck { continue; }
            g.vel.y -= 21.0 * dt;
            let step = g.vel * dt;
            let len = step.length();
            if len > 0.0001 {
                let hit = map.collision.trace_ray(g.pos, step / len, len, TraceMask::Projectile);
                if hit.hit {
                    g.pos = hit.point + hit.normal * 0.04;
                    if g.kind == Equipment::Sticky {
                        g.stuck = true;
                        g.vel = Vec3::ZERO;
                    } else {
                        let (rest, fric) = g.kind.physics();
                        g.vel = crate::math::bounce_velocity(g.vel, hit.normal, rest, fric);
                        if g.vel.length() < 0.6 && hit.normal.y > 0.6 {
                            g.vel = Vec3::ZERO;
                            g.stuck = true;
                        }
                    }
                } else {
                    g.pos += step;
                }
            }
        }
        // A grenade the server never detonated (we missed the event) is
        // dropped after its fuse so nothing lingers forever.
        self.grenades.retain(|g| g.age < g.kind.fuse() + 1.5);

        for s in self.smokes.iter_mut() {
            s.age += dt;
            s.emit += dt;
            let rate = if s.age < 1.5 { 0.02 } else { 0.06 };
            while s.emit > rate {
                s.emit -= rate;
                let r = (s.age / 1.4).clamp(0.15, 1.0) * 4.6;
                effects.smoke_puff(s.pos + Vec3::Y * 0.4, r);
            }
        }
        self.smokes.retain(|s| s.age < 13.0);

        for f in self.fires.iter_mut() {
            f.age += dt;
            f.emit += dt;
            while f.emit > 0.035 {
                f.emit -= 0.035;
                effects.fire_lick(f.pos, f.radius * 0.8);
            }
        }
        self.fires.retain(|f| f.age < 9.5);

        for p in self.pickups.iter_mut() {
            if !p.available {
                p.respawn_in -= dt;
                if p.respawn_in <= 0.0 { p.available = true; }
            }
        }
    }
}

/// Draws every remote player.
pub fn draw_players(r: &mut Renderer, client: &Client, map: &MapData, time: f32, shadows: bool) {
    for slot in 0..MAX_PLAYERS {
        if slot as u8 == client.slot { continue; }
        let p = &client.players[slot];
        if !p.present { continue; }
        let dead = p.snap.flags.contains(PFlags::DEAD);
        // A body stays for a few seconds after death, then is removed.
        if dead && p.death_time > 6.0 { continue; }

        let team = p.team;
        let team_index = match team { Team::Phantom => 1, Team::Vanguard => 2, _ => 0 };

        let def = WeaponId::from_u8(p.snap.weapon).def();
        let model = meshgen::weapon_model(def.shape);
        // Full length here: this is the weapon as it exists in the world.
        let model_scale = meshgen::weapon_model_scale(def);
        let scaled = |v: Vec3| v * model_scale;

        let pose = meshgen::pose_character(
            &PoseInput {
                yaw: p.render_yaw,
                pitch: p.render_pitch,
                speed: p.speed,
                phase: p.phase,
                height: p.render_height,
                grounded: p.snap.flags.contains(PFlags::GROUNDED),
                dead,
                death_time: p.death_time,
                firing: p.firing,
                reloading: p.snap.flags.contains(PFlags::RELOADING),
                grip: scaled(model.grip),
                fore: if def.shape.two_handed() { scaled(model.fore) } else { scaled(model.grip) + Vec3::new(-0.16, -0.02, 0.04) },
            },
            p.render_pos,
        );

        let tint = team_tint(team);
        for part in meshgen::ALL_PARTS {
            let look = meshgen::part_look(part);
            let mat = meshgen::look_material(look, team_index);
            // Fatigues take the team colour; kit stays neutral so the
            // silhouette still reads as a soldier rather than a colour swatch.
            let color = match look {
                PartLook::Skin => [0.72, 0.60, 0.50, 1.0],
                PartLook::Hard => [0.66, 0.67, 0.68, 1.0],
                PartLook::Webbing => [0.56, 0.54, 0.47, 1.0],
                PartLook::Boots => [0.46, 0.44, 0.42, 1.0],
                PartLook::Fatigues => tint,
            };
            r.push_part(PartInstance::from_matrix(
                pose.parts[part as usize], color, mat.layer(), [1.0, 0.0, 0.0]));
        }

        // The weapon they are actually carrying, built from the same box list
        // the viewmodel uses. A single grey brick in the hand was the loudest
        // thing wrong with these models at any range you could see them.
        for wp in model.parts {
            let m = meshgen::weapon_part_matrix(pose.weapon, model_scale, wp);
            r.push_part(PartInstance::from_matrix(
                m, [0.90, 0.90, 0.90, 1.0], wp.mat.layer(), [1.0, 0.0, 0.0]));
        }

        if shadows && !dead {
            if let Some((h, _)) = map.collision.ground_below(p.render_pos + Vec3::Y * 0.1, 0.4, 4.0) {
                let drop = (p.render_pos.y - h).clamp(0.0, 4.0);
                let fade = 1.0 - drop / 4.0;
                let size = 1.05 + drop * 0.18;
                r.push_sprite(SpriteInstance::ground(
                    Vec3::new(p.render_pos.x, h + 0.03, p.render_pos.z),
                    size, 0.0, Sprite::BlobShadow,
                    [0.0, 0.0, 0.0, 0.45 * fade],
                ));
            }
        }
    }
    let _ = time;
}

fn team_tint(team: Team) -> [f32; 4] {
    match team {
        Team::Phantom => [1.10, 0.86, 0.74, 1.0],
        Team::Vanguard => [0.78, 0.88, 1.10, 1.0],
        _ => [0.95, 0.95, 0.92, 1.0],
    }
}

/// Draws grenades, smoke sources, fires and pickups.
pub fn draw_entities(r: &mut Renderer, world: &ClientWorld, time: f32) {
    for g in &world.grenades {
        let spin = if g.stuck { 0.0 } else { g.age * 9.0 };
        let m = Mat4::from_translation(g.pos)
            * Mat4::from_rotation_y(spin)
            * Mat4::from_rotation_x(spin * 0.6)
            * Mat4::from_scale(Vec3::splat(0.11));
        let color = match g.kind {
            Equipment::Frag => [0.36, 0.42, 0.30, 1.0],
            Equipment::Sticky => [0.75, 0.55, 0.20, 1.0],
            Equipment::Incendiary => [0.72, 0.30, 0.18, 1.0],
            Equipment::Flashbang => [0.80, 0.80, 0.84, 1.0],
            Equipment::Smoke => [0.55, 0.60, 0.55, 1.0],
            Equipment::Concussion => [0.50, 0.55, 0.75, 1.0],
        };
        r.push_part(PartInstance::from_matrix(m, color, Mat::MetalPanel.layer(), [1.0, 0.0, 0.0]));
        // A blinking indicator on a live grenade, so it can be reacted to.
        let blink = ((g.age * 9.0).sin() * 0.5 + 0.5) * 0.9;
        r.push_sprite(SpriteInstance::billboard(
            g.pos + Vec3::Y * 0.12, 0.16, 0.0, Sprite::Glow,
            [1.0, 0.35, 0.2, blink * 0.8],
        ));
    }

    for p in &world.pickups {
        if !p.available { continue; }
        let bob = (time * 2.0 + p.pos.x).sin() * 0.06;
        let spin = time * 1.4 + p.pos.z;
        let (color, mat) = match p.kind {
            PickupKind::Ammo => ([0.85, 0.75, 0.35, 1.0], Mat::WoodCrate),
            PickupKind::Armor => ([0.45, 0.65, 0.95, 1.0], Mat::MetalPanel),
            PickupKind::Health => ([0.35, 0.85, 0.40, 1.0], Mat::Canvas),
            PickupKind::Grenade => ([0.75, 0.45, 0.25, 1.0], Mat::Sandbag),
            PickupKind::Weapon => ([0.90, 0.85, 0.75, 1.0], Mat::MetalRust),
        };
        let m = Mat4::from_translation(p.pos + Vec3::Y * bob)
            * Mat4::from_rotation_y(spin)
            * Mat4::from_scale(Vec3::new(0.34, 0.26, 0.34));
        r.push_part(PartInstance::from_matrix(m, color, mat.layer(), [1.0, 0.0, 0.0]));
        r.push_sprite(SpriteInstance::billboard(
            p.pos + Vec3::Y * 0.5, 0.5, 0.0, Sprite::Glow,
            [color[0], color[1], color[2], 0.22],
        ));
    }
}

/// State the viewmodel animator carries between frames.
pub struct ViewModel {
    pub sway: (f32, f32),
    pub bob_phase: f32,
    pub recoil: f32,
    pub recoil_rot: f32,
    pub ads: f32,
    pub sprint: f32,
    pub reload_t: f32,
    pub swap_t: f32,
    pub last_weapon: WeaponId,
    /// Where the muzzle ended up last frame, in world space.
    pub muzzle_world: Vec3,
    pub muzzle_dir: Vec3,
}

impl Default for ViewModel {
    fn default() -> Self {
        ViewModel {
            sway: (0.0, 0.0),
            bob_phase: 0.0,
            recoil: 0.0,
            recoil_rot: 0.0,
            ads: 0.0,
            sprint: 0.0,
            reload_t: 0.0,
            swap_t: 0.0,
            last_weapon: WeaponId::SidearmP9,
            muzzle_world: Vec3::ZERO,
            muzzle_dir: Vec3::NEG_Z,
        }
    }
}

impl ViewModel {
    pub fn kick(&mut self, amount: f32) {
        self.recoil = (self.recoil + amount).min(1.4);
        self.recoil_rot = (self.recoil_rot + amount * 0.7).min(1.2);
    }

    pub fn start_reload(&mut self, duration: f32) {
        self.reload_t = duration.max(0.2);
    }

    pub fn start_swap(&mut self, duration: f32) {
        self.swap_t = duration.max(0.15);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update(&mut self, dt: f32, mouse_dx: f32, mouse_dy: f32, speed: f32, ads: bool, sprinting: bool, bob_scale: f32) {
        // Sway lags the mouse, which is what makes a weapon feel like it has
        // mass rather than being welded to the camera.
        let target_x = (-mouse_dx * 0.0016).clamp(-0.05, 0.05);
        let target_y = (-mouse_dy * 0.0016).clamp(-0.05, 0.05);
        let k = 1.0 - (-9.0 * dt).exp();
        self.sway.0 += (target_x - self.sway.0) * k;
        self.sway.1 += (target_y - self.sway.1) * k;

        self.bob_phase += speed * dt * 1.5 * bob_scale;
        let ads_rate = dt * 7.0;
        self.ads += ((if ads { 1.0 } else { 0.0 }) - self.ads).clamp(-ads_rate, ads_rate);
        let sprint_rate = dt * 6.0;
        self.sprint += ((if sprinting { 1.0 } else { 0.0 }) - self.sprint).clamp(-sprint_rate, sprint_rate);

        self.recoil *= (-13.0 * dt).exp();
        self.recoil_rot *= (-10.0 * dt).exp();
        self.reload_t = (self.reload_t - dt).max(0.0);
        self.swap_t = (self.swap_t - dt).max(0.0);
    }

    /// Builds the viewmodel transform in view space and pushes its parts.
    pub fn submit(&mut self, r: &mut Renderer, camera: &Camera, weapon: WeaponId, speed: f32, bob_scale: f32) {
        let def = weapon.def();
        // Scoped weapons hide the model entirely once aimed, replaced by the
        // scope overlay, exactly as the era did it.
        if def.scoped && self.ads > 0.92 { return; }

        // Held out and to the right so the weapon reads in profile rather
        // than as a box pointing away from the camera.
        // The models are built at real proportions - a rifle is most of a
        // metre - so the carry positions sit further out than they did when a
        // rifle was seven boxes and sixty centimetres long.
        let hip = Vec3::new(0.150, -0.205, -0.80);
        // Aiming is derived, not authored: put the weapon where its own rear
        // sight lands on the centre of the screen. Every weapon then has a
        // correct sight picture for free, and moving a sight on a model moves
        // the sight picture with it.
        let sight_dist = 0.62f32;
        // Viewmodels are foreshortened along the weapon's own axis: the world
        // models are at real proportions and the shooter's eye is at the butt
        // of one, so drawn full length the stock fills the middle of the
        // screen. The third-person model keeps its real length.
        let vm_scale = meshgen::weapon_model_scale(def) * Vec3::new(1.0, 1.0, 0.74);
        let sight = meshgen::weapon_model(def.shape).sight * vm_scale;
        let aim = Vec3::new(-sight.x, -sight.y, -sight_dist - sight.z);
        let mut pos = hip.lerp(aim, self.ads);

        // Walk bob, damped hard while aiming.
        let bob = (1.0 - self.ads * 0.85) * bob_scale * (speed / 8.0).clamp(0.0, 1.0);
        pos.x += (self.bob_phase).sin() * 0.014 * bob;
        pos.y += (self.bob_phase * 2.0).cos() * 0.010 * bob;

        // Sway.
        pos.x += self.sway.0 * (1.0 - self.ads * 0.7);
        pos.y += self.sway.1 * (1.0 - self.ads * 0.7);

        // Recoil pushes the weapon back and up.
        pos.z += self.recoil * 0.055;
        pos.y += self.recoil * 0.012;

        // A little yaw and roll at the hip, straightened out as the sights
        // come up: the whole reason a viewmodel looks like a weapon.
        let hip_turn = 1.0 - self.ads;
        let mut rot_x = -self.sway.1 * 1.6 + self.recoil_rot * 0.22 + hip_turn * 0.03;
        let rot_y = -self.sway.0 * 1.6 + hip_turn * 0.055;
        let mut rot_z = self.sway.0 * 0.8 - hip_turn * 0.025;

        // Sprinting tilts the weapon aside and drops it.
        if self.sprint > 0.001 {
            pos.x += self.sprint * 0.06;
            pos.y -= self.sprint * 0.055;
            pos.z += self.sprint * 0.04;
            rot_z += self.sprint * 0.55;
            rot_x += self.sprint * 0.25;
        }

        // Reload dips the weapon and rolls it toward the player.
        if self.reload_t > 0.0 {
            let t = (self.reload_t * 3.0).min(1.0);
            let arc = (t * std::f32::consts::PI).sin();
            pos.y -= arc * 0.09;
            pos.x -= arc * 0.02;
            rot_x += arc * 0.55;
            rot_z += arc * 0.35;
        }

        // Swapping swings the weapon up from below.
        if self.swap_t > 0.0 {
            let t = (self.swap_t * 3.5).min(1.0);
            pos.y -= t * 0.22;
            rot_x += t * 0.9;
        }

        // The weapon is scaled per weapon; the hands are not, so they get a
        // frame of their own that shares only the placement and rotation.
        let hands = Mat4::from_translation(pos)
            * Mat4::from_euler(glam::EulerRot::YXZ, rot_y, rot_x, rot_z);
        let model = meshgen::weapon_model(def.shape);
        // Viewmodels are foreshortened. The world models are at real
        // proportions - a rifle is most of a metre - and the shooter's eye is
        // at the butt of it, so drawn full length the stock fills the middle
        // of the screen and the sights are the only part you can see past.
        // Every game of this kind compresses the weapon along its own axis for
        // the first-person view; the third-person model keeps its real length.
        let model_scale = vm_scale;

        for part in model.parts {
            let m = meshgen::weapon_part_matrix(hands, model_scale, part);
            r.push_viewmodel(PartInstance::from_matrix(m, [1.0, 1.0, 1.0, 1.0], part.mat.layer(), [1.0, 0.0, 0.0]));
        }

        // Gloved hands on the points the model declares. Inferring them from
        // the box list - firing hand on the lowest, support hand on the most
        // forward - is a fair guess for a rifle and quite wrong for a
        // revolver, whose most forward box is the barrel.
        fn glove(r: &mut Renderer, hands: Mat4, at: Vec3, size: Vec3) {
            let m = hands * Mat4::from_translation(at) * Mat4::from_scale(size);
            r.push_viewmodel(PartInstance::from_matrix(
                m, [0.88, 0.86, 0.82, 1.0], Mat::Tarp.layer(), [1.0, 0.0, 0.0]));
        }
        // A forearm is a box aimed along `dir`, built from an explicit basis so
        // the angles cannot be got wrong.
        fn forearm(r: &mut Renderer, hands: Mat4, from: Vec3, dir: Vec3, len: f32, thick: f32) {
            let f = dir.normalize();
            let right = Vec3::Y.cross(f).normalize();
            let up = f.cross(right);
            let m = hands * Mat4::from_cols(
                (right * thick).extend(0.0),
                (up * thick * 0.86).extend(0.0),
                (f * len).extend(0.0),
                (from + f * (len * 0.5)).extend(1.0),
            );
            r.push_viewmodel(PartInstance::from_matrix(
                m, [0.72, 0.72, 0.70, 1.0], Mat::Camo.layer(), [1.0, 0.0, 0.0]));
        }

        // Hands and sleeves scale with the weapon only in the axes the weapon
        // grew in; a hand is a hand whatever it is holding.
        let hand_at = |p: Vec3| Vec3::new(p.x * model_scale.x, p.y * model_scale.y, p.z * model_scale.z);
        let gp = hand_at(model.grip);
        glove(r, hands, Vec3::new(0.004, gp.y + 0.020, gp.z + 0.004), Vec3::new(0.062, 0.086, 0.080));
        forearm(r, hands, Vec3::new(0.014, gp.y - 0.012, gp.z + 0.046),
                Vec3::new(0.30, -0.50, 0.81), 0.24, 0.052);
        if def.shape.two_handed() {
            let fp = hand_at(model.fore);
            glove(r, hands, Vec3::new(-0.004, fp.y - 0.010, fp.z + 0.010), Vec3::new(0.060, 0.070, 0.092));
            forearm(r, hands, Vec3::new(-0.026, fp.y - 0.044, fp.z + 0.056),
                    Vec3::new(-0.50, -0.52, 0.69), 0.25, 0.050);
        }

        // Track the muzzle in world space so effects can be spawned there.
        let muzzle_view = (hands * Mat4::from_scale(model_scale))
            * model.muzzle.extend(1.0);
        let view = camera.view();
        let inv_view = view.inverse();
        self.muzzle_world = (inv_view * muzzle_view).truncate();
        self.muzzle_dir = crate::math::dir_from_angles(camera.yaw, camera.pitch);
    }

    /// A rough world-space position for shell ejection.
    pub fn ejection_point(&self, camera: &Camera) -> (Vec3, Vec3, Vec3) {
        let dir = crate::math::dir_from_angles(camera.yaw, camera.pitch);
        let right = dir.cross(Vec3::Y).normalize_or_zero();
        let up = right.cross(dir);
        (camera.position + dir * 0.35 + right * 0.16 - up * 0.05, right, up)
    }
}

/// Where a remote player's muzzle is, for their tracers and flashes.
pub fn remote_muzzle(client: &Client, slot: u8) -> Option<(Vec3, Vec3)> {
    let p = client.players.get(slot as usize)?;
    if !p.present { return None; }
    let eye = p.render_pos + Vec3::Y * (p.render_height * 0.91);
    let dir = crate::math::dir_from_angles(p.render_yaw, p.render_pitch);
    let right = dir.cross(Vec3::Y).normalize_or_zero();
    Some((eye + dir * 0.45 + right * 0.12, dir))
}

/// The size of a character's head box, exported for aim assistance debugging.
pub fn head_size() -> Vec3 { Vec3::from(PART_SIZE[Part::Head as usize]) }
