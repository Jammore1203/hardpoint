//! The network client.
//!
//! Three jobs: keep a connection alive, predict the local player so input
//! feels instant, and interpolate everyone else so they move smoothly despite
//! arriving twenty-two times a second.
//!
//! Prediction runs the same movement and weapon code the server does. When a
//! snapshot disagrees with what was predicted, the client snaps to the
//! server's state and replays every command the server has not acknowledged
//! yet; the visible position is then eased back over about a tenth of a
//! second so a correction reads as a nudge rather than a teleport.

use super::bits::{Reader, Writer};
use super::channel::Connection;
use super::protocol::*;
use super::socket::{parse_endpoint, Socket};
use crate::core::Ring;
use crate::game::events::GameEvent;
use crate::game::loadout::Loadout;
use crate::game::movement::{self, MoveState};
use crate::game::player::Player;
use crate::game::sim::INTERP_DELAY_MS;
use crate::game::types::*;
use crate::maps::brush::CollisionWorld;
use crate::maps::MapId;
use crate::modes::ModeId;
use glam::Vec3;
use std::collections::VecDeque;
use std::net::SocketAddr;

/// How long to wait for each stage of the handshake before retrying.
const HANDSHAKE_RETRY: f64 = 0.35;
const CONNECT_TIMEOUT: f64 = 8.0;
const DISCONNECT_TIMEOUT: f64 = 10.0;
/// Snapshots kept for interpolation.
const SNAPSHOT_HISTORY: usize = 32;
/// Prediction error above which the client snaps instead of easing.
const HARD_SNAP: f32 = 2.5;

#[derive(Clone, Debug, PartialEq)]
pub enum ClientState {
    Disconnected,
    Connecting,
    Challenged,
    Joining,
    Playing,
    Failed(String),
}

impl ClientState {
    pub fn is_live(&self) -> bool { matches!(self, ClientState::Playing) }
    pub fn is_connecting(&self) -> bool {
        matches!(self, ClientState::Connecting | ClientState::Challenged | ClientState::Joining)
    }
}

/// A remote player as the client understands them.
#[derive(Clone, Copy, Debug, Default)]
pub struct RemotePlayer {
    pub present: bool,
    pub snap: PlayerSnap,
    /// Smoothed render position after interpolation.
    pub render_pos: Vec3,
    pub render_yaw: f32,
    pub render_pitch: f32,
    pub render_height: f32,
    /// Stride phase for the walk cycle.
    pub phase: f32,
    pub speed: f32,
    /// Seconds since this player died, for the collapse animation.
    pub death_time: f32,
    pub firing: f32,
    pub team: Team,
    pub last_update: f64,
}

/// Roster entry, from the reliable channel.
#[derive(Clone, Debug, Default)]
pub struct PlayerInfo {
    pub present: bool,
    pub name: String,
    pub team: Team,
    pub is_bot: bool,
    pub level: u8,
    pub ping: u16,
    pub ready: bool,
    pub kills: u16,
    pub deaths: u16,
    pub assists: u16,
    pub score: i32,
}

/// Everything the client knows about the match it is in.
#[derive(Clone, Debug)]
pub struct MatchInfo {
    pub server_name: String,
    pub map: MapId,
    pub mode: ModeId,
    pub score_limit: u16,
    pub time_limit: u16,
    pub max_players: u8,
    pub friendly_fire: bool,
    pub bot_count: u8,
    pub phase: u8,
    pub seconds_left: u16,
    pub round: u16,
    pub team_scores: [u16; 2],
    pub hud: [u8; 8],
    pub winner: Team,
    pub mvp: u8,
}

impl Default for MatchInfo {
    fn default() -> Self {
        MatchInfo {
            server_name: String::new(),
            map: MapId::Ironveil,
            mode: ModeId::TeamDeathmatch,
            score_limit: 75,
            time_limit: 600,
            max_players: 12,
            friendly_fire: false,
            bot_count: 0,
            phase: 0,
            seconds_left: 0,
            round: 0,
            team_scores: [0; 2],
            hud: [0; 8],
            winner: Team::None,
            mvp: NO_PLAYER,
        }
    }
}

/// A chat line as displayed.
#[derive(Clone, Debug)]
pub struct ChatLine {
    pub slot: u8,
    pub team_only: bool,
    pub text: String,
    pub time: f64,
}

pub struct Client {
    sock: Socket,
    pub server: SocketAddr,
    pub address_text: String,
    pub state: ClientState,
    conn: Option<Connection>,

    client_salt: u64,
    server_salt: u64,
    token: u64,
    handshake_at: f64,
    started_at: f64,
    password: String,
    needs_password: bool,

    pub slot: u8,
    pub time: f64,
    /// Server clock estimate, in seconds.
    pub server_time: f64,
    server_time_offset: f64,
    /// Match clock as of the last Phase message, and when that arrived.
    clock_ref: f32,
    clock_at: f64,

    // ------------------------------------------------------- prediction
    pub local: Player,
    /// Speech frames received since the last drain, as `(speaker, payload)`.
    /// The client does not decode them: the app owns the audio engine.
    pub voice_in: Vec<(u8, Vec<u8>)>,
    pub predicted: MoveState,
    /// Difference between where we drew the player and where the server put
    /// them, eased out over time.
    pub error_offset: Vec3,
    pub cmd_seq: u32,
    history: Ring<(InputCmd, MoveState), 128>,
    pub last_acked_input: u32,
    pub corrections: u32,
    pub max_error: f32,
    /// What the local weapon did during the most recent command.
    pub last_output: crate::game::player::WeaponOutput,

    // ---------------------------------------------------- interpolation
    snapshots: VecDeque<Snapshot>,
    pub players: [RemotePlayer; MAX_PLAYERS],
    baseline: [PlayerSnap; MAX_PLAYERS],
    baseline_tick: u32,

    // ---------------------------------------------------------- received
    pub events: Vec<GameEvent>,
    pub roster: Vec<PlayerInfo>,
    pub match_info: MatchInfo,
    pub chat: VecDeque<ChatLine>,
    pub kicked: Option<String>,
    pub map_change: Option<(MapId, ModeId)>,

    // -------------------------------------------------------------- stats
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub snapshot_hz: f32,
    snapshot_times: VecDeque<f64>,
    outgoing: Vec<u8>,
}

impl Client {
    pub fn new() -> std::io::Result<Client> {
        let sock = Socket::bind_any()?;
        Ok(Client {
            sock,
            server: SocketAddr::from(([127, 0, 0, 1], DEFAULT_PORT)),
            address_text: String::new(),
            state: ClientState::Disconnected,
            conn: None,
            client_salt: 0,
            server_salt: 0,
            token: 0,
            handshake_at: 0.0,
            started_at: 0.0,
            password: String::new(),
            needs_password: false,
            slot: NO_PLAYER,
            time: 0.0,
            server_time: 0.0,
            server_time_offset: 0.0,
            clock_ref: 0.0,
            clock_at: 0.0,
            local: Player::new(0),
            voice_in: Vec::new(),
            predicted: MoveState::default(),
            error_offset: Vec3::ZERO,
            cmd_seq: 0,
            history: Ring::new(),
            last_acked_input: 0,
            corrections: 0,
            max_error: 0.0,
            last_output: crate::game::player::WeaponOutput::default(),
            snapshots: VecDeque::with_capacity(SNAPSHOT_HISTORY),
            players: [RemotePlayer::default(); MAX_PLAYERS],
            baseline: [PlayerSnap::default(); MAX_PLAYERS],
            baseline_tick: 0,
            events: Vec::with_capacity(128),
            roster: vec![PlayerInfo::default(); MAX_PLAYERS],
            match_info: MatchInfo::default(),
            chat: VecDeque::with_capacity(16),
            kicked: None,
            map_change: None,
            bytes_in: 0,
            bytes_out: 0,
            snapshot_hz: 0.0,
            snapshot_times: VecDeque::with_capacity(32),
            outgoing: vec![0u8; MAX_PACKET],
        })
    }

    pub fn ping_ms(&self) -> u16 { self.conn.as_ref().map(|c| c.ping_ms()).unwrap_or(0) }
    pub fn loss(&self) -> f32 { self.conn.as_ref().map(|c| c.loss).unwrap_or(0.0) }
    pub fn jitter_ms(&self) -> f32 { self.conn.as_ref().map(|c| c.jitter_ms).unwrap_or(0.0) }

    /// Starts a connection. `address` may be "host", "host:port" or a port.
    pub fn connect(&mut self, address: &str, password: &str, now: f64) -> Result<(), String> {
        let addr = parse_endpoint(address, DEFAULT_PORT)
            .ok_or_else(|| format!("could not resolve '{}'", address))?;
        self.server = addr;
        self.address_text = address.to_string();
        self.password = password.to_string();
        self.state = ClientState::Connecting;
        self.client_salt = crate::core::Rng::from_clock().next_u32() as u64
            | ((crate::core::Rng::from_clock().next_u32() as u64) << 32);
        self.handshake_at = 0.0;
        self.started_at = now;
        self.slot = NO_PLAYER;
        self.reset_world_state();
        Ok(())
    }

    pub fn disconnect(&mut self) {
        if let Some(conn) = &mut self.conn {
            let mut buf = [0u8; 64];
            let n = {
                let mut w = Writer::new(&mut buf);
                let h = conn.next_header(self.time);
                h.write(&mut w, PacketKind::Disconnect);
                w.finish()
            };
            // Send it a few times: this is the only unacknowledged message
            // whose loss the player would actually notice.
            for _ in 0..3 { self.sock.send(&buf[..n], self.server); }
        }
        self.conn = None;
        self.state = ClientState::Disconnected;
        self.slot = NO_PLAYER;
        self.reset_world_state();
    }

    fn reset_world_state(&mut self) {
        self.snapshots.clear();
        self.players = [RemotePlayer::default(); MAX_PLAYERS];
        self.baseline = [PlayerSnap::default(); MAX_PLAYERS];
        self.baseline_tick = 0;
        self.history.clear();
        self.events.clear();
        self.chat.clear();
        self.roster = vec![PlayerInfo::default(); MAX_PLAYERS];
        self.error_offset = Vec3::ZERO;
        self.corrections = 0;
        self.max_error = 0.0;
    }

    /// Pumps the socket. Call once per frame before anything else.
    pub fn update(&mut self, now: f64) {
        self.time = now;
        self.receive();

        match self.state {
            ClientState::Connecting | ClientState::Challenged | ClientState::Joining => {
                if now - self.started_at > CONNECT_TIMEOUT {
                    self.state = ClientState::Failed("NO RESPONSE FROM SERVER".into());
                    return;
                }
                if now - self.handshake_at > HANDSHAKE_RETRY {
                    self.handshake_at = now;
                    self.send_handshake();
                }
            }
            ClientState::Playing => {
                if let Some(c) = &self.conn {
                    if c.timed_out(now, DISCONNECT_TIMEOUT) {
                        self.state = ClientState::Failed("CONNECTION LOST".into());
                        self.conn = None;
                    }
                }
                // Server clock estimate advances with real time between
                // snapshots, so interpolation never stalls.
                self.server_time = now + self.server_time_offset;
                // The match clock is only re-sent every couple of seconds, so
                // run it down locally from the last anchor. Each Phase message
                // re-anchors it, which keeps drift bounded by one sync period.
                let elapsed = (now - self.clock_at).max(0.0) as f32;
                self.match_info.seconds_left =
                    (self.clock_ref - elapsed).max(0.0).ceil() as u16;
            }
            _ => {}
        }
    }

    fn send_handshake(&mut self) {
        let mut buf = [0u8; 128];
        let n = match self.state {
            ClientState::Connecting => {
                let mut w = Writer::new(&mut buf);
                w.u32(PROTOCOL_MAGIC);
                w.u8(PacketKind::ConnectRequest as u8);
                w.u16(PROTOCOL_VERSION);
                w.u64(self.client_salt);
                w.finish()
            }
            ClientState::Challenged | ClientState::Joining => {
                let mut w = Writer::new(&mut buf);
                w.u32(PROTOCOL_MAGIC);
                w.u8(PacketKind::ConnectResponse as u8);
                w.u64(self.client_salt ^ self.server_salt);
                w.u64(password_hash(&self.password, self.server_salt));
                w.finish()
            }
            _ => 0,
        };
        if n > 0 {
            self.sock.send(&buf[..n], self.server);
            self.bytes_out += n as u64;
        }
    }

    fn receive(&mut self) {
        let mut scratch = [0u8; MAX_PACKET];
        loop {
            let (len, from) = match self.sock.recv() {
                Some((data, from)) => {
                    let n = data.len().min(MAX_PACKET);
                    scratch[..n].copy_from_slice(&data[..n]);
                    (n, from)
                }
                None => break,
            };
            if from != self.server { continue; }
            self.bytes_in += len as u64;
            self.handle(&scratch[..len]);
        }
    }

    fn handle(&mut self, data: &[u8]) {
        let Some(kind) = peek_kind(data) else { return };
        match kind {
            PacketKind::Challenge => {
                if self.state != ClientState::Connecting { return; }
                let mut r = Reader::new(data);
                let _ = r.u32();
                let _ = r.u8();
                let Some(echo) = r.u64() else { return };
                if echo != self.client_salt { return; }
                let Some(server_salt) = r.u64() else { return };
                self.needs_password = r.bool().unwrap_or(false);
                self.server_salt = server_salt;
                self.token = self.client_salt ^ server_salt;
                self.state = ClientState::Challenged;
                self.handshake_at = 0.0;
            }
            PacketKind::Accepted => {
                let mut r = Reader::new(data);
                let _ = r.u32();
                let _ = r.u8();
                let Some(token) = r.u64() else { return };
                if token != self.token { return; }
                let Some(slot) = r.u8() else { return };
                let Some(max) = r.u8() else { return };
                let _hz = r.f32();
                self.slot = slot;
                self.match_info.max_players = max;
                // The identity and loadout were set by `hello` before the
                // server ever answered, and a fresh Player would throw both
                // away. Losing the loadout is not cosmetic: the predicted
                // player's three weapon slots would keep their default
                // contents forever, the server never sends weapon ids in a
                // snapshot, and every slot would read - and draw - as the
                // sidearm no matter which one the player selected.
                let (name, level, loadout) = (
                    std::mem::take(&mut self.local.name),
                    self.local.level,
                    self.local.loadout,
                );
                self.local = Player::new(slot);
                self.local.name = name;
                self.local.level = level;
                self.local.loadout = loadout;
                self.local.equip();
                self.local.in_use = true;
                self.conn = Some(Connection::new(self.server, self.token, self.time));
                self.state = ClientState::Playing;
            }
            PacketKind::Voice => {
                let mut r = Reader::new(data);
                let _ = r.u32();
                let _ = r.u8();
                let Some(speaker) = r.u8() else { return };
                let Some(len) = r.u16() else { return };
                let Some(frame) = r.bytes(len as usize) else { return };
                if frame.len() >= crate::audio::voice::FRAME_BYTES {
                    self.voice_in.push((speaker, frame.to_vec()));
                }
            }
            PacketKind::Denied => {
                let mut r = Reader::new(data);
                let _ = r.u32();
                let _ = r.u8();
                let reason = DenyReason::from_u8(r.u8().unwrap_or(0));
                self.state = ClientState::Failed(reason.text().to_string());
            }
            PacketKind::Disconnect => {
                self.state = ClientState::Failed("DISCONNECTED BY SERVER".into());
                self.conn = None;
            }
            PacketKind::Payload => self.handle_payload(data),
            _ => {}
        }
    }

    fn handle_payload(&mut self, data: &[u8]) {
        let mut r = Reader::new(data);
        let Some((_, header)) = PacketHeader::read(&mut r) else { return };
        if header.token != self.token { return; }
        let Some(conn) = self.conn.as_mut() else { return };
        conn.on_header(&header, self.time, data.len());

        let mut msgs: Vec<&[u8]> = Vec::new();
        if !conn.read_reliable(&mut r, &mut msgs) { return; }
        for body in msgs {
            let mut mr = Reader::new(body);
            if let Some(msg) = ServerMsg::decode(&mut mr) {
                self.handle_message(msg);
            }
        }

        let Some(kind) = r.u8() else { return };
        if kind != 1 { return; }
        self.read_snapshot(&mut r);
    }

    fn handle_message(&mut self, msg: ServerMsg) {
        match msg {
            ServerMsg::MatchConfig { server_name, map, mode, score_limit, time_limit, max_players, friendly_fire, bot_count, phase } => {
                let changed = map != self.match_info.map;
                self.match_info.server_name = server_name;
                self.match_info.map = map;
                self.match_info.mode = ModeId::from_u8(mode);
                self.match_info.score_limit = score_limit;
                self.match_info.time_limit = time_limit;
                self.match_info.max_players = max_players;
                self.match_info.friendly_fire = friendly_fire;
                self.match_info.bot_count = bot_count;
                self.match_info.phase = phase;
                if changed {
                    self.map_change = Some((map, self.match_info.mode));
                }
            }
            ServerMsg::PlayerInfo { slot, name, team, is_bot, level, ping, ready } => {
                if (slot as usize) < self.roster.len() {
                    let e = &mut self.roster[slot as usize];
                    e.present = true;
                    e.name = name;
                    e.team = team;
                    e.is_bot = is_bot;
                    e.level = level;
                    e.ping = ping;
                    e.ready = ready;
                }
            }
            ServerMsg::PlayerLeft { slot } => {
                if (slot as usize) < self.roster.len() {
                    self.roster[slot as usize] = PlayerInfo::default();
                    self.players[slot as usize] = RemotePlayer::default();
                }
            }
            ServerMsg::Scores { entries } => {
                for (slot, kills, deaths, assists, score, ping) in entries {
                    if (slot as usize) < self.roster.len() {
                        let e = &mut self.roster[slot as usize];
                        e.kills = kills;
                        e.deaths = deaths;
                        e.assists = assists;
                        e.score = score;
                        e.ping = ping;
                    }
                }
            }
            ServerMsg::TeamScores { phantom, vanguard } => {
                self.match_info.team_scores = [phantom, vanguard];
            }
            ServerMsg::Chat { slot, team_only, text } => {
                self.chat.push_back(ChatLine { slot, team_only, text, time: self.time });
                while self.chat.len() > 12 { self.chat.pop_front(); }
            }
            ServerMsg::Announce { line } => {
                self.events.push(GameEvent::Announce { line });
            }
            ServerMsg::Phase { phase, seconds_left, round } => {
                self.clock_ref = seconds_left as f32;
                self.clock_at = self.time;
                self.match_info.phase = phase;
                self.match_info.seconds_left = seconds_left;
                self.match_info.round = round;
                self.events.push(GameEvent::MatchState { phase, seconds: seconds_left });
            }
            ServerMsg::LoadMap { map, mode } => {
                self.map_change = Some((map, ModeId::from_u8(mode)));
                self.match_info.map = map;
                self.match_info.mode = ModeId::from_u8(mode);
            }
            ServerMsg::Objectives { data } => self.match_info.hud = data,
            ServerMsg::Results { winner, mvp } => {
                self.match_info.winner = winner;
                self.match_info.mvp = mvp;
            }
            ServerMsg::Kick { reason } => {
                self.kicked = Some(reason.clone());
                self.state = ClientState::Failed(reason);
            }
            ServerMsg::LobbyInfo { .. } => {}
        }
    }

    fn read_snapshot(&mut self, r: &mut Reader) {
        let (Some(tick), Some(time_ms), Some(acked)) = (r.u32(), r.u32(), r.u32()) else { return };
        let Some(base_tick) = r.u32() else { return };

        // Delta base: either a snapshot we still hold, or nothing.
        let base: [PlayerSnap; MAX_PLAYERS] = if base_tick != 0 && base_tick == self.baseline_tick {
            self.baseline
        } else if base_tick != 0 {
            match self.snapshots.iter().find(|s| s.tick == base_tick) {
                Some(s) => s.players,
                None => [PlayerSnap::default(); MAX_PLAYERS],
            }
        } else {
            [PlayerSnap::default(); MAX_PLAYERS]
        };

        let mut snap = Snapshot {
            tick,
            server_time_ms: time_ms,
            acked_input: acked,
            players: [PlayerSnap::default(); MAX_PLAYERS],
            local: None,
            received_at: self.time,
        };

        let Some(count) = r.u8() else { return };
        if count as usize > MAX_PLAYERS { return; }
        for _ in 0..count {
            let Some((slot, mask)) = PlayerSnap::read_delta_header(r) else { return };
            if slot as usize >= MAX_PLAYERS { return; }
            if mask.contains(SnapField::GONE) {
                snap.players[slot as usize] = PlayerSnap::default();
                continue;
            }
            let Some(s) = PlayerSnap::read_delta_body(r, &base[slot as usize], mask) else { return };
            snap.players[slot as usize] = s;
        }

        let Some(ev_count) = r.u8() else { return };
        for _ in 0..ev_count {
            match decode_event(r) {
                Some(e) => self.events.push(e),
                None => break,
            }
        }

        if let Some(bytes) = r.bytes(8) {
            let mut hud = [0u8; 8];
            hud.copy_from_slice(bytes);
            self.match_info.hud = hud;
        }
        snap.local = LocalState::read(r);

        // Clock synchronisation. The offset is smoothed so a single late
        // packet cannot make remote players stutter.
        let server_now = time_ms as f64 / 1000.0;
        let offset = server_now - self.time;
        if self.snapshots.is_empty() {
            self.server_time_offset = offset;
        } else {
            self.server_time_offset += (offset - self.server_time_offset) * 0.08;
        }

        self.snapshot_times.push_back(self.time);
        while self.snapshot_times.len() > 24 { self.snapshot_times.pop_front(); }
        if self.snapshot_times.len() > 4 {
            let span = self.snapshot_times.back().unwrap() - self.snapshot_times.front().unwrap();
            if span > 0.01 { self.snapshot_hz = (self.snapshot_times.len() - 1) as f32 / span as f32; }
        }

        self.baseline = snap.players;
        self.baseline_tick = tick;
        self.last_acked_input = acked;

        self.snapshots.push_back(snap);
        while self.snapshots.len() > SNAPSHOT_HISTORY { self.snapshots.pop_front(); }
    }

    // ======================================================== prediction

    /// Applies a freshly sampled command locally and queues it for sending.
    pub fn push_command(&mut self, mut cmd: InputCmd, world: &CollisionWorld, bounds: &crate::math::Aabb) {
        self.cmd_seq = self.cmd_seq.wrapping_add(1);
        cmd.seq = self.cmd_seq;
        cmd.sanitize();

        self.simulate(&cmd, world, bounds);
        self.history.push((cmd, self.predicted));
    }

    /// Runs one command against the local player. This is the same code the
    /// server runs, which is the only reason prediction can agree with it.
    fn simulate(&mut self, cmd: &InputCmd, world: &CollisionWorld, bounds: &crate::math::Aabb) {
        self.last_output = crate::game::player::WeaponOutput::default();
        if !self.local.alive {
            self.local.trigger_was_held = cmd.held(Buttons::FIRE);
            self.predicted = self.local.mv;
            return;
        }
        let dt = cmd.dt();
        let want_ads = cmd.held(Buttons::ADS);
        let mods = self.local.move_mods(want_ads);
        movement::move_player(&mut self.local.mv, cmd, &mods, world, dt);
        movement::clamp_to_bounds(&mut self.local.mv, bounds);
        self.local.update_status(dt);
        self.last_output = self.local.update_weapon(cmd, dt);
        self.predicted = self.local.mv;
    }

    /// Reconciles prediction with the newest snapshot.
    pub fn reconcile(&mut self, world: &CollisionWorld, bounds: &crate::math::Aabb) {
        let Some(snap) = self.snapshots.back().cloned() else { return };
        let Some(local) = snap.local else { return };

        // Authoritative state that is never predicted.
        self.local.health = local.health;
        self.local.armor = local.armor;
        self.local.lethal_count = local.lethal;
        self.local.tactical_count = local.tactical;
        self.local.flash = local.flash;
        self.local.concussion = local.concussion;
        let was_alive = self.local.alive;
        self.local.alive = local.alive;
        if !was_alive && local.alive {
            // Fresh spawn: the server has just re-equipped this player from
            // their loadout, so the prediction has to do the same or the two
            // disagree about which weapons are even in the slots.
            self.local.equip();
            for i in 0..3 {
                self.local.weapons[i].ammo = local.ammo[i];
                self.local.weapons[i].reserve = local.reserve[i];
            }
            if local.weapon_slot < 3 {
                self.local.cur = local.weapon_slot;
                self.local.queued_slot = local.weapon_slot;
            }
            // Take the server's position wholesale.
            self.local.mv = MoveState {
                pos: local.pos,
                vel: local.vel,
                yaw: self.local.mv.yaw,
                pitch: self.local.mv.pitch,
                height: local.height,
                stance: Stance::from_u8(local.stance),
                grounded: local.grounded,
                ..MoveState::default()
            };
            self.predicted = self.local.mv;
            self.error_offset = Vec3::ZERO;
            self.history.clear();
            return;
        }
        for i in 0..3 {
            self.local.weapons[i].ammo = local.ammo[i];
            self.local.weapons[i].reserve = local.reserve[i];
        }
        if local.weapon_slot < 3 { self.local.cur = local.weapon_slot; }

        if !local.alive {
            self.local.mv.pos = local.pos;
            self.predicted = self.local.mv;
            return;
        }

        // Find what we predicted for the command the server has executed.
        let acked = snap.acked_input;
        let mut predicted_then: Option<MoveState> = None;
        let mut replay_from = 0usize;
        for i in 0..self.history.len() {
            if let Some((cmd, state)) = self.history.back(i) {
                if cmd.seq == acked {
                    predicted_then = Some(*state);
                    replay_from = i;
                    break;
                }
            }
        }

        let error = match predicted_then {
            Some(p) => (p.pos - local.pos).length(),
            None => (self.local.mv.pos - local.pos).length(),
        };
        self.max_error = self.max_error.max(error);

        // Below a couple of centimetres the disagreement is quantisation and
        // replaying would cost more than it fixes.
        if error < 0.03 {
            return;
        }
        self.corrections += 1;

        let visual_before = self.local.mv.pos + self.error_offset;

        // Snap to the server, then replay everything it has not seen yet.
        self.local.mv.pos = local.pos;
        self.local.mv.vel = local.vel;
        self.local.mv.height = local.height;
        self.local.mv.stance = Stance::from_u8(local.stance);
        self.local.mv.grounded = local.grounded;
        self.local.mv.sprint_t = local.sprint_t;
        self.local.mv.ads_t = local.ads_t;

        if predicted_then.is_some() {
            for i in (0..replay_from).rev() {
                if let Some((cmd, _)) = self.history.back(i).copied() {
                    let dt = cmd.dt();
                    let want_ads = cmd.held(Buttons::ADS);
                    let mods = self.local.move_mods(want_ads);
                    movement::move_player(&mut self.local.mv, &cmd, &mods, world, dt);
                    movement::clamp_to_bounds(&mut self.local.mv, bounds);
                    if let Some(slot) = self.history.back_mut(i) { slot.1 = self.local.mv; }
                }
            }
        }
        self.predicted = self.local.mv;

        // Carry the visible position forward and ease it back, unless the
        // correction is so large that hiding it would be a lie.
        let new_error = visual_before - self.local.mv.pos;
        self.error_offset = if new_error.length() > HARD_SNAP { Vec3::ZERO } else { new_error };
    }

    /// Decays the visual correction offset. Call once per rendered frame.
    pub fn smooth_error(&mut self, dt: f32) {
        if self.error_offset.length_squared() < 1e-8 {
            self.error_offset = Vec3::ZERO;
            return;
        }
        let k = (-12.0 * dt).exp();
        self.error_offset *= k;
    }

    /// Where to draw the local player's camera.
    pub fn view_position(&self) -> Vec3 {
        self.local.mv.eye() + self.error_offset
    }

    // ===================================================== interpolation

    /// Updates every remote player's render state for the current frame.
    pub fn interpolate(&mut self, dt: f32) {
        let render_time = self.server_time - INTERP_DELAY_MS / 1000.0;

        // The two snapshots that bracket the render time.
        let mut older: Option<&Snapshot> = None;
        let mut newer: Option<&Snapshot> = None;
        for s in self.snapshots.iter() {
            let t = s.server_time_ms as f64 / 1000.0;
            if t <= render_time { older = Some(s); } else if newer.is_none() { newer = Some(s); }
        }

        for slot in 0..MAX_PLAYERS {
            if slot as u8 == self.slot { continue; }
            let p = &mut self.players[slot];

            let (target_pos, target_yaw, target_pitch, target_height, snap, present) =
                match (older, newer) {
                    (Some(a), Some(b)) => {
                        let ta = a.server_time_ms as f64 / 1000.0;
                        let tb = b.server_time_ms as f64 / 1000.0;
                        let span = (tb - ta).max(0.001);
                        let t = (((render_time - ta) / span) as f32).clamp(0.0, 1.0);
                        let sa = a.players[slot];
                        let sb = b.players[slot];
                        if !sa.present && !sb.present {
                            (Vec3::ZERO, 0.0, 0.0, 1.78, sa, false)
                        } else if !sa.present {
                            (sb.pos, sb.yaw, sb.pitch, sb.height, sb, true)
                        } else if !sb.present {
                            (sa.pos, sa.yaw, sa.pitch, sa.height, sa, true)
                        } else {
                            (
                                sa.pos.lerp(sb.pos, t),
                                sa.yaw + crate::core::angle_delta(sa.yaw, sb.yaw) * t,
                                sa.pitch + (sb.pitch - sa.pitch) * t,
                                sa.height + (sb.height - sa.height) * t,
                                sb,
                                true,
                            )
                        }
                    }
                    (Some(a), None) => {
                        let s = a.players[slot];
                        // Beyond the newest snapshot, extrapolate briefly
                        // rather than freezing: a short slide reads far better
                        // than a stutter when a packet is late.
                        let ta = a.server_time_ms as f64 / 1000.0;
                        let ahead = ((render_time - ta) as f32).clamp(0.0, 0.12);
                        (s.pos + s.vel * ahead, s.yaw, s.pitch, s.height, s, s.present)
                    }
                    (None, Some(b)) => {
                        let s = b.players[slot];
                        (s.pos, s.yaw, s.pitch, s.height, s, s.present)
                    }
                    (None, None) => continue,
                };

            if !present {
                p.present = false;
                continue;
            }

            let was_dead = p.snap.flags.contains(PFlags::DEAD);
            let now_dead = snap.flags.contains(PFlags::DEAD);
            if now_dead && !was_dead { p.death_time = 0.0; }
            if now_dead { p.death_time += dt; } else { p.death_time = 0.0; }

            let moved = (target_pos - p.render_pos).length();
            p.speed = if dt > 0.0 { (moved / dt).min(12.0) } else { 0.0 };
            // Advance the stride phase by distance, so the walk cycle stays in
            // step with the feet at any speed.
            p.phase += moved * 3.0;
            p.render_pos = target_pos;
            p.render_yaw = target_yaw;
            p.render_pitch = target_pitch;
            p.render_height = target_height;
            p.snap = snap;
            p.team = Team::from_u8(snap.team);
            p.present = true;
            p.firing = (p.firing - dt * 6.0).max(0.0);
            p.last_update = self.time;
        }
    }

    /// Flags a player as firing, so the animator can kick their arms.
    pub fn note_fired(&mut self, slot: u8) {
        if (slot as usize) < MAX_PLAYERS {
            self.players[slot as usize].firing = 1.0;
        }
    }

    // ========================================================== sending

    /// Sends the queued input and any reliable messages.
    pub fn send(&mut self) {
        let Some(conn) = self.conn.as_mut() else { return };
        let mut buf = std::mem::take(&mut self.outgoing);
        let n = {
            let mut w = Writer::new(&mut buf);
            let header = conn.next_header(self.time);
            header.write(&mut w, PacketKind::Payload);
            conn.write_reliable(&mut w);

            w.u8(1);
            w.u32(self.baseline_tick);
            // Send the last few commands so a dropped packet costs nothing.
            let count = self.history.len().min(INPUT_REDUNDANCY);
            w.u8(count as u8);
            for i in (0..count).rev() {
                if let Some((cmd, _)) = self.history.back(i) {
                    write_input(&mut w, cmd);
                }
            }
            if w.overflowed() { 0 } else { w.finish() }
        };
        if n > 0 {
            self.sock.send(&buf[..n], self.server);
            conn.note_sent(n);
            self.bytes_out += n as u64;
        }
        self.outgoing = buf;
    }

    pub fn send_message(&mut self, msg: &ClientMsg) {
        let Some(conn) = self.conn.as_mut() else { return };
        let mut buf = vec![0u8; 320];
        let n = {
            let mut w = Writer::new(&mut buf);
            msg.encode(&mut w);
            if w.overflowed() { 0 } else { w.finish() }
        };
        if n > 0 {
            buf.truncate(n);
            conn.send_reliable(buf);
        }
    }

    pub fn say(&mut self, text: &str, team_only: bool) {
        let text = text.trim();
        if text.is_empty() { return; }
        self.send_message(&ClientMsg::Chat { team_only, text: text.to_string() });
    }

    /// Sends one encoded frame of speech. Fire and forget by design: a voice
    /// frame that needs retransmitting has already missed its moment.
    pub fn send_voice(&mut self, frame: &[u8]) {
        let Some(conn) = self.conn.as_ref() else { return };
        let mut buf = [0u8; crate::net::protocol::MAX_PACKET];
        let n = {
            let mut w = Writer::new(&mut buf);
            w.u32(crate::net::protocol::PROTOCOL_MAGIC);
            w.u8(PacketKind::Voice as u8);
            w.u64(self.token);
            w.u16(frame.len() as u16);
            w.bytes(frame);
            w.len()
        };
        let _ = conn;
        self.sock.send(&buf[..n], self.server);
    }

    /// Takes the frames that have arrived since the last call.
    pub fn take_voice(&mut self) -> Vec<(u8, Vec<u8>)> {
        std::mem::take(&mut self.voice_in)
    }

    pub fn set_loadout(&mut self, l: &Loadout) {
        self.local.loadout = *l;
        // The server applies a new loadout at the next spawn, and so does the
        // prediction, but a player who is already dead when they change it
        // would otherwise see the old weapons on the scoreboard until then.
        if !self.local.alive { self.local.equip(); }
        self.send_message(&ClientMsg::SetLoadout { loadout: l.encode() });
    }

    pub fn hello(&mut self, name: &str, level: u8, loadout: &Loadout) {
        self.local.loadout = *loadout;
        self.local.name = name.to_string();
        self.local.level = level;
        self.send_message(&ClientMsg::Hello {
            name: name.to_string(),
            level,
            loadout: loadout.encode(),
        });
    }

    pub fn request_respawn(&mut self) {
        self.send_message(&ClientMsg::RequestRespawn);
    }

    pub fn take_events(&mut self) -> Vec<GameEvent> {
        std::mem::take(&mut self.events)
    }

    /// Roster entry for a slot, if any.
    pub fn player_info(&self, slot: u8) -> Option<&PlayerInfo> {
        self.roster.get(slot as usize).filter(|p| p.present)
    }

    pub fn name_of(&self, slot: u8) -> &str {
        self.player_info(slot).map(|p| p.name.as_str()).unwrap_or("UNKNOWN")
    }

    /// Seconds until the local player may respawn, from the newest snapshot.
    pub fn snapshots_respawn_in(&self) -> f32 {
        self.snapshots.back().and_then(|s| s.local).map(|l| l.respawn_in).unwrap_or(0.0)
    }

    pub fn my_team(&self) -> Team {
        self.player_info(self.slot).map(|p| p.team).unwrap_or(Team::None)
    }
}
