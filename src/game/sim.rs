//! The authoritative world simulation.
//!
//! One `World` holds a map and up to sixteen players and advances them at a
//! fixed tick rate. The dedicated server owns one; a listen server owns one on
//! a background thread; the client owns none, and instead predicts only its
//! own player using the same movement and weapon code.
//!
//! Nothing in this module knows what a game mode is. Modes read the world and
//! push it around from outside, which keeps five very different rule sets from
//! tangling themselves into the combat code.

use super::events::{EventQueue, GameEvent, ImpactKind};
use super::loadout::Equipment;
use super::movement;
use super::player::{Player, SLOT_MELEE};
use super::projectiles::{blast_falloff, flash_strength, throw_velocity, Fire, Grenade, Smoke};
use super::types::*;
use super::weapons::{spread_direction, WeaponId};
use crate::assets::materials::Surface;
use crate::core::Rng;
use crate::maps::brush::TraceMask;
use crate::maps::{MapData, MapId};
use crate::math::Aabb;
use glam::Vec3;

/// Server tick rate. Sixty is enough for the movement to feel exact while
/// leaving a comfortable CPU budget for sixteen players and their bots.
pub const TICK_HZ: f32 = 60.0;
pub const TICK_DT: f32 = 1.0 / TICK_HZ;
/// Longest a shot may be rewound for lag compensation.
pub const MAX_REWIND: f64 = 0.30;
/// Maximum hitscan range.
pub const MAX_SHOT_RANGE: f32 = 250.0;

/// How the world should handle a dead player.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum RespawnPolicy {
    /// Respawn automatically after a delay.
    Auto { delay: f32 },
    /// Player must press a key; used for the round-based modes' warmup.
    OnRequest { min_delay: f32 },
    /// No respawns at all until the mode says otherwise.
    None,
}

/// A world pickup in play.
#[derive(Clone, Debug)]
pub struct PickupInstance {
    pub spot: PickupSpot,
    /// Seconds until it returns; zero means available.
    pub cooldown: f32,
    /// Weapon offered by a weapon crate, chosen when the map loads.
    pub weapon: WeaponId,
}

impl PickupInstance {
    #[inline]
    pub fn available(&self) -> bool { self.cooldown <= 0.0 }
}

pub struct World {
    pub map: MapData,
    pub map_id: MapId,
    pub players: Vec<Player>,
    pub grenades: Vec<Grenade>,
    pub smokes: Vec<Smoke>,
    pub fires: Vec<Fire>,
    pub pickups: Vec<PickupInstance>,
    pub events: EventQueue,
    /// Remaining health of every brush, for the breakable ones. Round state,
    /// not map state: the map is shared and reused between rounds.
    pub brush_health: Vec<f32>,

    pub time: f64,
    pub tick: u32,
    pub rng: Rng,

    // ------------------------------------------------------------ rules
    pub friendly_fire: bool,
    pub lag_compensation: bool,
    pub respawn: RespawnPolicy,
    /// Set by team modes; free-for-all leaves it false.
    pub team_game: bool,

    next_entity: u16,
    /// Scratch buffer reused by the spawn chooser so selection never allocates.
    spawn_scores: Vec<(usize, f32)>,
}

impl World {
    pub fn new(map_id: MapId, seed: u32) -> World {
        let map = map_id.build();
        let mut rng = Rng::seeded(seed);
        let pickups = map.pickups.iter().map(|spot| {
            let weapon = if spot.kind == PickupKind::Weapon {
                // Floor weapons are drawn from a fixed, punchy shortlist so a
                // pickup is always a meaningful upgrade over a starting gun.
                const FLOOR: [WeaponId; 6] = [
                    WeaponId::Longbow, WeaponId::Hammerhead, WeaponId::Breacher12,
                    WeaponId::Kr44, WeaponId::Anvil44, WeaponId::Marksman,
                ];
                FLOOR[rng.below(FLOOR.len() as u32) as usize]
            } else {
                WeaponId::SidearmP9
            };
            PickupInstance { spot: *spot, cooldown: 0.0, weapon }
        }).collect();

        let players = (0..MAX_PLAYERS).map(|i| Player::new(i as u8)).collect();

        World {
            brush_health: map.collision.brushes.iter().map(|b| b.health).collect(),
            map, map_id, players,
            grenades: Vec::with_capacity(32),
            smokes: Vec::with_capacity(8),
            fires: Vec::with_capacity(8),
            pickups,
            events: EventQueue::new(512),
            time: 0.0,
            tick: 0,
            rng,
            friendly_fire: false,
            lag_compensation: true,
            respawn: RespawnPolicy::Auto { delay: 5.0 },
            team_game: true,
            next_entity: 1,
            spawn_scores: Vec::with_capacity(64),
        }
    }

    /// Swaps in a different map, keeping player identities and scores.
    pub fn load_map(&mut self, map_id: MapId, seed: u32) {
        let carried: Vec<(String, Team, bool, super::loadout::Loadout, u8, u16)> = self.players.iter()
            .map(|p| (p.name.clone(), p.team, p.is_bot, p.loadout, p.level, p.ping_ms))
            .collect();
        let in_use: Vec<bool> = self.players.iter().map(|p| p.in_use).collect();
        let mut fresh = World::new(map_id, seed);
        for (i, p) in fresh.players.iter_mut().enumerate() {
            let (name, team, bot, loadout, level, ping) = carried[i].clone();
            p.name = name;
            p.team = team;
            p.is_bot = bot;
            p.loadout = loadout;
            p.level = level;
            p.ping_ms = ping;
            p.in_use = in_use[i];
        }
        fresh.friendly_fire = self.friendly_fire;
        fresh.lag_compensation = self.lag_compensation;
        fresh.team_game = self.team_game;
        fresh.time = self.time;
        *self = fresh;
    }

    fn alloc_entity(&mut self) -> u16 {
        let id = self.next_entity;
        self.next_entity = self.next_entity.wrapping_add(1).max(1);
        id
    }

    #[inline]
    pub fn player(&self, slot: u8) -> Option<&Player> {
        self.players.get(slot as usize).filter(|p| p.in_use)
    }

    #[inline]
    pub fn player_mut(&mut self, slot: u8) -> Option<&mut Player> {
        self.players.get_mut(slot as usize).filter(|p| p.in_use)
    }

    pub fn active_players(&self) -> impl Iterator<Item = &Player> {
        self.players.iter().filter(|p| p.in_use)
    }

    pub fn living_players(&self) -> impl Iterator<Item = &Player> {
        self.players.iter().filter(|p| p.in_use && p.alive)
    }

    pub fn team_count(&self, team: Team) -> usize {
        self.players.iter().filter(|p| p.in_use && p.team == team).count()
    }

    pub fn team_alive(&self, team: Team) -> usize {
        self.players.iter().filter(|p| p.in_use && p.alive && p.team == team).count()
    }

    /// Are these two players hostile to each other?
    #[inline]
    pub fn hostile(&self, a: u8, b: u8) -> bool {
        if a == b { return false; }
        let (pa, pb) = match (self.player(a), self.player(b)) {
            (Some(x), Some(y)) => (x, y),
            _ => return false,
        };
        if !self.team_game { return true; }
        pa.team != pb.team || pa.team == Team::None
    }

    // ================================================================ step

    /// Advances the world by one fixed tick. `commands` supplies the input for
    /// each player slot; a slot with no command coasts on its last one.
    pub fn step(&mut self, dt: f32) {
        self.time += dt as f64;
        self.tick = self.tick.wrapping_add(1);

        self.step_projectiles(dt);
        self.step_volumes(dt);
        self.step_pickups(dt);

        for i in 0..self.players.len() {
            if !self.players[i].in_use { continue; }
            self.players[i].update_status(dt);
            self.players[i].record_history(self.time);
        }
        self.step_burning(dt);
    }

    /// Runs one player's command: movement, then weapon, then whatever the
    /// weapon produced. This is the function the client also calls for
    /// prediction, minus the damage resolution.
    pub fn run_command(&mut self, slot: u8, cmd: &InputCmd) {
        let idx = slot as usize;
        if idx >= self.players.len() || !self.players[idx].in_use { return; }
        if !self.players[idx].alive {
            self.players[idx].trigger_was_held = cmd.held(Buttons::FIRE);
            return;
        }
        let dt = cmd.dt();
        self.players[idx].last_buttons = cmd.buttons;

        // -------------------------------------------------------- movement
        let want_ads = cmd.held(Buttons::ADS);
        let mods = self.players[idx].move_mods(want_ads);
        let ev = {
            let p = &mut self.players[idx];
            movement::move_player(&mut p.mv, cmd, &mods, &self.map.collision, dt)
        };
        {
            let bounds = self.map.bounds;
            let p = &mut self.players[idx];
            movement::clamp_to_bounds(&mut p.mv, &bounds);
        }

        if ev.footstep {
            let (surface, volume) = {
                let p = &self.players[idx];
                let s = self.map.collision.material_at(ev.surface_brush, Vec3::Y).surface();
                let vol = p.perk().footstep_volume() * (0.55 + 0.45 * p.mv.sprint_t);
                (s, vol)
            };
            let pos = self.players[idx].mv.pos;
            self.events.push(GameEvent::Footstep { player: slot, pos, surface, volume });
        }
        if let Some(speed) = ev.landed {
            if speed > movement::tune::LAND_HARD {
                let surface = self.map.collision.material_at(ev.surface_brush, Vec3::Y).surface();
                let pos = self.players[idx].mv.pos;
                self.events.push(GameEvent::Land { player: slot, pos, speed, surface });
            }
        }
        if ev.jumped {
            self.events.push(GameEvent::Jump { player: slot });
        }
        if ev.fall_damage > 0.0 {
            self.apply_damage(slot, slot, ev.fall_damage, DeathCause::Fall, WeaponId::CombatKnife,
                              self.players[idx].mv.pos, HitZone::Body);
        }
        // Falling out of the world is always fatal, whatever caused it.
        if movement::out_of_world(&self.players[idx].mv, &self.map.bounds) {
            let pos = self.players[idx].mv.pos;
            self.apply_damage(slot, slot, 1000.0, DeathCause::World, WeaponId::CombatKnife, pos, HitZone::Body);
            return;
        }

        // ---------------------------------------------------------- weapon
        let out = self.players[idx].update_weapon(cmd, dt);

        if let Some(to) = out.swapped_to {
            self.events.push(GameEvent::Swap { player: slot, slot: to });
        }
        if out.started_reload {
            let empty = self.players[idx].weapon().is_empty();
            self.events.push(GameEvent::Reload { player: slot, empty });
        }
        if out.loaded_shell {
            self.events.push(GameEvent::ShellLoaded { player: slot });
        }
        if out.dry_fire {
            self.events.push(GameEvent::DryFire { player: slot });
        }
        if out.melee {
            self.melee_attack(slot);
        }
        if let Some(kind) = out.threw {
            self.throw_equipment(slot, kind, out.throw_cooked);
        }
        for shot in 0..out.shots {
            self.fire_weapon(slot, cmd.seq, shot);
        }

        self.check_pickups(slot);
    }

    // ============================================================= combat

    /// Time to rewind the world to when resolving a shot from `slot`.
    fn rewind_time_for(&self, slot: u8) -> f64 {
        if !self.lag_compensation { return self.time; }
        let p = match self.player(slot) { Some(p) => p, None => return self.time };
        if p.is_bot { return self.time; }
        // Half the round trip plus the client's interpolation delay: that is
        // what the shooter actually saw on their screen.
        let delay = (p.ping_ms as f64 * 0.5 + INTERP_DELAY_MS) / 1000.0;
        self.time - delay.clamp(0.0, MAX_REWIND)
    }

    /// Applies damage to a breakable brush, destroying it when it runs out.
    ///
    /// Health lives on the simulation rather than on the map, because the map
    /// is shared, immutable and reused between rounds; this is round state.
    pub fn damage_brush(&mut self, brush: u32, damage: f32, pos: Vec3, surface: Surface) {
        let Some(b) = self.map.collision.brushes.get(brush as usize) else { return };
        if !b.is_breakable() || self.map.collision.is_destroyed(brush) { return; }
        let i = brush as usize;
        if self.brush_health.len() != self.map.collision.brushes.len() {
            self.brush_health = self.map.collision.brushes.iter().map(|b| b.health).collect();
        }
        self.brush_health[i] -= damage;
        if self.brush_health[i] > 0.0 { return; }
        if self.map.collision.destroy(brush) {
            self.events.push(GameEvent::BrushBroken { brush, pos, surface });
        }
    }

    /// Resolves one shot: spread, trace, penetration, damage, effects.
    pub fn fire_weapon(&mut self, slot: u8, seq: u32, shot_index: u8) {
        let (origin, base_dir, def, cone, weapon_id) = {
            let p = match self.player(slot) { Some(p) => p, None => return };
            if !p.alive { return; }
            let def = p.def();
            (p.eye(), p.aim_dir(), def, p.spread(), p.weapon().id)
        };

        let seed = shot_seed(slot, seq, shot_index);
        self.events.push(GameEvent::Shot {
            player: slot, weapon: weapon_id, origin, dir: base_dir, seed,
        });

        let rewind = self.rewind_time_for(slot);
        for pellet in 0..def.pellets.max(1) {
            let mut rng = Rng::seeded(seed ^ (pellet as u32).wrapping_mul(0x9E37_79B9));
            let dir = spread_direction(base_dir, cone, &mut rng);
            self.trace_bullet(slot, origin, dir, def, rewind);
        }
    }

    /// Traces a single round through the world, allowing one penetration.
    fn trace_bullet(
        &mut self,
        shooter: u8,
        mut origin: Vec3,
        dir: Vec3,
        def: &'static super::weapons::WeaponDef,
        rewind: f64,
    ) {
        let mut damage_scale = 1.0f32;
        let mut budget = def.penetration;
        let mut travelled = 0.0f32;

        for _segment in 0..3 {
            let range = MAX_SHOT_RANGE - travelled;
            if range <= 0.1 { return; }

            let world_hit = self.map.collision.trace_ray(origin, dir, range, TraceMask::Shot);
            let world_t = if world_hit.hit { (world_hit.point - origin).length() } else { range };

            // Nearest player in front of the wall.
            let mut best: Option<(u8, f32, HitZone, Vec3)> = None;
            for i in 0..self.players.len() {
                let victim = i as u8;
                if victim == shooter { continue; }
                if !self.players[i].in_use || !self.players[i].alive { continue; }
                if !self.friendly_fire && !self.hostile(shooter, victim) { continue; }
                let sample = self.players[i].rewind(rewind);
                let (pos, height) = match sample {
                    Some(s) if s.alive => (s.pos, s.height),
                    _ => (self.players[i].mv.pos, self.players[i].mv.height),
                };
                if let Some((t, zone)) = ray_vs_player(origin, dir, pos, height, world_t.min(range)) {
                    if best.map_or(true, |(_, bt, _, _)| t < bt) {
                        best = Some((victim, t, zone, origin + dir * t));
                    }
                }
            }

            if let Some((victim, t, zone, point)) = best {
                let distance = travelled + t;
                let base = def.damage_at(distance) * damage_scale;
                let mult = match zone {
                    HitZone::Head => def.headshot_mult,
                    HitZone::Limb => def.limb_mult,
                    HitZone::Body => 1.0,
                };
                let cause = if zone == HitZone::Head { DeathCause::Headshot } else { DeathCause::Bullet };
                self.apply_damage(shooter, victim, base * mult, cause, def.id, point, zone);
                // A round stops in the body it hits. Overpenetration would be
                // realistic and would make shotguns and LMGs absurd.
                return;
            }

            if !world_hit.hit {
                return;
            }

            // World impact. Nothing is drawn for a surface that is not drawn:
            // the map boundary stops rounds, and sparking off it would leave
            // decals and puffs hanging in mid-air at the edge of the world.
            let surface = self.map.collision.material_at(world_hit.brush, world_hit.normal).surface();
            let visible = self.map.collision.brushes.get(world_hit.brush as usize)
                .is_some_and(|b| !b.flags.contains(crate::maps::brush::BrushFlags::NODRAW));
            if visible {
                let kind = if def.pellets > 1 { ImpactKind::Pellet } else { ImpactKind::Bullet };
                self.events.push(GameEvent::Impact {
                    pos: world_hit.point, normal: world_hit.normal, surface, kind,
                });
            }

            // Breakable geometry takes the hit. Cover that can be removed is
            // what stops a strong position being a permanent one, and it is
            // the only thing on these maps that changes shape during a round.
            let brush_damage = def.damage_at(distance_to(origin, world_hit.point)) * damage_scale;
            self.damage_brush(world_hit.brush, brush_damage, world_hit.point, surface);

            // Can the round get through?
            let brush = match self.map.collision.brushes.get(world_hit.brush as usize) {
                Some(b) => b,
                None => return,
            };
            let thickness = slab_exit_distance(&brush.aabb, world_hit.point + dir * 0.001, dir);
            let cost = thickness * surface_hardness(surface);
            if cost > budget { return; }
            budget -= cost;
            damage_scale *= 0.55 - 0.10 * cost;
            if damage_scale <= 0.12 { return; }

            let step = thickness + 0.02;
            origin = world_hit.point + dir * step;
            travelled += (world_hit.point - origin).length() + step;
        }
    }

    /// A melee swing: a short cone in front of the attacker.
    pub fn melee_attack(&mut self, slot: u8) {
        let (origin, dir, def) = {
            let p = match self.player(slot) { Some(p) => p, None => return };
            if !p.alive { return; }
            // Melee always uses the dedicated melee weapon, whatever is held.
            (p.eye(), p.aim_dir(), p.weapons[SLOT_MELEE as usize].def())
        };
        let range = def.range_near;
        let mut hit_slot = None;
        let mut best = f32::MAX;
        for i in 0..self.players.len() {
            let victim = i as u8;
            if victim == slot || !self.players[i].in_use || !self.players[i].alive { continue; }
            if !self.friendly_fire && !self.hostile(slot, victim) { continue; }
            let p = &self.players[i];
            let centre = p.mv.pos + Vec3::Y * (p.mv.height * 0.55);
            let to = centre - origin;
            let d = to.length();
            if d > range || d >= best { continue; }
            // Generous cone; melee that whiffs on a technicality is miserable.
            if to.normalize_or_zero().dot(dir) < 0.55 { continue; }
            if !self.map.collision.line_of_sight(origin, centre) { continue; }
            best = d;
            hit_slot = Some((victim, centre));
        }
        match hit_slot {
            Some((victim, point)) => {
                self.events.push(GameEvent::Melee { player: slot, hit: true });
                self.apply_damage(slot, victim, def.damage, DeathCause::Melee, def.id, point, HitZone::Body);
            }
            None => self.events.push(GameEvent::Melee { player: slot, hit: false }),
        }
    }

    /// Throws a piece of equipment.
    pub fn throw_equipment(&mut self, slot: u8, kind: Equipment, cooked: f32) {
        let (pos, vel, team) = {
            let p = match self.player(slot) { Some(p) => p, None => return };
            let aim = p.aim_dir();
            let origin = p.eye() + aim * 0.4;
            (origin, throw_velocity(aim, p.mv.vel, kind), p.team)
        };
        let id = self.alloc_entity();
        self.grenades.push(Grenade::new(id, slot, team, kind, pos, vel, cooked));
        self.events.push(GameEvent::GrenadeThrown { id, player: slot, kind, pos, vel });
    }

    fn step_projectiles(&mut self, dt: f32) {
        let mut detonations: Vec<(Vec3, Equipment, u8, Team)> = Vec::new();
        let mut bounces: Vec<(u16, Vec3, u32)> = Vec::new();

        for g in self.grenades.iter_mut() {
            if let Some((pos, brush)) = g.step(&self.map.collision, dt) {
                bounces.push((g.id, pos, brush));
            }
            if g.spent {
                detonations.push((g.pos, g.kind, g.owner, g.team));
            }
        }
        self.grenades.retain(|g| !g.spent);

        for (id, pos, brush) in bounces {
            let surface = self.map.collision.material_at(brush, Vec3::Y).surface();
            self.events.push(GameEvent::GrenadeBounce { id, pos, surface });
        }
        for (pos, kind, owner, team) in detonations {
            self.detonate(pos, kind, owner, team);
        }
    }

    /// Resolves a detonation of any kind.
    pub fn detonate(&mut self, pos: Vec3, kind: Equipment, owner: u8, team: Team) {
        match kind {
            Equipment::Smoke => {
                let id = self.alloc_entity();
                self.smokes.push(Smoke::new(id, pos, team));
                self.events.push(GameEvent::SmokeStarted { id, pos });
                return;
            }
            Equipment::Incendiary => {
                let id = self.alloc_entity();
                let f = Fire::new(id, owner, team, pos);
                let radius = f.radius;
                self.fires.push(f);
                self.events.push(GameEvent::FireStarted { id, pos, radius });
                self.events.push(GameEvent::Explosion { pos, kind, radius });
                return;
            }
            _ => {}
        }

        let radius = kind.blast_radius();
        self.events.push(GameEvent::Explosion { pos, kind, radius });

        let damage = kind.blast_damage();
        for i in 0..self.players.len() {
            let victim = i as u8;
            if !self.players[i].in_use || !self.players[i].alive { continue; }
            let hostile = self.hostile(owner, victim);
            let is_self = victim == owner;
            if !is_self && !hostile && !self.friendly_fire { continue; }

            let (centre, eye, look) = {
                let p = &self.players[i];
                (p.mv.pos + Vec3::Y * (p.mv.height * 0.5), p.eye(), p.aim_dir())
            };
            let dist = (centre - pos).length();
            if dist > radius { continue; }
            // Cover matters: a wall between you and the blast stops it.
            if !self.map.collision.line_of_sight(pos + Vec3::Y * 0.1, centre) { continue; }

            if damage > 0.0 {
                let amount = damage * blast_falloff(dist, radius);
                if amount > 1.0 {
                    self.apply_damage(owner, victim, amount, DeathCause::Explosion, WeaponId::CombatKnife, centre, HitZone::Body);
                }
            }

            if matches!(kind, Equipment::Flashbang | Equipment::Concussion) {
                let strength = flash_strength(pos, eye, look, radius, false);
                if strength > 0.0 {
                    let conc = kind == Equipment::Concussion;
                    let p = &mut self.players[i];
                    if conc {
                        p.concussion = p.concussion.max(strength * 0.9);
                        p.flash = p.flash.max(strength * 0.25);
                    } else {
                        p.flash = p.flash.max(strength);
                    }
                    self.events.push(GameEvent::Blinded { player: victim, strength, concussion: conc });
                }
            }
        }
    }

    fn step_volumes(&mut self, dt: f32) {
        for s in self.smokes.iter_mut() { s.age += dt; }
        self.smokes.retain(|s| !s.expired());

        // Incendiary zones tick damage on everyone standing in them.
        let mut burns: Vec<(u8, u8, f32)> = Vec::new();
        for f in self.fires.iter_mut() {
            f.age += dt;
            f.tick_accum += dt;
            if f.tick_accum < 0.35 { continue; }
            f.tick_accum = 0.0;
            for p in self.players.iter() {
                if !p.in_use || !p.alive { continue; }
                if f.contains(p.mv.pos) {
                    burns.push((f.owner, p.slot, 1.0));
                }
            }
        }
        self.fires.retain(|f| !f.expired());
        for (owner, victim, _) in burns {
            if victim != owner && !self.hostile(owner, victim) && !self.friendly_fire { continue; }
            let p = &mut self.players[victim as usize];
            p.burning = 2.2;
            p.burning_from = owner;
        }
    }

    fn step_burning(&mut self, dt: f32) {
        let mut ticks: Vec<(u8, u8, f32)> = Vec::new();
        for p in self.players.iter_mut() {
            if !p.in_use || !p.alive || p.burning <= 0.0 { continue; }
            p.burning -= dt;
            ticks.push((p.burning_from, p.slot, 26.0 * dt));
        }
        for (from, victim, amount) in ticks {
            let pos = self.players[victim as usize].mv.pos;
            self.apply_damage(from, victim, amount, DeathCause::Fire, WeaponId::CombatKnife, pos, HitZone::Body);
        }
    }

    // ============================================================= damage

    /// The single funnel for all damage. Every source goes through here so
    /// scoring, assists, death and events can never disagree.
    #[allow(clippy::too_many_arguments)]
    pub fn apply_damage(
        &mut self,
        attacker: u8,
        victim: u8,
        amount: f32,
        cause: DeathCause,
        weapon: WeaponId,
        point: Vec3,
        zone: HitZone,
    ) {
        let vi = victim as usize;
        if vi >= self.players.len() || !self.players[vi].in_use || !self.players[vi].alive { return; }
        if attacker != victim && !self.friendly_fire && !self.hostile(attacker, victim) { return; }

        let now = self.time;
        let dealt = self.players[vi].take_damage(amount, attacker, now);
        if dealt <= 0.0 { return; }

        let died = self.players[vi].health <= 0.0;
        if let Some(a) = self.players.get_mut(attacker as usize) {
            if a.in_use && attacker != victim {
                a.score.damage += dealt as u32;
            }
        }
        self.events.push(GameEvent::HitPlayer {
            attacker, victim, pos: point, zone,
            damage: dealt.round().clamp(0.0, 65535.0) as u16,
            lethal: died,
        });

        if died {
            self.kill_player(attacker, victim, weapon, cause, point);
        }
    }

    fn kill_player(&mut self, killer: u8, victim: u8, weapon: WeaponId, cause: DeathCause, point: Vec3) {
        let now = self.time;
        let distance = match (self.player(killer), self.player(victim)) {
            (Some(k), Some(v)) => (k.mv.pos - v.mv.pos).length(),
            _ => 0.0,
        };

        let assisters: Vec<u8> = {
            let v = &self.players[victim as usize];
            v.assisters(killer, now).collect()
        };

        let self_kill = killer == victim || killer == NO_PLAYER;
        let friendly = !self_kill && !self.hostile(killer, victim);

        let mut streak = 0u16;
        if !self_kill && !friendly {
            if let Some(k) = self.players.get_mut(killer as usize) {
                k.score.kills += 1;
                k.score.streak += 1;
                k.score.best_streak = k.score.best_streak.max(k.score.streak);
                k.score.score += 100;
                if cause == DeathCause::Headshot { k.score.headshots += 1; }
                streak = k.score.streak;
            }
        } else if friendly {
            if let Some(k) = self.players.get_mut(killer as usize) {
                k.score.score -= 50;
            }
        } else if let Some(k) = self.players.get_mut(killer as usize) {
            // Suicide or world death.
            k.score.score -= 25;
        }

        for a in assisters {
            if let Some(p) = self.players.get_mut(a as usize) {
                p.score.assists += 1;
                p.score.score += 35;
            }
        }

        {
            let v = &mut self.players[victim as usize];
            v.score.deaths += 1;
            v.score.streak = 0;
            v.alive = false;
            v.health = 0.0;
            v.armor = 0.0;
            v.mv.vel = Vec3::ZERO;
            v.cooking = None;
            v.burning = 0.0;
            v.flash = 0.0;
            v.concussion = 0.0;
            v.respawn_at = match self.respawn {
                RespawnPolicy::Auto { delay } => now + delay as f64,
                RespawnPolicy::OnRequest { min_delay } => now + min_delay as f64,
                RespawnPolicy::None => f64::MAX,
            };
        }

        self.events.push(GameEvent::Kill {
            killer: if self_kill { victim } else { killer },
            victim, weapon, cause, distance, streak,
        });
        let _ = point;
    }

    // ============================================================ spawning

    /// Chooses a spawn point for a player, preferring somewhere the enemy
    /// cannot immediately see or reach.
    pub fn choose_spawn(&mut self, slot: u8, initial_only: bool) -> Option<(Vec3, f32)> {
        let team = self.player(slot)?.team;
        self.spawn_scores.clear();

        // Gather enemy positions once rather than per candidate.
        let mut enemies: Vec<Vec3> = Vec::with_capacity(8);
        let mut friends: Vec<Vec3> = Vec::with_capacity(8);
        for p in self.players.iter() {
            if !p.in_use || !p.alive || p.slot == slot { continue; }
            if self.team_game && p.team == team { friends.push(p.mv.pos); }
            else { enemies.push(p.mv.pos); }
        }

        for (i, sp) in self.map.spawns.iter().enumerate() {
            if initial_only && !sp.initial { continue; }
            if self.team_game && sp.team != Team::None && sp.team != team { continue; }
            if !self.team_game && sp.team != Team::None {
                // Free-for-all uses every spawn, but prefers neutral ones.
            }

            let mut score = 0.0f32;
            let mut nearest_enemy = f32::MAX;
            for e in &enemies {
                let d = (*e - sp.pos).length();
                nearest_enemy = nearest_enemy.min(d);
                // Being visible to an enemy is far worse than being near one.
                if d < 40.0 && self.map.collision.line_of_sight(sp.pos + Vec3::Y * 1.5, *e + Vec3::Y * 1.5) {
                    score -= 900.0 - d * 12.0;
                }
            }
            if nearest_enemy < f32::MAX {
                score += nearest_enemy.min(45.0) * 12.0;
                if nearest_enemy < 12.0 { score -= 700.0; }
            } else {
                score += 300.0;
            }
            // A friend nearby is mildly good: it keeps teams together.
            for f in &friends {
                let d = (*f - sp.pos).length();
                if d < 20.0 { score += 60.0 - d * 2.0; }
            }
            if sp.team == team { score += 220.0; }
            // Never drop two players onto the same tile.
            if self.players.iter().any(|p| p.in_use && p.alive && (p.mv.pos - sp.pos).length() < 1.6) {
                score -= 1200.0;
            }
            score += self.rng.range(0.0, 90.0);
            self.spawn_scores.push((i, score));
        }

        if self.spawn_scores.is_empty() {
            // Fall back to any spawn at all rather than refusing to spawn.
            let sp = self.map.spawns.first()?;
            return Some((sp.pos, sp.yaw));
        }
        self.spawn_scores.sort_unstable_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        // Pick randomly from the best few so spawns are not perfectly
        // predictable to someone who has learned the map.
        let pool = self.spawn_scores.len().min(4);
        let pick = self.spawn_scores[self.rng.below(pool as u32) as usize].0;
        let sp = self.map.spawns[pick];
        Some((sp.pos, sp.yaw))
    }

    pub fn respawn_player(&mut self, slot: u8, initial: bool) {
        let Some((pos, yaw)) = self.choose_spawn(slot, initial) else { return };
        let now = self.time;
        if let Some(p) = self.player_mut(slot) {
            p.spawn_at(pos, yaw, now);
        }
        self.events.push(GameEvent::Spawned { player: slot, pos });
    }

    // ============================================================= pickups

    fn step_pickups(&mut self, dt: f32) {
        for (i, p) in self.pickups.iter_mut().enumerate() {
            if p.cooldown > 0.0 {
                p.cooldown -= dt;
                if p.cooldown <= 0.0 {
                    p.cooldown = 0.0;
                    self.events.push(GameEvent::PickupRespawned { index: i as u16 });
                }
            }
        }
    }

    fn check_pickups(&mut self, slot: u8) {
        let (pos, height) = {
            let p = match self.player(slot) { Some(p) => p, None => return };
            if !p.alive { return; }
            (p.mv.pos, p.mv.height)
        };
        let body = Aabb::from_base(pos, movement::tune::RADIUS + 0.5, height + 0.4);

        for i in 0..self.pickups.len() {
            if !self.pickups[i].available() { continue; }
            let spot = self.pickups[i].spot;
            if !body.contains_point(spot.pos) && (spot.pos - pos).length() > 1.5 { continue; }

            let weapon = self.pickups[i].weapon;
            let taken = self.grant_pickup(slot, spot.kind, weapon);
            if taken {
                self.pickups[i].cooldown = spot.respawn;
                self.events.push(GameEvent::PickupTaken { player: slot, index: i as u16, kind: spot.kind });
            }
        }
    }

    fn grant_pickup(&mut self, slot: u8, kind: PickupKind, weapon: WeaponId) -> bool {
        let p = match self.player_mut(slot) { Some(p) => p, None => return false };
        match kind {
            PickupKind::Ammo => {
                let mut any = false;
                for w in p.weapons.iter_mut() {
                    let def = w.def();
                    if def.is_melee() { continue; }
                    if w.reserve < def.reserve {
                        w.reserve = (w.reserve + def.mag * 2).min(def.reserve);
                        any = true;
                    }
                }
                any
            }
            PickupKind::Armor => {
                let cap = 50.0 + p.perk().bonus_armor();
                if p.armor >= cap { return false; }
                p.armor = cap;
                true
            }
            PickupKind::Health => {
                if p.health >= p.max_health { return false; }
                p.health = p.max_health;
                p.regen_delay = 0.0;
                true
            }
            PickupKind::Grenade => {
                let lethal_cap = p.loadout.lethal.count() + p.perk().extra_grenade();
                let tac_cap = p.loadout.tactical.count();
                if p.lethal_count >= lethal_cap && p.tactical_count >= tac_cap { return false; }
                p.lethal_count = lethal_cap;
                p.tactical_count = tac_cap;
                true
            }
            PickupKind::Weapon => {
                // Floor weapons replace the primary; the old one is gone.
                if p.weapons[0].id == weapon { return false; }
                p.weapons[0] = super::weapons::WeaponSlot::new(weapon);
                p.cur = 0;
                p.queued_slot = 0;
                p.action = super::player::Action::Raising;
                p.action_timer = weapon.def().swap_in;
                true
            }
        }
    }

    /// Scavenger perk: take a magazine from someone you killed.
    pub fn scavenge(&mut self, slot: u8) {
        let Some(p) = self.player_mut(slot) else { return };
        if p.perk() != super::loadout::Perk::Scavenger { return; }
        for w in p.weapons.iter_mut() {
            let def = w.def();
            if def.is_melee() { continue; }
            w.reserve = (w.reserve + def.mag).min((def.reserve as f32 * 1.5) as u16);
        }
    }
}

/// Client interpolation delay in milliseconds. The server needs the same
/// number to rewind correctly, so it lives here rather than in the client.
pub const INTERP_DELAY_MS: f64 = 100.0;

/// Deterministic per-shot seed. The client computes the identical value, so
/// its predicted tracers and pellet spread match the server's exactly.
#[inline]
pub fn shot_seed(slot: u8, seq: u32, shot_index: u8) -> u32 {
    let mut h = seq
        .wrapping_mul(0x9E37_79B9)
        ^ ((slot as u32) << 24)
        ^ ((shot_index as u32).wrapping_mul(0x85EB_CA6B));
    h ^= h >> 15;
    h = h.wrapping_mul(0x2545_F491);
    h ^ (h >> 13)
}

/// Ray against a player's three hitboxes. Returns the nearest hit and its zone.
fn ray_vs_player(origin: Vec3, dir: Vec3, pos: Vec3, height: f32, max_t: f32) -> Option<(f32, HitZone)> {
    let inv = Vec3::new(
        if dir.x.abs() < 1e-8 { f32::MAX } else { 1.0 / dir.x },
        if dir.y.abs() < 1e-8 { f32::MAX } else { 1.0 / dir.y },
        if dir.z.abs() < 1e-8 { f32::MAX } else { 1.0 / dir.z },
    );

    let body = Aabb::from_base(pos, movement::tune::RADIUS, height);
    let (body_t, _) = body.ray_hit(origin, inv, max_t)?;

    // Only if the body was hit do we bother with the finer boxes.
    let head = Aabb::new(
        Vec3::new(pos.x - 0.16, pos.y + height - height * 0.175, pos.z - 0.16),
        Vec3::new(pos.x + 0.16, pos.y + height, pos.z + 0.16),
    );
    if let Some((t, _)) = head.ray_hit(origin, inv, max_t) {
        return Some((t, HitZone::Head));
    }
    let legs = Aabb::new(
        Vec3::new(pos.x - 0.30, pos.y, pos.z - 0.30),
        Vec3::new(pos.x + 0.30, pos.y + height * 0.42, pos.z + 0.30),
    );
    if let Some((t, _)) = legs.ray_hit(origin, inv, max_t) {
        if t <= body_t + 0.01 { return Some((t, HitZone::Limb)); }
    }
    Some((body_t, HitZone::Body))
}

/// Distance from a point inside a box to where the ray leaves it.
fn slab_exit_distance(b: &Aabb, inside: Vec3, dir: Vec3) -> f32 {
    let mut t = f32::MAX;
    for axis in 0..3 {
        let d = dir[axis];
        if d.abs() < 1e-8 { continue; }
        let target = if d > 0.0 { b.max[axis] } else { b.min[axis] };
        let ti = (target - inside[axis]) / d;
        if ti > 0.0 { t = t.min(ti); }
    }
    if t == f32::MAX { 0.05 } else { t.clamp(0.01, 8.0) }
}

/// How hard a surface is to shoot through, per metre.
fn surface_hardness(s: Surface) -> f32 {
    match s {
        Surface::Glass => 0.15,
        Surface::Soft | Surface::Grass | Surface::Snow => 0.7,
        Surface::Wood => 1.0,
        Surface::Metal => 1.8,
        Surface::Dirt | Surface::Sand => 2.2,
        Surface::Gravel => 2.6,
        Surface::Concrete => 3.0,
        Surface::Water => 0.4,
    }
}


#[inline]
fn distance_to(a: Vec3, b: Vec3) -> f32 { (b - a).length() }

#[cfg(test)]
mod breakable_tests {
    use super::*;
    use crate::maps::brush::TraceMask;

    /// A breakable brush absorbs damage, disappears when it runs out, stops
    /// blocking movement and bullets, and comes back when the round resets.
    #[test]
    fn breakables_break_and_come_back() {
        let mut world = World::new(crate::maps::MapId::Ironveil, 1);
        let breakable = world.map.collision.brushes.iter().position(|b| b.is_breakable())
            .expect("Ironveil has breakable geometry");

        let b = world.map.collision.brushes[breakable].clone();
        let centre = b.aabb.center();
        let health = b.health;
        assert!(health > 0.0, "a breakable brush needs health");

        // A ray just long enough to cross this brush and nothing else.
        let reach = (b.aabb.max.x - b.aabb.min.x) * 0.5 + 0.35;
        let from = centre - Vec3::X * reach;
        let to = centre + Vec3::X * reach;
        assert!(!world.map.collision.line_of_sight(from, to), "intact cover should block");

        // Half its health leaves it standing.
        world.damage_brush(breakable as u32, health * 0.5, centre, Surface::Concrete);
        assert!(!world.map.collision.is_destroyed(breakable as u32));

        // The rest takes it out, exactly once.
        world.damage_brush(breakable as u32, health, centre, Surface::Concrete);
        assert!(world.map.collision.is_destroyed(breakable as u32));
        let breaks = world.events.iter()
            .filter(|e| matches!(e, GameEvent::BrushBroken { .. })).count();
        assert_eq!(breaks, 1, "breaking is announced once");

        // And it no longer stops anything.
        assert!(world.map.collision.line_of_sight(from, to), "broken cover should not block");
        let hit = world.map.collision.trace_ray(from, Vec3::X, reach * 2.0, TraceMask::Solid);
        assert!(!hit.hit, "movement should pass through a broken brush");

        world.map.collision.reset_destruction();
        assert!(!world.map.collision.is_destroyed(breakable as u32), "a new round restores cover");
    }
}
