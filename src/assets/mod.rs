//! Asset generation.
//!
//! The game ships no binary assets. Textures, meshes, the UI font and every
//! sound are generated procedurally at startup from a few kilobytes of
//! parameters. That keeps the repository tiny, makes load times a function of
//! CPU speed rather than disk, and guarantees the whole game shares one
//! coherent visual and sonic language.

pub mod font;
pub mod materials;
pub mod meshgen;
pub mod texgen;
