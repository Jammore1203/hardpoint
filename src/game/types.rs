//! Small shared gameplay types used by the simulation, the netcode, the UI and
//! the map library alike.

use glam::Vec3;

pub const MAX_PLAYERS: usize = 16;
/// Slot value meaning "no player".
pub const NO_PLAYER: u8 = 0xFF;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum Team {
    /// Free-for-all / unassigned.
    #[default]
    None = 0,
    /// Attacker-flavoured faction.
    Phantom = 1,
    /// Defender-flavoured faction.
    Vanguard = 2,
    /// Watching, not playing.
    Spectator = 3,
}

impl Team {
    pub fn from_u8(v: u8) -> Team {
        match v { 1 => Team::Phantom, 2 => Team::Vanguard, 3 => Team::Spectator, _ => Team::None }
    }
    pub fn opposite(self) -> Team {
        match self { Team::Phantom => Team::Vanguard, Team::Vanguard => Team::Phantom, t => t }
    }
    pub fn name(self) -> &'static str {
        match self {
            Team::Phantom => "PHANTOM",
            Team::Vanguard => "VANGUARD",
            Team::Spectator => "SPECTATOR",
            Team::None => "FREELANCE",
        }
    }
    pub fn short(self) -> &'static str {
        match self {
            Team::Phantom => "PHN", Team::Vanguard => "VNG",
            Team::Spectator => "SPC", Team::None => "---",
        }
    }
    /// Primary UI colour, in the game's restrained palette.
    pub fn color(self) -> [f32; 3] {
        match self {
            Team::Phantom => [0.90, 0.42, 0.24],
            Team::Vanguard => [0.36, 0.62, 0.92],
            Team::Spectator => [0.62, 0.62, 0.62],
            Team::None => [0.82, 0.78, 0.58],
        }
    }
    pub fn index(self) -> usize { self as usize }
    /// Iterates the two playable factions.
    pub const PLAYING: [Team; 2] = [Team::Phantom, Team::Vanguard];
}

/// Which team a spawn point belongs to, and in which phase it is valid.
#[derive(Copy, Clone, Debug)]
pub struct SpawnPoint {
    pub pos: Vec3,
    pub yaw: f32,
    /// `Team::None` means the spawn is usable by anyone (free-for-all, or a
    /// neutral respawn once a team-based match is under way).
    pub team: Team,
    /// Initial spawns are used for round starts in Search & Destroy and for
    /// the opening moments of other modes; the rest are respawn points.
    pub initial: bool,
}

impl SpawnPoint {
    pub fn new(pos: Vec3, yaw_deg: f32) -> SpawnPoint {
        SpawnPoint { pos, yaw: yaw_deg.to_radians(), team: Team::None, initial: false }
    }
    pub fn team(mut self, t: Team) -> SpawnPoint { self.team = t; self }
    pub fn initial(mut self) -> SpawnPoint { self.initial = true; self }
}

/// Named objective volume. Domination uses three of them; Search & Destroy
/// uses the first two as bomb sites.
#[derive(Clone, Debug)]
pub struct Objective {
    pub label: &'static str,
    pub pos: Vec3,
    pub radius: f32,
    pub height: f32,
}

impl Objective {
    pub fn new(label: &'static str, pos: Vec3, radius: f32) -> Objective {
        Objective { label, pos, radius, height: 3.0 }
    }
    #[inline]
    pub fn contains(&self, p: Vec3) -> bool {
        let dy = p.y - self.pos.y;
        if dy < -1.2 || dy > self.height { return false; }
        let dx = p.x - self.pos.x;
        let dz = p.z - self.pos.z;
        dx * dx + dz * dz <= self.radius * self.radius
    }
}

/// A world pickup: ammo crates, armour plates and floor weapons.
#[derive(Copy, Clone, Debug)]
pub struct PickupSpot {
    pub pos: Vec3,
    pub kind: PickupKind,
    pub respawn: f32,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PickupKind {
    Ammo = 0,
    Armor = 1,
    /// Index into the weapon table; resolved by the caller.
    Weapon = 2,
    Health = 3,
    Grenade = 4,
}

impl PickupKind {
    pub fn from_u8(v: u8) -> PickupKind {
        match v {
            1 => PickupKind::Armor, 2 => PickupKind::Weapon,
            3 => PickupKind::Health, 4 => PickupKind::Grenade,
            _ => PickupKind::Ammo,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            PickupKind::Ammo => "AMMO", PickupKind::Armor => "ARMOR",
            PickupKind::Weapon => "WEAPON", PickupKind::Health => "MEDKIT",
            PickupKind::Grenade => "GRENADE",
        }
    }
}

/// Stance affects height, speed, spread and hitbox.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum Stance {
    #[default]
    Stand = 0,
    Crouch = 1,
    Prone = 2,
}

impl Stance {
    pub fn from_u8(v: u8) -> Stance {
        match v { 1 => Stance::Crouch, 2 => Stance::Prone, _ => Stance::Stand }
    }
    /// Collision-box height in metres.
    pub fn height(self) -> f32 {
        match self { Stance::Stand => 1.78, Stance::Crouch => 1.20, Stance::Prone => 0.62 }
    }
    /// Eye height above the feet.
    pub fn eye(self) -> f32 {
        match self { Stance::Stand => 1.62, Stance::Crouch => 1.04, Stance::Prone => 0.44 }
    }
    pub fn speed_mult(self) -> f32 {
        match self { Stance::Stand => 1.0, Stance::Crouch => 0.52, Stance::Prone => 0.22 }
    }
    /// Multiplier on weapon spread; going low is rewarded.
    pub fn spread_mult(self) -> f32 {
        match self { Stance::Stand => 1.0, Stance::Crouch => 0.72, Stance::Prone => 0.50 }
    }
}

/// Where a shot landed, for damage multipliers and hit feedback.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum HitZone {
    Body = 0,
    Head = 1,
    Limb = 2,
}

/// How a player died, used by the kill feed and the medal system.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum DeathCause {
    Bullet = 0,
    Headshot = 1,
    Explosion = 2,
    Melee = 3,
    Fire = 4,
    Fall = 5,
    World = 6,
    Bomb = 7,
    /// The player asked to be reprinted. Dying on purpose is a movement
    /// option in a game where everybody is a copy.
    Reinstance = 8,
}

impl DeathCause {
    pub fn from_u8(v: u8) -> DeathCause {
        match v {
            1 => DeathCause::Headshot, 2 => DeathCause::Explosion, 3 => DeathCause::Melee,
            4 => DeathCause::Fire, 5 => DeathCause::Fall, 6 => DeathCause::World,
            7 => DeathCause::Bomb, 8 => DeathCause::Reinstance,
            _ => DeathCause::Bullet,
        }
    }
}

bitflags_lite! {
    /// Buttons packed into the input command. Sixteen bits is plenty and keeps
    /// the command small enough that we can send several per packet for
    /// redundancy against packet loss.
    pub struct Buttons: u16 {
        const FIRE      = 1 << 0;
        const ADS       = 1 << 1;
        const JUMP      = 1 << 2;
        const CROUCH    = 1 << 3;
        const PRONE     = 1 << 4;
        const SPRINT    = 1 << 5;
        const RELOAD    = 1 << 6;
        const MELEE     = 1 << 7;
        const LETHAL    = 1 << 8;
        const TACTICAL  = 1 << 9;
        const USE       = 1 << 10;
        const NEXT_WEAP = 1 << 11;
        const PREV_WEAP = 1 << 12;
        const RESPAWN   = 1 << 13;
    }
}

bitflags_lite! {
    /// Replicated per-player state bits.
    pub struct PFlags: u16 {
        const GROUNDED  = 1 << 0;
        const SPRINTING = 1 << 1;
        const ADS       = 1 << 2;
        const RELOADING = 1 << 3;
        const DEAD      = 1 << 4;
        const FIRING    = 1 << 5;
        const MELEEING  = 1 << 6;
        const SWITCHING = 1 << 7;
        const ON_LADDER = 1 << 8;
        const FLASHED   = 1 << 9;
        const PLANTING  = 1 << 10;
        const CARRYING  = 1 << 11;
        const SPAWNPROT = 1 << 12;
    }
}

/// Input sampled by the client and executed by the server. This is the only
/// thing the client is ever authoritative about, and even here the server
/// clamps every field.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct InputCmd {
    pub seq: u32,
    /// Duration this command covers, milliseconds. Clamped server-side to a
    /// sane window so a client cannot claim a 10-second step.
    pub dt_ms: u8,
    pub move_f: i8,
    pub move_r: i8,
    pub yaw: f32,
    pub pitch: f32,
    pub buttons: Buttons,
    /// Requested weapon slot, or `0xFF` for no change.
    pub weapon: u8,
}

impl InputCmd {
    pub const MAX_DT_MS: u8 = 60;
    pub const MIN_DT_MS: u8 = 1;

    pub fn dt(&self) -> f32 {
        (self.dt_ms.clamp(Self::MIN_DT_MS, Self::MAX_DT_MS) as f32) / 1000.0
    }

    /// Server-side sanitisation. Applied to every command received before it
    /// touches the simulation.
    pub fn sanitize(&mut self) {
        if !self.yaw.is_finite() { self.yaw = 0.0; }
        if !self.pitch.is_finite() { self.pitch = 0.0; }
        self.pitch = self.pitch.clamp(-1.53, 1.53);
        self.yaw = self.yaw.rem_euclid(std::f32::consts::TAU);
        self.dt_ms = self.dt_ms.clamp(Self::MIN_DT_MS, Self::MAX_DT_MS);
        // Analogue-style axes are normalised so diagonal input cannot exceed
        // the straight-line speed.
        let f = self.move_f as f32 / 127.0;
        let r = self.move_r as f32 / 127.0;
        let len = (f * f + r * r).sqrt();
        if len > 1.0 {
            self.move_f = (f / len * 127.0) as i8;
            self.move_r = (r / len * 127.0) as i8;
        }
    }

    #[inline] pub fn forward(&self) -> f32 { self.move_f as f32 / 127.0 }
    #[inline] pub fn strafe(&self) -> f32 { self.move_r as f32 / 127.0 }
    #[inline] pub fn held(&self, b: Buttons) -> bool { self.buttons.contains(b) }
}

/// A single scoreboard row, replicated to every client.
#[derive(Clone, Debug, Default)]
pub struct ScoreEntry {
    pub kills: u16,
    pub deaths: u16,
    pub assists: u16,
    pub score: i32,
    pub streak: u16,
    pub best_streak: u16,
    pub captures: u16,
    pub defends: u16,
    pub plants: u16,
    pub defuses: u16,
    pub headshots: u16,
    pub damage: u32,
}

impl ScoreEntry {
    pub fn kd(&self) -> f32 {
        if self.deaths == 0 { self.kills as f32 } else { self.kills as f32 / self.deaths as f32 }
    }
    pub fn reset_match(&mut self) { *self = ScoreEntry::default(); }
}
