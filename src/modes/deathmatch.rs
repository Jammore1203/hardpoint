//! Kill-driven modes: Team Deathmatch, Free For All and Gun Game.

use super::{KillInfo, MatchState, Mode, ModeId, Phase};
use crate::game::events::{AnnounceLine, GameEvent};
use crate::game::sim::{RespawnPolicy, World};
use crate::game::types::{Team, NO_PLAYER};
use crate::game::weapons::{WeaponId, WeaponSlot};

/// Two teams, first to the score limit.
pub struct TeamDeathmatch {
    /// Set once, so "first blood" only fires for the first kill of the match.
    first_blood: bool,
    /// Which side was ahead last tick, so the lead-change cue is not spammed.
    last_leader: Team,
}

impl TeamDeathmatch {
    pub fn new() -> TeamDeathmatch {
        TeamDeathmatch { first_blood: false, last_leader: Team::None }
    }
}

impl Mode for TeamDeathmatch {
    fn id(&self) -> ModeId { ModeId::TeamDeathmatch }

    fn begin_match(&mut self, world: &mut World, state: &mut MatchState) {
        world.team_game = true;
        world.respawn = RespawnPolicy::Auto { delay: 5.0 };
        state.scores = [0; 4];
        self.first_blood = false;
        self.last_leader = Team::None;
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
        // Lead changes are worth calling out: it is the only feedback a team
        // gets about a mode with no objectives.
        let leader = state.leader();
        if leader != self.last_leader && leader != Team::None && state.score(leader) > 5 {
            world.events.push(GameEvent::Announce { line: AnnounceLine::TakingLead });
            self.last_leader = leader;
        }
    }

    fn on_kill(&mut self, world: &mut World, state: &mut MatchState, info: KillInfo) {
        if state.phase != Phase::Live { return; }
        if info.suicide || info.friendly { return; }
        let team = match world.player(info.killer) { Some(p) => p.team, None => return };
        state.add_score(team, 1);
        if !self.first_blood {
            self.first_blood = true;
            world.events.push(GameEvent::Announce { line: AnnounceLine::FirstBlood });
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
        let p = state.score(Team::Phantom).to_le_bytes();
        let v = state.score(Team::Vanguard).to_le_bytes();
        [p[0], p[1], v[0], v[1], 0, 0, 0, 0]
    }
}

/// Everyone against everyone.
pub struct FreeForAll {
    first_blood: bool,
}

impl FreeForAll {
    pub fn new() -> FreeForAll { FreeForAll { first_blood: false } }

    fn leader(world: &World) -> (u8, u16) {
        let mut best = (NO_PLAYER, 0u16);
        for p in world.active_players() {
            if p.score.kills > best.1 { best = (p.slot, p.score.kills); }
        }
        best
    }
}

impl Mode for FreeForAll {
    fn id(&self) -> ModeId { ModeId::FreeForAll }

    fn begin_match(&mut self, world: &mut World, state: &mut MatchState) {
        world.team_game = false;
        world.respawn = RespawnPolicy::Auto { delay: 4.0 };
        for p in world.players.iter_mut() { p.team = Team::None; }
        state.scores = [0; 4];
        self.first_blood = false;
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

    fn update(&mut self, _world: &mut World, state: &mut MatchState, dt: f32) {
        if state.phase != Phase::Live { return; }
        state.clock -= dt;
    }

    fn on_kill(&mut self, world: &mut World, state: &mut MatchState, info: KillInfo) {
        if state.phase != Phase::Live || info.suicide { return; }
        if !self.first_blood {
            self.first_blood = true;
            world.events.push(GameEvent::Announce { line: AnnounceLine::FirstBlood });
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

    fn match_over(&self, world: &World, state: &MatchState) -> Option<Team> {
        let (_, kills) = FreeForAll::leader(world);
        if kills >= state.score_limit || state.clock <= 0.0 { return Some(Team::None); }
        None
    }

    fn hud_state(&self, world: &World, _state: &MatchState) -> [u8; 8] {
        let (slot, kills) = FreeForAll::leader(world);
        let k = kills.to_le_bytes();
        [slot, k[0], k[1], 0, 0, 0, 0, 0]
    }
}

/// The ladder every player climbs, one rung per kill. Ordered so it starts
/// forgiving, gets awkward in the middle, and finishes on a knife.
pub const GUN_LADDER: [WeaponId; 20] = [
    WeaponId::SidearmP9,
    WeaponId::Wasp9,
    WeaponId::Vectra5,
    WeaponId::Viper,
    WeaponId::Kr44,
    WeaponId::Breacher12,
    WeaponId::Kestrel,
    WeaponId::Shrike,
    WeaponId::Bulwark60,
    WeaponId::Tempest,
    WeaponId::Longshore,
    WeaponId::Marksman,
    WeaponId::Drumfire,
    WeaponId::Scarab,
    WeaponId::Hammerhead,
    WeaponId::Longbow,
    WeaponId::BurstP3,
    WeaponId::TrenchM4,
    WeaponId::Anvil44,
    WeaponId::CombatKnife,
];

/// Free-for-all where every kill promotes you to the next weapon.
pub struct GunGame {
    winner: u8,
}

impl GunGame {
    pub fn new() -> GunGame { GunGame { winner: NO_PLAYER } }

    /// Forces a player's loadout to match their rank.
    fn equip_rank(world: &mut World, slot: u8) {
        let Some(p) = world.player_mut(slot) else { return };
        let rank = p.gun_rank.min(GUN_LADDER.len() as u8 - 1);
        let id = GUN_LADDER[rank as usize];
        p.weapons[0] = WeaponSlot::new(id);
        // No sidearm and no grenades: the ladder is the whole game.
        p.weapons[1] = WeaponSlot::new(WeaponId::CombatKnife);
        p.weapons[2] = WeaponSlot::new(WeaponId::CombatKnife);
        p.cur = 0;
        p.queued_slot = 0;
        p.lethal_count = 0;
        p.tactical_count = 0;
    }
}

impl Mode for GunGame {
    fn id(&self) -> ModeId { ModeId::GunGame }

    fn begin_match(&mut self, world: &mut World, state: &mut MatchState) {
        world.team_game = false;
        world.respawn = RespawnPolicy::Auto { delay: 3.0 };
        for p in world.players.iter_mut() {
            p.team = Team::None;
            p.gun_rank = 0;
        }
        state.score_limit = GUN_LADDER.len() as u16;
        self.winner = NO_PLAYER;
    }

    fn begin_round(&mut self, world: &mut World, state: &mut MatchState) {
        state.clock = state.time_limit as f32;
        for i in 0..world.players.len() {
            if !world.players[i].in_use { continue; }
            world.players[i].score.reset_match();
            world.players[i].gun_rank = 0;
            world.respawn_player(i as u8, true);
            GunGame::equip_rank(world, i as u8);
        }
    }

    fn update(&mut self, _world: &mut World, state: &mut MatchState, dt: f32) {
        if state.phase != Phase::Live { return; }
        state.clock -= dt;
    }

    fn on_kill(&mut self, world: &mut World, state: &mut MatchState, info: KillInfo) {
        if state.phase != Phase::Live { return; }
        if info.suicide {
            // Demotion for taking yourself out; keeps the mode honest.
            if let Some(p) = world.player_mut(info.victim) {
                p.gun_rank = p.gun_rank.saturating_sub(1);
            }
            GunGame::equip_rank(world, info.victim);
            return;
        }
        let promoted = {
            let Some(k) = world.player_mut(info.killer) else { return };
            k.gun_rank = k.gun_rank.saturating_add(1);
            k.score.score = k.gun_rank as i32 * 100;
            k.gun_rank
        };
        if promoted as usize >= GUN_LADDER.len() {
            self.winner = info.killer;
        } else {
            GunGame::equip_rank(world, info.killer);
            world.events.push(GameEvent::Announce { line: AnnounceLine::Promoted });
        }
        let _ = state;
    }

    fn may_respawn(&self, _world: &World, state: &MatchState, _slot: u8) -> bool {
        state.phase.playable()
    }

    fn round_over(&self, world: &World, state: &MatchState) -> Option<Team> {
        self.match_over(world, state)
    }

    fn match_over(&self, _world: &World, state: &MatchState) -> Option<Team> {
        if self.winner != NO_PLAYER { return Some(Team::None); }
        if state.clock <= 0.0 { return Some(Team::None); }
        None
    }

    fn hud_state(&self, world: &World, _state: &MatchState) -> [u8; 8] {
        // Highest rank on the server, and who holds it.
        let mut best = (NO_PLAYER, 0u8);
        for p in world.active_players() {
            if p.gun_rank >= best.1 { best = (p.slot, p.gun_rank); }
        }
        [best.0, best.1, GUN_LADDER.len() as u8, 0, 0, 0, 0, 0]
    }

    fn on_player_spawn(&mut self, world: &mut World, _state: &MatchState, slot: u8) {
        GunGame::equip_rank(world, slot);
    }
}
