//! In-game update: sampling input, driving prediction, and turning the
//! server's events into sound and effects.

use super::{App, Screen, TextTarget};
use crate::assets::materials::Surface;
use crate::game::events::{DeathCause, GameEvent, ImpactKind};
use crate::game::types::{Buttons, HitZone, InputCmd, Stance, Team};
use crate::input::Action;
use crate::modes::Phase;
use crate::ui::widgets::UiSound;
use glam::Vec3;

impl App {
    /// One frame of play.
    pub(super) fn update_game(&mut self, dt: f32, now: f64) {
        let has_map = self.map.is_some();
        if self.client.is_none() || !has_map {
            self.goto(Screen::MainMenu);
            return;
        }

        self.handle_game_keys(now);
        self.capture_mouse(!self.hud.chat_open);

        if self.hud.chat_open {
            self.update_chat_entry();
        } else {
            self.apply_look(dt);
        }

        let cmd = self.build_command(dt);
        self.run_local_frame(cmd, dt);
        self.consume_events(now, true);
        self.update_world_and_audio(dt, now);
        self.check_phase(now);
    }

    fn handle_game_keys(&mut self, _now: f64) {
        let b = self.settings.bindings.clone();
        if self.input.pressed(&b, Action::Pause) {
            if self.hud.chat_open {
                self.hud.chat_open = false;
                self.hud.chat_buffer.clear();
                self.text_target = None;
            } else {
                self.goto(Screen::Paused);
                self.play_ui(UiSound::Back);
            }
            return;
        }
        if !self.hud.chat_open {
            if self.input.pressed(&b, Action::Chat) {
                self.hud.chat_open = true;
                self.hud.chat_team = false;
                self.hud.chat_buffer.clear();
                self.text_target = Some(TextTarget::Chat);
            }
            if self.input.pressed(&b, Action::TeamChat) {
                self.hud.chat_open = true;
                self.hud.chat_team = true;
                self.hud.chat_buffer.clear();
                self.text_target = Some(TextTarget::Chat);
            }
            if self.input.pressed(&b, Action::ToggleStats) {
                self.show_perf = !self.show_perf;
            }
        }
        self.hud.scoreboard_open = self.input.held(&b, Action::Scoreboard);
    }

    fn update_chat_entry(&mut self) {
        for c in self.input.typed.clone().chars() {
            if self.hud.chat_buffer.chars().count() < 100 {
                self.hud.chat_buffer.push(c);
            }
        }
        if self.input.backspace { self.hud.chat_buffer.pop(); }
        if self.input.enter {
            let text = self.hud.chat_buffer.clone();
            let team = self.hud.chat_team;
            if let Some(c) = &mut self.client { c.say(&text, team); }
            self.hud.chat_buffer.clear();
            self.hud.chat_open = false;
            self.text_target = None;
        }
    }

    fn apply_look(&mut self, dt: f32) {
        let sens = self.settings.sensitivity * 0.00042;
        let ads = self.client.as_ref().map(|c| c.local.mv.ads_t).unwrap_or(0.0);
        // Aiming slows the mouse in proportion to the zoom, which is what
        // keeps a scoped weapon usable.
        let ads_scale = 1.0 + (self.settings.ads_sensitivity - 1.0) * ads;
        let dx = self.input.mouse_dx * sens * ads_scale;
        let dy = self.input.mouse_dy * sens * ads_scale * if self.settings.invert_y { -1.0 } else { 1.0 };

        self.yaw -= dx;
        self.pitch -= dy;
        self.yaw = self.yaw.rem_euclid(std::f32::consts::TAU);
        self.pitch = self.pitch.clamp(-1.53, 1.53);

        // Recoil recovery pulls the view back toward where the player was
        // pointing, which is handled inside the shared recoil state.
        let _ = dt;
    }

    /// A do-nothing command carrying only the current view angles.
    ///
    /// Menus opened during a match still have to talk to the server. A client
    /// that stops sending is a client the server drops after ten seconds, so
    /// standing in the loadout screen used to end the match with "connection
    /// lost". The player stands still while the menu is open, which is the
    /// honest behaviour: the world does not pause for one person.
    fn idle_command(&mut self, dt: f32) -> InputCmd {
        let mut cmd = InputCmd {
            seq: 0,
            dt_ms: (dt * 1000.0).clamp(1.0, 60.0) as u8,
            move_f: 0,
            move_r: 0,
            yaw: self.yaw,
            pitch: self.pitch.clamp(-1.53, 1.53),
            buttons: Buttons::empty(),
            weapon: 0xFF,
        };
        cmd.sanitize();
        cmd
    }

    /// One frame of a screen that sits on top of a live match.
    pub(super) fn tick_connected_menu(&mut self, dt: f32, now: f64) {
        self.consume_events(now, false);
        self.capture_mouse(false);
        if self.client.is_none() { return; }
        let cmd = self.idle_command(dt);
        self.run_local_frame(cmd, dt);
    }

    fn build_command(&mut self, dt: f32) -> InputCmd {
        let b = self.settings.bindings.clone();
        let mut buttons = Buttons::empty();
        let mut f = 0.0f32;
        let mut r = 0.0f32;

        if !self.hud.chat_open {
            if self.input.held(&b, Action::MoveForward) { f += 1.0; }
            if self.input.held(&b, Action::MoveBack) { f -= 1.0; }
            if self.input.held(&b, Action::MoveRight) { r += 1.0; }
            if self.input.held(&b, Action::MoveLeft) { r -= 1.0; }

            if self.input.held(&b, Action::Fire) { buttons.insert(Buttons::FIRE); }
            if self.settings.toggle_ads {
                if self.input.pressed(&b, Action::Aim) { self.toggle_ads_state = !self.toggle_ads_state; }
                if self.toggle_ads_state { buttons.insert(Buttons::ADS); }
            } else if self.input.held(&b, Action::Aim) {
                buttons.insert(Buttons::ADS);
            }
            if self.input.held(&b, Action::Jump) { buttons.insert(Buttons::JUMP); }
            if self.settings.toggle_crouch {
                if self.input.pressed(&b, Action::Crouch) { self.toggle_crouch_state = !self.toggle_crouch_state; }
                if self.toggle_crouch_state { buttons.insert(Buttons::CROUCH); }
            } else if self.input.held(&b, Action::Crouch) {
                buttons.insert(Buttons::CROUCH);
            }
            if self.input.held(&b, Action::Prone) { buttons.insert(Buttons::PRONE); }
            let sprinting = self.input.held(&b, Action::Sprint) || (self.settings.auto_sprint && f > 0.5);
            if sprinting { buttons.insert(Buttons::SPRINT); }
            if self.input.held(&b, Action::Reload) { buttons.insert(Buttons::RELOAD); }
            if self.input.held(&b, Action::Melee) { buttons.insert(Buttons::MELEE); }
            if self.input.held(&b, Action::Lethal) { buttons.insert(Buttons::LETHAL); }
            if self.input.held(&b, Action::Tactical) { buttons.insert(Buttons::TACTICAL); }
            if self.input.held(&b, Action::Use) { buttons.insert(Buttons::USE); }
            if self.input.pressed(&b, Action::Respawn) { buttons.insert(Buttons::RESPAWN); }
        }

        let mut weapon = 0xFFu8;
        if !self.hud.chat_open {
            if self.input.pressed(&b, Action::Weapon1) { weapon = 0; }
            if self.input.pressed(&b, Action::Weapon2) { weapon = 1; }
            if self.input.pressed(&b, Action::Weapon3) { weapon = 2; }
            if self.input.pressed(&b, Action::NextWeapon) { buttons.insert(Buttons::NEXT_WEAP); }
            if self.input.pressed(&b, Action::PrevWeapon) { buttons.insert(Buttons::PREV_WEAP); }
        }

        // Aim sent to the server includes recoil, because that is genuinely
        // where the barrel is pointing.
        let (recoil_pitch, recoil_yaw) = self.client.as_ref()
            .map(|c| (c.local.recoil.pitch_kick, c.local.recoil.yaw_kick))
            .unwrap_or((0.0, 0.0));

        let mut cmd = InputCmd {
            seq: 0,
            dt_ms: (dt * 1000.0).clamp(1.0, 60.0) as u8,
            move_f: (f.clamp(-1.0, 1.0) * 127.0) as i8,
            move_r: (r.clamp(-1.0, 1.0) * 127.0) as i8,
            yaw: self.yaw + recoil_yaw,
            pitch: (self.pitch + recoil_pitch).clamp(-1.53, 1.53),
            buttons,
            weapon,
        };
        cmd.sanitize();
        cmd
    }

    fn run_local_frame(&mut self, cmd: InputCmd, dt: f32) {
        let Some(map) = &self.map else { return };
        let bounds = map.bounds;
        let collision = &map.collision;

        let Some(client) = &mut self.client else { return };
        let alive_before = client.local.alive;
        client.push_command(cmd, collision, &bounds);
        client.reconcile(collision, &bounds);
        client.send();
        client.smooth_error(dt);
        client.interpolate(dt);

        // Everything the local weapon did this frame becomes feedback now,
        // not when the server confirms it: that is the whole point of
        // predicting the weapon as well as the movement.
        let out = client.last_output;
        let weapon = client.local.weapon().id;
        let def = weapon.def();
        let ads = client.local.mv.ads_t;
        let ammo_left = client.local.weapon().ammo;
        let speed = client.local.mv.horizontal_speed();
        let sprinting = client.local.mv.sprint_t > 0.5;
        let alive = client.local.alive;

        let camera = self.camera;
        self.viewmodel.update(
            dt,
            self.input.mouse_dx,
            self.input.mouse_dy,
            speed,
            ads > 0.5,
            sprinting,
            self.settings.view_bob,
        );

        if out.shots > 0 {
            let scale = def.flash_scale;
            let muzzle = self.viewmodel.muzzle_world;
            let dir = self.viewmodel.muzzle_dir;
            for _ in 0..out.shots {
                self.viewmodel.kick(0.35 + def.recoil_up * 22.0);
                self.effects.muzzle_flash(muzzle, dir, scale);
                if def.ejects_shells {
                    let (p, right, up) = self.viewmodel.ejection_point(&camera);
                    self.effects.eject_shell(p, right, up);
                }
                self.shake = (self.shake + 0.12 + def.recoil_up * 6.0).min(1.2);
                if let Some(bank) = self.audio.bank.clone() {
                    let clip = bank.shot(weapon);
                    let action = bank.action(weapon);
                    let rate = self.audio.vary(0.04);
                    self.audio.play_ui(clip, 0.9, rate);
                    if def.ejects_shells {
                        self.audio.play_ui(action, 0.35, 1.0);
                    }
                }
            }
            self.progression.record_shot(weapon, out.shots as u32);
        }
        if out.dry_fire {
            if let Some(bank) = self.audio.bank.clone() {
                let c = bank.dry_fire.clone();
                self.audio.play_ui(c, 0.5, 1.0);
            }
        }
        if out.started_reload {
            let empty = ammo_left == 0;
            let dur = if empty { def.reload_empty } else { def.reload_time };
            self.viewmodel.start_reload(dur);
            if let Some(bank) = self.audio.bank.clone() {
                let c = bank.reload[0].clone();
                self.audio.play_ui(c, 0.6, 1.0);
            }
        }
        if out.finished_reload || out.loaded_shell {
            if let Some(bank) = self.audio.bank.clone() {
                let c = bank.reload[if out.loaded_shell { 2 } else { 1 }].clone();
                self.audio.play_ui(c, 0.6, 1.0);
            }
        }
        if let Some(slot) = out.swapped_to {
            let _ = slot;
            self.viewmodel.start_swap(def.swap_in);
        }
        if out.melee {
            if let Some(bank) = self.audio.bank.clone() {
                let c = bank.melee_swing.clone();
                self.audio.play_ui(c, 0.5, 1.2);
            }
        }
        if out.threw.is_some() {
            if let Some(bank) = self.audio.bank.clone() {
                let c = bank.whoosh.clone();
                self.audio.play_ui(c, 0.5, 1.0);
            }
        }

        if !alive && alive_before {
            self.damage_flash = 1.0;
            self.shake = 1.0;
        }

        // Asking to respawn while dead.
        if !alive {
            let b = self.settings.bindings.clone();
            if self.input.pressed(&b, Action::Respawn) || self.input.pressed(&b, Action::Fire) {
                if let Some(c) = &mut self.client { c.request_respawn(); }
            }
        }
    }

    /// Applies everything the server told us happened.
    pub(super) fn consume_events(&mut self, now: f64, in_game: bool) {
        let Some(client) = &mut self.client else { return };
        let events = client.take_events();
        if events.is_empty() { return; }

        let me = client.slot;
        let my_team = client.my_team();
        let my_pos = client.local.mv.pos;

        for e in events {
            match e {
                GameEvent::Shot { player, weapon, origin, dir, .. } => {
                    if player == me { continue; }
                    if let Some(c) = &mut self.client { c.note_fired(player); }
                    let def = weapon.def();
                    let muzzle = super::world_view::remote_muzzle(
                        self.client.as_ref().unwrap(), player,
                    ).map(|(p, _)| p).unwrap_or(origin);
                    self.effects.muzzle_flash(muzzle, dir, def.flash_scale);
                    // A visible tracer for a fraction of shots, which is what
                    // lets a player read where fire is coming from.
                    if def.pellets == 1 {
                        let end = muzzle + dir * 60.0;
                        self.effects.tracer(muzzle, end, [1.0, 0.82, 0.45, 0.75]);
                    }
                    if let Some(bank) = self.audio.bank.clone() {
                        let rate = self.audio.vary(0.05);
                        let clip = bank.shot(weapon);
                        self.audio.play_at(clip, muzzle, 1.0, rate);
                    }
                }
                GameEvent::Impact { pos, normal, surface, kind } => {
                    self.effects.bullet_impact(pos, normal, surface);
                    if kind != ImpactKind::Pellet || self.effects.density > 0.5 {
                        if let Some(bank) = self.audio.bank.clone() {
                            let v = self.audio.random_variant(3);
                            let rate = self.audio.vary(0.08);
                            let clip = bank.impact(surface, v);
                            self.audio.play_at(clip, pos, 0.65, rate);
                        }
                    }
                }
                GameEvent::HitPlayer { attacker, victim, pos, zone, damage, lethal } => {
                    if std::env::var_os("HARDPOINT_TRACE").is_some() {
                        eprintln!("[hit] attacker={} victim={} me={} dmg={} lethal={}", attacker, victim, me, damage, lethal);
                    }
                    let dir = if victim == me { (pos - my_pos).normalize_or_zero() } else { Vec3::Y };
                    self.effects.blood(pos, dir);
                    if attacker == me {
                        self.hud.hit_marker(zone, lethal);
                        self.hud.damage_number(pos, damage, lethal);
                        self.progression.record_hit(self.client.as_ref().unwrap().local.weapon().id);
                        if let Some(bank) = self.audio.bank.clone() {
                            let clip = if lethal {
                                bank.hitmarker_kill.clone()
                            } else if zone == HitZone::Head {
                                bank.headshot.clone()
                            } else {
                                bank.hitmarker.clone()
                            };
                            self.audio.play_ui(clip, 0.6, 1.0);
                        }
                    }
                    if victim == me {
                        self.damage_flash = (self.damage_flash + damage as f32 / 90.0).min(1.0);
                        let from = self.client.as_ref()
                            .and_then(|c| c.players.get(attacker as usize))
                            .map(|p| p.render_pos)
                            .unwrap_or(pos);
                        self.hud.damage_from((from - my_pos).normalize_or_zero(), damage as f32 / 60.0);
                        self.shake = (self.shake + damage as f32 / 120.0).min(1.0);
                    }
                }
                GameEvent::Kill { killer, victim, weapon, cause, distance, .. } => {
                    let (kn, kt) = self.name_and_team(killer);
                    let (vn, vt) = self.name_and_team(victim);
                    self.hud.push_kill(super::hud::KillEntry {
                        killer: kn,
                        victim: vn,
                        killer_team: kt,
                        victim_team: vt,
                        weapon,
                        cause,
                        time: now,
                        involves_me: killer == me || victim == me,
                    });
                    if killer == me && victim != me {
                        self.progression.record_kill(
                            weapon,
                            cause == DeathCause::Headshot,
                            distance,
                            cause == DeathCause::Melee,
                            cause == DeathCause::Explosion || cause == DeathCause::Fire,
                        );
                    }
                    if victim == me {
                        self.progression.record_death();
                        if let Some(bank) = self.audio.bank.clone() {
                            let c = bank.death.clone();
                            self.audio.play_ui(c, 0.7, 1.0);
                        }
                    }
                }
                GameEvent::Explosion { pos, kind, radius } => {
                    self.effects.explosion(pos, radius.max(1.0));
                    let dist = (pos - my_pos).length();
                    self.shake = (self.shake + (1.0 - (dist / 30.0).clamp(0.0, 1.0)) * 1.1).min(1.6);
                    if let Some(bank) = self.audio.bank.clone() {
                        let big = kind == crate::game::loadout::Equipment::Frag;
                        let clip = if big { bank.explosion.clone() } else { bank.explosion_small.clone() };
                        let rate = self.audio.vary(0.05);
                        self.audio.play_at(clip, pos, 1.0, rate);
                    }
                }
                GameEvent::Blinded { player, strength, concussion } => {
                    if player == me {
                        self.flash_blind = (self.flash_blind + strength * 0.35).min(1.4);
                        if !concussion {
                            if let Some(bank) = self.audio.bank.clone() {
                                let c = bank.flash.clone();
                                self.audio.play_ui(c, 0.8, 1.0);
                            }
                        }
                    }
                }
                GameEvent::GrenadeThrown { id, player, kind, pos, vel } => {
                    self.world.throw(id, kind, pos, vel);
                    if player != me {
                        if let Some(bank) = self.audio.bank.clone() {
                            let c = bank.whoosh.clone();
                            self.audio.play_at(c, pos, 0.6, 1.0);
                        }
                    }
                }
                GameEvent::GrenadeBounce { pos, .. } => {
                    if let Some(bank) = self.audio.bank.clone() {
                        let c = bank.bounce.clone();
                        let rate = self.audio.vary(0.1);
                        self.audio.play_at(c, pos, 0.55, rate);
                    }
                }
                GameEvent::SmokeStarted { id, pos } => {
                    self.world.add_smoke(id, pos);
                    self.world.remove_grenade(id);
                }
                GameEvent::FireStarted { id, pos, radius } => {
                    self.world.add_fire(id, pos, radius);
                    self.world.remove_grenade(id);
                }
                GameEvent::Footstep { player, pos, surface, volume } => {
                    if player == me { continue; }
                    if let Some(bank) = self.audio.bank.clone() {
                        let v = self.audio.random_variant(3);
                        let rate = self.audio.vary(0.10);
                        let clip = bank.footstep(surface, v);
                        self.audio.play_at(clip, pos, volume * 0.8, rate);
                    }
                }
                GameEvent::Land { player, pos, speed, surface } => {
                    self.effects.landing_dust(pos, (speed / 14.0).clamp(0.2, 1.4));
                    if let Some(bank) = self.audio.bank.clone() {
                        let v = self.audio.random_variant(3);
                        let clip = bank.footstep(surface, v);
                        self.audio.play_at(clip, pos, 1.0, 0.75);
                    }
                    if player == me {
                        self.shake = (self.shake + (speed / 30.0).clamp(0.0, 0.6)).min(1.0);
                    }
                }
                GameEvent::Spawned { player, pos } => {
                    if player == me {
                        self.viewmodel = super::world_view::ViewModel::default();
                        self.damage_flash = 0.0;
                        self.flash_blind = 0.0;
                        if let Some(bank) = self.audio.bank.clone() {
                            let c = bank.spawn.clone();
                            self.audio.play_ui(c, 0.5, 1.0);
                        }
                    }
                    let _ = pos;
                }
                GameEvent::PickupTaken { player, index, kind } => {
                    self.world.take_pickup(index);
                    if player == me {
                        self.hud.pickup_text = Some((format!("{} ACQUIRED", kind.label()), now));
                        if let Some(bank) = self.audio.bank.clone() {
                            let c = bank.pickup.clone();
                            self.audio.play_ui(c, 0.6, 1.0);
                        }
                    }
                }
                GameEvent::PickupRespawned { index } => self.world.respawn_pickup(index),
                GameEvent::Announce { line } => {
                    self.hud.announce = Some((line, now));
                    if let Some(bank) = self.audio.bank.clone() {
                        let c = bank.announce(line);
                        self.audio.play_voice(c, 0.9);
                    }
                }
                GameEvent::CapturePoint { team, .. } => {
                    if team == my_team && team != Team::None {
                        if let Some(bank) = self.audio.bank.clone() {
                            let c = bank.ui_select.clone();
                            self.audio.play_ui(c, 0.5, 0.8);
                        }
                    }
                }
                GameEvent::BombPlanted { .. } | GameEvent::BombDefused { .. } => {
                    if let Some(bank) = self.audio.bank.clone() {
                        let c = bank.ui_select.clone();
                        self.audio.play_ui(c, 0.7, 0.7);
                    }
                }
                GameEvent::Melee { player, hit } => {
                    if player == me { continue; }
                    if let Some(bank) = self.audio.bank.clone() {
                        let c = if hit { bank.melee_hit.clone() } else { bank.melee_swing.clone() };
                        if let Some(p) = self.client.as_ref().and_then(|c| c.players.get(player as usize)) {
                            self.audio.play_at(c, p.render_pos, 0.7, 1.0);
                        }
                    }
                }
                GameEvent::Reload { player, .. } => {
                    if player == me { continue; }
                    if let Some(bank) = self.audio.bank.clone() {
                        let c = bank.reload[0].clone();
                        if let Some(p) = self.client.as_ref().and_then(|c| c.players.get(player as usize)) {
                            self.audio.play_at(c, p.render_pos, 0.5, 1.0);
                        }
                    }
                }
                _ => {}
            }
        }
        let _ = in_game;
    }

    fn name_and_team(&self, slot: u8) -> (String, Team) {
        match self.client.as_ref().and_then(|c| c.player_info(slot)) {
            Some(p) => (p.name.clone(), p.team),
            None => ("WORLD".to_string(), Team::None),
        }
    }

    fn update_world_and_audio(&mut self, dt: f32, _now: f64) {
        let listener = self.camera.position;
        if let Some(map) = &self.map {
            let mut effects = std::mem::replace(&mut self.effects, super::effects::Effects::new());
            self.world.update(dt, map, &mut effects);
            effects.update(dt, Some(&map.collision), listener);
            self.effects = effects;
            self.effects.set_weather(map.env.weather, listener);
        }

        let dir = crate::math::dir_from_angles(self.camera.yaw, self.camera.pitch);
        let right = dir.cross(Vec3::Y).normalize_or_zero();
        self.audio.listener = crate::audio::Listener {
            position: listener,
            forward: dir,
            right,
        };
    }

    fn check_phase(&mut self, now: f64) {
        let Some((phase, won)) = self.client.as_ref().map(|c| {
            (
                Phase::from_u8(c.match_info.phase),
                c.match_info.winner == c.my_team() && c.match_info.winner != Team::None,
            )
        }) else { return };

        if phase == Phase::MatchOver && self.screen == Screen::InGame {
            self.finish_match_stats();
            self.goto(Screen::Results);
            self.audio.play_music(if won {
                crate::maps::MusicTrack::Victory
            } else {
                crate::maps::MusicTrack::Defeat
            });
        }
        if phase != Phase::MatchOver {
            self.results_shown = false;
        }
        let _ = now;
    }

    /// The stance the local player is in, for the HUD.
    pub fn local_stance(&self) -> Stance {
        self.client.as_ref().map(|c| c.local.mv.stance).unwrap_or(Stance::Stand)
    }

    /// The surface under the local player, used for footstep audio.
    pub fn local_surface(&self) -> Surface {
        let Some(client) = &self.client else { return Surface::Concrete };
        let Some(map) = &self.map else { return Surface::Concrete };
        map.collision.material_at(client.local.mv.ground_brush, Vec3::Y).surface()
    }
}
