//! A single bot's decision making.
//!
//! Bots run the exact same command path as a human: they fill in an
//! `InputCmd` every tick and the server executes it. That means they obey the
//! same movement rules, the same weapon timings and the same spread, and
//! anything that feels wrong about a bot is a gameplay problem rather than a
//! separate AI physics bug.
//!
//! Expensive work is staggered. Pathfinding runs at most a couple of times a
//! second per bot, target selection about ten times a second, and everything
//! in between is cheap steering.

use crate::core::{clampf, Rng};

use crate::game::loadout::{ClassId, Equipment, Loadout, ALL_PERKS, LETHAL_EQUIPMENT, TACTICAL_EQUIPMENT};
use crate::game::projectiles::smoke_occlusion;
use crate::game::sim::World;
use crate::game::types::{Buttons, InputCmd, Stance, Team, NO_PLAYER};
use crate::game::weapons::{WeaponClass, WeaponId};
use crate::maps::nav::PathFinder;
use crate::core::angle_delta;
use crate::math::angles_from_dir;
use glam::Vec3;

/// A* expansion budget. A one-metre lattice on a large map runs to several
/// thousand nodes, and the route the long way round a dock needs most of them.
const PATH_BUDGET: u32 = 20_000;

/// What the bot is trying to do right now.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Goal {
    /// Move toward the objective, or roam if there is none.
    Advance,
    /// An enemy is visible and in range.
    Engage,
    /// Enemy seen recently but not now: push to where they were.
    Hunt,
    /// Hurt and out of the fight for a moment.
    Retreat,
    /// Standing on or defending an objective.
    Hold,
    Reload,
}

/// Per-bot personality, which is most of what stops twelve bots feeling like
/// one bot twelve times.
#[derive(Copy, Clone, Debug)]
pub struct Personality {
    /// How readily it pushes rather than holds. 0..1.
    pub aggression: f32,
    /// Aim error scale; lower is deadlier.
    pub jitter: f32,
    /// Seconds between seeing a target and acting on it.
    pub reaction: f32,
    /// How fast it can swing its view, radians per second.
    pub turn_rate: f32,
    /// Chance per opportunity of throwing a grenade.
    pub nade_appetite: f32,
    /// How much it strafes while fighting.
    pub strafe: f32,
    /// Preferred distance multiplier applied to its weapon's ideal range.
    pub range_bias: f32,
}

impl Personality {
    /// Builds a personality from a difficulty tier plus per-bot variation.
    pub fn roll(difficulty: u8, rng: &mut Rng) -> Personality {
        // 0 recruit, 1 regular, 2 veteran, 3 elite.
        let d = difficulty.min(3) as f32 / 3.0;
        Personality {
            aggression: clampf(0.30 + d * 0.45 + rng.range(-0.15, 0.15), 0.05, 1.0),
            jitter: clampf((1.35 - d * 1.05) * rng.range(0.75, 1.30), 0.05, 2.0),
            reaction: clampf((0.62 - d * 0.44) * rng.range(0.7, 1.35), 0.06, 1.2),
            turn_rate: clampf(3.2 + d * 5.5 + rng.range(-0.8, 0.8), 1.5, 12.0),
            nade_appetite: clampf(0.12 + d * 0.28 + rng.range(-0.08, 0.08), 0.0, 0.8),
            strafe: clampf(0.35 + d * 0.45 + rng.range(-0.2, 0.2), 0.0, 1.0),
            range_bias: rng.range(0.75, 1.25),
        }
    }
}

pub struct Bot {
    pub slot: u8,
    pub personality: Personality,
    pub rng: Rng,
    pub goal: Goal,

    // ------------------------------------------------------------- target
    pub target: u8,
    pub target_last_seen: f64,
    pub target_last_pos: Vec3,
    /// Time the current target first came into view, for the reaction delay.
    pub target_acquired: f64,
    pub can_see_target: bool,

    // --------------------------------------------------------- navigation
    pub finder: PathFinder,
    pub path: Vec<Vec3>,
    pub path_index: usize,
    pub goal_pos: Vec3,
    pub repath_at: f64,
    /// Where around an objective this bot posts up, re-rolled periodically so
    /// a defence shifts instead of freezing.
    post_angle: f32,
    post_radius: f32,
    post_until: f64,
    /// Cooldown so a bot does not flick between weapons every tick.
    swap_ready_at: f64,
    /// Position at the last stuck check, and when it was taken.
    stuck_from: Vec3,
    stuck_at: f64,
    unstick_until: f64,
    unstick_dir: f32,

    // -------------------------------------------------------------- aiming
    pub aim_yaw: f32,
    pub aim_pitch: f32,
    /// Slowly wandering offset so the aim is never perfectly still.
    wander_phase: f32,

    // ------------------------------------------------------------ timings
    pub think_at: f64,
    pub strafe_dir: f32,
    pub strafe_until: f64,
    pub jump_at: f64,
    pub nade_ready_at: f64,
    pub crouch_until: f64,
    /// Cached command so the bot only rebuilds what changed.
    pub cmd: InputCmd,
}

impl Bot {
    pub fn new(slot: u8, difficulty: u8, seed: u32) -> Bot {
        let mut rng = Rng::seeded(seed ^ (slot as u32) << 8 ^ 0xB0_7B0_7);
        let personality = Personality::roll(difficulty, &mut rng);
        Bot {
            slot,
            personality,
            rng,
            goal: Goal::Advance,
            target: NO_PLAYER,
            target_last_seen: -999.0,
            target_last_pos: Vec3::ZERO,
            target_acquired: 0.0,
            can_see_target: false,
            finder: PathFinder::new(),
            path: Vec::new(),
            path_index: 0,
            goal_pos: Vec3::ZERO,
            repath_at: 0.0,
            post_angle: 0.0,
            post_radius: 0.0,
            post_until: 0.0,
            swap_ready_at: 0.0,
            stuck_from: Vec3::ZERO,
            stuck_at: 0.0,
            unstick_until: 0.0,
            unstick_dir: 1.0,
            aim_yaw: 0.0,
            aim_pitch: 0.0,
            wander_phase: 0.0,
            think_at: 0.0,
            strafe_dir: 1.0,
            strafe_until: 0.0,
            jump_at: 0.0,
            nade_ready_at: 0.0,
            crouch_until: 0.0,
            cmd: InputCmd { weapon: 0xFF, ..InputCmd::default() },
        }
    }

    /// Picks a loadout that suits this bot's personality.
    pub fn choose_loadout(&mut self, level: u8) -> Loadout {
        let class = if self.personality.aggression > 0.72 {
            if self.rng.chance(0.5) { ClassId::Scout } else { ClassId::Assault }
        } else if self.personality.aggression < 0.32 {
            if self.rng.chance(0.45) { ClassId::Marksman } else { ClassId::Heavy }
        } else {
            ClassId::Assault
        };
        let mut l = class.preset();
        // A little variety inside the class so a team is not identical.
        if self.rng.chance(0.4) {
            const AR: [WeaponId; 4] = [WeaponId::Kr44, WeaponId::Vectra5, WeaponId::Tempest, WeaponId::Kestrel];
            const SMG: [WeaponId; 4] = [WeaponId::Wasp9, WeaponId::Viper, WeaponId::Shrike, WeaponId::HornetC];
            l.primary = match l.primary.def().class {
                WeaponClass::Smg => SMG[self.rng.below(4) as usize],
                WeaponClass::Assault => AR[self.rng.below(4) as usize],
                _ => l.primary,
            };
        }
        // Vary the kit too, not just the gun. With everyone on the class
        // preset a whole team carried identical equipment and two of the four
        // tactical types never appeared in a match at all.
        if self.rng.chance(0.55) {
            l.lethal = LETHAL_EQUIPMENT[self.rng.below(LETHAL_EQUIPMENT.len() as u32) as usize];
        }
        if self.rng.chance(0.65) {
            l.tactical = TACTICAL_EQUIPMENT[self.rng.below(TACTICAL_EQUIPMENT.len() as u32) as usize];
        }
        if self.rng.chance(0.5) {
            l.perk = ALL_PERKS[self.rng.below(ALL_PERKS.len() as u32) as usize];
        }
        l.sanitize(level);
        l
    }

    /// The range this bot wants to fight at, given what it is holding.
    fn preferred_range(&self, world: &World) -> f32 {
        let Some(p) = world.player(self.slot) else { return 15.0 };
        let d = p.def();
        let base = match d.class {
            WeaponClass::Sniper => 55.0,
            WeaponClass::Lmg => 32.0,
            WeaponClass::Assault => 26.0,
            WeaponClass::Smg => 12.0,
            WeaponClass::Shotgun => 7.0,
            WeaponClass::Pistol => 14.0,
            WeaponClass::Melee => 2.0,
        };
        base * self.personality.range_bias
    }

    /// Can this bot see the given player right now?
    fn sees(&self, world: &World, other: u8) -> bool {
        let (Some(me), Some(them)) = (world.player(self.slot), world.player(other)) else { return false };
        if !them.alive { return false; }
        let eye = me.eye();
        let mut visible = false;
        // Check the head, chest and feet: peeking round cover should register.
        for h in [0.9, 0.55, 0.15] {
            let p = them.mv.pos + Vec3::Y * (them.mv.height * h);
            if world.map.collision.line_of_sight(eye, p) { visible = true; break; }
        }
        if !visible { return false; }

        let centre = them.mv.pos + Vec3::Y * (them.mv.height * 0.6);
        let to = centre - eye;
        let dist = to.length();
        if dist > crate::game::movement::MAX_ENGAGE_RANGE { return false; }
        // A field of view, so bots can be flanked.
        let look = crate::math::dir_from_angles(self.aim_yaw, self.aim_pitch);
        if to.normalize_or_zero().dot(look) < 0.28 && dist > 6.0 { return false; }
        // Smoke actually blocks them, which is the whole point of smoke.
        if smoke_occlusion(&world.smokes, eye, centre) > 0.6 { return false; }
        // Being flashed blinds a bot as thoroughly as a player.
        if me.flash > 0.9 { return false; }
        true
    }

    /// Re-evaluates who to shoot at. Runs about ten times a second.
    fn select_target(&mut self, world: &World, now: f64) {
        let Some(me) = world.player(self.slot) else { return };
        let my_pos = me.mv.pos;

        let mut best = (NO_PLAYER, f32::MAX);
        for other in world.active_players() {
            if other.slot == self.slot || !other.alive { continue; }
            if !world.hostile(self.slot, other.slot) { continue; }
            if !self.sees(world, other.slot) { continue; }
            let d = (other.mv.pos - my_pos).length();
            // Prefer whoever is closest, but strongly prefer whoever is
            // already shooting at us.
            let score = if me.last_hurt_by == other.slot && now - me.last_hurt_time < 3.0 { d * 0.4 } else { d };
            if score < best.1 { best = (other.slot, score); }
        }

        if best.0 != NO_PLAYER {
            if self.target != best.0 {
                self.target = best.0;
                self.target_acquired = now;
            }
            self.can_see_target = true;
            self.target_last_seen = now;
            if let Some(t) = world.player(best.0) { self.target_last_pos = t.mv.pos; }
        } else {
            self.can_see_target = false;
            // Somebody just shot us from somewhere: go and find out where.
            if me.last_hurt_by != NO_PLAYER && now - me.last_hurt_time < 2.0 {
                if let Some(t) = world.player(me.last_hurt_by) {
                    if world.hostile(self.slot, me.last_hurt_by) {
                        self.target = me.last_hurt_by;
                        self.target_last_pos = t.mv.pos;
                        self.target_last_seen = now - 0.5;
                    }
                }
            }
            if now - self.target_last_seen > 6.0 { self.target = NO_PLAYER; }
        }
    }

    fn choose_goal(&mut self, world: &World, now: f64) {
        let Some(me) = world.player(self.slot) else { return };
        let health_frac = (me.health + me.armor) / me.max_health.max(1.0);
        let ammo = me.weapon().ammo;
        let mag = me.def().mag.max(1);

        self.goal = if self.can_see_target && self.target != NO_PLAYER {
            if ammo == 0 && me.weapon().reserve > 0 { Goal::Reload }
            else if health_frac < 0.28 && self.rng.chance(0.02) { Goal::Retreat }
            else { Goal::Engage }
        } else if ammo * 3 < mag && me.weapon().can_reload() {
            Goal::Reload
        } else if self.target != NO_PLAYER && now - self.target_last_seen < 6.0 {
            Goal::Hunt
        } else {
            Goal::Advance
        };
    }

    /// Where the bot should be heading when it is not shooting at anyone.
    fn objective_goal(&mut self, world: &World, mode: &dyn crate::modes::Mode) -> Vec3 {
        use crate::modes::ModeId;
        let Some(me) = world.player(self.slot) else { return self.goal_pos };
        let my_pos = me.mv.pos;

        match mode.id() {
            ModeId::Domination => {
                let hud = mode.hud_state(world, &crate::modes::MatchState::new(ModeId::Domination));
                let held = (0..3)
                    .filter(|i| Team::from_u8(hud[*i] & 0x7F) == me.team)
                    .count();
                // Two of three already wins the tick, so past that a bot's job
                // is to go and find the enemy. Without this, both teams sit on
                // their own flags and a match can run its whole clock with
                // nobody meeting anybody.
                let hunting = held >= 2;

                // Roughly a third of the team holds what it has and the rest
                // push. The penalty for an owned point has to be additive: a
                // multiplier leaves a bot standing on its own flag with a cost
                // of zero, which is how both teams end up camping their own
                // corner and a match runs its whole clock with nobody meeting.
                let defender = self.slot as usize % 3 == 0;
                let own_penalty = if hunting { 400.0 } else if defender { 0.0 } else { 90.0 };

                let mut best = (self.goal_pos, f32::MAX);
                for (i, obj) in world.map.domination.iter().enumerate().take(3) {
                    let owner = Team::from_u8(hud[i] & 0x7F);
                    let contested = hud[i] & 0x80 != 0;
                    let d = (obj.pos - my_pos).length();
                    let want = if contested {
                        d * 0.5                      // someone is on it: that is the fight
                    } else if owner == me.team {
                        d + own_penalty
                    } else {
                        d                            // neutral or theirs: take it
                    };
                    if want < best.1 { best = (obj.pos, want); }
                }

                if hunting {
                    if let Some(p) = self.nearest_enemy_pos(world, my_pos) {
                        if (p - my_pos).length() < best.1 { best = (p, 0.0); }
                    }
                }
                best.0
            }
            ModeId::SearchDestroy => {
                if me.carrying_bomb {
                    // The nearest site, not always the first: a carrier that
                    // walks the length of the map dies before it plants, which
                    // is why a whole match could pass with no bomb ever down.
                    world.map.bomb_sites.iter()
                        .min_by(|a, b| (a.pos - my_pos).length()
                            .total_cmp(&(b.pos - my_pos).length()))
                        .map(|o| o.pos)
                        .unwrap_or(my_pos)
                } else {
                    // Attackers converge on a site; defenders hold one. Both
                    // take a post around it rather than the exact centre: a
                    // whole team standing on one marker is neither a defence
                    // nor an attack, and it leaves half the roster motionless
                    // for the entire round.
                    let sites = &world.map.bomb_sites;
                    if sites.is_empty() { return self.wander_target(world); }
                    let i = (self.slot as usize) % sites.len();
                    let site = &sites[i];
                    let a = self.post_angle;
                    let r = site.radius + 4.0 + self.post_radius;
                    site.pos + Vec3::new(a.cos() * r, 0.0, a.sin() * r)
                }
            }
            _ => {
                // Deathmatch: head toward the noisiest part of the map, which
                // in practice means toward the nearest enemy we know about.
                self.nearest_enemy_pos(world, my_pos)
                    .unwrap_or_else(|| self.wander_target(world))
            }
        }
    }

    /// The closest living enemy, if there is one.
    fn nearest_enemy_pos(&self, world: &World, from: Vec3) -> Option<Vec3> {
        let mut best: Option<(Vec3, f32)> = None;
        for other in world.active_players() {
            if !other.alive || !world.hostile(self.slot, other.slot) { continue; }
            let d = (other.mv.pos - from).length();
            if best.is_none_or(|(_, bd)| d < bd) { best = Some((other.mv.pos, d)); }
        }
        best.map(|(p, _)| p)
    }

    fn wander_target(&mut self, world: &World) -> Vec3 {
        match world.map.nav.random_node(&mut self.rng) {
            Some(n) => world.map.nav.node(n).pos,
            None => world.player(self.slot).map(|p| p.mv.pos).unwrap_or(Vec3::ZERO),
        }
    }

    fn repath(&mut self, world: &World, to: Vec3, now: f64) {
        let Some(me) = world.player(self.slot) else { return };
        self.goal_pos = to;
        self.repath_at = now + 0.9 + self.rng.range(0.0, 0.5) as f64;
        // A budget this small used to fail on the long way round a map like
        // the shipyard, and a bot with no path simply stands there. Search
        // properly, and if there is genuinely no route, go somewhere else
        // rather than stall.
        let from = me.mv.pos;
        if !self.take_path(world, from, to) {
            let alt = self.wander_target(world);
            self.take_path(world, from, alt);
        }
    }

    fn take_path(&mut self, world: &World, from: Vec3, to: Vec3) -> bool {
        if self.finder.find(&world.map.nav, &world.map.collision, from, to, PATH_BUDGET) {
            self.path.clear();
            self.path.extend_from_slice(&self.finder.path);
            self.path_index = 0;
            true
        } else {
            self.path.clear();
            false
        }
    }

    /// The next point on the path to steer toward.
    fn steer_point(&mut self, from: Vec3) -> Option<Vec3> {
        while self.path_index < self.path.len() {
            let p = self.path[self.path_index];
            let flat = Vec3::new(p.x - from.x, 0.0, p.z - from.z).length();
            if flat < 1.1 && (p.y - from.y).abs() < 2.0 {
                self.path_index += 1;
                continue;
            }
            return Some(p);
        }
        None
    }

    /// Produces this tick's command.
    pub fn think(&mut self, world: &World, mode: &dyn crate::modes::Mode, dt: f32, now: f64) -> InputCmd {
        let Some(me) = world.player(self.slot) else { return self.cmd };
        if !me.alive {
            self.cmd.buttons = Buttons::empty();
            self.cmd.move_f = 0;
            self.cmd.move_r = 0;
            self.path.clear();
            return self.cmd;
        }

        // ------------------------------------------------- staggered thinking
        if now >= self.post_until {
            self.post_until = now + 9.0 + self.rng.range(0.0, 6.0) as f64;
            self.post_angle = self.rng.range(0.0, std::f32::consts::TAU);
            self.post_radius = self.rng.range(0.0, 7.0);
        }
        if now >= self.think_at {
            self.think_at = now + 0.10;
            self.select_target(world, now);
            self.choose_goal(world, now);
        }

        let my_pos = me.mv.pos;
        let eye = me.eye();

        // ------------------------------------------------------- destination
        let destination = match self.goal {
            Goal::Engage => {
                let t = world.player(self.target).map(|p| p.mv.pos).unwrap_or(self.target_last_pos);
                let want = self.preferred_range(world);
                let to = t - my_pos;
                let d = to.length().max(0.01);
                // Close the gap or open it, depending on the weapon.
                if d > want * 1.25 { t }
                else if d < want * 0.55 { my_pos - to / d * 6.0 }
                else { my_pos }
            }
            Goal::Hunt => self.target_last_pos,
            Goal::Retreat => {
                let t = self.target_last_pos;
                my_pos + (my_pos - t).normalize_or_zero() * 14.0
            }
            _ => {
                if now >= self.repath_at || self.path.is_empty() {
                    self.objective_goal(world, mode)
                } else {
                    self.goal_pos
                }
            }
        };

        let needs_path = (destination - self.goal_pos).length() > 3.0 || self.path.is_empty();
        if now >= self.repath_at && needs_path && (destination - my_pos).length() > 1.5 {
            self.repath(world, destination, now);
        }

        // ------------------------------------------------------- stuck check
        if now - self.stuck_at > 1.0 {
            let moved = (my_pos - self.stuck_from).length();
            if moved < 0.55 && self.goal != Goal::Engage && now > self.unstick_until {
                // Sidestep and re-path; a bot pressed into a corner is the
                // most visible failure an AI can have.
                self.unstick_until = now + 0.7;
                self.unstick_dir = if self.rng.chance(0.5) { 1.0 } else { -1.0 };
                self.repath_at = 0.0;
                self.path.clear();
            }
            self.stuck_from = my_pos;
            self.stuck_at = now;
        }

        // ---------------------------------------------------------- steering
        let steer = self.steer_point(my_pos);
        let mut move_f = 0.0f32;
        let mut move_r = 0.0f32;
        let mut buttons = Buttons::empty();

        let move_dir = match self.goal {
            Goal::Engage => {
                let t = world.player(self.target).map(|p| p.mv.pos).unwrap_or(self.target_last_pos);
                let to = t - my_pos;
                let d = to.length().max(0.01);
                let want = self.preferred_range(world);
                let radial = if d > want * 1.25 { 1.0 } else if d < want * 0.6 { -1.0 } else { 0.0 };
                Some(to.normalize_or_zero() * radial)
            }
            _ => steer.map(|p| Vec3::new(p.x - my_pos.x, 0.0, p.z - my_pos.z).normalize_or_zero()),
        };

        // ------------------------------------------------------------- aiming
        self.wander_phase += dt * 1.7;
        let aim_at = if self.target != NO_PLAYER {
            world.player(self.target).map(|t| {
                // Aim at the chest, drifting toward the head as skill rises.
                let head_bias = clampf(1.0 - self.personality.jitter * 0.5, 0.0, 1.0);
                let h = 0.55 + head_bias * 0.32;
                let lead = t.mv.vel * (self.personality.jitter * 0.06);
                t.mv.pos + Vec3::Y * (t.mv.height * h) + lead
            })
        } else {
            steer.map(|p| p + Vec3::Y * 1.5)
        };

        if let Some(point) = aim_at {
            let to = point - eye;
            if to.length_squared() > 1e-4 {
                let (want_yaw, want_pitch) = angles_from_dir(to.normalize());
                // Error grows with distance and shrinks with skill.
                let dist = to.length();
                let err = self.personality.jitter * 0.011 * (1.0 + dist * 0.014);
                let ex = (self.wander_phase * 1.31).sin() * err;
                let ey = (self.wander_phase * 0.97).cos() * err * 0.6;
                let target_yaw = want_yaw + ex;
                let target_pitch = clampf(want_pitch + ey, -1.5, 1.5);

                let rate = self.personality.turn_rate * dt;
                let dyaw = angle_delta(self.aim_yaw, target_yaw);
                self.aim_yaw += clampf(dyaw, -rate, rate);
                let dpitch = target_pitch - self.aim_pitch;
                self.aim_pitch += clampf(dpitch, -rate, rate);
            }
        }
        self.aim_yaw = self.aim_yaw.rem_euclid(std::f32::consts::TAU);
        self.aim_pitch = clampf(self.aim_pitch, -1.5, 1.5);

        // ------------------------------------------------------------ combat
        let mut wants_fire = false;
        if self.goal == Goal::Engage && self.can_see_target {
            let reacted = now - self.target_acquired >= self.personality.reaction as f64;
            if reacted {
                if let Some(t) = world.player(self.target) {
                    let to = t.mv.pos + Vec3::Y * (t.mv.height * 0.6) - eye;
                    let dist = to.length();
                    let look = crate::math::dir_from_angles(self.aim_yaw, self.aim_pitch);
                    let cos = to.normalize_or_zero().dot(look);
                    // Only shoot when actually pointed at them, scaled by range
                    // so bots do not spray at a distant pixel.
                    let need = (1.0 - (0.02 + 2.0 / dist.max(4.0)).min(0.25)).max(0.90);
                    let def = me.def();
                    let in_range = dist < def.range_far * 1.4 || def.class == WeaponClass::Sniper;
                    wants_fire = cos > need && in_range && me.weapon().ammo > 0;
                    // Aim down sights at distance; hip fire up close.
                    if dist > 14.0 && def.class != WeaponClass::Shotgun {
                        buttons.insert(Buttons::ADS);
                    }
                    // Crouch to steady a long shot now and then.
                    if dist > 30.0 && now > self.crouch_until && self.rng.chance(0.01) {
                        self.crouch_until = now + 1.6;
                    }
                    // Melee at arm's length, and always when the gun is dry:
                    // a bot that stands there clicking an empty weapon reads
                    // as broken rather than as a bot.
                    let dry = me.weapon().ammo == 0;
                    if dist < 2.4 && (dry || self.rng.chance(0.06)) {
                        buttons.insert(Buttons::MELEE);
                        wants_fire = false;
                    }
                }
            }
        }
        if wants_fire { buttons.insert(Buttons::FIRE); }

        if self.goal == Goal::Reload { buttons.insert(Buttons::RELOAD); }
        if now < self.crouch_until { buttons.insert(Buttons::CROUCH); }

        // ---------------------------------------------------------- grenades
        if self.goal == Goal::Engage
            && now > self.nade_ready_at
            && me.lethal_count > 0
            && self.rng.chance(self.personality.nade_appetite as f32 * dt * 2.0)
        {
            if let Some(t) = world.player(self.target) {
                let d = (t.mv.pos - my_pos).length();
                if (8.0..38.0).contains(&d) {
                    buttons.insert(Buttons::LETHAL);
                    self.nade_ready_at = now + 9.0;
                }
            }
        }
        // Tactical equipment. Smoke covers a retreat; a flashbang or an
        // incendiary goes in ahead of a push. Without this branch the whole
        // tactical slot was decoration: bots only ever threw lethals.
        if me.tactical_count > 0 && now > self.nade_ready_at {
            let want = match me.loadout.tactical {
                Equipment::Smoke => self.goal == Goal::Retreat,
                _ => {
                    // Flash or burn someone we know about but cannot shoot:
                    // exactly the moment a human reaches for one.
                    self.target != NO_PLAYER
                        && !self.can_see_target
                        && now - self.target_last_seen < 4.0
                        && (6.0..30.0).contains(&(self.target_last_pos - my_pos).length())
                        && self.rng.chance(self.personality.nade_appetite as f32 * dt * 3.0)
                }
            };
            if want {
                buttons.insert(Buttons::TACTICAL);
                self.nade_ready_at = now + 11.0;
            }
        }

        // ----------------------------------------------------------- movement
        if let Some(dir) = move_dir {
            if dir.length_squared() > 1e-4 {
                let (fwd, right) = crate::math::move_basis(self.aim_yaw);
                move_f = dir.dot(fwd);
                move_r = dir.dot(right);
            }
        }

        // Strafe while fighting so the bot is not a static target.
        if self.goal == Goal::Engage && self.personality.strafe > 0.05 {
            if now > self.strafe_until {
                self.strafe_until = now + self.rng.range(0.5, 1.6) as f64;
                self.strafe_dir = if self.rng.chance(0.5) { 1.0 } else { -1.0 };
            }
            move_r += self.strafe_dir * self.personality.strafe;
        }
        if now < self.unstick_until {
            move_r += self.unstick_dir * 0.9;
            move_f = move_f.max(0.35);
        }

        // Sprint when there is ground to cover and nothing to shoot.
        let far = steer.map(|p| (p - my_pos).length()).unwrap_or(0.0);
        if self.goal != Goal::Engage && far > 6.0 && move_f > 0.6 {
            buttons.insert(Buttons::SPRINT);
        }

        // Hop over things the path expects us to climb.
        if let Some(p) = steer {
            if p.y > my_pos.y + 0.55 && (p - my_pos).length() < 2.4 && now > self.jump_at {
                buttons.insert(Buttons::JUMP);
                self.jump_at = now + 0.8;
            }
        }

        // Plant or defuse when standing on the objective and asked to.
        if me.carrying_bomb || self.goal == Goal::Hold {
            buttons.insert(Buttons::USE);
        }
        if !world.map.bomb_sites.is_empty() {
            for site in world.map.bomb_sites.iter() {
                if site.contains(my_pos) { buttons.insert(Buttons::USE); }
            }
        }

        let len = (move_f * move_f + move_r * move_r).sqrt();
        if len > 1.0 { move_f /= len; move_r /= len; }

        self.cmd.seq = self.cmd.seq.wrapping_add(1);
        self.cmd.dt_ms = (dt * 1000.0).clamp(1.0, 60.0) as u8;
        self.cmd.move_f = (move_f * 127.0).clamp(-127.0, 127.0) as i8;
        self.cmd.move_r = (move_r * 127.0).clamp(-127.0, 127.0) as i8;
        self.cmd.yaw = self.aim_yaw;
        self.cmd.pitch = self.aim_pitch;
        self.cmd.buttons = buttons;
        self.cmd.weapon = self.pick_weapon(world, now);
        self.cmd.sanitize();
        self.cmd
    }

    /// Which weapon slot the bot wants this tick, or 0xFF for no change.
    ///
    /// Bots used to hard-code "no change", so a bot whose primary ran dry
    /// stood in the open reloading forever with a loaded sidearm on its hip.
    fn pick_weapon(&mut self, world: &World, now: f64) -> u8 {
        if now < self.swap_ready_at { return 0xFF; }
        let Some(me) = world.player(self.slot) else { return 0xFF };
        let cur = me.cur as usize;
        let held = &me.weapons[cur];

        let usable = |w: &crate::game::weapons::WeaponSlot| w.ammo > 0 || w.reserve > 0;

        // An empty magazine in someone's face is a pistol, not a reload: a
        // two-and-a-half second reload loses that fight every time.
        if held.ammo == 0 && self.can_see_target {
            if let Some(t) = world.player(self.target) {
                if (t.mv.pos - me.mv.pos).length() < 20.0 {
                    for (i, w) in me.weapons.iter().enumerate() {
                        if i != cur && w.ammo > 0 && w.def().class != WeaponClass::Melee {
                            self.swap_ready_at = now + 2.0;
                            return i as u8;
                        }
                    }
                }
            }
        }

        // A weapon with rounds left, or rounds to load, is fine where it is.
        if usable(held) {
            // Except at knife range with a sniper rifle, where anything else
            // is better than a scope.
            if held.def().class == WeaponClass::Sniper && self.can_see_target {
                if let Some(t) = world.player(self.target) {
                    if (t.mv.pos - me.mv.pos).length() < 6.0 {
                        for (i, w) in me.weapons.iter().enumerate() {
                            if i != cur && usable(w) && w.def().class != WeaponClass::Sniper {
                                self.swap_ready_at = now + 2.5;
                                return i as u8;
                            }
                        }
                    }
                }
            }
            return 0xFF;
        }

        // Dry: take the best of what is left, preferring a real gun.
        let mut best: Option<(usize, f32)> = None;
        for (i, w) in me.weapons.iter().enumerate() {
            if i == cur || !usable(w) { continue; }
            let score = if w.def().class == WeaponClass::Melee { 0.1 } else { w.ammo as f32 + 1.0 };
            if best.is_none_or(|(_, b)| score > b) { best = Some((i, score)); }
        }
        match best {
            Some((i, _)) => { self.swap_ready_at = now + 1.2; i as u8 }
            None => 0xFF,
        }
    }

    /// Called when the bot spawns, to reset transient state.
    pub fn on_spawn(&mut self, yaw: f32) {
        self.aim_yaw = yaw;
        self.aim_pitch = 0.0;
        self.path.clear();
        self.path_index = 0;
        self.repath_at = 0.0;
        self.target = NO_PLAYER;
        self.can_see_target = false;
        self.goal = Goal::Advance;
        let _ = Stance::Stand;
    }
}
