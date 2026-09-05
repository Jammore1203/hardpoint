//! HARDPOINT: OPERATION IRONVEIL
//!
//! A multiplayer first-person shooter in the register of the early 2000s
//! console era: fast movement, tight maps, chunky geometry, and an
//! authoritative server behind every match including the ones you host
//! yourself.
//!
//! Everything lives in the library so the game and the dedicated server are
//! the same code: the server a player hosts and the server a box runs are the
//! same type, which is the only way the networking stays honest.

#[macro_use]
mod macros;

pub mod app;
pub mod assets;
pub mod audio;
pub mod bots;
pub mod core;
pub mod devtools;
pub mod game;
pub mod input;
pub mod maps;
pub mod math;
pub mod modes;
pub mod net;
pub mod progression;
pub mod render;
pub mod settings;
pub mod ui;

