//! Wire protocol.
//!
//! UDP, hand-rolled, byte-aligned. The design targets a twelve-player match on
//! a domestic connection: a snapshot for a full server fits comfortably inside
//! one datagram, and the only things sent reliably are the things a client
//! genuinely cannot infer.

use super::bits::*;
use crate::game::events::{AnnounceLine, GameEvent, ImpactKind};
use crate::game::loadout::{Equipment, Loadout};
use crate::game::types::*;
use crate::game::weapons::WeaponId;
use crate::assets::materials::Surface;
use crate::maps::MapId;
use glam::Vec3;

/// "HPN" plus a version nibble. Changing this makes old clients bounce off
/// rather than misinterpret a newer server's packets.
pub const PROTOCOL_MAGIC: u32 = 0x4850_4E31;
pub const PROTOCOL_VERSION: u16 = 1;

/// Kept below the smallest MTU we are likely to meet, so nothing fragments.
pub const MAX_PACKET: usize = 1200;
pub const DEFAULT_PORT: u16 = 27015;
/// Ports scanned when looking for games on the local network.
pub const DISCOVERY_PORTS: std::ops::Range<u16> = 27015..27023;

pub const MAX_NAME_LEN: usize = 20;
pub const MAX_CHAT_LEN: usize = 120;
/// Commands buffered into one input packet, for redundancy against loss.
pub const INPUT_REDUNDANCY: usize = 4;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PacketKind {
    ConnectRequest = 1,
    Challenge = 2,
    ConnectResponse = 3,
    Accepted = 4,
    Denied = 5,
    Payload = 6,
    Disconnect = 7,
    Discovery = 8,
    DiscoveryReply = 9,
    KeepAlive = 10,
    TrackerRegister = 11,
    TrackerQuery = 12,
    TrackerList = 13,
    TrackerHeartbeat = 14,
}

impl PacketKind {
    pub fn from_u8(v: u8) -> Option<PacketKind> {
        use PacketKind::*;
        Some(match v {
            1 => ConnectRequest, 2 => Challenge, 3 => ConnectResponse,
            4 => Accepted, 5 => Denied, 6 => Payload, 7 => Disconnect,
            8 => Discovery, 9 => DiscoveryReply, 10 => KeepAlive,
            11 => TrackerRegister, 12 => TrackerQuery, 13 => TrackerList,
            14 => TrackerHeartbeat,
            _ => return None,
        })
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum DenyReason {
    ServerFull = 0,
    BadVersion = 1,
    BadPassword = 2,
    Banned = 3,
    Duplicate = 4,
}

impl DenyReason {
    pub fn from_u8(v: u8) -> DenyReason {
        match v {
            1 => DenyReason::BadVersion, 2 => DenyReason::BadPassword,
            3 => DenyReason::Banned, 4 => DenyReason::Duplicate,
            _ => DenyReason::ServerFull,
        }
    }
    pub fn text(self) -> &'static str {
        match self {
            DenyReason::ServerFull => "SERVER FULL",
            DenyReason::BadVersion => "VERSION MISMATCH",
            DenyReason::BadPassword => "WRONG PASSWORD",
            DenyReason::Banned => "CONNECTION REFUSED",
            DenyReason::Duplicate => "ALREADY CONNECTED",
        }
    }
}

// ===================================================================== header

/// Common header on every connected packet.
#[derive(Copy, Clone, Debug)]
pub struct PacketHeader {
    pub token: u64,
    pub seq: u16,
    pub ack: u16,
    pub ack_bits: u32,
    /// Highest reliable message the sender has processed in order.
    pub reliable_ack: u32,
}

impl PacketHeader {
    pub const SIZE: usize = 4 + 1 + 8 + 2 + 2 + 4 + 4;

    pub fn write(&self, w: &mut Writer, kind: PacketKind) {
        w.u32(PROTOCOL_MAGIC);
        w.u8(kind as u8);
        w.u64(self.token);
        w.u16(self.seq);
        w.u16(self.ack);
        w.u32(self.ack_bits);
        w.u32(self.reliable_ack);
    }

    pub fn read(r: &mut Reader) -> Option<(PacketKind, PacketHeader)> {
        if r.u32()? != PROTOCOL_MAGIC { return None; }
        let kind = PacketKind::from_u8(r.u8()?)?;
        Some((kind, PacketHeader {
            token: r.u64()?,
            seq: r.u16()?,
            ack: r.u16()?,
            ack_bits: r.u32()?,
            reliable_ack: r.u32()?,
        }))
    }
}

/// Reads only the magic and kind, for unconnected packets.
pub fn peek_kind(buf: &[u8]) -> Option<PacketKind> {
    let mut r = Reader::new(buf);
    if r.u32()? != PROTOCOL_MAGIC { return None; }
    PacketKind::from_u8(r.u8()?)
}

// =================================================================== messages

/// Reliable, ordered messages from server to client.
#[derive(Clone, Debug)]
pub enum ServerMsg {
    /// Sent once on connect, then whenever the match configuration changes.
    MatchConfig {
        server_name: String,
        map: MapId,
        mode: u8,
        score_limit: u16,
        time_limit: u16,
        max_players: u8,
        friendly_fire: bool,
        bot_count: u8,
        phase: u8,
    },
    PlayerInfo {
        slot: u8,
        name: String,
        team: Team,
        is_bot: bool,
        level: u8,
        ping: u16,
        ready: bool,
    },
    PlayerLeft { slot: u8 },
    Scores { entries: Vec<(u8, u16, u16, u16, i32, u16)> },
    TeamScores { phantom: u16, vanguard: u16 },
    Chat { slot: u8, team_only: bool, text: String },
    /// Announcer cue plus its subtitle.
    Announce { line: AnnounceLine },
    /// Phase change; the client uses this to drive the match flow UI.
    Phase { phase: u8, seconds_left: u16, round: u16 },
    /// Load a different map; the client shows the loading screen.
    LoadMap { map: MapId, mode: u8 },
    /// Objective state for the HUD: domination ownership or bomb status.
    Objectives { data: [u8; 8] },
    /// Final results, sent once at match end.
    Results { winner: Team, mvp: u8 },
    Kick { reason: String },
    /// The server's own view of the lobby, used before a match starts.
    LobbyInfo { host_slot: u8, countdown: u16, ready_mask: u16 },
}

/// Reliable, ordered messages from client to server.
#[derive(Clone, Debug)]
pub enum ClientMsg {
    Hello { name: String, level: u8, loadout: [u8; 7] },
    SetLoadout { loadout: [u8; 7] },
    Chat { team_only: bool, text: String },
    ChangeTeam { team: Team },
    Ready { ready: bool },
    RequestRespawn,
    /// Host-only: change map, mode or bot count from the lobby.
    HostConfig { map: u8, mode: u8, bots: u8, score_limit: u16, time_limit: u16, friendly_fire: bool },
    HostStart,
    Disconnecting,
}

impl ServerMsg {
    pub fn encode(&self, w: &mut Writer) {
        match self {
            ServerMsg::MatchConfig { server_name, map, mode, score_limit, time_limit, max_players, friendly_fire, bot_count, phase } => {
                w.u8(1);
                w.string(server_name, 31);
                w.u8(*map as u8);
                w.u8(*mode);
                w.u16(*score_limit);
                w.u16(*time_limit);
                w.u8(*max_players);
                w.bool(*friendly_fire);
                w.u8(*bot_count);
                w.u8(*phase);
            }
            ServerMsg::PlayerInfo { slot, name, team, is_bot, level, ping, ready } => {
                w.u8(2);
                w.u8(*slot);
                w.string(name, MAX_NAME_LEN);
                w.u8(*team as u8);
                w.bool(*is_bot);
                w.u8(*level);
                w.u16(*ping);
                w.bool(*ready);
            }
            ServerMsg::PlayerLeft { slot } => { w.u8(3); w.u8(*slot); }
            ServerMsg::Scores { entries } => {
                w.u8(4);
                w.u8(entries.len().min(255) as u8);
                for (slot, kills, deaths, assists, score, ping) in entries.iter().take(255) {
                    w.u8(*slot);
                    w.u16(*kills);
                    w.u16(*deaths);
                    w.u16(*assists);
                    w.u32(*score as u32);
                    w.u16(*ping);
                }
            }
            ServerMsg::TeamScores { phantom, vanguard } => {
                w.u8(5); w.u16(*phantom); w.u16(*vanguard);
            }
            ServerMsg::Chat { slot, team_only, text } => {
                w.u8(6); w.u8(*slot); w.bool(*team_only); w.string(text, MAX_CHAT_LEN);
            }
            ServerMsg::Announce { line } => { w.u8(7); w.u8(*line as u8); }
            ServerMsg::Phase { phase, seconds_left, round } => {
                w.u8(8); w.u8(*phase); w.u16(*seconds_left); w.u16(*round);
            }
            ServerMsg::LoadMap { map, mode } => { w.u8(9); w.u8(*map as u8); w.u8(*mode); }
            ServerMsg::Objectives { data } => { w.u8(10); w.bytes(data); }
            ServerMsg::Results { winner, mvp } => { w.u8(11); w.u8(*winner as u8); w.u8(*mvp); }
            ServerMsg::Kick { reason } => { w.u8(12); w.string(reason, 63); }
            ServerMsg::LobbyInfo { host_slot, countdown, ready_mask } => {
                w.u8(13); w.u8(*host_slot); w.u16(*countdown); w.u16(*ready_mask);
            }
        }
    }

    pub fn decode(r: &mut Reader) -> Option<ServerMsg> {
        Some(match r.u8()? {
            1 => ServerMsg::MatchConfig {
                server_name: r.lossy_string(31)?,
                map: MapId::from_u8(r.u8()?),
                mode: r.u8()?,
                score_limit: r.u16()?,
                time_limit: r.u16()?,
                max_players: r.u8()?,
                friendly_fire: r.bool()?,
                bot_count: r.u8()?,
                phase: r.u8()?,
            },
            2 => ServerMsg::PlayerInfo {
                slot: r.u8()?,
                name: r.lossy_string(MAX_NAME_LEN)?,
                team: Team::from_u8(r.u8()?),
                is_bot: r.bool()?,
                level: r.u8()?,
                ping: r.u16()?,
                ready: r.bool()?,
            },
            3 => ServerMsg::PlayerLeft { slot: r.u8()? },
            4 => {
                let n = r.u8()? as usize;
                let mut entries = Vec::with_capacity(n);
                for _ in 0..n {
                    entries.push((r.u8()?, r.u16()?, r.u16()?, r.u16()?, r.u32()? as i32, r.u16()?));
                }
                ServerMsg::Scores { entries }
            }
            5 => ServerMsg::TeamScores { phantom: r.u16()?, vanguard: r.u16()? },
            6 => ServerMsg::Chat {
                slot: r.u8()?, team_only: r.bool()?, text: r.lossy_string(MAX_CHAT_LEN)?,
            },
            7 => ServerMsg::Announce { line: AnnounceLine::from_u8(r.u8()?) },
            8 => ServerMsg::Phase { phase: r.u8()?, seconds_left: r.u16()?, round: r.u16()? },
            9 => ServerMsg::LoadMap { map: MapId::from_u8(r.u8()?), mode: r.u8()? },
            10 => {
                let b = r.bytes(8)?;
                let mut data = [0u8; 8];
                data.copy_from_slice(b);
                ServerMsg::Objectives { data }
            }
            11 => ServerMsg::Results { winner: Team::from_u8(r.u8()?), mvp: r.u8()? },
            12 => ServerMsg::Kick { reason: r.lossy_string(63)? },
            13 => ServerMsg::LobbyInfo { host_slot: r.u8()?, countdown: r.u16()?, ready_mask: r.u16()? },
            _ => return None,
        })
    }
}

impl ClientMsg {
    pub fn encode(&self, w: &mut Writer) {
        match self {
            ClientMsg::Hello { name, level, loadout } => {
                w.u8(1); w.string(name, MAX_NAME_LEN); w.u8(*level); w.bytes(loadout);
            }
            ClientMsg::SetLoadout { loadout } => { w.u8(2); w.bytes(loadout); }
            ClientMsg::Chat { team_only, text } => {
                w.u8(3); w.bool(*team_only); w.string(text, MAX_CHAT_LEN);
            }
            ClientMsg::ChangeTeam { team } => { w.u8(4); w.u8(*team as u8); }
            ClientMsg::Ready { ready } => { w.u8(5); w.bool(*ready); }
            ClientMsg::RequestRespawn => { w.u8(6); }
            ClientMsg::HostConfig { map, mode, bots, score_limit, time_limit, friendly_fire } => {
                w.u8(7); w.u8(*map); w.u8(*mode); w.u8(*bots);
                w.u16(*score_limit); w.u16(*time_limit); w.bool(*friendly_fire);
            }
            ClientMsg::HostStart => { w.u8(8); }
            ClientMsg::Disconnecting => { w.u8(9); }
        }
    }

    pub fn decode(r: &mut Reader) -> Option<ClientMsg> {
        Some(match r.u8()? {
            1 => {
                let name = r.lossy_string(MAX_NAME_LEN)?;
                let level = r.u8()?;
                let mut loadout = [0u8; 7];
                loadout.copy_from_slice(r.bytes(7)?);
                ClientMsg::Hello { name, level, loadout }
            }
            2 => {
                let mut loadout = [0u8; 7];
                loadout.copy_from_slice(r.bytes(7)?);
                ClientMsg::SetLoadout { loadout }
            }
            3 => ClientMsg::Chat { team_only: r.bool()?, text: r.lossy_string(MAX_CHAT_LEN)? },
            4 => ClientMsg::ChangeTeam { team: Team::from_u8(r.u8()?) },
            5 => ClientMsg::Ready { ready: r.bool()? },
            6 => ClientMsg::RequestRespawn,
            7 => ClientMsg::HostConfig {
                map: r.u8()?, mode: r.u8()?, bots: r.u8()?,
                score_limit: r.u16()?, time_limit: r.u16()?, friendly_fire: r.bool()?,
            },
            8 => ClientMsg::HostStart,
            9 => ClientMsg::Disconnecting,
            _ => return None,
        })
    }
}

// ================================================================== snapshots

bitflags_lite! {
    /// Which fields of a player changed since the client's baseline.
    pub struct SnapField: u16 {
        const POS     = 1 << 0;
        const VEL     = 1 << 1;
        const ANGLES  = 1 << 2;
        const HEIGHT  = 1 << 3;
        const FLAGS   = 1 << 4;
        const HEALTH  = 1 << 5;
        const WEAPON  = 1 << 6;
        const TEAM    = 1 << 7;
        /// The entity left the client's relevance set.
        const GONE    = 1 << 8;
    }
}

/// One player's replicated state.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct PlayerSnap {
    pub pos: Vec3,
    pub vel: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub height: f32,
    pub flags: PFlags,
    pub health: u8,
    pub armor: u8,
    pub weapon: u8,
    pub ammo: u8,
    pub team: u8,
    pub present: bool,
}

impl PlayerSnap {
    pub fn write_delta(&self, w: &mut Writer, base: &PlayerSnap, slot: u8) {
        let mut mask = SnapField::empty();
        if !base.present
            || quantize_pos(self.pos.x) != quantize_pos(base.pos.x)
            || quantize_pos(self.pos.y) != quantize_pos(base.pos.y)
            || quantize_pos(self.pos.z) != quantize_pos(base.pos.z)
        { mask.insert(SnapField::POS); }
        if !base.present
            || quantize_vel(self.vel.x) != quantize_vel(base.vel.x)
            || quantize_vel(self.vel.y) != quantize_vel(base.vel.y)
            || quantize_vel(self.vel.z) != quantize_vel(base.vel.z)
        { mask.insert(SnapField::VEL); }
        if !base.present || quantize_yaw(self.yaw) != quantize_yaw(base.yaw)
            || quantize_pitch(self.pitch) != quantize_pitch(base.pitch)
        { mask.insert(SnapField::ANGLES); }
        if !base.present || quantize_height(self.height) != quantize_height(base.height) {
            mask.insert(SnapField::HEIGHT);
        }
        if !base.present || self.flags != base.flags { mask.insert(SnapField::FLAGS); }
        if !base.present || self.health != base.health || self.armor != base.armor {
            mask.insert(SnapField::HEALTH);
        }
        if !base.present || self.weapon != base.weapon || self.ammo != base.ammo {
            mask.insert(SnapField::WEAPON);
        }
        if !base.present || self.team != base.team { mask.insert(SnapField::TEAM); }

        w.u8(slot);
        w.u16(mask.bits());
        if mask.contains(SnapField::POS) {
            w.i16(quantize_pos(self.pos.x));
            w.i16(quantize_pos(self.pos.y));
            w.i16(quantize_pos(self.pos.z));
        }
        if mask.contains(SnapField::VEL) {
            w.i16(quantize_vel(self.vel.x));
            w.i16(quantize_vel(self.vel.y));
            w.i16(quantize_vel(self.vel.z));
        }
        if mask.contains(SnapField::ANGLES) {
            w.u16(quantize_yaw(self.yaw));
            w.i16(quantize_pitch(self.pitch));
        }
        if mask.contains(SnapField::HEIGHT) { w.u8(quantize_height(self.height)); }
        if mask.contains(SnapField::FLAGS) { w.u16(self.flags.bits()); }
        if mask.contains(SnapField::HEALTH) { w.u8(self.health); w.u8(self.armor); }
        if mask.contains(SnapField::WEAPON) { w.u8(self.weapon); w.u8(self.ammo); }
        if mask.contains(SnapField::TEAM) { w.u8(self.team); }
    }

    /// Reads which slot the next delta is for, and which fields it carries.
    /// Read separately from the body so the caller can look up the right
    /// baseline before decoding: deltaing against the wrong player's previous
    /// state would silently corrupt every field the packet omitted.
    pub fn read_delta_header(r: &mut Reader) -> Option<(u8, SnapField)> {
        let slot = r.u8()?;
        let mask = SnapField::from_bits_truncate(r.u16()?);
        Some((slot, mask))
    }

    /// Applies a delta body on top of the correct baseline.
    pub fn read_delta_body(r: &mut Reader, base: &PlayerSnap, mask: SnapField) -> Option<PlayerSnap> {
        let mut s = *base;
        s.present = true;
        if mask.contains(SnapField::POS) {
            s.pos = Vec3::new(
                dequantize_pos(r.i16()?),
                dequantize_pos(r.i16()?),
                dequantize_pos(r.i16()?),
            );
        }
        if mask.contains(SnapField::VEL) {
            s.vel = Vec3::new(
                dequantize_vel(r.i16()?),
                dequantize_vel(r.i16()?),
                dequantize_vel(r.i16()?),
            );
        }
        if mask.contains(SnapField::ANGLES) {
            s.yaw = dequantize_yaw(r.u16()?);
            s.pitch = dequantize_pitch(r.i16()?);
        }
        if mask.contains(SnapField::HEIGHT) { s.height = dequantize_height(r.u8()?); }
        if mask.contains(SnapField::FLAGS) { s.flags = PFlags::from_bits_truncate(r.u16()?); }
        if mask.contains(SnapField::HEALTH) { s.health = r.u8()?; s.armor = r.u8()?; }
        if mask.contains(SnapField::WEAPON) { s.weapon = r.u8()?; s.ammo = r.u8()?; }
        if mask.contains(SnapField::TEAM) { s.team = r.u8()?; }
        Some(s)
    }
}

/// A complete snapshot as the client sees it.
#[derive(Clone)]
pub struct Snapshot {
    pub tick: u32,
    pub server_time_ms: u32,
    /// Last input command the server had executed for this client.
    pub acked_input: u32,
    pub players: [PlayerSnap; MAX_PLAYERS],
    /// Local player's authoritative movement state, sent at full precision so
    /// reconciliation is exact rather than fighting quantisation.
    pub local: Option<LocalState>,
    pub received_at: f64,
}

impl Default for Snapshot {
    fn default() -> Self {
        Snapshot {
            tick: 0, server_time_ms: 0, acked_input: 0,
            players: [PlayerSnap::default(); MAX_PLAYERS],
            local: None,
            received_at: 0.0,
        }
    }
}

/// The authoritative state of the receiving client's own player. Full
/// precision, because any rounding here shows up as prediction error.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct LocalState {
    pub pos: Vec3,
    pub vel: Vec3,
    pub height: f32,
    pub stance: u8,
    pub grounded: bool,
    pub sprint_t: f32,
    pub ads_t: f32,
    pub health: f32,
    pub armor: f32,
    pub weapon_slot: u8,
    pub ammo: [u16; 3],
    pub reserve: [u16; 3],
    pub lethal: u8,
    pub tactical: u8,
    pub flash: f32,
    pub concussion: f32,
    pub alive: bool,
    pub respawn_in: f32,
}

impl LocalState {
    pub fn write(&self, w: &mut Writer) {
        w.f32(self.pos.x); w.f32(self.pos.y); w.f32(self.pos.z);
        w.f32(self.vel.x); w.f32(self.vel.y); w.f32(self.vel.z);
        w.f32(self.height);
        w.u8(self.stance);
        w.bool(self.grounded);
        w.u8((self.sprint_t * 255.0) as u8);
        w.u8((self.ads_t * 255.0) as u8);
        w.u16((self.health * 4.0).clamp(0.0, 65535.0) as u16);
        w.u16((self.armor * 4.0).clamp(0.0, 65535.0) as u16);
        w.u8(self.weapon_slot);
        for i in 0..3 { w.u16(self.ammo[i]); }
        for i in 0..3 { w.u16(self.reserve[i]); }
        w.u8(self.lethal);
        w.u8(self.tactical);
        w.u8((self.flash * 32.0).clamp(0.0, 255.0) as u8);
        w.u8((self.concussion * 32.0).clamp(0.0, 255.0) as u8);
        w.bool(self.alive);
        w.u8((self.respawn_in * 8.0).clamp(0.0, 255.0) as u8);
    }

    pub fn read(r: &mut Reader) -> Option<LocalState> {
        let mut s = LocalState {
            pos: Vec3::new(r.f32()?, r.f32()?, r.f32()?),
            vel: Vec3::new(r.f32()?, r.f32()?, r.f32()?),
            height: r.f32()?,
            stance: r.u8()?,
            grounded: r.bool()?,
            sprint_t: r.u8()? as f32 / 255.0,
            ads_t: r.u8()? as f32 / 255.0,
            health: r.u16()? as f32 / 4.0,
            armor: r.u16()? as f32 / 4.0,
            weapon_slot: r.u8()?,
            ammo: [0; 3],
            reserve: [0; 3],
            lethal: 0, tactical: 0, flash: 0.0, concussion: 0.0,
            alive: false, respawn_in: 0.0,
        };
        for i in 0..3 { s.ammo[i] = r.u16()?; }
        for i in 0..3 { s.reserve[i] = r.u16()?; }
        s.lethal = r.u8()?;
        s.tactical = r.u8()?;
        s.flash = r.u8()? as f32 / 32.0;
        s.concussion = r.u8()? as f32 / 32.0;
        s.alive = r.bool()?;
        s.respawn_in = r.u8()? as f32 / 8.0;
        Some(s)
    }
}

// ===================================================================== events

/// Encodes an event for the wire. Returns false for events that are not
/// replicated (purely server-side bookkeeping).
pub fn encode_event(w: &mut Writer, e: &GameEvent) -> bool {
    use GameEvent::*;
    match e {
        Shot { player, weapon, origin, dir, seed } => {
            w.u8(1); w.u8(*player); w.u8(*weapon as u8);
            w.i16(quantize_pos(origin.x)); w.i16(quantize_pos(origin.y)); w.i16(quantize_pos(origin.z));
            let d = quantize_dir(*dir);
            w.i8(d[0]); w.i8(d[1]); w.i8(d[2]);
            w.u32(*seed);
        }
        Impact { pos, normal, surface, kind } => {
            w.u8(2);
            w.i16(quantize_pos(pos.x)); w.i16(quantize_pos(pos.y)); w.i16(quantize_pos(pos.z));
            let n = quantize_dir(*normal);
            w.i8(n[0]); w.i8(n[1]); w.i8(n[2]);
            w.u8(surface.index() as u8);
            w.u8(*kind as u8);
        }
        HitPlayer { attacker, victim, pos, zone, damage, lethal } => {
            w.u8(3); w.u8(*attacker); w.u8(*victim);
            w.i16(quantize_pos(pos.x)); w.i16(quantize_pos(pos.y)); w.i16(quantize_pos(pos.z));
            w.u8(*zone as u8); w.u16(*damage); w.bool(*lethal);
        }
        Kill { killer, victim, weapon, cause, distance, streak } => {
            w.u8(4); w.u8(*killer); w.u8(*victim); w.u8(*weapon as u8);
            w.u8(*cause as u8); w.u16((*distance * 4.0).clamp(0.0, 65535.0) as u16); w.u16(*streak);
        }
        Melee { player, hit } => { w.u8(5); w.u8(*player); w.bool(*hit); }
        Reload { player, empty } => { w.u8(6); w.u8(*player); w.bool(*empty); }
        Swap { player, slot } => { w.u8(7); w.u8(*player); w.u8(*slot); }
        DryFire { player } => { w.u8(8); w.u8(*player); }
        GrenadeThrown { id, player, kind, pos, vel } => {
            w.u8(9); w.u16(*id); w.u8(*player); w.u8(*kind as u8);
            w.i16(quantize_pos(pos.x)); w.i16(quantize_pos(pos.y)); w.i16(quantize_pos(pos.z));
            w.i16(quantize_vel(vel.x)); w.i16(quantize_vel(vel.y)); w.i16(quantize_vel(vel.z));
        }
        GrenadeBounce { id, pos, surface } => {
            w.u8(10); w.u16(*id);
            w.i16(quantize_pos(pos.x)); w.i16(quantize_pos(pos.y)); w.i16(quantize_pos(pos.z));
            w.u8(surface.index() as u8);
        }
        Explosion { pos, kind, radius } => {
            w.u8(11);
            w.i16(quantize_pos(pos.x)); w.i16(quantize_pos(pos.y)); w.i16(quantize_pos(pos.z));
            w.u8(*kind as u8); w.u8((radius * 8.0).clamp(0.0, 255.0) as u8);
        }
        Blinded { player, strength, concussion } => {
            w.u8(12); w.u8(*player); w.u8((strength * 32.0).clamp(0.0, 255.0) as u8); w.bool(*concussion);
        }
        SmokeStarted { id, pos } => {
            w.u8(13); w.u16(*id);
            w.i16(quantize_pos(pos.x)); w.i16(quantize_pos(pos.y)); w.i16(quantize_pos(pos.z));
        }
        FireStarted { id, pos, radius } => {
            w.u8(14); w.u16(*id);
            w.i16(quantize_pos(pos.x)); w.i16(quantize_pos(pos.y)); w.i16(quantize_pos(pos.z));
            w.u8((radius * 8.0).clamp(0.0, 255.0) as u8);
        }
        Footstep { player, pos, surface, volume } => {
            w.u8(15); w.u8(*player);
            w.i16(quantize_pos(pos.x)); w.i16(quantize_pos(pos.y)); w.i16(quantize_pos(pos.z));
            w.u8(surface.index() as u8); w.u8((volume * 255.0).clamp(0.0, 255.0) as u8);
        }
        Land { player, pos, speed, surface } => {
            w.u8(16); w.u8(*player);
            w.i16(quantize_pos(pos.x)); w.i16(quantize_pos(pos.y)); w.i16(quantize_pos(pos.z));
            w.u8((speed * 4.0).clamp(0.0, 255.0) as u8); w.u8(surface.index() as u8);
        }
        Jump { player } => { w.u8(17); w.u8(*player); }
        Spawned { player, pos } => {
            w.u8(18); w.u8(*player);
            w.i16(quantize_pos(pos.x)); w.i16(quantize_pos(pos.y)); w.i16(quantize_pos(pos.z));
        }
        PickupTaken { player, index, kind } => {
            w.u8(19); w.u8(*player); w.u16(*index); w.u8(*kind as u8);
        }
        PickupRespawned { index } => { w.u8(20); w.u16(*index); }
        ShellLoaded { player } => { w.u8(21); w.u8(*player); }
        CapturePoint { point, team, contested } => {
            w.u8(22); w.u8(*point); w.u8(*team as u8); w.bool(*contested);
        }
        CaptureProgress { point, team, progress } => {
            w.u8(23); w.u8(*point); w.u8(*team as u8); w.u8(*progress);
        }
        BombPlanted { site, by } => { w.u8(24); w.u8(*site); w.u8(*by); }
        BombDefused { by } => { w.u8(25); w.u8(*by); }
        BombExploded { site } => { w.u8(26); w.u8(*site); }
        BombPickedUp { by } => { w.u8(27); w.u8(*by); }
        RoundReset => { w.u8(30); }
        BrushBroken { brush, pos, surface } => {
            w.u8(29); w.u32(*brush);
            w.i16(quantize_pos(pos.x)); w.i16(quantize_pos(pos.y)); w.i16(quantize_pos(pos.z));
            w.u8(surface.index() as u8);
        }
        BombDropped { pos } => {
            w.u8(28);
            w.i16(quantize_pos(pos.x)); w.i16(quantize_pos(pos.y)); w.i16(quantize_pos(pos.z));
        }
        _ => return false,
    }
    true
}

pub fn decode_event(r: &mut Reader) -> Option<GameEvent> {
    use GameEvent::*;
    let tag = r.u8()?;
    Some(match tag {
        1 => Shot {
            player: r.u8()?,
            weapon: WeaponId::from_u8(r.u8()?),
            origin: read_pos(r)?,
            dir: dequantize_dir([r.i8()?, r.i8()?, r.i8()?]),
            seed: r.u32()?,
        },
        2 => Impact {
            pos: read_pos(r)?,
            normal: dequantize_dir([r.i8()?, r.i8()?, r.i8()?]),
            surface: surface_from(r.u8()?),
            kind: match r.u8()? { 1 => ImpactKind::Pellet, 2 => ImpactKind::Explosion, 3 => ImpactKind::Melee, _ => ImpactKind::Bullet },
        },
        3 => HitPlayer {
            attacker: r.u8()?, victim: r.u8()?, pos: read_pos(r)?,
            zone: match r.u8()? { 1 => HitZone::Head, 2 => HitZone::Limb, _ => HitZone::Body },
            damage: r.u16()?, lethal: r.bool()?,
        },
        4 => Kill {
            killer: r.u8()?, victim: r.u8()?, weapon: WeaponId::from_u8(r.u8()?),
            cause: DeathCause::from_u8(r.u8()?), distance: r.u16()? as f32 / 4.0, streak: r.u16()?,
        },
        5 => Melee { player: r.u8()?, hit: r.bool()? },
        6 => Reload { player: r.u8()?, empty: r.bool()? },
        7 => Swap { player: r.u8()?, slot: r.u8()? },
        8 => DryFire { player: r.u8()? },
        9 => GrenadeThrown {
            id: r.u16()?, player: r.u8()?, kind: Equipment::from_u8(r.u8()?),
            pos: read_pos(r)?,
            vel: Vec3::new(dequantize_vel(r.i16()?), dequantize_vel(r.i16()?), dequantize_vel(r.i16()?)),
        },
        10 => GrenadeBounce { id: r.u16()?, pos: read_pos(r)?, surface: surface_from(r.u8()?) },
        11 => Explosion { pos: read_pos(r)?, kind: Equipment::from_u8(r.u8()?), radius: r.u8()? as f32 / 8.0 },
        12 => Blinded { player: r.u8()?, strength: r.u8()? as f32 / 32.0, concussion: r.bool()? },
        13 => SmokeStarted { id: r.u16()?, pos: read_pos(r)? },
        14 => FireStarted { id: r.u16()?, pos: read_pos(r)?, radius: r.u8()? as f32 / 8.0 },
        15 => Footstep {
            player: r.u8()?, pos: read_pos(r)?,
            surface: surface_from(r.u8()?), volume: r.u8()? as f32 / 255.0,
        },
        16 => Land {
            player: r.u8()?, pos: read_pos(r)?,
            speed: r.u8()? as f32 / 4.0, surface: surface_from(r.u8()?),
        },
        17 => Jump { player: r.u8()? },
        18 => Spawned { player: r.u8()?, pos: read_pos(r)? },
        19 => PickupTaken { player: r.u8()?, index: r.u16()?, kind: PickupKind::from_u8(r.u8()?) },
        20 => PickupRespawned { index: r.u16()? },
        21 => ShellLoaded { player: r.u8()? },
        22 => CapturePoint { point: r.u8()?, team: Team::from_u8(r.u8()?), contested: r.bool()? },
        29 => BrushBroken { brush: r.u32()?, pos: read_pos(r)?, surface: surface_from(r.u8()?) },
        30 => RoundReset,
        23 => CaptureProgress { point: r.u8()?, team: Team::from_u8(r.u8()?), progress: r.u8()? },
        24 => BombPlanted { site: r.u8()?, by: r.u8()? },
        25 => BombDefused { by: r.u8()? },
        26 => BombExploded { site: r.u8()? },
        27 => BombPickedUp { by: r.u8()? },
        28 => BombDropped { pos: read_pos(r)? },
        _ => return None,
    })
}

#[inline]
fn read_pos(r: &mut Reader) -> Option<Vec3> {
    Some(Vec3::new(
        dequantize_pos(r.i16()?),
        dequantize_pos(r.i16()?),
        dequantize_pos(r.i16()?),
    ))
}

fn surface_from(v: u8) -> Surface {
    use Surface::*;
    match v {
        1 => Metal, 2 => Wood, 3 => Dirt, 4 => Sand, 5 => Gravel,
        6 => Grass, 7 => Snow, 8 => Glass, 9 => Soft, 10 => Water,
        _ => Concrete,
    }
}

// ===================================================================== inputs

pub fn write_input(w: &mut Writer, cmd: &InputCmd) {
    w.u32(cmd.seq);
    w.u8(cmd.dt_ms);
    w.i8(cmd.move_f);
    w.i8(cmd.move_r);
    w.u16(quantize_yaw(cmd.yaw));
    w.i16(quantize_pitch(cmd.pitch));
    w.u16(cmd.buttons.bits());
    w.u8(cmd.weapon);
}

pub fn read_input(r: &mut Reader) -> Option<InputCmd> {
    let mut cmd = InputCmd {
        seq: r.u32()?,
        dt_ms: r.u8()?,
        move_f: r.i8()?,
        move_r: r.i8()?,
        yaw: dequantize_yaw(r.u16()?),
        pitch: dequantize_pitch(r.i16()?),
        buttons: Buttons::from_bits_truncate(r.u16()?),
        weapon: r.u8()?,
    };
    cmd.sanitize();
    Some(cmd)
}

/// Server description advertised to the browser.
#[derive(Clone, Debug)]
pub struct ServerInfo {
    pub name: String,
    pub map: MapId,
    pub mode: u8,
    pub players: u8,
    pub bots: u8,
    pub max_players: u8,
    pub passworded: bool,
    pub phase: u8,
    pub version: u16,
}

impl ServerInfo {
    pub fn write(&self, w: &mut Writer) {
        w.u16(self.version);
        w.string(&self.name, 31);
        w.u8(self.map as u8);
        w.u8(self.mode);
        w.u8(self.players);
        w.u8(self.bots);
        w.u8(self.max_players);
        w.bool(self.passworded);
        w.u8(self.phase);
    }

    pub fn read(r: &mut Reader) -> Option<ServerInfo> {
        Some(ServerInfo {
            version: r.u16()?,
            name: r.lossy_string(31)?,
            map: MapId::from_u8(r.u8()?),
            mode: r.u8()?,
            players: r.u8()?,
            bots: r.u8()?,
            max_players: r.u8()?,
            passworded: r.bool()?,
            phase: r.u8()?,
        })
    }
}

/// Hash used for the password handshake. Not cryptography: it exists so a
/// password is not sent in the clear across a LAN, nothing more.
pub fn password_hash(password: &str, salt: u64) -> u64 {
    let mut h = salt ^ 0xCBF2_9CE4_8422_2325;
    for b in password.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x1000_0000_01B3);
    }
    h ^= h >> 33;
    h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    h ^ (h >> 29)
}

/// Loadout helpers shared by both sides.
pub fn loadout_to_bytes(l: &Loadout) -> [u8; 7] { l.encode() }
pub fn loadout_from_bytes(b: [u8; 7]) -> Loadout { Loadout::decode(b) }
