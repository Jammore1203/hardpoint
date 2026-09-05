//! Gameplay simulation, shared verbatim between the authoritative server and
//! the client's prediction. Nothing in here touches rendering, audio or IO.

pub mod events;
pub mod loadout;
pub mod movement;
pub mod player;
pub mod projectiles;
pub mod sim;
pub mod types;
pub mod weapons;
