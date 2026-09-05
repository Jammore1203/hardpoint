//! Simulation events.
//!
//! The authoritative simulation never touches audio, particles or the HUD.
//! Instead it emits events, which the server replicates and the client turns
//! into sound and effects. That separation is what lets the dedicated server
//! run headless, lets bots "hear" gunfire through the same channel a player
//! does, and keeps the replay of a match a pure data problem.

use super::loadout::Equipment;
pub use super::types::{DeathCause, HitZone};
use super::types::{PickupKind, Team};
use super::weapons::WeaponId;
use crate::assets::materials::Surface;
use glam::Vec3;

/// How an event must be delivered.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Delivery {
    /// Must arrive, and in order: kills, objectives, match state.
    Reliable,
    /// Cosmetic and time-sensitive; dropping one is better than delaying it.
    Unreliable,
}

/// What kind of thing a bullet hit, which selects the impact effect.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ImpactKind {
    Bullet = 0,
    Pellet = 1,
    Explosion = 2,
    Melee = 3,
}

#[derive(Clone, Debug)]
pub enum GameEvent {
    /// A weapon was fired. Carries the seed so every client reproduces the
    /// same pellet spread and tracer directions the server used.
    Shot {
        player: u8,
        weapon: WeaponId,
        origin: Vec3,
        dir: Vec3,
        seed: u32,
    },
    /// A round struck the world.
    Impact {
        pos: Vec3,
        normal: Vec3,
        surface: Surface,
        kind: ImpactKind,
    },
    /// A round struck a player. Sent to the shooter for the hit marker and to
    /// everyone nearby for the blood effect.
    HitPlayer {
        attacker: u8,
        victim: u8,
        pos: Vec3,
        zone: HitZone,
        damage: u16,
        lethal: bool,
    },
    Kill {
        killer: u8,
        victim: u8,
        weapon: WeaponId,
        cause: DeathCause,
        /// Metres between them, for the "long shot" medal.
        distance: f32,
        /// The killer's streak after this kill.
        streak: u16,
    },
    Melee { player: u8, hit: bool },
    Reload { player: u8, empty: bool },
    ShellLoaded { player: u8 },
    Swap { player: u8, slot: u8 },
    DryFire { player: u8 },
    GrenadeThrown {
        id: u16,
        player: u8,
        kind: Equipment,
        pos: Vec3,
        vel: Vec3,
    },
    GrenadeBounce { id: u16, pos: Vec3, surface: Surface },
    Explosion { pos: Vec3, kind: Equipment, radius: f32 },
    /// A flashbang or concussion caught someone.
    Blinded { player: u8, strength: f32, concussion: bool },
    SmokeStarted { id: u16, pos: Vec3 },
    FireStarted { id: u16, pos: Vec3, radius: f32 },
    Footstep { player: u8, pos: Vec3, surface: Surface, volume: f32 },
    Land { player: u8, pos: Vec3, speed: f32, surface: Surface },
    Jump { player: u8 },
    Spawned { player: u8, pos: Vec3 },
    PickupTaken { player: u8, index: u16, kind: PickupKind },
    PickupRespawned { index: u16 },

    // ------------------------------------------------------- objectives
    CapturePoint { point: u8, team: Team, contested: bool },
    CaptureProgress { point: u8, team: Team, progress: u8 },
    BombPlanted { site: u8, by: u8 },
    BombDefused { by: u8 },
    BombExploded { site: u8 },
    BombPickedUp { by: u8 },
    BombDropped { pos: Vec3 },

    // ------------------------------------------------------ match state
    MatchState { phase: u8, seconds: u16 },
    RoundStart { round: u16 },
    RoundEnd { winner: Team, reason: u8 },
    ScoreChanged { team: Team, score: u16 },
    /// Radio-style announcer line.
    Announce { line: AnnounceLine },
    /// Text from a player, already validated by the server.
    Chat { player: u8, team_only: bool, text: String },
    PlayerJoined { player: u8 },
    PlayerLeft { player: u8 },
    TeamChanged { player: u8, team: Team },
}

/// Announcer cues. Deliberately terse and military.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum AnnounceLine {
    MatchStarting = 0,
    Fight,
    OneMinute,
    ThirtySeconds,
    Overtime,
    Victory,
    Defeat,
    Draw,
    LosingLead,
    TakingLead,
    PointCaptured,
    PointLost,
    PointContested,
    BombPlanted,
    BombDefused,
    BombDown,
    LastManStanding,
    EnemiesRemaining,
    Killstreak5,
    Killstreak10,
    Headshot,
    Revenge,
    FirstBlood,
    Promoted,
}

impl AnnounceLine {
    pub fn from_u8(v: u8) -> AnnounceLine {
        // The table is dense, so a bounds check plus transmute is safe and
        // avoids a twenty-arm match that nothing would ever read.
        const MAX: u8 = AnnounceLine::Promoted as u8;
        let v = if v <= MAX { v } else { 0 };
        unsafe { std::mem::transmute::<u8, AnnounceLine>(v) }
    }

    /// Subtitle text, shown briefly in the HUD.
    pub fn text(self) -> &'static str {
        use AnnounceLine::*;
        match self {
            MatchStarting => "STAND BY",
            Fight => "ENGAGE",
            OneMinute => "ONE MINUTE REMAINING",
            ThirtySeconds => "THIRTY SECONDS",
            Overtime => "OVERTIME",
            Victory => "OBJECTIVE COMPLETE",
            Defeat => "MISSION FAILED",
            Draw => "STALEMATE",
            LosingLead => "WE ARE LOSING THE LEAD",
            TakingLead => "WE HAVE THE LEAD",
            PointCaptured => "POINT SECURED",
            PointLost => "POINT LOST",
            PointContested => "POINT UNDER ATTACK",
            BombPlanted => "CHARGE ARMED",
            BombDefused => "CHARGE DISARMED",
            BombDown => "CHARGE DROPPED",
            LastManStanding => "YOU ARE THE LAST ONE LEFT",
            EnemiesRemaining => "ENEMIES REMAINING",
            Killstreak5 => "FIVE CONFIRMED",
            Killstreak10 => "TEN CONFIRMED",
            Headshot => "HEADSHOT",
            Revenge => "PAYBACK",
            FirstBlood => "FIRST BLOOD",
            Promoted => "PROMOTION",
        }
    }
}

impl GameEvent {
    /// Where the event happens, for relevance culling. `None` means it is
    /// global and every client needs it.
    pub fn position(&self) -> Option<Vec3> {
        use GameEvent::*;
        match self {
            Shot { origin, .. } => Some(*origin),
            Impact { pos, .. } => Some(*pos),
            HitPlayer { pos, .. } => Some(*pos),
            GrenadeThrown { pos, .. } => Some(*pos),
            GrenadeBounce { pos, .. } => Some(*pos),
            Explosion { pos, .. } => Some(*pos),
            SmokeStarted { pos, .. } => Some(*pos),
            FireStarted { pos, .. } => Some(*pos),
            Footstep { pos, .. } => Some(*pos),
            Land { pos, .. } => Some(*pos),
            Spawned { pos, .. } => Some(*pos),
            BombDropped { pos } => Some(*pos),
            _ => None,
        }
    }

    pub fn delivery(&self) -> Delivery {
        use GameEvent::*;
        match self {
            Kill { .. } | CapturePoint { .. } | BombPlanted { .. } | BombDefused { .. }
            | BombExploded { .. } | BombPickedUp { .. } | BombDropped { .. }
            | MatchState { .. } | RoundStart { .. } | RoundEnd { .. } | ScoreChanged { .. }
            | Announce { .. } | Chat { .. } | PlayerJoined { .. } | PlayerLeft { .. }
            | TeamChanged { .. } | PickupTaken { .. } => Delivery::Reliable,
            _ => Delivery::Unreliable,
        }
    }

    /// Radius beyond which a client does not need to know. Zero means global.
    pub fn relevance_radius(&self) -> f32 {
        use GameEvent::*;
        match self {
            Footstep { .. } => 22.0,
            Impact { .. } => 45.0,
            GrenadeBounce { .. } => 30.0,
            Land { .. } => 26.0,
            Jump { .. } => 20.0,
            Shot { .. } => 120.0,
            Explosion { .. } => 90.0,
            _ => 0.0,
        }
    }

    /// Whether the event is only meaningful to one specific player.
    pub fn private_to(&self) -> Option<u8> {
        match self {
            GameEvent::DryFire { player } => Some(*player),
            GameEvent::ShellLoaded { player } => Some(*player),
            _ => None,
        }
    }
}

/// A fixed-capacity event queue. The simulation writes into it every tick and
/// the server drains it; capping the size means a pathological frame cannot
/// balloon memory or bandwidth.
pub struct EventQueue {
    events: Vec<GameEvent>,
    dropped: u32,
    cap: usize,
}

impl EventQueue {
    pub fn new(cap: usize) -> EventQueue {
        EventQueue { events: Vec::with_capacity(cap), dropped: 0, cap }
    }

    #[inline]
    pub fn push(&mut self, e: GameEvent) {
        if self.events.len() >= self.cap {
            // Cosmetic events are the ones we can afford to lose.
            if e.delivery() == Delivery::Unreliable {
                self.dropped += 1;
                return;
            }
            if let Some(i) = self.events.iter().position(|x| x.delivery() == Delivery::Unreliable) {
                self.events.swap_remove(i);
                self.dropped += 1;
            } else {
                self.dropped += 1;
                return;
            }
        }
        self.events.push(e);
    }

    pub fn drain(&mut self) -> std::vec::Drain<'_, GameEvent> { self.events.drain(..) }
    pub fn iter(&self) -> std::slice::Iter<'_, GameEvent> { self.events.iter() }
    pub fn clear(&mut self) { self.events.clear(); }
    pub fn len(&self) -> usize { self.events.len() }
    pub fn is_empty(&self) -> bool { self.events.is_empty() }
    pub fn dropped(&self) -> u32 { self.dropped }
}
