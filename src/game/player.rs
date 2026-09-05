//! The player entity and its weapon state machine.
//!
//! The weapon logic here runs on the server for every player and bot, and on
//! the client for the local player as part of prediction. Given the same
//! command stream it produces the same shots at the same times, which is what
//! lets the client draw a muzzle flash the instant you click without lying
//! about whether the shot happened.

use super::loadout::{Equipment, Loadout, Perk};
use super::movement::{MoveMods, MoveState, RecoilState};
use super::types::{Buttons, InputCmd, PFlags, ScoreEntry, Stance, Team};
use super::weapons::{current_spread, FireMode, WeaponDef, WeaponId, WeaponSlot};
use crate::core::{Ring, Rng};
use glam::Vec3;

/// Weapon slot indices.
pub const SLOT_PRIMARY: u8 = 0;
pub const SLOT_SECONDARY: u8 = 1;
pub const SLOT_MELEE: u8 = 2;
pub const SLOT_COUNT: usize = 3;

pub const BASE_HEALTH: f32 = 100.0;
/// Delay before health starts coming back, and the rate once it does.
pub const REGEN_DELAY: f32 = 4.5;
pub const REGEN_RATE: f32 = 32.0;
pub const SPAWN_PROTECT_TIME: f32 = 1.6;

/// What the weapon is currently doing. Only one thing at a time, which keeps
/// the transitions easy to reason about and impossible to desynchronise.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Ready,
    /// Between shots at the weapon's rate of fire, or cycling a pump/bolt.
    Cooling,
    Reloading,
    Swapping,
    Meleeing,
    /// Bringing the weapon back up after sprinting.
    Raising,
    /// Winding up a throw.
    Throwing,
}

/// A position sample kept for lag compensation.
#[derive(Copy, Clone, Debug, Default)]
pub struct HistorySample {
    pub time: f64,
    pub pos: Vec3,
    pub height: f32,
    pub alive: bool,
}

/// What the weapon did during one update. No allocation: a tick can fire at
/// most a handful of rounds even at the highest rate of fire.
#[derive(Copy, Clone, Debug, Default)]
pub struct WeaponOutput {
    pub shots: u8,
    pub melee: bool,
    pub threw: Option<Equipment>,
    /// Cook time already elapsed when the throw happened.
    pub throw_cooked: f32,
    pub started_reload: bool,
    pub finished_reload: bool,
    pub loaded_shell: bool,
    pub swapped_to: Option<u8>,
    pub dry_fire: bool,
}

pub struct Player {
    // ---------------------------------------------------------- identity
    pub slot: u8,
    pub name: String,
    pub team: Team,
    pub is_bot: bool,
    pub in_use: bool,
    /// Set once the client has finished loading and wants to play.
    pub ready: bool,
    pub ping_ms: u16,
    pub level: u8,

    // ------------------------------------------------------------ motion
    pub mv: MoveState,
    pub history: Ring<HistorySample, 32>,

    // ------------------------------------------------------------ combat
    pub alive: bool,
    pub health: f32,
    pub armor: f32,
    pub max_health: f32,
    pub loadout: Loadout,
    pub weapons: [WeaponSlot; SLOT_COUNT],
    pub cur: u8,
    pub queued_slot: u8,
    pub action: Action,
    pub action_timer: f32,
    /// Countdown to the next allowed shot.
    pub fire_timer: f32,
    pub burst_left: u8,
    /// Accumulated spread growth from sustained fire.
    pub bloom: f32,
    pub recoil: RecoilState,
    pub trigger_was_held: bool,
    /// Buttons from the most recent command, so modes can read holds like USE.
    pub last_buttons: Buttons,
    /// Shotgun reloads can be interrupted by firing; this tracks the state.
    pub reload_shells_left: u16,

    // --------------------------------------------------------- equipment
    pub lethal_count: u8,
    pub tactical_count: u8,
    pub cooking: Option<Equipment>,
    pub cook_time: f32,

    // ---------------------------------------------------------- statuses
    /// Remaining blindness from a flashbang, in seconds.
    pub flash: f32,
    /// Remaining disorientation from a concussion.
    pub concussion: f32,
    /// Burning damage-over-time remaining.
    pub burning: f32,
    pub burning_from: u8,
    pub spawn_protect: f32,
    pub regen_delay: f32,

    // ------------------------------------------------------------- match
    pub score: ScoreEntry,
    pub respawn_at: f64,
    pub last_hurt_by: u8,
    pub last_hurt_time: f64,
    /// Recent attackers, for assist credit: (slot, damage, time).
    pub assist_damage: [(u8, f32, f64); 4],
    /// Search & Destroy lives, and Gun Game progression.
    pub lives: u8,
    pub gun_rank: u8,
    pub carrying_bomb: bool,
    /// Deterministic per-player randomness, seeded from the slot.
    pub rng: Rng,
}

impl Player {
    pub fn new(slot: u8) -> Player {
        Player {
            slot,
            name: String::new(),
            team: Team::None,
            is_bot: false,
            in_use: false,
            ready: false,
            ping_ms: 0,
            level: 1,
            mv: MoveState::default(),
            history: Ring::new(),
            alive: false,
            health: 0.0,
            armor: 0.0,
            max_health: BASE_HEALTH,
            loadout: Loadout::default(),
            weapons: [WeaponSlot::default(); SLOT_COUNT],
            cur: SLOT_PRIMARY,
            queued_slot: SLOT_PRIMARY,
            action: Action::Ready,
            action_timer: 0.0,
            fire_timer: 0.0,
            burst_left: 0,
            bloom: 0.0,
            recoil: RecoilState::default(),
            trigger_was_held: false,
            last_buttons: Buttons::empty(),
            reload_shells_left: 0,
            lethal_count: 0,
            tactical_count: 0,
            cooking: None,
            cook_time: 0.0,
            flash: 0.0,
            concussion: 0.0,
            burning: 0.0,
            burning_from: super::types::NO_PLAYER,
            spawn_protect: 0.0,
            regen_delay: 0.0,
            score: ScoreEntry::default(),
            respawn_at: 0.0,
            last_hurt_by: super::types::NO_PLAYER,
            last_hurt_time: -999.0,
            assist_damage: [(super::types::NO_PLAYER, 0.0, -999.0); 4],
            lives: 1,
            gun_rank: 0,
            carrying_bomb: false,
            rng: Rng::seeded(0x51ED_0000 ^ slot as u32),
        }
    }

    #[inline]
    pub fn weapon(&self) -> &WeaponSlot { &self.weapons[self.cur as usize] }
    #[inline]
    pub fn weapon_mut(&mut self) -> &mut WeaponSlot { &mut self.weapons[self.cur as usize] }
    #[inline]
    pub fn def(&self) -> &'static WeaponDef { self.weapons[self.cur as usize].def() }
    #[inline]
    pub fn perk(&self) -> Perk { self.loadout.perk }

    /// Eye position used for shooting and line of sight.
    #[inline]
    pub fn eye(&self) -> Vec3 { self.mv.eye() }

    #[inline]
    pub fn aim_dir(&self) -> Vec3 { crate::math::dir_from_angles(self.mv.yaw, self.mv.pitch) }

    /// Hitbox for the whole body.
    #[inline]
    pub fn hitbox(&self) -> crate::math::Aabb {
        crate::math::Aabb::from_base(self.mv.pos, super::movement::tune::RADIUS, self.mv.height)
    }

    /// Head hitbox: the top fifth of the body, slightly narrower.
    #[inline]
    pub fn head_box(&self) -> crate::math::Aabb {
        let h = self.mv.height;
        let top = self.mv.pos.y + h;
        crate::math::Aabb::new(
            Vec3::new(self.mv.pos.x - 0.16, top - h * 0.175, self.mv.pos.z - 0.16),
            Vec3::new(self.mv.pos.x + 0.16, top, self.mv.pos.z + 0.16),
        )
    }

    /// Legs hitbox, for the limb damage multiplier.
    #[inline]
    pub fn legs_box(&self) -> crate::math::Aabb {
        let h = self.mv.height;
        crate::math::Aabb::new(
            Vec3::new(self.mv.pos.x - 0.30, self.mv.pos.y, self.mv.pos.z - 0.30),
            Vec3::new(self.mv.pos.x + 0.30, self.mv.pos.y + h * 0.42, self.mv.pos.z + 0.30),
        )
    }

    pub fn total_health(&self) -> f32 { self.health + self.armor }

    /// Applies the loadout: fills weapons, grenades and armour.
    pub fn equip(&mut self) {
        let l = self.loadout;
        self.weapons[SLOT_PRIMARY as usize] = WeaponSlot::new(l.primary);
        self.weapons[SLOT_SECONDARY as usize] = WeaponSlot::new(l.secondary);
        self.weapons[SLOT_MELEE as usize] = WeaponSlot::new(l.melee);
        let scale = l.perk.reserve_scale();
        for w in self.weapons.iter_mut() {
            w.reserve = (w.reserve as f32 * scale) as u16;
        }
        self.cur = SLOT_PRIMARY;
        self.queued_slot = SLOT_PRIMARY;
        self.lethal_count = l.lethal.count() + l.perk.extra_grenade();
        self.tactical_count = l.tactical.count();
        self.armor = l.perk.bonus_armor();
        self.max_health = BASE_HEALTH;
        self.health = BASE_HEALTH;
        self.action = Action::Ready;
        self.action_timer = 0.0;
        self.fire_timer = 0.0;
        self.burst_left = 0;
        self.bloom = 0.0;
        self.recoil.reset();
        self.cooking = None;
        self.cook_time = 0.0;
        self.flash = 0.0;
        self.concussion = 0.0;
        self.burning = 0.0;
        self.regen_delay = 0.0;
    }

    pub fn spawn_at(&mut self, pos: Vec3, yaw: f32, now: f64) {
        self.mv = MoveState {
            pos,
            yaw,
            stance: Stance::Stand,
            height: Stance::Stand.height(),
            ..MoveState::default()
        };
        self.alive = true;
        self.equip();
        self.spawn_protect = SPAWN_PROTECT_TIME;
        self.last_hurt_by = super::types::NO_PLAYER;
        self.assist_damage = [(super::types::NO_PLAYER, 0.0, -999.0); 4];
        self.history.clear();
        self.carrying_bomb = false;
        let _ = now;
    }

    /// Movement modifiers derived from the player's current condition.
    pub fn move_mods(&self, want_ads: bool) -> MoveMods {
        let d = self.def();
        let busy = matches!(self.action, Action::Reloading | Action::Swapping | Action::Meleeing | Action::Throwing);
        // Concussion cuts your speed; it is the whole point of the grenade.
        let conc = if self.concussion > 0.0 { 0.62 } else { 1.0 };
        MoveMods {
            weapon_scale: d.move_scale,
            ads_scale: d.ads_move_scale,
            perk_scale: self.perk().move_scale() * conc,
            block_sprint: busy || !self.alive,
            want_ads: want_ads && self.can_aim(),
            ads_time: d.ads_time * self.perk().handling_scale().max(0.8),
        }
    }

    #[inline]
    pub fn can_aim(&self) -> bool {
        self.alive
            && !matches!(self.action, Action::Swapping | Action::Meleeing | Action::Throwing)
            && !self.def().is_melee()
    }

    /// Current spread cone, in radians.
    pub fn spread(&self) -> f32 {
        let d = self.def();
        let max_speed = super::movement::tune::WALK_SPEED * d.move_scale;
        current_spread(
            d,
            self.mv.speed_fraction(max_speed),
            !self.mv.grounded,
            self.mv.ads_t,
            self.bloom,
            self.mv.stance.spread_mult(),
        ) * self.perk().spread_scale()
    }

    /// Replicated status bits.
    pub fn flags(&self) -> PFlags {
        let mut f = PFlags::empty();
        f.set(PFlags::GROUNDED, self.mv.grounded);
        f.set(PFlags::SPRINTING, self.mv.sprint_t > 0.5);
        f.set(PFlags::ADS, self.mv.ads_t > 0.5);
        f.set(PFlags::RELOADING, self.action == Action::Reloading);
        f.set(PFlags::DEAD, !self.alive);
        f.set(PFlags::MELEEING, self.action == Action::Meleeing);
        f.set(PFlags::SWITCHING, self.action == Action::Swapping);
        f.set(PFlags::FLASHED, self.flash > 0.0);
        f.set(PFlags::CARRYING, self.carrying_bomb);
        f.set(PFlags::SPAWNPROT, self.spawn_protect > 0.0);
        f
    }

    /// Records a lag-compensation sample. Called once per server tick.
    pub fn record_history(&mut self, time: f64) {
        self.history.push(HistorySample {
            time,
            pos: self.mv.pos,
            height: self.mv.height,
            alive: self.alive,
        });
    }

    /// Rewinds this player's hitbox to where it was at `time`, interpolating
    /// between the two samples that bracket it. Returns `None` when the
    /// requested time is outside the recorded window, in which case the caller
    /// should use the present position.
    pub fn rewind(&self, time: f64) -> Option<HistorySample> {
        let newest = self.history.back(0)?;
        if time >= newest.time { return Some(*newest); }
        let oldest = self.history.back(self.history.len() - 1)?;
        if time < oldest.time { return None; }
        for i in 0..self.history.len().saturating_sub(1) {
            let a = self.history.back(i)?;
            let b = self.history.back(i + 1)?;
            if time <= a.time && time >= b.time {
                let span = (a.time - b.time).max(1e-6);
                let t = ((time - b.time) / span) as f32;
                return Some(HistorySample {
                    time,
                    pos: b.pos.lerp(a.pos, t),
                    height: b.height + (a.height - b.height) * t,
                    alive: a.alive && b.alive,
                });
            }
        }
        Some(*newest)
    }

    /// Advances timers that tick regardless of input.
    pub fn update_status(&mut self, dt: f32) {
        if self.spawn_protect > 0.0 { self.spawn_protect = (self.spawn_protect - dt).max(0.0); }
        if self.flash > 0.0 { self.flash = (self.flash - dt).max(0.0); }
        if self.concussion > 0.0 { self.concussion = (self.concussion - dt).max(0.0); }
        if self.regen_delay > 0.0 { self.regen_delay = (self.regen_delay - dt).max(0.0); }
        else if self.alive && self.health < self.max_health {
            self.health = (self.health + REGEN_RATE * dt).min(self.max_health);
        }
        // Spread bloom decays toward zero whenever we are not adding to it.
        let d = self.def();
        self.bloom = (self.bloom - d.spread_recover * dt).max(0.0);
        self.recoil.update(d, dt);
    }

    /// Runs the weapon state machine for one command.
    ///
    /// Returns what happened; the caller resolves shots into damage. Nothing
    /// in here touches the world, so it is identical on client and server.
    pub fn update_weapon(&mut self, cmd: &InputCmd, dt: f32) -> WeaponOutput {
        let mut out = WeaponOutput::default();
        if !self.alive {
            self.trigger_was_held = cmd.held(Buttons::FIRE);
            return out;
        }

        let handling = self.perk().handling_scale();

        // --------------------------------------------------------- timers
        if self.fire_timer > 0.0 { self.fire_timer = (self.fire_timer - dt).max(0.0); }
        if self.action_timer > 0.0 {
            self.action_timer -= dt;
            if self.action_timer <= 0.0 {
                self.action_timer = 0.0;
                match self.action {
                    Action::Reloading => {
                        let per_round = self.def().reload_per_round;
                        if per_round {
                            if self.weapon_mut().reload_one() {
                                out.loaded_shell = true;
                            }
                            let slot = *self.weapon();
                            if slot.can_reload() {
                                // Keep feeding shells one at a time.
                                self.action_timer = self.def().reload_time * handling;
                            } else {
                                self.action = Action::Ready;
                                out.finished_reload = true;
                            }
                        } else {
                            self.weapon_mut().reload();
                            self.action = Action::Ready;
                            out.finished_reload = true;
                        }
                    }
                    Action::Swapping => {
                        self.cur = self.queued_slot;
                        self.action = Action::Raising;
                        self.action_timer = self.def().swap_in * handling;
                        out.swapped_to = Some(self.cur);
                    }
                    Action::Throwing => {
                        self.action = Action::Ready;
                    }
                    _ => self.action = Action::Ready,
                }
            }
        }
        if self.action == Action::Cooling && self.fire_timer <= 0.0 {
            self.action = Action::Ready;
        }

        // ------------------------------------------------- sprint interrupt
        // Coming out of a sprint costs a moment before the weapon is usable.
        if self.mv.sprint_t > 0.7 && self.action == Action::Ready {
            self.action = Action::Raising;
            self.action_timer = self.def().sprint_out;
        }

        // ---------------------------------------------------- weapon switch
        let want_slot = if cmd.weapon != 0xFF && (cmd.weapon as usize) < SLOT_COUNT {
            Some(cmd.weapon)
        } else if cmd.held(Buttons::NEXT_WEAP) && !self.trigger_was_held {
            Some((self.cur + 1) % SLOT_COUNT as u8)
        } else {
            None
        };
        if let Some(slot) = want_slot {
            if slot != self.cur && self.can_interrupt() {
                self.queued_slot = slot;
                self.action = Action::Swapping;
                self.action_timer = self.def().swap_out * handling;
            }
        }

        // ------------------------------------------------------ grenades
        let lethal_held = cmd.held(Buttons::LETHAL);
        let tactical_held = cmd.held(Buttons::TACTICAL);
        if let Some(kind) = self.cooking {
            self.cook_time += dt;
            let released = if kind.is_lethal() { !lethal_held } else { !tactical_held };
            // A cooked grenade that reaches its fuse goes off in your hand.
            if released || self.cook_time >= kind.fuse() {
                out.threw = Some(kind);
                out.throw_cooked = self.cook_time;
                if kind.is_lethal() { self.lethal_count = self.lethal_count.saturating_sub(1); }
                else { self.tactical_count = self.tactical_count.saturating_sub(1); }
                self.cooking = None;
                self.cook_time = 0.0;
                self.action = Action::Throwing;
                self.action_timer = 0.35;
            }
        } else if self.can_interrupt() {
            if lethal_held && self.lethal_count > 0 {
                self.cooking = Some(self.loadout.lethal);
                self.cook_time = 0.0;
            } else if tactical_held && self.tactical_count > 0 {
                self.cooking = Some(self.loadout.tactical);
                self.cook_time = 0.0;
            }
        }

        // --------------------------------------------------------- melee
        if cmd.held(Buttons::MELEE) && self.action != Action::Meleeing && self.can_interrupt() {
            self.action = Action::Meleeing;
            self.action_timer = 0.45;
            out.melee = true;
        }

        // -------------------------------------------------------- reload
        let d = self.def();
        let slot = *self.weapon();
        let wants_reload = cmd.held(Buttons::RELOAD) || (slot.is_empty() && !d.is_melee() && slot.reserve > 0);
        if wants_reload && slot.can_reload() && self.action == Action::Ready {
            self.action = Action::Reloading;
            self.action_timer = if d.reload_per_round {
                d.reload_time * handling
            } else if slot.is_empty() {
                d.reload_empty * handling
            } else {
                d.reload_time * handling
            };
            out.started_reload = true;
        }

        // ---------------------------------------------------------- fire
        let trigger = cmd.held(Buttons::FIRE);
        let fresh_pull = trigger && !self.trigger_was_held;
        let d = self.def();

        // Firing cancels a shell-by-shell reload, which is what makes pump
        // shotguns feel responsive rather than committal.
        if self.action == Action::Reloading && d.reload_per_round && fresh_pull && !self.weapon().is_empty() {
            self.action = Action::Ready;
            self.action_timer = 0.0;
        }

        if self.burst_left > 0 && self.action == Action::Ready && self.fire_timer <= 0.0 {
            if self.consume_round() {
                self.burst_left -= 1;
                out.shots += 1;
                self.after_shot(d);
                if self.burst_left == 0 { self.fire_timer += d.cycle_time; }
            } else {
                self.burst_left = 0;
            }
        } else if self.action == Action::Ready && self.fire_timer <= 0.0 && !d.is_melee() {
            let may_fire = match d.fire_mode {
                FireMode::Auto => trigger,
                FireMode::Semi | FireMode::Pump | FireMode::Bolt | FireMode::Burst(_) => fresh_pull,
                FireMode::Melee => false,
            };
            if may_fire {
                if self.weapon().is_empty() {
                    if fresh_pull { out.dry_fire = true; }
                } else if self.consume_round() {
                    out.shots += 1;
                    self.after_shot(d);
                    match d.fire_mode {
                        FireMode::Burst(n) => self.burst_left = n.saturating_sub(1),
                        FireMode::Pump | FireMode::Bolt => self.fire_timer += d.cycle_time,
                        _ => {}
                    }
                }
            }
        }

        self.trigger_was_held = trigger;
        out
    }

    /// True when the current action can be abandoned for something else.
    #[inline]
    fn can_interrupt(&self) -> bool {
        matches!(self.action, Action::Ready | Action::Cooling | Action::Reloading | Action::Raising)
    }

    fn consume_round(&mut self) -> bool {
        let w = self.weapon_mut();
        if w.ammo == 0 { return false; }
        w.ammo -= 1;
        true
    }

    fn after_shot(&mut self, d: &'static WeaponDef) {
        self.fire_timer += d.shot_interval();
        self.action = Action::Cooling;
        self.bloom = (self.bloom + d.spread_per_shot).min(d.spread_max);
        let ads = self.mv.ads_t;
        let scale = self.perk().recoil_scale();
        // Recoil is applied through the shared state so bots climb too.
        let mut rng = self.rng.clone();
        self.recoil.kick(d, &mut rng, ads);
        self.recoil.pitch_kick *= scale;
        self.recoil.yaw_kick *= scale;
        self.rng = rng;
    }

    /// Applies damage. Returns the amount actually taken, after armour.
    pub fn take_damage(&mut self, amount: f32, from: u8, now: f64) -> f32 {
        if !self.alive || amount <= 0.0 { return 0.0; }
        if self.spawn_protect > 0.0 && from != self.slot { return 0.0; }

        let mut left = amount;
        let absorbed = left.min(self.armor);
        self.armor -= absorbed;
        left -= absorbed;
        let to_health = left.min(self.health);
        self.health -= to_health;

        self.regen_delay = REGEN_DELAY;
        if from != self.slot && from != super::types::NO_PLAYER {
            self.last_hurt_by = from;
            self.last_hurt_time = now;
            self.credit_assist(from, amount, now);
        }
        if self.health <= 0.0 {
            self.health = 0.0;
        }
        absorbed + to_health
    }

    fn credit_assist(&mut self, from: u8, amount: f32, now: f64) {
        // Keep the four most recent contributors; anything older than ten
        // seconds is no longer an assist.
        if let Some(e) = self.assist_damage.iter_mut().find(|e| e.0 == from) {
            e.1 += amount;
            e.2 = now;
            return;
        }
        let mut oldest = 0usize;
        for i in 1..self.assist_damage.len() {
            if self.assist_damage[i].2 < self.assist_damage[oldest].2 { oldest = i; }
        }
        self.assist_damage[oldest] = (from, amount, now);
    }

    /// Everyone who damaged this player recently other than the killer.
    pub fn assisters(&self, killer: u8, now: f64) -> impl Iterator<Item = u8> + '_ {
        self.assist_damage.iter()
            .filter(move |(s, dmg, t)| {
                *s != super::types::NO_PLAYER && *s != killer && *dmg >= 15.0 && now - *t < 10.0
            })
            .map(|(s, _, _)| *s)
    }

    pub fn reset_for_round(&mut self) {
        self.alive = false;
        self.health = 0.0;
        self.armor = 0.0;
        self.respawn_at = 0.0;
        self.carrying_bomb = false;
        self.recoil.reset();
    }

    /// Weapon the player should be holding in Gun Game, given their rank.
    pub fn gun_game_weapon(rank: u8, ladder: &[WeaponId]) -> WeaponId {
        let i = (rank as usize).min(ladder.len().saturating_sub(1));
        ladder.get(i).copied().unwrap_or(WeaponId::SidearmP9)
    }
}
