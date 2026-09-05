//! Game modes.
//!
//! A mode is a small object that reads the world, pushes it around and keeps
//! score. It never touches combat, movement or networking, which is what lets
//! five very different rule sets coexist without any of them special-casing
//! the others.

pub mod deathmatch;
pub mod objective;

use crate::game::events::{AnnounceLine, GameEvent};
use crate::game::sim::World;
use crate::game::types::{DeathCause, Team};
use crate::game::weapons::WeaponId;

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum ModeId {
    TeamDeathmatch = 0,
    FreeForAll = 1,
    Domination = 2,
    SearchDestroy = 3,
    GunGame = 4,
}

pub const MODE_COUNT: usize = 5;
pub const ALL_MODES: [ModeId; MODE_COUNT] = [
    ModeId::TeamDeathmatch, ModeId::FreeForAll, ModeId::Domination,
    ModeId::SearchDestroy, ModeId::GunGame,
];

impl ModeId {
    pub fn from_u8(v: u8) -> ModeId {
        if (v as usize) < MODE_COUNT { ALL_MODES[v as usize] } else { ModeId::TeamDeathmatch }
    }
    pub fn name(self) -> &'static str {
        match self {
            ModeId::TeamDeathmatch => "TEAM DEATHMATCH",
            ModeId::FreeForAll => "FREE FOR ALL",
            ModeId::Domination => "DOMINATION",
            ModeId::SearchDestroy => "SEARCH & DESTROY",
            ModeId::GunGame => "GUN GAME",
        }
    }
    pub fn short(self) -> &'static str {
        match self {
            ModeId::TeamDeathmatch => "TDM",
            ModeId::FreeForAll => "FFA",
            ModeId::Domination => "DOM",
            ModeId::SearchDestroy => "S&D",
            ModeId::GunGame => "GUN",
        }
    }
    pub fn blurb(self) -> &'static str {
        match self {
            ModeId::TeamDeathmatch => "Two teams. Reach the score limit first.",
            ModeId::FreeForAll => "Everyone for themselves.",
            ModeId::Domination => "Hold three points. Score ticks up while you own them.",
            ModeId::SearchDestroy => "One life a round. Attackers plant, defenders stop them.",
            ModeId::GunGame => "Every kill promotes you. Finish the ladder to win.",
        }
    }
    pub fn is_team_game(self) -> bool {
        !matches!(self, ModeId::FreeForAll | ModeId::GunGame)
    }
    pub fn is_round_based(self) -> bool { self == ModeId::SearchDestroy }

    /// Sensible defaults, which the lobby can override.
    pub fn default_score_limit(self) -> u16 {
        match self {
            ModeId::TeamDeathmatch => 75,
            ModeId::FreeForAll => 30,
            ModeId::Domination => 200,
            ModeId::SearchDestroy => 4,
            ModeId::GunGame => 20,
        }
    }
    pub fn default_time_limit(self) -> u16 {
        match self {
            ModeId::TeamDeathmatch => 600,
            ModeId::FreeForAll => 600,
            ModeId::Domination => 900,
            ModeId::SearchDestroy => 120,
            ModeId::GunGame => 720,
        }
    }
    pub fn score_label(self) -> &'static str {
        match self {
            ModeId::SearchDestroy => "ROUNDS",
            ModeId::GunGame => "RANK",
            _ => "SCORE",
        }
    }

    pub fn build(self) -> Box<dyn Mode> {
        match self {
            ModeId::TeamDeathmatch => Box::new(deathmatch::TeamDeathmatch::new()),
            ModeId::FreeForAll => Box::new(deathmatch::FreeForAll::new()),
            ModeId::GunGame => Box::new(deathmatch::GunGame::new()),
            ModeId::Domination => Box::new(objective::Domination::new()),
            ModeId::SearchDestroy => Box::new(objective::SearchDestroy::new()),
        }
    }
}

/// Where a match is in its life cycle. Drives both rules and the client's UI.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Phase {
    /// Waiting in the lobby before anyone has started.
    Lobby = 0,
    /// Free play while the server fills up. No scoring.
    Warmup = 1,
    /// Frozen countdown before the round goes live.
    Countdown = 2,
    Live = 3,
    /// Between rounds in a round-based mode.
    RoundOver = 4,
    /// Match finished; scoreboard and results.
    MatchOver = 5,
}

impl Phase {
    pub fn from_u8(v: u8) -> Phase {
        match v {
            1 => Phase::Warmup, 2 => Phase::Countdown, 3 => Phase::Live,
            4 => Phase::RoundOver, 5 => Phase::MatchOver, _ => Phase::Lobby,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Phase::Lobby => "LOBBY",
            Phase::Warmup => "WARMUP",
            Phase::Countdown => "GET READY",
            Phase::Live => "",
            Phase::RoundOver => "ROUND OVER",
            Phase::MatchOver => "MATCH OVER",
        }
    }
    /// Can players move and shoot?
    pub fn playable(self) -> bool { matches!(self, Phase::Warmup | Phase::Live) }
}

/// Everything about the current match that is not the world itself.
#[derive(Clone, Debug)]
pub struct MatchState {
    pub phase: Phase,
    /// Seconds left in the current phase.
    pub timer: f32,
    /// Seconds left in the round or match, for the HUD clock.
    pub clock: f32,
    pub round: u16,
    /// Score per team index; index 0 is used by free-for-all modes.
    pub scores: [u16; 4],
    pub round_wins: [u16; 4],
    pub score_limit: u16,
    pub time_limit: u16,
    pub winner: Team,
    /// Slot of the individual winner in free-for-all modes.
    pub winner_slot: u8,
    /// Set for one tick when the match has just finished.
    pub just_ended: bool,
}

impl MatchState {
    pub fn new(mode: ModeId) -> MatchState {
        MatchState {
            phase: Phase::Lobby,
            timer: 0.0,
            clock: mode.default_time_limit() as f32,
            round: 0,
            scores: [0; 4],
            round_wins: [0; 4],
            score_limit: mode.default_score_limit(),
            time_limit: mode.default_time_limit(),
            winner: Team::None,
            winner_slot: crate::game::types::NO_PLAYER,
            just_ended: false,
        }
    }

    #[inline]
    pub fn score(&self, team: Team) -> u16 { self.scores[team.index().min(3)] }
    #[inline]
    pub fn add_score(&mut self, team: Team, n: u16) {
        let i = team.index().min(3);
        self.scores[i] = self.scores[i].saturating_add(n);
    }
    pub fn leader(&self) -> Team {
        if self.scores[Team::Phantom.index()] > self.scores[Team::Vanguard.index()] { Team::Phantom }
        else if self.scores[Team::Vanguard.index()] > self.scores[Team::Phantom.index()] { Team::Vanguard }
        else { Team::None }
    }
}

/// A kill, handed to the mode after the world has already scored it.
#[derive(Copy, Clone, Debug)]
pub struct KillInfo {
    pub killer: u8,
    pub victim: u8,
    pub weapon: WeaponId,
    pub cause: DeathCause,
    pub suicide: bool,
    pub friendly: bool,
}

pub trait Mode: Send {
    fn id(&self) -> ModeId;

    /// Called once when the match is created or the map changes.
    fn begin_match(&mut self, world: &mut World, state: &mut MatchState);

    /// Called at the start of every round (once, for non-round modes).
    fn begin_round(&mut self, world: &mut World, state: &mut MatchState);

    /// Runs every tick while the phase is playable.
    fn update(&mut self, world: &mut World, state: &mut MatchState, dt: f32);

    /// Called for each kill after the world has updated scores.
    fn on_kill(&mut self, world: &mut World, state: &mut MatchState, info: KillInfo);

    /// Should this player respawn now? Modes with limited lives say no.
    fn may_respawn(&self, world: &World, state: &MatchState, slot: u8) -> bool;

    /// Has the round ended, and who won it?
    fn round_over(&self, world: &World, state: &MatchState) -> Option<Team>;

    /// Has the match ended overall?
    fn match_over(&self, world: &World, state: &MatchState) -> Option<Team>;

    /// Eight bytes of mode-specific state for the HUD (point ownership, bomb
    /// timer, gun-game rank). Interpreted by the client per mode.
    fn hud_state(&self, world: &World, state: &MatchState) -> [u8; 8];

    /// Called when a player joins mid-match, so the mode can equip them.
    fn on_player_spawn(&mut self, world: &mut World, state: &MatchState, slot: u8) {
        let _ = (world, state, slot);
    }
}

/// Shared helper: scores a team kill and announces milestones.
pub fn announce_streak(world: &mut World, slot: u8, streak: u16) {
    match streak {
        5 => world.events.push(GameEvent::Announce { line: AnnounceLine::Killstreak5 }),
        10 => world.events.push(GameEvent::Announce { line: AnnounceLine::Killstreak10 }),
        _ => {}
    }
    let _ = slot;
}

/// Assigns a player to whichever team needs them, keeping sides even.
pub fn auto_assign_team(world: &World, mode: ModeId) -> Team {
    if !mode.is_team_game() { return Team::None; }
    let p = world.team_count(Team::Phantom);
    let v = world.team_count(Team::Vanguard);
    if p <= v { Team::Phantom } else { Team::Vanguard }
}
