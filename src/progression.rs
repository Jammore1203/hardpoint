//! Lightweight progression: rank, unlocks, challenges and career statistics.
//!
//! The point is the small pull of seeing a number go up and a weapon become
//! available, not a second economy. There is nothing to buy, nothing to grind
//! past, and every weapon is reachable inside a few evenings.

use crate::core::kv::{data_dir, Kv};
use crate::game::weapons::{WeaponId, ALL_WEAPONS, WEAPON_COUNT};
use std::path::PathBuf;

pub const MAX_LEVEL: u8 = 40;

/// Experience needed to go from `level` to the next one.
pub fn xp_for_level(level: u8) -> u32 {
    // A gentle curve: early ranks come quickly, later ones take a few matches.
    600 + level as u32 * 260
}

/// Total experience needed to reach a level from scratch.
pub fn xp_total_for(level: u8) -> u32 {
    (1..level).map(xp_for_level).sum()
}

/// Rank names, indexed by level band.
pub fn rank_name(level: u8) -> &'static str {
    match level {
        0..=2 => "RECRUIT",
        3..=5 => "PRIVATE",
        6..=8 => "LANCE CORPORAL",
        9..=12 => "CORPORAL",
        13..=16 => "SERGEANT",
        17..=20 => "STAFF SERGEANT",
        21..=24 => "WARRANT OFFICER",
        25..=28 => "LIEUTENANT",
        29..=32 => "CAPTAIN",
        33..=36 => "MAJOR",
        37..=39 => "COLONEL",
        _ => "COMMANDER",
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CareerStats {
    pub matches: u32,
    pub wins: u32,
    pub losses: u32,
    pub kills: u32,
    pub deaths: u32,
    pub assists: u32,
    pub headshots: u32,
    pub captures: u32,
    pub plants: u32,
    pub defuses: u32,
    pub best_streak: u16,
    pub time_played: u32,
    pub shots_fired: u32,
    pub shots_hit: u32,
    pub distance: u32,
}

impl CareerStats {
    pub fn kd(&self) -> f32 {
        if self.deaths == 0 { self.kills as f32 } else { self.kills as f32 / self.deaths as f32 }
    }
    pub fn accuracy(&self) -> f32 {
        if self.shots_fired == 0 { 0.0 } else { self.shots_hit as f32 / self.shots_fired as f32 }
    }
    pub fn win_rate(&self) -> f32 {
        if self.matches == 0 { 0.0 } else { self.wins as f32 / self.matches as f32 }
    }
}

/// What a challenge is counting.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ChallengeKind {
    Kills,
    Headshots,
    LongShots,
    MeleeKills,
    GrenadeKills,
    Captures,
    Plants,
    Defuses,
    Wins,
    Streak,
    WeaponKills(WeaponId),
}

#[derive(Clone, Debug)]
pub struct Challenge {
    pub name: &'static str,
    pub description: &'static str,
    pub kind: ChallengeKind,
    pub target: u32,
    pub reward: u32,
}

/// The challenge list. Short, achievable, and each one nudges the player to
/// try something they might not have.
pub fn challenges() -> Vec<Challenge> {
    use ChallengeKind::*;
    vec![
        Challenge { name: "BLOODED", description: "Get 50 kills.", kind: Kills, target: 50, reward: 500 },
        Challenge { name: "MARKSMAN", description: "Get 25 headshots.", kind: Headshots, target: 25, reward: 900 },
        Challenge { name: "REACH OUT", description: "Kill from over 60 metres 10 times.", kind: LongShots, target: 10, reward: 800 },
        Challenge { name: "UP CLOSE", description: "Get 10 melee kills.", kind: MeleeKills, target: 10, reward: 700 },
        Challenge { name: "FRAGMENTED", description: "Get 15 kills with equipment.", kind: GrenadeKills, target: 15, reward: 700 },
        Challenge { name: "GROUND TAKEN", description: "Capture 25 objectives.", kind: Captures, target: 25, reward: 900 },
        Challenge { name: "DEMOLITION", description: "Plant the charge 10 times.", kind: Plants, target: 10, reward: 800 },
        Challenge { name: "COOL HANDS", description: "Defuse the charge 10 times.", kind: Defuses, target: 10, reward: 900 },
        Challenge { name: "ON A ROLL", description: "Reach a ten-kill streak.", kind: Streak, target: 10, reward: 1200 },
        Challenge { name: "VETERAN", description: "Win 20 matches.", kind: Wins, target: 20, reward: 1500 },
        Challenge { name: "RIFLEMAN", description: "Get 100 kills with the VECTRA 5.", kind: WeaponKills(WeaponId::Vectra5), target: 100, reward: 1000 },
        Challenge { name: "CLOSE QUARTERS", description: "Get 100 kills with the VIPER.", kind: WeaponKills(WeaponId::Viper), target: 100, reward: 1000 },
        Challenge { name: "OVERWATCH", description: "Get 50 kills with the LONGBOW .338.", kind: WeaponKills(WeaponId::Longbow), target: 50, reward: 1200 },
    ]
}

#[derive(Clone)]
pub struct Progression {
    pub xp: u32,
    pub level: u8,
    pub career: CareerStats,
    pub weapon_kills: [u32; WEAPON_COUNT],
    pub weapon_shots: [u32; WEAPON_COUNT],
    pub weapon_hits: [u32; WEAPON_COUNT],
    /// Progress toward each challenge, parallel to `challenges()`.
    pub challenge_progress: Vec<u32>,
    pub challenge_done: Vec<bool>,
    /// Counters that only challenges use.
    pub long_shots: u32,
    pub melee_kills: u32,
    pub grenade_kills: u32,
    path: PathBuf,
    dirty: bool,
}

impl Default for Progression {
    fn default() -> Self {
        let n = challenges().len();
        Progression {
            xp: 0,
            level: 1,
            career: CareerStats::default(),
            weapon_kills: [0; WEAPON_COUNT],
            weapon_shots: [0; WEAPON_COUNT],
            weapon_hits: [0; WEAPON_COUNT],
            challenge_progress: vec![0; n],
            challenge_done: vec![false; n],
            long_shots: 0,
            melee_kills: 0,
            grenade_kills: 0,
            path: data_dir().join("profile.cfg"),
            dirty: false,
        }
    }
}

impl Progression {
    pub fn load() -> Progression {
        let mut p = Progression::default();
        let kv = Kv::load(&p.path);
        p.xp = kv.u32_or("xp", 0);
        p.level = (kv.u32_or("level", 1) as u8).clamp(1, MAX_LEVEL);
        p.career.matches = kv.u32_or("career.matches", 0);
        p.career.wins = kv.u32_or("career.wins", 0);
        p.career.losses = kv.u32_or("career.losses", 0);
        p.career.kills = kv.u32_or("career.kills", 0);
        p.career.deaths = kv.u32_or("career.deaths", 0);
        p.career.assists = kv.u32_or("career.assists", 0);
        p.career.headshots = kv.u32_or("career.headshots", 0);
        p.career.captures = kv.u32_or("career.captures", 0);
        p.career.plants = kv.u32_or("career.plants", 0);
        p.career.defuses = kv.u32_or("career.defuses", 0);
        p.career.best_streak = kv.u32_or("career.best_streak", 0) as u16;
        p.career.time_played = kv.u32_or("career.time_played", 0);
        p.career.shots_fired = kv.u32_or("career.shots_fired", 0);
        p.career.shots_hit = kv.u32_or("career.shots_hit", 0);
        p.career.distance = kv.u32_or("career.distance", 0);
        p.long_shots = kv.u32_or("stat.long_shots", 0);
        p.melee_kills = kv.u32_or("stat.melee_kills", 0);
        p.grenade_kills = kv.u32_or("stat.grenade_kills", 0);

        for (i, w) in ALL_WEAPONS.iter().enumerate() {
            p.weapon_kills[i] = kv.u32_or(&format!("weapon.{}.kills", *w as u8), 0);
            p.weapon_shots[i] = kv.u32_or(&format!("weapon.{}.shots", *w as u8), 0);
            p.weapon_hits[i] = kv.u32_or(&format!("weapon.{}.hits", *w as u8), 0);
        }
        for i in 0..p.challenge_progress.len() {
            p.challenge_progress[i] = kv.u32_or(&format!("challenge.{}.progress", i), 0);
            p.challenge_done[i] = kv.bool_or(&format!("challenge.{}.done", i), false);
        }
        p.recompute_level();
        p
    }

    pub fn save(&self) -> std::io::Result<()> {
        let mut kv = Kv::new();
        kv.set_i32("xp", self.xp as i32);
        kv.set_i32("level", self.level as i32);
        kv.set_i32("career.matches", self.career.matches as i32);
        kv.set_i32("career.wins", self.career.wins as i32);
        kv.set_i32("career.losses", self.career.losses as i32);
        kv.set_i32("career.kills", self.career.kills as i32);
        kv.set_i32("career.deaths", self.career.deaths as i32);
        kv.set_i32("career.assists", self.career.assists as i32);
        kv.set_i32("career.headshots", self.career.headshots as i32);
        kv.set_i32("career.captures", self.career.captures as i32);
        kv.set_i32("career.plants", self.career.plants as i32);
        kv.set_i32("career.defuses", self.career.defuses as i32);
        kv.set_i32("career.best_streak", self.career.best_streak as i32);
        kv.set_i32("career.time_played", self.career.time_played as i32);
        kv.set_i32("career.shots_fired", self.career.shots_fired as i32);
        kv.set_i32("career.shots_hit", self.career.shots_hit as i32);
        kv.set_i32("career.distance", self.career.distance as i32);
        kv.set_i32("stat.long_shots", self.long_shots as i32);
        kv.set_i32("stat.melee_kills", self.melee_kills as i32);
        kv.set_i32("stat.grenade_kills", self.grenade_kills as i32);
        for (i, w) in ALL_WEAPONS.iter().enumerate() {
            if self.weapon_kills[i] > 0 { kv.set_i32(&format!("weapon.{}.kills", *w as u8), self.weapon_kills[i] as i32); }
            if self.weapon_shots[i] > 0 { kv.set_i32(&format!("weapon.{}.shots", *w as u8), self.weapon_shots[i] as i32); }
            if self.weapon_hits[i] > 0 { kv.set_i32(&format!("weapon.{}.hits", *w as u8), self.weapon_hits[i] as i32); }
        }
        for i in 0..self.challenge_progress.len() {
            kv.set_i32(&format!("challenge.{}.progress", i), self.challenge_progress[i] as i32);
            kv.set_bool(&format!("challenge.{}.done", i), self.challenge_done[i]);
        }
        kv.save(&self.path)
    }

    pub fn save_if_dirty(&mut self) {
        if !self.dirty { return; }
        self.dirty = false;
        let _ = self.save();
    }

    fn recompute_level(&mut self) {
        let mut level = 1u8;
        let mut spent = 0u32;
        while level < MAX_LEVEL && self.xp >= spent + xp_for_level(level) {
            spent += xp_for_level(level);
            level += 1;
        }
        self.level = level;
    }

    /// Experience into the current level, and what the level requires.
    pub fn level_progress(&self) -> (u32, u32) {
        if self.level >= MAX_LEVEL { return (1, 1); }
        let spent = xp_total_for(self.level);
        (self.xp.saturating_sub(spent), xp_for_level(self.level))
    }

    /// Awards experience and returns true if the player ranked up.
    pub fn award(&mut self, amount: u32) -> bool {
        let before = self.level;
        self.xp = self.xp.saturating_add(amount);
        self.recompute_level();
        self.dirty = true;
        self.level > before
    }

    pub fn is_unlocked(&self, weapon: WeaponId) -> bool {
        weapon.def().unlock_level <= self.level
    }

    /// Weapons that became available at the current level.
    pub fn unlocked_at(&self, level: u8) -> Vec<WeaponId> {
        ALL_WEAPONS.iter().copied().filter(|w| w.def().unlock_level == level).collect()
    }

    // ---------------------------------------------------------- recording

    pub fn record_shot(&mut self, weapon: WeaponId, shots: u32) {
        self.weapon_shots[weapon.index()] += shots;
        self.career.shots_fired += shots;
        self.dirty = true;
    }

    pub fn record_hit(&mut self, weapon: WeaponId) {
        self.weapon_hits[weapon.index()] += 1;
        self.career.shots_hit += 1;
        self.dirty = true;
    }

    pub fn record_kill(&mut self, weapon: WeaponId, headshot: bool, distance: f32, cause_melee: bool, cause_explosive: bool) {
        self.weapon_kills[weapon.index()] += 1;
        self.career.kills += 1;
        if headshot { self.career.headshots += 1; }
        if distance > 60.0 { self.long_shots += 1; }
        if cause_melee { self.melee_kills += 1; }
        if cause_explosive { self.grenade_kills += 1; }
        self.dirty = true;
    }

    pub fn record_death(&mut self) {
        self.career.deaths += 1;
        self.dirty = true;
    }

    /// Folds a finished match into the career record and awards experience.
    /// Returns the experience gained and any newly completed challenges.
    pub fn finish_match(&mut self, summary: &MatchSummary) -> (u32, Vec<usize>) {
        self.career.matches += 1;
        if summary.won { self.career.wins += 1; } else { self.career.losses += 1; }
        self.career.assists += summary.assists;
        self.career.captures += summary.captures;
        self.career.plants += summary.plants;
        self.career.defuses += summary.defuses;
        self.career.best_streak = self.career.best_streak.max(summary.best_streak);
        self.career.time_played += summary.duration_secs;
        self.career.distance += summary.distance as u32;

        // Experience is mostly performance, with a solid participation floor
        // so a bad match is never a wasted one.
        let mut xp = 250;
        xp += summary.kills * 50;
        xp += summary.assists * 20;
        xp += summary.captures * 60;
        xp += (summary.plants + summary.defuses) * 80;
        xp += summary.score.max(0) as u32 / 4;
        if summary.won { xp += 400; }
        if summary.mvp { xp += 300; }

        let completed = self.update_challenges(summary);
        for &i in &completed {
            xp += challenges()[i].reward;
        }
        self.award(xp);
        (xp, completed)
    }

    fn update_challenges(&mut self, summary: &MatchSummary) -> Vec<usize> {
        use ChallengeKind::*;
        let list = challenges();
        let mut completed = Vec::new();
        for (i, c) in list.iter().enumerate() {
            if self.challenge_done[i] { continue; }
            let value = match c.kind {
                Kills => self.career.kills,
                Headshots => self.career.headshots,
                LongShots => self.long_shots,
                MeleeKills => self.melee_kills,
                GrenadeKills => self.grenade_kills,
                Captures => self.career.captures,
                Plants => self.career.plants,
                Defuses => self.career.defuses,
                Wins => self.career.wins,
                Streak => self.career.best_streak as u32,
                WeaponKills(w) => self.weapon_kills[w.index()],
            };
            self.challenge_progress[i] = value.min(c.target);
            if value >= c.target {
                self.challenge_done[i] = true;
                completed.push(i);
            }
        }
        let _ = summary;
        self.dirty = true;
        completed
    }
}

/// What a finished match contributed.
#[derive(Clone, Copy, Debug, Default)]
pub struct MatchSummary {
    pub won: bool,
    pub mvp: bool,
    pub kills: u32,
    pub deaths: u32,
    pub assists: u32,
    pub score: i32,
    pub captures: u32,
    pub plants: u32,
    pub defuses: u32,
    pub best_streak: u16,
    pub duration_secs: u32,
    pub distance: f32,
}
