//! Objective modes: Domination and Search & Destroy.

use super::{KillInfo, MatchState, Mode, ModeId, Phase};
use crate::game::events::{AnnounceLine, GameEvent};
use crate::game::sim::{RespawnPolicy, World};
use crate::game::types::{Buttons, Team, NO_PLAYER};
use glam::Vec3;

// ================================================================ domination

/// Capture progress runs from -1 (fully Phantom) through 0 to +1 (Vanguard),
/// which makes flipping a point a two-stage job rather than an instant swap.
#[derive(Copy, Clone, Debug)]
pub struct CapturePoint {
    pub owner: Team,
    pub progress: f32,
    pub contested: bool,
    /// Team currently pushing the progress bar.
    pub capturing: Team,
}

impl Default for CapturePoint {
    fn default() -> Self {
        CapturePoint { owner: Team::None, progress: 0.0, contested: false, capturing: Team::None }
    }
}

pub struct Domination {
    pub points: [CapturePoint; 3],
    /// Seconds until the next score tick.
    tick_accum: f32,
}

impl Domination {
    pub const CAPTURE_TIME: f32 = 7.0;
    pub const SCORE_INTERVAL: f32 = 5.0;

    pub fn new() -> Domination {
        Domination { points: [CapturePoint::default(); 3], tick_accum: 0.0 }
    }
}

impl Mode for Domination {
    fn id(&self) -> ModeId { ModeId::Domination }

    fn begin_match(&mut self, world: &mut World, state: &mut MatchState) {
        world.team_game = true;
        world.respawn = RespawnPolicy::Auto { delay: 6.0 };
        state.scores = [0; 4];
        self.points = [CapturePoint::default(); 3];
        // The outer points start owned, so the middle is the contested one.
        if world.map.domination.len() == 3 {
            self.points[0].owner = Team::Phantom;
            self.points[0].progress = -1.0;
            self.points[2].owner = Team::Vanguard;
            self.points[2].progress = 1.0;
        }
    }

    fn begin_round(&mut self, world: &mut World, state: &mut MatchState) {
        state.clock = state.time_limit as f32;
        for i in 0..world.players.len() {
            if world.players[i].in_use {
                world.players[i].score.reset_match();
                world.respawn_player(i as u8, true);
            }
        }
    }

    fn update(&mut self, world: &mut World, state: &mut MatchState, dt: f32) {
        if state.phase != Phase::Live { return; }
        state.clock -= dt;

        // The objective list is copied out first so the loop can mutate the
        // players it finds without fighting the borrow of the map.
        let objectives: Vec<crate::game::types::Objective> =
            world.map.domination.iter().take(3).cloned().collect();
        for (i, obj_copy) in objectives.into_iter().enumerate() {
            let obj = &obj_copy;
            let mut phantom = 0i32;
            let mut vanguard = 0i32;
            for p in world.players.iter() {
                if !p.in_use || !p.alive { continue; }
                if !obj.contains(p.mv.pos) { continue; }
                match p.team {
                    Team::Phantom => phantom += 1,
                    Team::Vanguard => vanguard += 1,
                    _ => {}
                }
            }

            let pt = &mut self.points[i];
            let was_owner = pt.owner;
            pt.contested = phantom > 0 && vanguard > 0;

            if pt.contested || (phantom == 0 && vanguard == 0) {
                pt.capturing = Team::None;
                continue;
            }

            // Extra bodies help, but with diminishing returns, so stacking a
            // point is worth doing and not worth doing with the whole team.
            let (team, count) = if phantom > 0 { (Team::Phantom, phantom) } else { (Team::Vanguard, vanguard) };
            let rate = (1.0 + (count.min(3) as f32 - 1.0) * 0.45) / Self::CAPTURE_TIME;
            let dir = if team == Team::Phantom { -1.0 } else { 1.0 };
            pt.capturing = team;
            pt.progress = (pt.progress + dir * rate * dt).clamp(-1.0, 1.0);

            let new_owner = if pt.progress <= -0.999 { Team::Phantom }
                            else if pt.progress >= 0.999 { Team::Vanguard }
                            else if pt.progress.abs() < 0.001 { Team::None }
                            else { pt.owner };

            // Losing a point the moment the bar leaves your end keeps the
            // feedback immediate rather than waiting for a full flip.
            let owner = if (pt.owner == Team::Phantom && pt.progress > -0.999)
                || (pt.owner == Team::Vanguard && pt.progress < 0.999) {
                if new_owner == Team::None || new_owner != pt.owner { Team::None } else { pt.owner }
            } else {
                new_owner
            };

            if owner != was_owner {
                pt.owner = owner;
                world.events.push(GameEvent::CapturePoint { point: i as u8, team: owner, contested: false });
                if owner != Team::None {
                    world.events.push(GameEvent::Announce { line: AnnounceLine::PointCaptured });
                    // Everyone standing on it when it flips gets the credit.
                    for p in world.players.iter_mut() {
                        if p.in_use && p.alive && p.team == owner && obj_copy.contains(p.mv.pos) {
                            p.score.captures += 1;
                            p.score.score += 200;
                        }
                    }
                }
            }
            let progress = ((pt.progress.abs()) * 255.0) as u8;
            world.events.push(GameEvent::CaptureProgress { point: i as u8, team: pt.capturing, progress });
        }

        // Score ticks for held points.
        self.tick_accum += dt;
        if self.tick_accum >= Self::SCORE_INTERVAL {
            self.tick_accum -= Self::SCORE_INTERVAL;
            for t in Team::PLAYING {
                let held = self.points.iter().filter(|p| p.owner == t).count() as u16;
                if held > 0 {
                    state.add_score(t, held);
                    world.events.push(GameEvent::ScoreChanged { team: t, score: state.score(t) });
                }
            }
        }
    }

    fn on_kill(&mut self, world: &mut World, state: &mut MatchState, info: KillInfo) {
        if state.phase != Phase::Live || info.suicide || info.friendly { return; }
        // Defending a point you own is worth more than a kill in the open.
        let victim_pos = match world.player(info.victim) { Some(p) => p.mv.pos, None => return };
        let killer_team = match world.player(info.killer) { Some(p) => p.team, None => return };
        let defended = world.map.domination.iter().enumerate().take(3)
            .any(|(i, obj)| obj.contains(victim_pos) && self.points[i].owner == killer_team);
        if defended {
            if let Some(k) = world.player_mut(info.killer) {
                k.score.defends += 1;
                k.score.score += 75;
            }
        }
        let streak = world.player(info.killer).map(|p| p.score.streak).unwrap_or(0);
        super::announce_streak(world, info.killer, streak);
    }

    fn may_respawn(&self, _world: &World, state: &MatchState, _slot: u8) -> bool {
        state.phase.playable()
    }

    fn round_over(&self, world: &World, state: &MatchState) -> Option<Team> {
        self.match_over(world, state)
    }

    fn match_over(&self, _world: &World, state: &MatchState) -> Option<Team> {
        for t in Team::PLAYING {
            if state.score(t) >= state.score_limit { return Some(t); }
        }
        if state.clock <= 0.0 { return Some(state.leader()); }
        None
    }

    fn hud_state(&self, _world: &World, state: &MatchState) -> [u8; 8] {
        let mut out = [0u8; 8];
        for i in 0..3 {
            out[i] = self.points[i].owner as u8 | if self.points[i].contested { 0x80 } else { 0 };
            out[3 + i] = (self.points[i].progress.abs() * 255.0) as u8;
        }
        let p = state.score(Team::Phantom).min(255) as u8;
        let v = state.score(Team::Vanguard).min(255) as u8;
        out[6] = p;
        out[7] = v;
        out
    }
}

// =========================================================== search & destroy

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BombState {
    /// Held by a player.
    Carried(u8),
    /// On the ground, waiting to be picked up.
    Dropped,
    /// Armed at a site.
    Planted(u8),
    Defused,
    Exploded,
}

pub struct SearchDestroy {
    pub bomb: BombState,
    pub bomb_pos: Vec3,
    /// Seconds left on the fuse once planted.
    pub fuse: f32,
    /// Progress of the current plant or defuse, 0..1.
    pub action_progress: f32,
    pub action_by: u8,
    /// Which team is attacking this half.
    pub attackers: Team,
    /// Round at which the sides swap.
    swap_round: u16,
    round_winner: Option<Team>,
    announced_last_man: bool,
}

impl SearchDestroy {
    pub const PLANT_TIME: f32 = 4.0;
    pub const DEFUSE_TIME: f32 = 6.0;
    pub const FUSE_TIME: f32 = 45.0;

    pub fn new() -> SearchDestroy {
        SearchDestroy {
            bomb: BombState::Dropped,
            bomb_pos: Vec3::ZERO,
            fuse: 0.0,
            action_progress: 0.0,
            action_by: NO_PLAYER,
            attackers: Team::Phantom,
            swap_round: 4,
            round_winner: None,
            announced_last_man: false,
        }
    }

    pub fn defenders(&self) -> Team { self.attackers.opposite() }

    fn assign_bomb(&mut self, world: &mut World) {
        let candidates: Vec<u8> = world.players.iter()
            .filter(|p| p.in_use && p.alive && p.team == self.attackers)
            .map(|p| p.slot)
            .collect();
        for p in world.players.iter_mut() { p.carrying_bomb = false; }
        if candidates.is_empty() {
            self.bomb = BombState::Dropped;
            return;
        }
        let pick = candidates[world.rng.below(candidates.len() as u32) as usize];
        self.bomb = BombState::Carried(pick);
        if let Some(p) = world.player_mut(pick) { p.carrying_bomb = true; }
        world.events.push(GameEvent::BombPickedUp { by: pick });
    }

    /// The site a player is standing in, if any.
    fn site_at(world: &World, pos: Vec3) -> Option<u8> {
        world.map.bomb_sites.iter().enumerate()
            .find(|(_, o)| o.contains(pos))
            .map(|(i, _)| i as u8)
    }
}

impl Mode for SearchDestroy {
    fn id(&self) -> ModeId { ModeId::SearchDestroy }

    fn begin_match(&mut self, world: &mut World, state: &mut MatchState) {
        world.team_game = true;
        world.respawn = RespawnPolicy::None;
        state.scores = [0; 4];
        state.round = 0;
        self.attackers = Team::Phantom;
        // Swap sides once either team is halfway to the win condition.
        self.swap_round = state.score_limit.max(1);
        self.round_winner = None;
    }

    fn begin_round(&mut self, world: &mut World, state: &mut MatchState) {
        state.clock = state.time_limit as f32;
        state.round += 1;
        self.bomb = BombState::Dropped;
        self.fuse = 0.0;
        self.action_progress = 0.0;
        self.action_by = NO_PLAYER;
        self.round_winner = None;
        self.announced_last_man = false;

        // Halftime: swap which side is attacking.
        if state.round == self.swap_round + 1 {
            self.attackers = self.attackers.opposite();
        }

        for i in 0..world.players.len() {
            if !world.players[i].in_use { continue; }
            world.players[i].lives = 1;
            world.respawn_player(i as u8, true);
        }
        self.assign_bomb(world);
    }

    fn update(&mut self, world: &mut World, state: &mut MatchState, dt: f32) {
        if state.phase != Phase::Live { return; }

        match self.bomb {
            BombState::Planted(site) => {
                self.fuse -= dt;
                if self.fuse <= 0.0 {
                    self.bomb = BombState::Exploded;
                    world.events.push(GameEvent::BombExploded { site });
                    // Everyone near the site goes with it.
                    if let Some(obj) = world.map.bomb_sites.get(site as usize) {
                        let pos = obj.pos;
                        world.detonate(pos + Vec3::Y * 0.5, crate::game::loadout::Equipment::Frag, NO_PLAYER, Team::None);
                    }
                    self.round_winner = Some(self.attackers);
                    return;
                }
            }
            _ => {
                state.clock -= dt;
            }
        }

        // Carrier died? The bomb drops where they fell.
        if let BombState::Carried(slot) = self.bomb {
            let dead = world.player(slot).map(|p| !p.alive).unwrap_or(true);
            if dead {
                let pos = world.player(slot).map(|p| p.mv.pos).unwrap_or(self.bomb_pos);
                self.bomb = BombState::Dropped;
                self.bomb_pos = pos;
                if let Some(p) = world.player_mut(slot) { p.carrying_bomb = false; }
                world.events.push(GameEvent::BombDropped { pos });
                world.events.push(GameEvent::Announce { line: AnnounceLine::BombDown });
            }
        }

        // Anyone on the attacking team can pick a dropped bomb back up.
        if self.bomb == BombState::Dropped {
            let mut taker = None;
            for p in world.players.iter() {
                if !p.in_use || !p.alive || p.team != self.attackers { continue; }
                if (p.mv.pos - self.bomb_pos).length() < 1.6 { taker = Some(p.slot); break; }
            }
            if let Some(slot) = taker {
                self.bomb = BombState::Carried(slot);
                if let Some(p) = world.player_mut(slot) { p.carrying_bomb = true; }
                world.events.push(GameEvent::BombPickedUp { by: slot });
            }
        }

        // Planting.
        if let BombState::Carried(slot) = self.bomb {
            let info = world.player(slot).map(|p| (p.mv.pos, p.last_buttons.contains(Buttons::USE), p.alive));
            if let Some((pos, holding, alive)) = info {
                let site = SearchDestroy::site_at(world, pos);
                if alive && holding && site.is_some() {
                    self.action_by = slot;
                    self.action_progress += dt / Self::PLANT_TIME;
                    if self.action_progress >= 1.0 {
                        let site = site.unwrap();
                        self.bomb = BombState::Planted(site);
                        self.bomb_pos = pos;
                        self.fuse = Self::FUSE_TIME;
                        self.action_progress = 0.0;
                        if let Some(p) = world.player_mut(slot) {
                            p.carrying_bomb = false;
                            p.score.plants += 1;
                            p.score.score += 300;
                        }
                        world.events.push(GameEvent::BombPlanted { site, by: slot });
                        world.events.push(GameEvent::Announce { line: AnnounceLine::BombPlanted });
                    }
                } else if self.action_by == slot {
                    self.action_progress = (self.action_progress - dt * 2.0).max(0.0);
                }
            }
        }

        // Defusing.
        if let BombState::Planted(_) = self.bomb {
            let mut defuser = None;
            for p in world.players.iter() {
                if !p.in_use || !p.alive || p.team != self.defenders() { continue; }
                if !p.last_buttons.contains(Buttons::USE) { continue; }
                if (p.mv.pos - self.bomb_pos).length() < 2.4 { defuser = Some(p.slot); break; }
            }
            match defuser {
                Some(slot) => {
                    if self.action_by != slot { self.action_progress = 0.0; }
                    self.action_by = slot;
                    self.action_progress += dt / Self::DEFUSE_TIME;
                    if self.action_progress >= 1.0 {
                        self.bomb = BombState::Defused;
                        if let Some(p) = world.player_mut(slot) {
                            p.score.defuses += 1;
                            p.score.score += 300;
                        }
                        world.events.push(GameEvent::BombDefused { by: slot });
                        world.events.push(GameEvent::Announce { line: AnnounceLine::BombDefused });
                        self.round_winner = Some(self.defenders());
                    }
                }
                None => self.action_progress = (self.action_progress - dt * 1.5).max(0.0),
            }
        }

        // Last man standing is worth telling someone about.
        if !self.announced_last_man {
            for t in Team::PLAYING {
                if world.team_alive(t) == 1 && world.team_count(t) > 2 {
                    world.events.push(GameEvent::Announce { line: AnnounceLine::LastManStanding });
                    self.announced_last_man = true;
                }
            }
        }
    }

    fn on_kill(&mut self, world: &mut World, state: &mut MatchState, info: KillInfo) {
        let _ = (world, state, info);
    }

    fn may_respawn(&self, _world: &World, _state: &MatchState, _slot: u8) -> bool { false }

    fn round_over(&self, world: &World, state: &MatchState) -> Option<Team> {
        if let Some(w) = self.round_winner { return Some(w); }
        if state.phase != Phase::Live { return None; }
        // A team wiped out loses, unless the bomb is already ticking - then
        // the attackers can still win by having planted.
        if world.team_alive(self.attackers) == 0 && world.team_count(self.attackers) > 0 {
            if !matches!(self.bomb, BombState::Planted(_)) {
                return Some(self.defenders());
            }
        }
        if world.team_alive(self.defenders()) == 0 && world.team_count(self.defenders()) > 0 {
            return Some(self.attackers);
        }
        if state.clock <= 0.0 && !matches!(self.bomb, BombState::Planted(_)) {
            return Some(self.defenders());
        }
        None
    }

    fn match_over(&self, _world: &World, state: &MatchState) -> Option<Team> {
        for t in Team::PLAYING {
            if state.round_wins[t.index()] >= state.score_limit { return Some(t); }
        }
        None
    }

    fn hud_state(&self, _world: &World, state: &MatchState) -> [u8; 8] {
        let planted = matches!(self.bomb, BombState::Planted(_));
        let site = match self.bomb { BombState::Planted(s) => s, _ => 0xFF };
        let carrier = match self.bomb { BombState::Carried(s) => s, _ => NO_PLAYER };
        [
            if planted { 1 } else { 0 },
            site,
            carrier,
            (self.fuse.max(0.0)) as u8,
            (self.action_progress * 255.0) as u8,
            self.attackers as u8,
            state.round_wins[Team::Phantom.index()].min(255) as u8,
            state.round_wins[Team::Vanguard.index()].min(255) as u8,
        ]
    }
}
