//! Headless dedicated server.
//!
//! Runs the same server the game hosts internally, with no window, no
//! renderer and no audio. Useful for a permanent LAN box, and the reason the
//! networking has to be real rather than a local shortcut.

fn main() {
    println!("hardpoint-server: use `hardpoint --server` from the game binary.");
    println!("This stub exists so the dedicated build target stays wired up.");
}
