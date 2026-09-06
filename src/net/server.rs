//! The authoritative server.
//!
//! Runs the world at a fixed tick, accepts input, and sends each client a
//! delta-compressed snapshot of what it is allowed to know. It is the same
//! code whether it is a dedicated process or a thread inside a player's game,
//! which means the single-player-feeling case is exercising the identical
//! network path as a match across the internet.

use super::bits::{Reader, Writer};
use super::channel::Connection;
use super::protocol::*;
use super::socket::Socket;
use crate::bots::Director;
use crate::core::Rng;
use crate::game::events::{Delivery, GameEvent};
use crate::game::loadout::Loadout;
use crate::game::sim::{World, TICK_DT};
use crate::game::types::*;
use crate::maps::{MapId, ALL_MAPS};
use crate::modes::{auto_assign_team, KillInfo, MatchState, Mode, ModeId, Phase};
use std::net::SocketAddr;

/// Connections drop after this long with nothing received.
const TIMEOUT: f64 = 10.0;
/// A client may not bank more than this much simulation time.
const INPUT_BUDGET_MAX: f32 = 0.30;
/// How far a player may be replicated before we stop bothering.
const RELEVANCE_RANGE: f32 = 150.0;

#[derive(Clone, Debug)]
pub struct ServerConfig {
    pub name: String,
    pub port: u16,
    pub max_players: u8,
    pub bot_count: u8,
    pub map: MapId,
    pub mode: ModeId,
    pub password: String,
    pub friendly_fire: bool,
    pub snapshot_hz: f32,
    pub dedicated: bool,
    pub rotation: Vec<MapId>,
    /// Seconds the lobby waits before starting itself.
    pub lobby_countdown: f32,
    pub bot_difficulty: u8,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            name: "HARDPOINT SERVER".to_string(),
            port: DEFAULT_PORT,
            max_players: 12,
            bot_count: 8,
            map: MapId::Ironveil,
            mode: ModeId::TeamDeathmatch,
            password: String::new(),
            friendly_fire: false,
            snapshot_hz: 22.0,
            dedicated: false,
            rotation: ALL_MAPS.to_vec(),
            lobby_countdown: 20.0,
            bot_difficulty: 2,
        }
    }
}

/// Per-connected-client state.
struct Client {
    /// When this client last sent a voice frame, for rate limiting.
    last_voice: f64,
    conn: Connection,
    slot: u8,
    /// Highest input command executed.
    last_input: u32,
    /// Simulation time this client is allowed to consume, to bound the effect
    /// of a client that sends more input than real time can account for.
    budget: f32,
    /// Snapshots we have sent, so we can delta against whichever one the
    /// client confirms.
    history: Vec<(u32, [PlayerSnap; MAX_PLAYERS])>,
    acked_snapshot: u32,
    loaded: bool,
    wants_respawn: bool,
    is_host: bool,
    ready: bool,
    /// Events queued for this specific client since the last snapshot.
    pending_events: Vec<GameEvent>,
    name_sent: bool,
}

impl Client {
    fn new(conn: Connection, slot: u8) -> Client {
        Client {
            last_voice: 0.0,
            conn, slot,
            last_input: 0,
            budget: 0.0,
            history: Vec::with_capacity(40),
            acked_snapshot: 0,
            loaded: false,
            wants_respawn: false,
            is_host: false,
            ready: false,
            pending_events: Vec::with_capacity(32),
            name_sent: false,
        }
    }

    fn baseline(&self, tick: u32) -> Option<&[PlayerSnap; MAX_PLAYERS]> {
        self.history.iter().find(|(t, _)| *t == tick).map(|(_, s)| s)
    }
}

/// A pending handshake, before a slot is assigned.
struct Pending {
    addr: SocketAddr,
    client_salt: u64,
    server_salt: u64,
    created: f64,
}

pub struct Server {
    pub cfg: ServerConfig,
    pub world: World,
    pub state: MatchState,
    pub mode: Box<dyn Mode>,
    pub director: Director,

    sock: Socket,
    clients: Vec<Option<Client>>,
    pending: Vec<Pending>,
    rng: Rng,
    /// Countdown to the next unsolicited clock broadcast.
    clock_sync: f32,
    /// Development hook: when set, every distributed event is copied here so
    /// the headless tools can check which systems actually ran.
    pub event_tap: Option<Vec<GameEvent>>,

    time: f64,
    accumulator: f32,
    snapshot_accum: f32,
    rotation_index: usize,

    /// Set when the server should stop.
    pub shutdown: bool,
    pub stats: ServerStats,
    scratch: Vec<u8>,
}

#[derive(Default, Clone, Copy, Debug)]
pub struct ServerStats {
    pub ticks: u64,
    pub snapshots_sent: u64,
    pub bytes_sent: u64,
    pub bytes_recv: u64,
    pub worst_tick_ms: f32,
    pub avg_tick_ms: f32,
}

impl Server {
    pub fn bind(cfg: ServerConfig) -> std::io::Result<Server> {
        let sock = if cfg.port == 0 {
            Socket::bind_range(DISCOVERY_PORTS)?
        } else {
            Socket::bind(cfg.port)?
        };
        let mut world = World::new(cfg.map, 0x5EED_1234);
        world.friendly_fire = cfg.friendly_fire;
        let mut mode = cfg.mode.build();
        let mut state = MatchState::new(cfg.mode);
        state.score_limit = cfg.mode.default_score_limit();
        state.time_limit = cfg.mode.default_time_limit();
        mode.begin_match(&mut world, &mut state);

        let mut cfg = cfg;
        cfg.port = sock.port();

        Ok(Server {
            director: Director::new(cfg.bot_difficulty),
            cfg,
            world,
            state,
            mode,
            sock,
            clients: (0..MAX_PLAYERS).map(|_| None).collect(),
            pending: Vec::new(),
            rng: Rng::from_clock(),
            clock_sync: 0.0,
            event_tap: None,
            time: 0.0,
            accumulator: 0.0,
            snapshot_accum: 0.0,
            rotation_index: 0,
            shutdown: false,
            stats: ServerStats::default(),
            scratch: vec![0u8; MAX_PACKET],
        })
    }

    pub fn port(&self) -> u16 { self.sock.port() }
    pub fn now(&self) -> f64 { self.time }

    pub fn info(&self) -> ServerInfo {
        ServerInfo {
            version: PROTOCOL_VERSION,
            name: self.cfg.name.clone(),
            map: self.world.map_id,
            mode: self.mode.id() as u8,
            players: self.world.players.iter().filter(|p| p.in_use && !p.is_bot).count() as u8,
            bots: self.world.players.iter().filter(|p| p.in_use && p.is_bot).count() as u8,
            max_players: self.cfg.max_players,
            passworded: !self.cfg.password.is_empty(),
            phase: self.state.phase as u8,
        }
    }

    // =============================================================== update

    /// Advances the server. Call as often as convenient; it accumulates real
    /// time and steps the simulation at a fixed rate.
    pub fn update(&mut self, dt: f32) {
        self.time += dt as f64;
        self.receive();
        self.expire();

        self.accumulator += dt;
        // Never try to catch up more than a few ticks: a stalled server that
        // then runs twenty ticks back to back would teleport everyone.
        let max_steps = 6;
        let mut steps = 0;
        while self.accumulator >= TICK_DT && steps < max_steps {
            let t0 = std::time::Instant::now();
            self.tick();
            let ms = t0.elapsed().as_secs_f32() * 1000.0;
            self.stats.worst_tick_ms = self.stats.worst_tick_ms.max(ms);
            self.stats.avg_tick_ms += (ms - self.stats.avg_tick_ms) * 0.02;
            self.accumulator -= TICK_DT;
            steps += 1;
        }
        if steps == max_steps { self.accumulator = 0.0; }

        self.snapshot_accum += dt;
        let period = 1.0 / self.cfg.snapshot_hz.max(5.0);
        if self.snapshot_accum >= period {
            self.snapshot_accum = 0.0;
            self.send_snapshots();
        }
    }

    fn tick(&mut self) {
        self.stats.ticks += 1;

        // Bots produce commands and run them through the same path as humans.
        if self.state.phase.playable() {
            self.director.update(&mut self.world, &self.state, self.mode.as_ref(), TICK_DT);
        }

        self.world.step(TICK_DT);

        // Hand kills to the mode after the world has already scored them.
        let kills: Vec<KillInfo> = self.world.events.iter().filter_map(|e| match e {
            GameEvent::Kill { killer, victim, weapon, cause, .. } => Some(KillInfo {
                killer: *killer,
                victim: *victim,
                weapon: *weapon,
                cause: *cause,
                suicide: killer == victim,
                friendly: killer != victim && !self.world.hostile(*killer, *victim),
            }),
            _ => None,
        }).collect();
        for k in kills {
            self.mode.on_kill(&mut self.world, &mut self.state, k);
            if !k.suicide && !k.friendly {
                self.world.scavenge(k.killer);
            }
        }

        self.update_match_flow();
        self.handle_respawns();
        self.distribute_events();
    }

    fn update_match_flow(&mut self) {
        self.tick_clock_sync(TICK_DT);
        let dt = TICK_DT;
        match self.state.phase {
            Phase::Lobby => {
                let humans = self.world.players.iter().filter(|p| p.in_use && !p.is_bot).count();
                if humans > 0 {
                    self.state.timer -= dt;
                    let all_ready = self.clients.iter().flatten().all(|c| c.ready || !c.loaded);
                    if self.state.timer <= 0.0 || (all_ready && humans > 0 && self.state.timer < self.cfg.lobby_countdown - 2.0) {
                        self.start_match();
                    }
                } else {
                    self.state.timer = self.cfg.lobby_countdown;
                }
            }
            Phase::Warmup => {
                self.state.timer -= dt;
                if self.state.timer <= 0.0 { self.begin_countdown(); }
                self.mode.update(&mut self.world, &mut self.state, dt);
            }
            Phase::Countdown => {
                self.state.timer -= dt;
                if self.state.timer <= 0.0 {
                    self.state.phase = Phase::Live;
                    self.broadcast_phase();
                    self.world.events.push(GameEvent::Announce {
                        line: crate::game::events::AnnounceLine::Fight,
                    });
                }
            }
            Phase::Live => {
                self.mode.update(&mut self.world, &mut self.state, dt);
                self.clock_announcements();

                if let Some(winner) = self.mode.round_over(&self.world, &self.state) {
                    if self.mode.id().is_round_based() {
                        self.state.round_wins[winner.index().min(3)] += 1;
                        self.state.scores[winner.index().min(3)] =
                            self.state.round_wins[winner.index().min(3)];
                    }
                    if let Some(final_winner) = self.mode.match_over(&self.world, &self.state) {
                        self.end_match(final_winner);
                    } else if self.mode.id().is_round_based() {
                        self.state.phase = Phase::RoundOver;
                        self.state.timer = 5.0;
                        self.broadcast_phase();
                        self.world.events.push(GameEvent::RoundEnd { winner, reason: 0 });
                    } else {
                        self.end_match(winner);
                    }
                }
            }
            Phase::RoundOver => {
                self.state.timer -= dt;
                if self.state.timer <= 0.0 { self.begin_countdown(); }
            }
            Phase::MatchOver => {
                self.state.timer -= dt;
                if self.state.timer <= 0.0 { self.next_map(); }
            }
        }
    }

    fn tick_clock_sync(&mut self, dt: f32) {
        self.clock_sync -= dt;
        if self.clock_sync <= 0.0 {
            self.clock_sync = 2.0;
            self.sync_clock();
        }
    }

    fn clock_announcements(&mut self) {
        use crate::game::events::AnnounceLine;
        let c = self.state.clock;
        let prev = c + TICK_DT;
        if c <= 60.0 && prev > 60.0 {
            self.world.events.push(GameEvent::Announce { line: AnnounceLine::OneMinute });
        }
        if c <= 30.0 && prev > 30.0 {
            self.world.events.push(GameEvent::Announce { line: AnnounceLine::ThirtySeconds });
        }
    }

    fn begin_countdown(&mut self) {
        self.state.phase = Phase::Countdown;
        self.state.timer = 5.0;
        // Cover that was shot away last round comes back for this one, or a
        // long match ends with nothing left to hide behind.
        self.world.map.collision.reset_destruction();
        self.world.brush_health = self.world.map.collision.brushes.iter().map(|b| b.health).collect();
        self.world.events.push(GameEvent::RoundReset);
        self.mode.begin_round(&mut self.world, &mut self.state);
        self.broadcast_phase();
        self.world.events.push(GameEvent::RoundStart { round: self.state.round });
        self.world.events.push(GameEvent::Announce {
            line: crate::game::events::AnnounceLine::MatchStarting,
        });
    }

    pub fn start_match(&mut self) {
        self.fill_bots();
        self.mode.begin_match(&mut self.world, &mut self.state);
        self.state.round = 0;
        self.state.round_wins = [0; 4];
        self.begin_countdown();
    }

    fn end_match(&mut self, winner: Team) {
        self.state.phase = Phase::MatchOver;
        self.state.timer = 15.0;
        self.state.winner = winner;
        self.state.just_ended = true;
        // Most valuable player: highest score, ties broken by fewest deaths.
        let mut best = (NO_PLAYER, i32::MIN, u16::MAX);
        for p in self.world.active_players() {
            if p.score.score > best.1 || (p.score.score == best.1 && p.score.deaths < best.2) {
                best = (p.slot, p.score.score, p.score.deaths);
            }
        }
        self.state.winner_slot = best.0;
        self.broadcast_reliable(&ServerMsg::Results { winner, mvp: best.0 });
        self.broadcast_phase();
        use crate::game::events::AnnounceLine;
        self.world.events.push(GameEvent::Announce {
            line: if winner == Team::None { AnnounceLine::Draw } else { AnnounceLine::Victory },
        });
    }

    fn next_map(&mut self) {
        if self.cfg.rotation.is_empty() {
            self.cfg.rotation = ALL_MAPS.to_vec();
        }
        self.rotation_index = (self.rotation_index + 1) % self.cfg.rotation.len();
        let map = self.cfg.rotation[self.rotation_index];
        self.change_map(map, self.mode.id());
    }

    pub fn change_map(&mut self, map: MapId, mode: ModeId) {
        let seed = self.rng.next_u32();
        self.world.load_map(map, seed);
        self.cfg.map = map;
        self.cfg.mode = mode;
        self.mode = mode.build();
        let limits = (self.state.score_limit, self.state.time_limit);
        self.state = MatchState::new(mode);
        self.state.score_limit = if limits.0 > 0 { mode.default_score_limit() } else { limits.0 };
        self.state.time_limit = mode.default_time_limit();
        self.mode.begin_match(&mut self.world, &mut self.state);
        self.state.phase = Phase::Lobby;
        self.state.timer = self.cfg.lobby_countdown;
        self.director.reset(&self.world);

        for c in self.clients.iter_mut().flatten() {
            c.loaded = false;
            c.ready = false;
            c.history.clear();
            c.acked_snapshot = 0;
            c.conn.send_reliable(encode_server(&ServerMsg::LoadMap { map, mode: mode as u8 }));
        }
        self.broadcast_config();
    }

    fn handle_respawns(&mut self) {
        let now = self.world.time;
        for i in 0..self.world.players.len() {
            if !self.world.players[i].in_use || self.world.players[i].alive { continue; }
            if !self.mode.may_respawn(&self.world, &self.state, i as u8) { continue; }
            if now < self.world.players[i].respawn_at { continue; }
            // Human players in round-based warmups must ask; everyone else
            // comes straight back so the action never stops.
            let wants = self.world.players[i].is_bot
                || self.clients[i].as_ref().map(|c| c.wants_respawn).unwrap_or(true)
                || matches!(self.world.respawn, crate::game::sim::RespawnPolicy::Auto { .. });
            if !wants { continue; }
            self.world.respawn_player(i as u8, self.state.phase == Phase::Countdown);
            self.mode.on_player_spawn(&mut self.world, &self.state, i as u8);
            if let Some(c) = self.clients[i].as_mut() { c.wants_respawn = false; }
        }
    }

    // ============================================================== networking

    fn receive(&mut self) {
        let mut buf = [0u8; MAX_PACKET];
        loop {
            let (len, from) = match self.sock.recv() {
                Some((data, from)) => {
                    let n = data.len().min(MAX_PACKET);
                    buf[..n].copy_from_slice(&data[..n]);
                    (n, from)
                }
                None => break,
            };
            self.stats.bytes_recv += len as u64;
            self.handle_packet(&buf[..len], from);
        }
    }

    fn handle_packet(&mut self, data: &[u8], from: SocketAddr) {
        let Some(kind) = peek_kind(data) else { return };
        match kind {
            PacketKind::Discovery => self.reply_discovery(from),
            PacketKind::ConnectRequest => self.handle_connect_request(data, from),
            PacketKind::ConnectResponse => self.handle_connect_response(data, from),
            PacketKind::Payload => self.handle_payload(data, from),
            PacketKind::Disconnect => self.handle_disconnect(data, from),
            PacketKind::KeepAlive => {}
            PacketKind::Voice => self.handle_voice(data, from),
            _ => {}
        }
    }

    fn reply_discovery(&mut self, to: SocketAddr) {
        let mut buf = [0u8; MAX_PACKET];
        let mut w = Writer::new(&mut buf);
        w.u32(PROTOCOL_MAGIC);
        w.u8(PacketKind::DiscoveryReply as u8);
        self.info().write(&mut w);
        let n = w.finish();
        self.sock.send(&buf[..n], to);
    }

    fn handle_connect_request(&mut self, data: &[u8], from: SocketAddr) {
        let mut r = Reader::new(data);
        if r.u32() != Some(PROTOCOL_MAGIC) { return; }
        if r.u8() != Some(PacketKind::ConnectRequest as u8) { return; }
        let Some(version) = r.u16() else { return };
        let Some(client_salt) = r.u64() else { return };

        if version != PROTOCOL_VERSION {
            self.send_denied(from, DenyReason::BadVersion);
            return;
        }
        if self.clients.iter().flatten().any(|c| c.conn.addr == from) {
            // Already connected: re-sending the challenge is harmless and
            // covers the case where our Accepted packet was lost.
            return;
        }

        let server_salt = self.rng.next_u32() as u64 | ((self.rng.next_u32() as u64) << 32);
        self.pending.retain(|p| p.addr != from);
        self.pending.push(Pending { addr: from, client_salt, server_salt, created: self.time });

        let mut buf = [0u8; MAX_PACKET];
        let mut w = Writer::new(&mut buf);
        w.u32(PROTOCOL_MAGIC);
        w.u8(PacketKind::Challenge as u8);
        w.u64(client_salt);
        w.u64(server_salt);
        w.bool(!self.cfg.password.is_empty());
        let n = w.finish();
        self.sock.send(&buf[..n], from);
    }

    fn handle_connect_response(&mut self, data: &[u8], from: SocketAddr) {
        let mut r = Reader::new(data);
        if r.u32() != Some(PROTOCOL_MAGIC) { return; }
        if r.u8() != Some(PacketKind::ConnectResponse as u8) { return; }
        let Some(token) = r.u64() else { return };
        let Some(pass_hash) = r.u64() else { return };

        let Some(idx) = self.pending.iter().position(|p| p.addr == from && (p.client_salt ^ p.server_salt) == token) else {
            return;
        };
        let pending = self.pending.remove(idx);

        if !self.cfg.password.is_empty() {
            let expect = password_hash(&self.cfg.password, pending.server_salt);
            if expect != pass_hash {
                self.send_denied(from, DenyReason::BadPassword);
                return;
            }
        }

        // Free a bot slot if the server is otherwise full of them.
        let Some(slot) = self.find_slot() else {
            self.send_denied(from, DenyReason::ServerFull);
            return;
        };

        let is_first = !self.clients.iter().flatten().any(|_| true);
        let conn = Connection::new(from, token, self.time);
        let mut client = Client::new(conn, slot);
        client.is_host = is_first || self.cfg.dedicated == false && is_first;
        self.clients[slot as usize] = Some(client);

        let team = auto_assign_team(&self.world, self.mode.id());
        {
            let p = &mut self.world.players[slot as usize];
            *p = crate::game::player::Player::new(slot);
            p.in_use = true;
            p.is_bot = false;
            p.name = format!("PLAYER {}", slot + 1);
            p.team = team;
            p.ready = false;
        }

        let mut buf = [0u8; MAX_PACKET];
        let mut w = Writer::new(&mut buf);
        w.u32(PROTOCOL_MAGIC);
        w.u8(PacketKind::Accepted as u8);
        w.u64(token);
        w.u8(slot);
        w.u8(self.cfg.max_players);
        w.f32(self.cfg.snapshot_hz);
        let n = w.finish();
        self.sock.send(&buf[..n], from);

        self.send_full_state(slot);
        self.broadcast_player_info(slot);
        self.world.events.push(GameEvent::PlayerJoined { player: slot });
    }

    fn find_slot(&mut self) -> Option<u8> {
        let humans = self.world.players.iter().filter(|p| p.in_use && !p.is_bot).count() as u8;
        if humans >= self.cfg.max_players { return None; }
        // Prefer a genuinely empty slot.
        if let Some(i) = self.world.players.iter().position(|p| !p.in_use) {
            return Some(i as u8);
        }
        // Otherwise evict a bot, taking the one with the lowest score.
        let mut worst: Option<(usize, i32)> = None;
        for (i, p) in self.world.players.iter().enumerate() {
            if !p.is_bot { continue; }
            if worst.map_or(true, |(_, s)| p.score.score < s) { worst = Some((i, p.score.score)); }
        }
        worst.map(|(i, _)| {
            self.director.remove_bot(i as u8);
            self.world.players[i].in_use = false;
            i as u8
        })
    }

    fn send_denied(&self, to: SocketAddr, reason: DenyReason) {
        let mut buf = [0u8; MAX_PACKET];
        let mut w = Writer::new(&mut buf);
        w.u32(PROTOCOL_MAGIC);
        w.u8(PacketKind::Denied as u8);
        w.u8(reason as u8);
        let n = w.finish();
        self.sock.send(&buf[..n], to);
    }

    fn handle_disconnect(&mut self, data: &[u8], from: SocketAddr) {
        let mut r = Reader::new(data);
        let Some((_, h)) = PacketHeader::read(&mut r) else { return };
        if let Some(slot) = self.slot_of(from, h.token) {
            self.drop_client(slot, "left");
        }
    }

    fn slot_of(&self, addr: SocketAddr, token: u64) -> Option<u8> {
        self.clients.iter().flatten()
            .find(|c| c.conn.addr == addr && c.conn.token == token)
            .map(|c| c.slot)
    }

    /// Relays one frame of speech to the sender's living teammates.
    ///
    /// The server does not decode it. Voice is opaque here on purpose: it
    /// keeps the codec entirely a client concern, and it means a malformed
    /// frame costs one player a syllable rather than costing the server
    /// anything at all. The payload is bounded and the rate is limited, which
    /// is the only part the server has to care about.
    fn handle_voice(&mut self, data: &[u8], from: SocketAddr) {
        let mut r = Reader::new(data);
        let Some(magic) = r.u32() else { return };
        if magic != PROTOCOL_MAGIC { return; }
        let Some(_kind) = r.u8() else { return };
        let Some(token) = r.u64() else { return };
        let Some(slot) = self.slot_of(from, token) else { return };
        let Some(len) = r.u16() else { return };
        let len = len as usize;
        if len == 0 || len > crate::audio::voice::FRAME_BYTES * 4 { return; }
        let Some(frame) = r.bytes(len) else { return };

        // Rate limit: a client that sends faster than it can speak is either
        // broken or trying to use voice as an amplifier against the server.
        let now = self.time;
        {
            let Some(c) = self.clients[slot as usize].as_mut() else { return };
            if now - c.last_voice < 0.012 { return; }
            c.last_voice = now;
        }

        let team = self.world.players.get(slot as usize).map(|p| p.team);
        let Some(team) = team else { return };

        let mut buf = [0u8; MAX_PACKET];
        let n = {
            let mut w = Writer::new(&mut buf);
            w.u32(PROTOCOL_MAGIC);
            w.u8(PacketKind::Voice as u8);
            w.u8(slot);
            w.u16(len as u16);
            w.bytes(frame);
            w.len()
        };

        let targets: Vec<SocketAddr> = self.clients.iter().enumerate()
            .filter_map(|(i, c)| {
                let c = c.as_ref()?;
                if i as u8 == slot { return None; }
                // Team-only, which is what makes it a tactical channel rather
                // than a shouting match.
                if self.world.players.get(i).map(|p| p.team) != Some(team) { return None; }
                Some(c.conn.addr)
            })
            .collect();
        for addr in targets {
            self.sock.send(&buf[..n], addr);
        }
    }

    fn handle_payload(&mut self, data: &[u8], from: SocketAddr) {
        let mut r = Reader::new(data);
        let Some((_, header)) = PacketHeader::read(&mut r) else { return };
        let Some(slot) = self.slot_of(from, header.token) else { return };
        let si = slot as usize;

        {
            let Some(c) = self.clients[si].as_mut() else { return };
            c.conn.on_header(&header, self.time, data.len());
        }

        // Reliable messages first: they may change the loadout the following
        // inputs are executed with.
        let mut msgs: Vec<&[u8]> = Vec::new();
        {
            let Some(c) = self.clients[si].as_mut() else { return };
            if !c.conn.read_reliable(&mut r, &mut msgs) { return; }
        }
        for body in msgs {
            let mut mr = Reader::new(body);
            if let Some(msg) = ClientMsg::decode(&mut mr) {
                self.handle_client_msg(slot, msg);
            }
        }

        // Unreliable section: input commands.
        let Some(kind) = r.u8() else { return };
        if kind != 1 { return; }
        let Some(ack_snapshot) = r.u32() else { return };
        let Some(count) = r.u8() else { return };
        if count as usize > INPUT_REDUNDANCY + 4 { return; }

        if let Some(c) = self.clients[si].as_mut() {
            if ack_snapshot > c.acked_snapshot { c.acked_snapshot = ack_snapshot; }
            c.loaded = true;
        }

        let mut cmds: Vec<InputCmd> = Vec::with_capacity(count as usize);
        for _ in 0..count {
            match read_input(&mut r) {
                Some(cmd) => cmds.push(cmd),
                None => return,
            }
        }
        cmds.sort_unstable_by_key(|c| c.seq);
        self.execute_input(slot, &cmds);
    }

    /// Executes new commands, refusing to simulate more time than the client
    /// could plausibly have generated.
    fn execute_input(&mut self, slot: u8, cmds: &[InputCmd]) {
        let si = slot as usize;
        let playable = self.state.phase.playable();
        let (mut last, budget) = match self.clients[si].as_ref() {
            Some(c) => (c.last_input, c.budget),
            None => return,
        };
        let mut budget = budget;

        for cmd in cmds {
            if cmd.seq <= last { continue; }
            // Anti-speed: the client's own dt is clamped, and the total is
            // capped by a budget that refills at real time.
            let dt = cmd.dt();
            if budget < dt {
                // Out of budget; accept the command but at reduced time so
                // the player stalls rather than teleports.
                last = cmd.seq;
                continue;
            }
            budget -= dt;
            last = cmd.seq;
            if !playable { continue; }
            self.world.run_command(slot, cmd);
        }

        if let Some(c) = self.clients[si].as_mut() {
            c.last_input = last;
            c.budget = budget;
        }
        if let Some(p) = self.world.player_mut(slot) {
            if let Some(c) = self.clients[si].as_ref() {
                p.ping_ms = c.conn.ping_ms();
            }
        }
    }

    fn handle_client_msg(&mut self, slot: u8, msg: ClientMsg) {
        match msg {
            ClientMsg::Hello { name, level, loadout } => {
                let level = level.min(60);
                let mut l = Loadout::decode(loadout);
                l.sanitize(level);
                if let Some(p) = self.world.player_mut(slot) {
                    p.name = sanitize_name(&name, slot);
                    p.level = level;
                    p.loadout = l;
                }
                if let Some(c) = self.clients[slot as usize].as_mut() { c.name_sent = true; }
                self.broadcast_player_info(slot);
            }
            ClientMsg::SetLoadout { loadout } => {
                let level = self.world.player(slot).map(|p| p.level).unwrap_or(1);
                let mut l = Loadout::decode(loadout);
                l.sanitize(level);
                if let Some(p) = self.world.player_mut(slot) {
                    p.loadout = l;
                    // Applies on the next spawn, never mid-life.
                    if !p.alive { p.equip(); }
                }
            }
            ClientMsg::Chat { team_only, text } => {
                let text = sanitize_chat(&text);
                if text.is_empty() { return; }
                let from_team = self.world.player(slot).map(|p| p.team).unwrap_or(Team::None);
                let msg = ServerMsg::Chat { slot, team_only, text };
                if team_only {
                    for i in 0..self.clients.len() {
                        let same = self.world.players[i].team == from_team;
                        if same {
                            if let Some(c) = self.clients[i].as_mut() {
                                c.conn.send_reliable(encode_server(&msg));
                            }
                        }
                    }
                } else {
                    self.broadcast_reliable(&msg);
                }
            }
            ClientMsg::ChangeTeam { team } => {
                if !self.mode.id().is_team_game() { return; }
                if team != Team::Phantom && team != Team::Vanguard && team != Team::Spectator { return; }
                // Refuse a swap that would unbalance the sides.
                let cur = self.world.player(slot).map(|p| p.team).unwrap_or(Team::None);
                if cur == team { return; }
                let want = self.world.team_count(team);
                let other = self.world.team_count(team.opposite());
                if team != Team::Spectator && want > other { return; }
                if let Some(p) = self.world.player_mut(slot) {
                    p.team = team;
                    p.alive = false;
                    p.respawn_at = 0.0;
                }
                self.broadcast_player_info(slot);
                self.world.events.push(GameEvent::TeamChanged { player: slot, team });
            }
            ClientMsg::Ready { ready } => {
                if let Some(c) = self.clients[slot as usize].as_mut() { c.ready = ready; }
                if let Some(p) = self.world.player_mut(slot) { p.ready = ready; }
                self.broadcast_player_info(slot);
            }
            ClientMsg::RequestRespawn => {
                if let Some(c) = self.clients[slot as usize].as_mut() { c.wants_respawn = true; }
            }
            ClientMsg::HostConfig { map, mode, bots, score_limit, time_limit, friendly_fire } => {
                if !self.is_host(slot) { return; }
                if self.state.phase != Phase::Lobby && self.state.phase != Phase::MatchOver { return; }
                self.cfg.bot_count = bots.min(15);
                self.cfg.friendly_fire = friendly_fire;
                self.world.friendly_fire = friendly_fire;
                let new_mode = ModeId::from_u8(mode);
                let new_map = MapId::from_u8(map);
                if new_map != self.world.map_id || new_mode != self.mode.id() {
                    self.change_map(new_map, new_mode);
                }
                if score_limit > 0 { self.state.score_limit = score_limit; }
                if time_limit > 0 { self.state.time_limit = time_limit; }
                self.state.clock = self.state.time_limit as f32;
                self.broadcast_config();
            }
            ClientMsg::HostStart => {
                if !self.is_host(slot) { return; }
                if self.state.phase == Phase::Lobby || self.state.phase == Phase::MatchOver {
                    self.start_match();
                }
            }
            ClientMsg::Disconnecting => self.drop_client(slot, "left"),
        }
    }

    fn is_host(&self, slot: u8) -> bool {
        self.clients[slot as usize].as_ref().map(|c| c.is_host).unwrap_or(false)
            || self.clients.iter().flatten().count() == 1
    }

    fn expire(&mut self) {
        self.pending.retain(|p| self.time - p.created < 10.0);
        let mut drop_list = Vec::new();
        for c in self.clients.iter().flatten() {
            if c.conn.timed_out(self.time, TIMEOUT) { drop_list.push(c.slot); }
        }
        for slot in drop_list { self.drop_client(slot, "timed out"); }

        // Keep the input budget topped up at real time, slightly generously so
        // a client with jittery timing is not punished.
        for c in self.clients.iter_mut().flatten() {
            c.budget = (c.budget + TICK_DT * 1.05).min(INPUT_BUDGET_MAX);
        }
        self.fill_bots();
    }

    fn drop_client(&mut self, slot: u8, _why: &str) {
        if self.clients[slot as usize].is_none() { return; }
        self.clients[slot as usize] = None;
        self.world.players[slot as usize].in_use = false;
        self.world.players[slot as usize].alive = false;
        self.broadcast_reliable(&ServerMsg::PlayerLeft { slot });
        self.world.events.push(GameEvent::PlayerLeft { player: slot });
    }

    /// Keeps the server topped up with bots to the configured count.
    fn fill_bots(&mut self) {
        let humans = self.world.players.iter().filter(|p| p.in_use && !p.is_bot).count();
        let bots = self.world.players.iter().filter(|p| p.in_use && p.is_bot).count();
        let target = (self.cfg.bot_count as usize).min(MAX_PLAYERS.saturating_sub(humans));

        if bots < target {
            for _ in bots..target {
                let Some(i) = self.world.players.iter().position(|p| !p.in_use) else { break };
                let team = auto_assign_team(&self.world, self.mode.id());
                self.director.add_bot(&mut self.world, i as u8, team);
                self.broadcast_player_info(i as u8);
            }
        } else if bots > target {
            let mut to_remove = bots - target;
            for i in 0..self.world.players.len() {
                if to_remove == 0 { break; }
                if self.world.players[i].in_use && self.world.players[i].is_bot {
                    self.director.remove_bot(i as u8);
                    self.world.players[i].in_use = false;
                    self.broadcast_reliable(&ServerMsg::PlayerLeft { slot: i as u8 });
                    to_remove -= 1;
                }
            }
        }
    }

    // ================================================================ sending

    fn broadcast_reliable(&mut self, msg: &ServerMsg) {
        let bytes = encode_server(msg);
        for c in self.clients.iter_mut().flatten() {
            c.conn.send_reliable(bytes.clone());
        }
    }

    fn broadcast_config(&mut self) {
        let msg = ServerMsg::MatchConfig {
            server_name: self.cfg.name.clone(),
            map: self.world.map_id,
            mode: self.mode.id() as u8,
            score_limit: self.state.score_limit,
            time_limit: self.state.time_limit,
            max_players: self.cfg.max_players,
            friendly_fire: self.cfg.friendly_fire,
            bot_count: self.cfg.bot_count,
            phase: self.state.phase as u8,
        };
        self.broadcast_reliable(&msg);
    }

    /// What the HUD clock should read: the round clock while play is live,
    /// otherwise whatever the current phase is counting down to.
    fn display_clock(&self) -> u16 {
        match self.state.phase {
            Phase::Live => self.state.clock.max(0.0).ceil() as u16,
            _ => self.state.timer.max(0.0).ceil() as u16,
        }
    }

    /// Just the phase/clock word, no scores. Sent every couple of seconds so a
    /// client that has been ticking its own clock cannot drift.
    fn sync_clock(&mut self) {
        let msg = ServerMsg::Phase {
            phase: self.state.phase as u8,
            seconds_left: self.display_clock(),
            round: self.state.round,
        };
        self.broadcast_reliable(&msg);
    }

    fn broadcast_phase(&mut self) {
        let msg = ServerMsg::Phase {
            phase: self.state.phase as u8,
            seconds_left: self.display_clock(),
            round: self.state.round,
        };
        self.broadcast_reliable(&msg);
        let scores = ServerMsg::TeamScores {
            phantom: self.state.score(Team::Phantom),
            vanguard: self.state.score(Team::Vanguard),
        };
        self.broadcast_reliable(&scores);
    }

    fn broadcast_player_info(&mut self, slot: u8) {
        let Some(p) = self.world.player(slot) else { return };
        let msg = ServerMsg::PlayerInfo {
            slot,
            name: p.name.clone(),
            team: p.team,
            is_bot: p.is_bot,
            level: p.level,
            ping: p.ping_ms,
            ready: p.ready,
        };
        self.broadcast_reliable(&msg);
    }

    /// Sends everything a freshly connected client needs.
    fn send_full_state(&mut self, slot: u8) {
        let config = ServerMsg::MatchConfig {
            server_name: self.cfg.name.clone(),
            map: self.world.map_id,
            mode: self.mode.id() as u8,
            score_limit: self.state.score_limit,
            time_limit: self.state.time_limit,
            max_players: self.cfg.max_players,
            friendly_fire: self.cfg.friendly_fire,
            bot_count: self.cfg.bot_count,
            phase: self.state.phase as u8,
        };
        let mut msgs = vec![config];
        for p in self.world.active_players() {
            msgs.push(ServerMsg::PlayerInfo {
                slot: p.slot,
                name: p.name.clone(),
                team: p.team,
                is_bot: p.is_bot,
                level: p.level,
                ping: p.ping_ms,
                ready: p.ready,
            });
        }
        msgs.push(ServerMsg::Phase {
            phase: self.state.phase as u8,
            seconds_left: self.display_clock(),
            round: self.state.round,
        });
        if let Some(c) = self.clients[slot as usize].as_mut() {
            for m in msgs { c.conn.send_reliable(encode_server(&m)); }
        }
    }

    /// Routes the tick's events to whichever clients can perceive them.
    fn distribute_events(&mut self) {
        if self.world.events.is_empty() { return; }
        let events: Vec<GameEvent> = self.world.events.drain().collect();
        if let Some(tap) = &mut self.event_tap { tap.extend(events.iter().cloned()); }

        for ev in &events {
            // Reliable, global events go through the message channel.
            if ev.delivery() == Delivery::Reliable {
                if let Some(msg) = event_to_message(ev) {
                    self.broadcast_reliable(&msg);
                }
            }
            let pos = ev.position();
            let radius = ev.relevance_radius();
            let private = ev.private_to();

            for i in 0..self.clients.len() {
                if self.clients[i].is_none() { continue; }
                if let Some(only) = private {
                    if only != i as u8 { continue; }
                }
                if let (Some(p), Some(r)) = (pos, if radius > 0.0 { Some(radius) } else { None }) {
                    let listener = self.world.players[i].mv.pos;
                    if (listener - p).length() > r { continue; }
                }
                if let Some(c) = self.clients[i].as_mut() {
                    if c.pending_events.len() < 96 {
                        c.pending_events.push(ev.clone());
                    }
                }
            }
        }
    }

    fn send_snapshots(&mut self) {
        let tick = self.world.tick;
        let time_ms = (self.world.time * 1000.0) as u32;

        // Build the full snapshot once, then delta it per client.
        let mut full = [PlayerSnap::default(); MAX_PLAYERS];
        for (i, p) in self.world.players.iter().enumerate() {
            if !p.in_use { continue; }
            full[i] = PlayerSnap {
                pos: p.mv.pos,
                vel: p.mv.vel,
                yaw: p.mv.yaw,
                pitch: p.mv.pitch,
                height: p.mv.height,
                flags: p.flags(),
                health: (p.health.clamp(0.0, 255.0)) as u8,
                armor: (p.armor.clamp(0.0, 255.0)) as u8,
                weapon: p.weapon().id as u8,
                ammo: p.weapon().ammo.min(255) as u8,
                team: p.team as u8,
                present: true,
            };
        }

        let hud = self.mode.hud_state(&self.world, &self.state);
        for i in 0..self.clients.len() {
            if self.clients[i].is_none() { continue; }
            self.send_snapshot_to(i as u8, tick, time_ms, &full, &hud);
        }
        self.stats.snapshots_sent += 1;
    }

    fn send_snapshot_to(&mut self, slot: u8, tick: u32, time_ms: u32, full: &[PlayerSnap; MAX_PLAYERS], hud: &[u8; 8]) {
        let si = slot as usize;
        let viewer_pos = self.world.players[si].mv.pos;

        let mut buf = std::mem::take(&mut self.scratch);
        let n = {
            let mut w = Writer::new(&mut buf);
            let header = {
                let Some(c) = self.clients[si].as_mut() else { return };
                c.conn.next_header(self.time)
            };
            header.write(&mut w, PacketKind::Payload);
            {
                let Some(c) = self.clients[si].as_ref() else { return };
                c.conn.write_reliable(&mut w);
            }

            w.u8(1); // unreliable kind: snapshot
            w.u32(tick);
            w.u32(time_ms);
            let last_input = self.clients[si].as_ref().map(|c| c.last_input).unwrap_or(0);
            w.u32(last_input);

            let base_tick = self.clients[si].as_ref().map(|c| c.acked_snapshot).unwrap_or(0);
            let empty = [PlayerSnap::default(); MAX_PLAYERS];
            let baseline: [PlayerSnap; MAX_PLAYERS] = self.clients[si].as_ref()
                .and_then(|c| c.baseline(base_tick))
                .copied()
                .unwrap_or(empty);
            let used_base = self.clients[si].as_ref()
                .map(|c| c.baseline(base_tick).is_some())
                .unwrap_or(false);
            w.u32(if used_base { base_tick } else { 0 });

            let count_at = w.reserve_u8();
            let mut count = 0u8;
            for (i, snap) in full.iter().enumerate() {
                if !snap.present { continue; }
                // Relevance: always send yourself and anyone close enough to
                // matter. Distant players are simply omitted.
                if i != si {
                    let d = (snap.pos - viewer_pos).length();
                    if d > RELEVANCE_RANGE { continue; }
                }
                snap.write_delta(&mut w, &baseline[i], i as u8);
                count += 1;
                if w.remaining() < 200 { break; }
            }
            w.patch_u8(count_at, count);

            // Events, newest last, trimmed to whatever space is left.
            let ev_at = w.reserve_u8();
            let mut ev_count = 0u8;
            if let Some(c) = self.clients[si].as_mut() {
                for ev in c.pending_events.iter() {
                    if w.remaining() < 40 { break; }
                    let before = w.len();
                    if encode_event(&mut w, ev) {
                        if w.overflowed() { break; }
                        ev_count += 1;
                    } else {
                        let _ = before;
                    }
                    if ev_count == 255 { break; }
                }
                c.pending_events.clear();
            }
            w.patch_u8(ev_at, ev_count);

            // Mode HUD state and the local player's authoritative state.
            w.bytes(hud);
            let local = self.local_state(slot);
            local.write(&mut w);

            if w.overflowed() { 0 } else { w.finish() }
        };

        if n > 0 {
            self.sock.send(&buf[..n], self.clients[si].as_ref().unwrap().conn.addr);
            self.stats.bytes_sent += n as u64;
            if let Some(c) = self.clients[si].as_mut() {
                c.conn.note_sent(n);
                c.history.push((tick, *full));
                if c.history.len() > 32 { c.history.remove(0); }
            }
        }
        self.scratch = buf;
    }

    fn local_state(&self, slot: u8) -> LocalState {
        let p = &self.world.players[slot as usize];
        let mut ammo = [0u16; 3];
        let mut reserve = [0u16; 3];
        for i in 0..3 {
            ammo[i] = p.weapons[i].ammo;
            reserve[i] = p.weapons[i].reserve;
        }
        LocalState {
            pos: p.mv.pos,
            vel: p.mv.vel,
            height: p.mv.height,
            stance: p.mv.stance as u8,
            grounded: p.mv.grounded,
            sprint_t: p.mv.sprint_t,
            ads_t: p.mv.ads_t,
            health: p.health,
            armor: p.armor,
            weapon_slot: p.cur,
            ammo,
            reserve,
            lethal: p.lethal_count,
            tactical: p.tactical_count,
            flash: p.flash,
            concussion: p.concussion,
            alive: p.alive,
            respawn_in: (p.respawn_at - self.world.time).max(0.0).min(60.0) as f32,
        }
    }

    /// Scoreboard refresh; called a couple of times a second by the host loop.
    pub fn broadcast_scores(&mut self) {
        let entries: Vec<(u8, u16, u16, u16, i32, u16)> = self.world.active_players()
            .map(|p| (p.slot, p.score.kills, p.score.deaths, p.score.assists, p.score.score, p.ping_ms))
            .collect();
        if entries.is_empty() { return; }
        self.broadcast_reliable(&ServerMsg::Scores { entries });
        let scores = ServerMsg::TeamScores {
            phantom: self.state.score(Team::Phantom),
            vanguard: self.state.score(Team::Vanguard),
        };
        self.broadcast_reliable(&scores);
    }

    pub fn client_count(&self) -> usize { self.clients.iter().flatten().count() }
}

fn encode_server(msg: &ServerMsg) -> Vec<u8> {
    let mut buf = vec![0u8; 640];
    let n = {
        let mut w = Writer::new(&mut buf);
        msg.encode(&mut w);
        w.finish()
    };
    buf.truncate(n);
    buf
}

/// Reliable events that are better expressed as messages.
fn event_to_message(_ev: &GameEvent) -> Option<ServerMsg> {
    // Kills, captures and the rest travel in the snapshot's event list, which
    // is already ordered and arrives promptly. Only chat and roster changes
    // need the reliable channel, and those are sent directly.
    None
}

/// Names are attacker-controlled text that ends up on everyone's screen.
fn sanitize_name(raw: &str, slot: u8) -> String {
    let cleaned: String = raw.chars()
        .filter(|c| !c.is_control() && *c != '\u{202e}' && *c != '\u{200b}')
        .take(MAX_NAME_LEN)
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() { format!("PLAYER {}", slot + 1) } else { trimmed.to_string() }
}

fn sanitize_chat(raw: &str) -> String {
    raw.chars()
        .filter(|c| !c.is_control())
        .take(MAX_CHAT_LEN)
        .collect::<String>()
        .trim()
        .to_string()
}
