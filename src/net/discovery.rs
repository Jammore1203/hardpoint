//! Finding games.
//!
//! Two mechanisms, both real. Broadcast discovery finds servers on the local
//! network without any configuration, and a tracker is a tiny registry any
//! machine can run so servers elsewhere can be listed too. Direct connection
//! by address always works and needs neither.
//!
//! The tracker is deliberately the same protocol and the same socket code as
//! everything else: it is a hundred lines, not a service.

use super::bits::{Reader, Writer};
use super::protocol::*;
use super::socket::{parse_endpoint, Socket};
use std::collections::HashMap;
use std::net::SocketAddr;

/// How long a server stays in the list after we last heard from it.
const ENTRY_TTL: f64 = 6.0;
/// How long a tracker keeps a server registered without a heartbeat.
const TRACKER_TTL: f64 = 45.0;

#[derive(Clone, Debug)]
pub struct ServerEntry {
    pub addr: SocketAddr,
    pub info: ServerInfo,
    pub ping_ms: u32,
    pub last_seen: f64,
    /// True when the entry came from a tracker rather than the local network.
    pub remote: bool,
    pub favourite: bool,
}

impl ServerEntry {
    pub fn slots_text(&self) -> String {
        format!("{}/{}", self.info.players + self.info.bots, self.info.max_players)
    }
    pub fn is_full(&self) -> bool {
        self.info.players >= self.info.max_players
    }
    pub fn status(&self) -> &'static str {
        match crate::modes::Phase::from_u8(self.info.phase) {
            crate::modes::Phase::Lobby => "IN LOBBY",
            crate::modes::Phase::Warmup => "WARMUP",
            crate::modes::Phase::Countdown => "STARTING",
            crate::modes::Phase::Live => "IN PROGRESS",
            crate::modes::Phase::RoundOver => "ROUND OVER",
            crate::modes::Phase::MatchOver => "MATCH OVER",
        }
    }
}

pub struct Browser {
    sock: Socket,
    entries: HashMap<SocketAddr, ServerEntry>,
    probes: HashMap<SocketAddr, f64>,
    pub last_refresh: f64,
    pub refreshing: bool,
    /// Address of a tracker to ask, if the player has configured one.
    pub tracker: Option<SocketAddr>,
}

impl Browser {
    pub fn new() -> std::io::Result<Browser> {
        Ok(Browser {
            sock: Socket::bind_any()?,
            entries: HashMap::new(),
            probes: HashMap::new(),
            last_refresh: -100.0,
            refreshing: false,
            tracker: None,
        })
    }

    pub fn set_tracker(&mut self, address: &str) {
        self.tracker = parse_endpoint(address, DEFAULT_PORT + 100);
    }

    /// Broadcasts a discovery request across the usual port range and asks the
    /// tracker, if one is configured.
    pub fn refresh(&mut self, now: f64) {
        self.last_refresh = now;
        self.refreshing = true;

        let mut buf = [0u8; 32];
        let n = {
            let mut w = Writer::new(&mut buf);
            w.u32(PROTOCOL_MAGIC);
            w.u8(PacketKind::Discovery as u8);
            w.u16(PROTOCOL_VERSION);
            w.finish()
        };
        self.sock.broadcast(&buf[..n], DISCOVERY_PORTS);
        let probes: Vec<SocketAddr> = self.probes.keys().copied().collect();
        for addr in probes {
            self.sock.send(&buf[..n], addr);
        }

        if let Some(tracker) = self.tracker {
            let mut q = [0u8; 32];
            let qn = {
                let mut w = Writer::new(&mut q);
                w.u32(PROTOCOL_MAGIC);
                w.u8(PacketKind::TrackerQuery as u8);
                w.u16(PROTOCOL_VERSION);
                w.finish()
            };
            self.sock.send(&q[..qn], tracker);
        }
    }

    /// Adds an address to probe every refresh, for favourites and manual entry.
    pub fn probe(&mut self, address: &str, now: f64) -> bool {
        let Some(addr) = parse_endpoint(address, DEFAULT_PORT) else { return false };
        self.probes.insert(addr, now);
        let mut buf = [0u8; 32];
        let n = {
            let mut w = Writer::new(&mut buf);
            w.u32(PROTOCOL_MAGIC);
            w.u8(PacketKind::Discovery as u8);
            w.u16(PROTOCOL_VERSION);
            w.finish()
        };
        self.sock.send(&buf[..n], addr);
        true
    }

    /// Reads replies and expires stale entries.
    pub fn update(&mut self, now: f64) {
        let mut scratch = [0u8; MAX_PACKET];
        loop {
            let (len, from) = match self.sock.recv() {
                Some((d, from)) => {
                    let n = d.len().min(MAX_PACKET);
                    scratch[..n].copy_from_slice(&d[..n]);
                    (n, from)
                }
                None => break,
            };
            let data = &scratch[..len];
            let Some(kind) = peek_kind(data) else { continue };
            match kind {
                PacketKind::DiscoveryReply => {
                    let mut r = Reader::new(data);
                    let _ = r.u32();
                    let _ = r.u8();
                    let Some(info) = ServerInfo::read(&mut r) else { continue };
                    if info.version != PROTOCOL_VERSION { continue; }
                    let ping = ((now - self.last_refresh) * 1000.0).max(0.0) as u32;
                    let entry = self.entries.entry(from).or_insert(ServerEntry {
                        addr: from,
                        info: info.clone(),
                        ping_ms: ping,
                        last_seen: now,
                        remote: false,
                        favourite: false,
                    });
                    entry.info = info;
                    entry.last_seen = now;
                    // Smooth the ping so the column does not flicker.
                    entry.ping_ms = ((entry.ping_ms as f32 * 0.6) + ping as f32 * 0.4) as u32;
                }
                PacketKind::TrackerList => {
                    let mut r = Reader::new(data);
                    let _ = r.u32();
                    let _ = r.u8();
                    let Some(count) = r.u8() else { continue };
                    for _ in 0..count {
                        let (Some(a), Some(b), Some(c), Some(d), Some(port)) =
                            (r.u8(), r.u8(), r.u8(), r.u8(), r.u16()) else { break };
                        let addr = SocketAddr::from(([a, b, c, d], port));
                        let Some(info) = ServerInfo::read(&mut r) else { break };
                        let entry = self.entries.entry(addr).or_insert(ServerEntry {
                            addr,
                            info: info.clone(),
                            ping_ms: 0,
                            last_seen: now,
                            remote: true,
                            favourite: false,
                        });
                        entry.info = info;
                        entry.last_seen = now;
                        entry.remote = true;
                        // Ping a tracker-listed server directly so the number
                        // shown is the player's real latency to it.
                        self.probes.insert(addr, now);
                    }
                }
                _ => {}
            }
        }

        self.entries.retain(|_, e| now - e.last_seen < ENTRY_TTL || e.favourite);
        if now - self.last_refresh > 1.2 { self.refreshing = false; }
    }

    pub fn mark_favourites(&mut self, list: &[String]) {
        for f in list {
            if let Some(addr) = parse_endpoint(f, DEFAULT_PORT) {
                if let Some(e) = self.entries.get_mut(&addr) { e.favourite = true; }
            }
        }
    }

    /// Servers sorted the way a player wants to see them: joinable first.
    pub fn sorted(&self) -> Vec<ServerEntry> {
        let mut v: Vec<ServerEntry> = self.entries.values().cloned().collect();
        v.sort_by(|a, b| {
            let ka = (a.is_full(), a.remote, a.ping_ms, a.info.name.clone());
            let kb = (b.is_full(), b.remote, b.ping_ms, b.info.name.clone());
            ka.cmp(&kb)
        });
        v
    }

    pub fn len(&self) -> usize { self.entries.len() }
    pub fn is_empty(&self) -> bool { self.entries.is_empty() }
    pub fn clear(&mut self) { self.entries.clear(); }
}

/// Server-side helper: announces this server to a tracker.
pub struct TrackerClient {
    tracker: SocketAddr,
    sock: Socket,
    next_beat: f64,
}

impl TrackerClient {
    pub fn new(address: &str) -> Option<TrackerClient> {
        let tracker = parse_endpoint(address, DEFAULT_PORT + 100)?;
        let sock = Socket::bind_any().ok()?;
        Some(TrackerClient { tracker, sock, next_beat: 0.0 })
    }

    /// Sends a heartbeat every fifteen seconds.
    pub fn update(&mut self, now: f64, port: u16, info: &ServerInfo) {
        if now < self.next_beat { return; }
        self.next_beat = now + 15.0;
        let mut buf = [0u8; 128];
        let n = {
            let mut w = Writer::new(&mut buf);
            w.u32(PROTOCOL_MAGIC);
            w.u8(PacketKind::TrackerHeartbeat as u8);
            w.u16(port);
            info.write(&mut w);
            w.finish()
        };
        self.sock.send(&buf[..n], self.tracker);
    }
}

/// The tracker itself: a registry of servers that have said hello recently.
///
/// This is what an online master server would be, minus the parts that need a
/// hosted machine. Anyone can run one and point their friends at it.
pub struct Tracker {
    sock: Socket,
    servers: HashMap<SocketAddr, (ServerInfo, f64)>,
}

impl Tracker {
    pub fn bind(port: u16) -> std::io::Result<Tracker> {
        Ok(Tracker { sock: Socket::bind(port)?, servers: HashMap::new() })
    }

    pub fn port(&self) -> u16 { self.sock.port() }
    pub fn count(&self) -> usize { self.servers.len() }

    pub fn update(&mut self, now: f64) {
        let mut scratch = [0u8; MAX_PACKET];
        loop {
            let (len, from) = match self.sock.recv() {
                Some((d, from)) => {
                    let n = d.len().min(MAX_PACKET);
                    scratch[..n].copy_from_slice(&d[..n]);
                    (n, from)
                }
                None => break,
            };
            let data = &scratch[..len];
            let Some(kind) = peek_kind(data) else { continue };
            match kind {
                PacketKind::TrackerHeartbeat | PacketKind::TrackerRegister => {
                    let mut r = Reader::new(data);
                    let _ = r.u32();
                    let _ = r.u8();
                    let Some(port) = r.u16() else { continue };
                    let Some(info) = ServerInfo::read(&mut r) else { continue };
                    if info.version != PROTOCOL_VERSION { continue; }
                    // Register the address we actually received from, with the
                    // port the server says it listens on. A server cannot
                    // register somebody else's address this way.
                    let addr = SocketAddr::new(from.ip(), port);
                    self.servers.insert(addr, (info, now));
                }
                PacketKind::TrackerQuery => {
                    self.reply(from);
                }
                _ => {}
            }
        }
        self.servers.retain(|_, (_, t)| now - *t < TRACKER_TTL);
    }

    fn reply(&self, to: SocketAddr) {
        let mut buf = [0u8; MAX_PACKET];
        let n = {
            let mut w = Writer::new(&mut buf);
            w.u32(PROTOCOL_MAGIC);
            w.u8(PacketKind::TrackerList as u8);
            let count_at = w.reserve_u8();
            let mut count = 0u8;
            for (addr, (info, _)) in self.servers.iter() {
                if w.remaining() < 64 { break; }
                let ip = match addr.ip() {
                    std::net::IpAddr::V4(v4) => v4.octets(),
                    // The protocol carries v4 only; a v6 server is reachable
                    // by direct connect but is not advertised here.
                    std::net::IpAddr::V6(_) => continue,
                };
                w.u8(ip[0]); w.u8(ip[1]); w.u8(ip[2]); w.u8(ip[3]);
                w.u16(addr.port());
                info.write(&mut w);
                count += 1;
                if count == 255 { break; }
            }
            w.patch_u8(count_at, count);
            w.finish()
        };
        self.sock.send(&buf[..n], to);
    }
}
