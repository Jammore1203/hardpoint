//! Per-connection reliability, acknowledgement and round-trip measurement.
//!
//! Two channels share one packet: an unreliable payload (inputs or a snapshot)
//! that is simply dropped if it is lost, and an ordered reliable stream that
//! is resent from the oldest unacknowledged message until it gets through.
//! Resending from the oldest keeps the receiver trivial - it accepts exactly
//! the message it is waiting for and ignores everything else - which removes a
//! whole class of reordering bugs.

use super::protocol::{PacketHeader, PacketKind, MAX_PACKET};
use std::collections::VecDeque;
use std::net::SocketAddr;

/// How many reliable messages may be in flight before we stop queuing more.
const MAX_RELIABLE_QUEUE: usize = 256;
/// Bytes of reliable data per packet, leaving room for the payload.
const RELIABLE_BUDGET: usize = 420;
const RTT_SAMPLES: usize = 64;

pub struct Connection {
    pub addr: SocketAddr,
    pub token: u64,

    // ------------------------------------------------------- sequencing
    seq: u16,
    remote_seq: u16,
    ack_bits: u32,
    /// Send time of each of our last 64 packets, for round-trip measurement.
    sent_at: [f64; RTT_SAMPLES],
    acked: [bool; RTT_SAMPLES],

    // ---------------------------------------------------------- reliable
    out_queue: VecDeque<(u32, Vec<u8>)>,
    next_out_seq: u32,
    /// Highest message the peer has confirmed receiving in order.
    peer_ack: u32,
    /// Next message we expect from the peer.
    next_in_seq: u32,

    // ------------------------------------------------------------- stats
    pub rtt_ms: f32,
    pub jitter_ms: f32,
    pub last_recv: f64,
    pub last_send: f64,
    pub sent_bytes: u64,
    pub recv_bytes: u64,
    pub sent_packets: u64,
    pub recv_packets: u64,
    pub lost_packets: u32,
    /// Smoothed fraction of packets that never arrived.
    pub loss: f32,
}

impl Connection {
    pub fn new(addr: SocketAddr, token: u64, now: f64) -> Connection {
        Connection {
            addr, token,
            seq: 0,
            remote_seq: 0,
            ack_bits: 0,
            sent_at: [0.0; RTT_SAMPLES],
            acked: [true; RTT_SAMPLES],
            out_queue: VecDeque::new(),
            next_out_seq: 1,
            peer_ack: 0,
            next_in_seq: 1,
            rtt_ms: 60.0,
            jitter_ms: 0.0,
            last_recv: now,
            last_send: now,
            sent_bytes: 0,
            recv_bytes: 0,
            sent_packets: 0,
            recv_packets: 0,
            lost_packets: 0,
            loss: 0.0,
        }
    }

    /// Queues a reliable message. Silently drops if the peer has stopped
    /// acknowledging entirely, which means the connection is already dead.
    pub fn send_reliable(&mut self, bytes: Vec<u8>) {
        if self.out_queue.len() >= MAX_RELIABLE_QUEUE { return; }
        let seq = self.next_out_seq;
        self.next_out_seq = self.next_out_seq.wrapping_add(1);
        self.out_queue.push_back((seq, bytes));
    }

    pub fn reliable_backlog(&self) -> usize { self.out_queue.len() }

    /// Builds the header for the next outgoing packet.
    pub fn next_header(&mut self, now: f64) -> PacketHeader {
        self.seq = self.seq.wrapping_add(1);
        let idx = (self.seq as usize) % RTT_SAMPLES;
        // If the slot we are about to reuse was never acknowledged, that
        // packet is lost for good.
        if !self.acked[idx] {
            self.lost_packets += 1;
            self.loss = self.loss * 0.95 + 0.05;
        } else {
            self.loss *= 0.98;
        }
        self.sent_at[idx] = now;
        self.acked[idx] = false;
        self.last_send = now;
        PacketHeader {
            token: self.token,
            seq: self.seq,
            ack: self.remote_seq,
            ack_bits: self.ack_bits,
            reliable_ack: self.next_in_seq.wrapping_sub(1),
        }
    }

    /// Writes pending reliable messages into a packet body.
    pub fn write_reliable(&self, w: &mut super::bits::Writer) {
        let count_at = w.reserve_u8();
        let mut count = 0u8;
        let mut used = 0usize;
        for (seq, bytes) in self.out_queue.iter() {
            if bytes.len() + 6 + used > RELIABLE_BUDGET { break; }
            if count == 255 { break; }
            w.u32(*seq);
            w.u16(bytes.len() as u16);
            w.bytes(bytes);
            used += bytes.len() + 6;
            count += 1;
        }
        w.patch_u8(count_at, count);
    }

    /// Processes an incoming header: updates acks and round-trip time.
    pub fn on_header(&mut self, h: &PacketHeader, now: f64, size: usize) {
        self.last_recv = now;
        self.recv_bytes += size as u64;
        self.recv_packets += 1;

        // Track which of the peer's packets we have seen, newest first.
        let diff = h.seq.wrapping_sub(self.remote_seq);
        if diff != 0 && diff < 0x8000 {
            // Newer than anything seen: shift the history along.
            self.ack_bits = if diff >= 32 { 0 } else { (self.ack_bits << diff) | (1 << (diff - 1)) };
            self.remote_seq = h.seq;
        } else {
            let back = self.remote_seq.wrapping_sub(h.seq);
            if back >= 1 && back <= 32 {
                self.ack_bits |= 1 << (back - 1);
            }
        }

        // Retire our packets that the peer has now acknowledged.
        self.retire(h.ack, now);
        for bit in 0..32u16 {
            if h.ack_bits & (1 << bit) != 0 {
                self.retire(h.ack.wrapping_sub(bit + 1), now);
            }
        }

        // Drop reliable messages the peer has confirmed.
        if h.reliable_ack.wrapping_sub(self.peer_ack) < 0x8000_0000 {
            self.peer_ack = h.reliable_ack;
        }
        while let Some((seq, _)) = self.out_queue.front() {
            if seq.wrapping_sub(self.peer_ack) > 0x8000_0000 || *seq <= self.peer_ack {
                self.out_queue.pop_front();
            } else {
                break;
            }
        }
    }

    fn retire(&mut self, seq: u16, now: f64) {
        let idx = (seq as usize) % RTT_SAMPLES;
        if self.acked[idx] { return; }
        // Only credit the sample if it plausibly belongs to this sequence.
        let age = now - self.sent_at[idx];
        if age < 0.0 || age > 2.0 { return; }
        self.acked[idx] = true;
        let sample = (age * 1000.0) as f32;
        let delta = (sample - self.rtt_ms).abs();
        self.jitter_ms += (delta - self.jitter_ms) * 0.1;
        // Slow smoothing: a single delayed packet should not move the estimate
        // enough to change how far the server rewinds for this player.
        self.rtt_ms += (sample - self.rtt_ms) * 0.10;
    }

    /// Extracts reliable messages from a packet body, delivering only the ones
    /// that continue the ordered stream.
    pub fn read_reliable<'a>(&mut self, r: &mut super::bits::Reader<'a>, out: &mut Vec<&'a [u8]>) -> bool {
        let count = match r.u8() { Some(c) => c, None => return false };
        for _ in 0..count {
            let seq = match r.u32() { Some(s) => s, None => return false };
            let len = match r.u16() { Some(l) => l as usize, None => return false };
            if len > MAX_PACKET { return false; }
            let body = match r.bytes(len) { Some(b) => b, None => return false };
            if seq == self.next_in_seq {
                out.push(body);
                self.next_in_seq = self.next_in_seq.wrapping_add(1);
            }
        }
        true
    }

    pub fn note_sent(&mut self, bytes: usize) {
        self.sent_bytes += bytes as u64;
        self.sent_packets += 1;
    }

    pub fn timed_out(&self, now: f64, timeout: f64) -> bool {
        now - self.last_recv > timeout
    }

    pub fn ping_ms(&self) -> u16 { self.rtt_ms.clamp(0.0, 999.0) as u16 }
}

/// Verifies that a packet is plausibly ours before doing anything with it.
pub fn validate_packet(buf: &[u8], expect: PacketKind, token: u64) -> Option<PacketHeader> {
    let mut r = super::bits::Reader::new(buf);
    let (kind, header) = PacketHeader::read(&mut r)?;
    if kind != expect { return None; }
    if header.token != token { return None; }
    Some(header)
}
