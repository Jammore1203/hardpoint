//! Networking: protocol, reliability, server, client and discovery.
//!
//! The game is multiplayer-first. Even a single-player-feeling session against
//! bots runs a real authoritative server (on a background thread) and a real
//! client talking to it over UDP, because having one code path is the only way
//! the networked case stays honest.

pub mod bits;
pub mod channel;
pub mod client;
pub mod discovery;
pub mod protocol;
pub mod server;
pub mod socket;
