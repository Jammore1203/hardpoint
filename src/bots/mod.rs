//! Bot management.
//!
//! The director owns every bot, keeps their work spread across ticks, and is
//! the only thing the server needs to know about the AI.

pub mod brain;
pub mod names;

use crate::core::Rng;
use crate::game::sim::World;
use crate::game::types::{Team, MAX_PLAYERS};
use crate::modes::{MatchState, Mode};
use brain::Bot;

pub struct Director {
    bots: Vec<Option<Bot>>,
    difficulty: u8,
    rng: Rng,
    /// Bots are updated in a rotating window so a full server never spends a
    /// whole tick's budget on AI.
    cursor: usize,
}

impl Director {
    pub fn new(difficulty: u8) -> Director {
        Director {
            bots: (0..MAX_PLAYERS).map(|_| None).collect(),
            difficulty: difficulty.min(3),
            rng: Rng::from_clock(),
            cursor: 0,
        }
    }

    pub fn difficulty(&self) -> u8 { self.difficulty }
    pub fn set_difficulty(&mut self, d: u8) { self.difficulty = d.min(3); }

    pub fn add_bot(&mut self, world: &mut World, slot: u8, team: Team) {
        let taken: Vec<String> = world.players.iter()
            .filter(|p| p.in_use)
            .map(|p| p.name.clone())
            .collect();
        let name = names::pick_unique(&taken, &mut self.rng);
        let seed = self.rng.next_u32();
        let mut bot = Bot::new(slot, self.difficulty, seed);
        let level = 5 + (seed % 40) as u8;
        let loadout = bot.choose_loadout(level);

        {
            let p = &mut world.players[slot as usize];
            *p = crate::game::player::Player::new(slot);
            p.in_use = true;
            p.is_bot = true;
            p.ready = true;
            p.name = name;
            p.team = team;
            p.level = level;
            p.loadout = loadout;
            p.ping_ms = 0;
        }
        self.bots[slot as usize] = Some(bot);
        world.respawn_player(slot, false);
    }

    pub fn remove_bot(&mut self, slot: u8) {
        self.bots[slot as usize] = None;
    }

    pub fn reset(&mut self, world: &World) {
        for (i, b) in self.bots.iter_mut().enumerate() {
            if let Some(b) = b {
                let yaw = world.players[i].mv.yaw;
                b.on_spawn(yaw);
            }
        }
    }

    pub fn count(&self) -> usize { self.bots.iter().flatten().count() }

    /// Produces and executes a command for every bot.
    pub fn update(&mut self, world: &mut World, state: &MatchState, mode: &dyn Mode, dt: f32) {
        let now = world.time;
        // Reset aim when a bot respawns, so it does not snap round instantly.
        for i in 0..self.bots.len() {
            let Some(bot) = self.bots[i].as_mut() else { continue };
            let p = &world.players[i];
            if p.alive && !p.mv.grounded && p.mv.vel.y == 0.0 { /* just spawned */ }
            if !p.in_use { continue; }
            if p.alive && bot.target == crate::game::types::NO_PLAYER && bot.path.is_empty()
                && (bot.aim_yaw - p.mv.yaw).abs() > 3.0 {
                bot.aim_yaw = p.mv.yaw;
            }
        }

        for i in 0..self.bots.len() {
            if self.bots[i].is_none() { continue; }
            if !world.players[i].in_use { continue; }
            // Take the bot out of the list while it reads the world, so it can
            // borrow the world immutably and then run its command.
            let mut bot = self.bots[i].take().unwrap();
            let cmd = bot.think(world, mode, dt, now);
            self.bots[i] = Some(bot);
            world.run_command(i as u8, &cmd);
        }
        let _ = state;
        self.cursor = self.cursor.wrapping_add(1);
    }
}
