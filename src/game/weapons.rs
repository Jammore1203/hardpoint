//! Weapon definitions and the firing state machine.
//!
//! Every weapon is a row in one static table. Nothing here allocates and
//! nothing is looked up by string, so the same table drives the server's
//! authoritative firing, the client's prediction, the bots' target selection
//! and the loadout UI.
//!
//! The stats are deliberately spread out. A weapon that differs only in damage
//! is not a weapon, it is a reskin, so each one is given a distinct
//! combination of pace, recoil shape, reload cost and effective range.

use crate::core::Rng;

/// Stable weapon identifier, sent over the wire as a `u8`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum WeaponId {
    // -- Assault rifles ---------------------------------------------------
    Kr44 = 0,
    Vectra5,
    Lancer,
    Tempest,
    Kestrel,
    // -- Submachine guns --------------------------------------------------
    Wasp9,
    Viper,
    Shrike,
    Scarab,
    HornetC,
    // -- Shotguns ---------------------------------------------------------
    Breacher12,
    Longshore,
    TrenchM4,
    // -- Sniper rifles ----------------------------------------------------
    Longbow,
    Marksman,
    Spectre50,
    // -- Light machine guns -----------------------------------------------
    Hammerhead,
    Bulwark60,
    Drumfire,
    // -- Sidearms ---------------------------------------------------------
    SidearmP9,
    Anvil44,
    BurstP3,
    Holdout,
    // -- Melee ------------------------------------------------------------
    CombatKnife,
    TrenchSpade,
}

pub const WEAPON_COUNT: usize = 25;

pub const ALL_WEAPONS: [WeaponId; WEAPON_COUNT] = {
    use WeaponId::*;
    [
        Kr44, Vectra5, Lancer, Tempest, Kestrel,
        Wasp9, Viper, Shrike, Scarab, HornetC,
        Breacher12, Longshore, TrenchM4,
        Longbow, Marksman, Spectre50,
        Hammerhead, Bulwark60, Drumfire,
        SidearmP9, Anvil44, BurstP3, Holdout,
        CombatKnife, TrenchSpade,
    ]
};

impl WeaponId {
    #[inline]
    pub fn index(self) -> usize { self as usize }

    pub fn from_u8(v: u8) -> WeaponId {
        if (v as usize) < WEAPON_COUNT { ALL_WEAPONS[v as usize] } else { WeaponId::SidearmP9 }
    }

    #[inline]
    pub fn def(self) -> &'static WeaponDef { &WEAPONS[self as usize] }

    #[inline]
    pub fn name(self) -> &'static str { self.def().name }

    #[inline]
    pub fn class(self) -> WeaponClass { self.def().class }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum WeaponClass {
    Assault,
    Smg,
    Shotgun,
    Sniper,
    Lmg,
    Pistol,
    Melee,
}

impl WeaponClass {
    pub fn label(self) -> &'static str {
        match self {
            WeaponClass::Assault => "ASSAULT RIFLE",
            WeaponClass::Smg => "SUBMACHINE GUN",
            WeaponClass::Shotgun => "SHOTGUN",
            WeaponClass::Sniper => "SNIPER RIFLE",
            WeaponClass::Lmg => "LIGHT MACHINE GUN",
            WeaponClass::Pistol => "SIDEARM",
            WeaponClass::Melee => "MELEE",
        }
    }
    pub fn short(self) -> &'static str {
        match self {
            WeaponClass::Assault => "AR", WeaponClass::Smg => "SMG",
            WeaponClass::Shotgun => "SG", WeaponClass::Sniper => "SR",
            WeaponClass::Lmg => "LMG", WeaponClass::Pistol => "PSTL",
            WeaponClass::Melee => "MEL",
        }
    }
    /// Can this weapon occupy a primary slot?
    pub fn is_primary(self) -> bool {
        !matches!(self, WeaponClass::Pistol | WeaponClass::Melee)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FireMode {
    Auto,
    Semi,
    /// Fires N rounds per trigger pull at an elevated rate.
    Burst(u8),
    /// Manually cycled between shots (pump / bolt): adds a fixed delay that
    /// cannot be shortened by holding the trigger.
    Pump,
    Bolt,
    Melee,
}

impl FireMode {
    pub fn label(self) -> &'static str {
        match self {
            FireMode::Auto => "AUTO",
            FireMode::Semi => "SEMI",
            FireMode::Burst(n) => if n == 2 { "2-BURST" } else { "3-BURST" },
            FireMode::Pump => "PUMP",
            FireMode::Bolt => "BOLT",
            FireMode::Melee => "MELEE",
        }
    }
    #[inline]
    pub fn is_automatic(self) -> bool { matches!(self, FireMode::Auto) }
}

/// Which viewmodel and world model to draw. Several weapons share a silhouette
/// family; the generator varies proportions from the weapon's own stats.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ModelShape {
    Rifle,
    Bullpup,
    Smg,
    Shotgun,
    SniperLong,
    Lmg,
    PistolSmall,
    PistolHeavy,
    Knife,
    Spade,
}

impl ModelShape {
    /// Whether the support hand rides the handguard. Pistols and the knife are
    /// held one-handed; everything else gets a second glove on the model.
    pub fn two_handed(self) -> bool {
        !matches!(self, ModelShape::PistolSmall | ModelShape::PistolHeavy | ModelShape::Knife)
    }
}

/// The full stat block for one weapon.
#[derive(Clone, Debug)]
pub struct WeaponDef {
    pub id: WeaponId,
    pub name: &'static str,
    pub class: WeaponClass,
    pub blurb: &'static str,

    // -- Damage -----------------------------------------------------------
    /// Damage at or inside `range_near`.
    pub damage: f32,
    /// Damage at or beyond `range_far`; linear in between.
    pub damage_far: f32,
    pub range_near: f32,
    pub range_far: f32,
    pub headshot_mult: f32,
    pub limb_mult: f32,
    /// Pellets per shot; one for everything but shotguns.
    pub pellets: u8,
    /// How much of a wall this round will punch through, in "penetration
    /// units". Thin sheet metal is 1, a brick wall is 4.
    pub penetration: f32,

    // -- Cadence ----------------------------------------------------------
    pub rpm: f32,
    pub fire_mode: FireMode,
    /// Extra delay after a burst finishes, or between pump/bolt cycles.
    pub cycle_time: f32,

    // -- Ammunition -------------------------------------------------------
    pub mag: u16,
    pub reserve: u16,
    /// Tactical reload, i.e. a round still in the chamber.
    pub reload_time: f32,
    /// Reload from empty.
    pub reload_empty: f32,
    /// Shotguns and some LMGs load one shell at a time.
    pub reload_per_round: bool,

    // -- Accuracy ---------------------------------------------------------
    /// Cone half-angle in radians while standing still and hip-firing.
    pub spread_base: f32,
    /// Added at full movement speed.
    pub spread_move: f32,
    /// Added while airborne.
    pub spread_jump: f32,
    /// Multiplier applied while fully aimed.
    pub spread_ads: f32,
    /// Growth per shot, and the ceiling it grows to.
    pub spread_per_shot: f32,
    pub spread_max: f32,
    /// Recovery rate per second toward the base cone.
    pub spread_recover: f32,

    // -- Recoil -----------------------------------------------------------
    /// Vertical kick per shot, radians.
    pub recoil_up: f32,
    /// Horizontal kick magnitude, randomised per shot.
    pub recoil_side: f32,
    /// How quickly the view returns, as a fraction per second.
    pub recoil_recover: f32,
    /// Fraction of recoil that is permanent (not recovered) - gives each gun a
    /// distinct climb pattern rather than a pure spring.
    pub recoil_keep: f32,

    // -- Handling ---------------------------------------------------------
    pub ads_time: f32,
    /// Vertical FOV multiplier while aimed. Snipers go lowest.
    pub ads_fov_scale: f32,
    /// Movement speed multiplier while holding this weapon.
    pub move_scale: f32,
    /// Additional multiplier while aimed.
    pub ads_move_scale: f32,
    pub swap_out: f32,
    pub swap_in: f32,
    /// Sprint-to-fire delay.
    pub sprint_out: f32,

    // -- Presentation -----------------------------------------------------
    pub shape: ModelShape,
    /// Drives the procedural gunshot: (body pitch, brightness, body length).
    pub sound_pitch: f32,
    pub sound_bright: f32,
    pub sound_body: f32,
    /// Muzzle flash scale and light radius.
    pub flash_scale: f32,
    pub ejects_shells: bool,
    /// Scoped weapons draw a scope overlay instead of a viewmodel while aimed.
    pub scoped: bool,

    // -- Progression ------------------------------------------------------
    /// Player level at which the weapon unlocks. Zero means available at once.
    pub unlock_level: u8,
}

impl WeaponDef {
    /// Seconds between shots at the weapon's rate of fire.
    #[inline]
    pub fn shot_interval(&self) -> f32 {
        if self.rpm <= 0.0 { 1.0 } else { 60.0 / self.rpm }
    }

    /// Damage after range falloff.
    #[inline]
    pub fn damage_at(&self, distance: f32) -> f32 {
        if distance <= self.range_near { return self.damage; }
        if distance >= self.range_far { return self.damage_far; }
        let t = (distance - self.range_near) / (self.range_far - self.range_near).max(0.001);
        self.damage + (self.damage_far - self.damage) * t
    }

    /// Rounds needed to kill an unarmoured target at a given range.
    pub fn shots_to_kill(&self, distance: f32, health: f32) -> u32 {
        let per_shot = self.damage_at(distance) * self.pellets as f32;
        if per_shot <= 0.0 { return 99; }
        (health / per_shot).ceil() as u32
    }

    /// Time to kill in seconds, used to sort weapons in the UI and to let bots
    /// judge which of two guns to pick up.
    pub fn time_to_kill(&self, distance: f32) -> f32 {
        let shots = self.shots_to_kill(distance, 100.0);
        if shots <= 1 { return 0.0; }
        (shots - 1) as f32 * self.shot_interval()
    }

    #[inline]
    pub fn is_melee(&self) -> bool { self.class == WeaponClass::Melee }
}

/// Builds the static weapon table. Written as a function so the defaults can
/// be expressed once and each weapon only states what makes it different.
const fn base(id: WeaponId, name: &'static str, class: WeaponClass, blurb: &'static str, shape: ModelShape) -> WeaponDef {
    WeaponDef {
        id, name, class, blurb, shape,
        damage: 30.0, damage_far: 20.0, range_near: 20.0, range_far: 50.0,
        headshot_mult: 1.5, limb_mult: 0.9, pellets: 1, penetration: 1.0,
        rpm: 600.0, fire_mode: FireMode::Auto, cycle_time: 0.0,
        mag: 30, reserve: 180, reload_time: 2.1, reload_empty: 2.8, reload_per_round: false,
        spread_base: 0.020, spread_move: 0.030, spread_jump: 0.070, spread_ads: 0.10,
        spread_per_shot: 0.004, spread_max: 0.070, spread_recover: 0.10,
        recoil_up: 0.0075, recoil_side: 0.0032, recoil_recover: 7.0, recoil_keep: 0.18,
        ads_time: 0.24, ads_fov_scale: 0.78, move_scale: 0.95, ads_move_scale: 0.55,
        swap_out: 0.22, swap_in: 0.40, sprint_out: 0.18,
        sound_pitch: 1.0, sound_bright: 0.5, sound_body: 0.5,
        flash_scale: 1.0, ejects_shells: true, scoped: false,
        unlock_level: 0,
    }
}

/// The weapon table. Order must match `WeaponId`.
pub static WEAPONS: [WeaponDef; WEAPON_COUNT] = build_table();

const fn build_table() -> [WeaponDef; WEAPON_COUNT] {
    use WeaponClass::*;
    use WeaponId as W;

    // ---------------------------------------------------------- assault
    let mut kr44 = base(W::Kr44, "KR-44", Assault, "Heavy-calibre rifle. Slow, loud, and lethal in three.", ModelShape::Rifle);
    kr44.damage = 36.0; kr44.damage_far = 25.0; kr44.range_near = 28.0; kr44.range_far = 62.0;
    kr44.rpm = 545.0; kr44.mag = 30; kr44.reserve = 180;
    kr44.recoil_up = 0.0106; kr44.recoil_side = 0.0044; kr44.recoil_keep = 0.30;
    kr44.spread_base = 0.021; kr44.penetration = 2.4; kr44.move_scale = 0.93;
    kr44.sound_pitch = 0.82; kr44.sound_body = 0.72; kr44.flash_scale = 1.2;

    let mut vectra = base(W::Vectra5, "VECTRA 5", Assault, "Balanced service rifle. Forgiving recoil, unremarkable punch.", ModelShape::Rifle);
    vectra.damage = 29.0; vectra.damage_far = 21.0; vectra.range_near = 26.0; vectra.range_far = 58.0;
    vectra.rpm = 700.0; vectra.recoil_up = 0.0068; vectra.recoil_side = 0.0026; vectra.recoil_keep = 0.14;
    vectra.spread_base = 0.017; vectra.ads_time = 0.22; vectra.penetration = 1.8;
    vectra.sound_pitch = 1.0; vectra.sound_body = 0.55;

    let mut lancer = base(W::Lancer, "SR-9 LANCER", Assault, "Marksman rifle. Semi-automatic, cruel at distance.", ModelShape::Rifle);
    lancer.damage = 46.0; lancer.damage_far = 38.0; lancer.range_near = 40.0; lancer.range_far = 90.0;
    lancer.rpm = 400.0; lancer.fire_mode = FireMode::Semi; lancer.mag = 20; lancer.reserve = 120;
    lancer.headshot_mult = 2.0; lancer.recoil_up = 0.0140; lancer.recoil_side = 0.0022;
    lancer.spread_base = 0.010; lancer.spread_ads = 0.03; lancer.ads_time = 0.30; lancer.ads_fov_scale = 0.62;
    lancer.penetration = 2.8; lancer.move_scale = 0.92; lancer.unlock_level = 4;
    lancer.sound_pitch = 0.88; lancer.sound_bright = 0.66; lancer.sound_body = 0.62;

    let mut tempest = base(W::Tempest, "TEMPEST", Assault, "Bullpup burst rifle. Devastating if all three land.", ModelShape::Bullpup);
    tempest.damage = 33.0; tempest.damage_far = 24.0; tempest.range_near = 24.0; tempest.range_far = 55.0;
    tempest.rpm = 900.0; tempest.fire_mode = FireMode::Burst(3); tempest.cycle_time = 0.26;
    tempest.mag = 30; tempest.recoil_up = 0.0090; tempest.recoil_side = 0.0018; tempest.recoil_keep = 0.42;
    tempest.spread_base = 0.015; tempest.ads_time = 0.21; tempest.unlock_level = 8;
    tempest.sound_pitch = 1.08; tempest.sound_body = 0.44;

    let mut kestrel = base(W::Kestrel, "KESTREL M1", Assault, "Fast, light and twitchy. Rewards a steady hand.", ModelShape::Bullpup);
    kestrel.damage = 26.0; kestrel.damage_far = 18.0; kestrel.range_near = 22.0; kestrel.range_far = 50.0;
    kestrel.rpm = 820.0; kestrel.mag = 35; kestrel.reserve = 210;
    kestrel.recoil_up = 0.0058; kestrel.recoil_side = 0.0042; kestrel.recoil_keep = 0.10;
    kestrel.spread_base = 0.019; kestrel.spread_per_shot = 0.0032; kestrel.ads_time = 0.19;
    kestrel.move_scale = 0.97; kestrel.unlock_level = 12;
    kestrel.sound_pitch = 1.18; kestrel.sound_body = 0.38;

    // -------------------------------------------------------------- smg
    let mut wasp = base(W::Wasp9, "WASP-9", Smg, "Blistering rate of fire, no reach whatsoever.", ModelShape::Smg);
    wasp.damage = 22.0; wasp.damage_far = 12.0; wasp.range_near = 12.0; wasp.range_far = 30.0;
    wasp.rpm = 1000.0; wasp.mag = 32; wasp.reserve = 224; wasp.reload_time = 1.7; wasp.reload_empty = 2.3;
    wasp.recoil_up = 0.0048; wasp.recoil_side = 0.0046; wasp.recoil_keep = 0.08;
    wasp.spread_base = 0.030; wasp.spread_move = 0.018; wasp.ads_time = 0.16;
    wasp.move_scale = 1.03; wasp.ads_move_scale = 0.72; wasp.penetration = 0.7;
    wasp.sound_pitch = 1.30; wasp.sound_body = 0.30; wasp.flash_scale = 0.8;

    let mut viper = base(W::Viper, "VIPER", Smg, "Controllable and quick to aim. The safe close-range pick.", ModelShape::Smg);
    viper.damage = 25.0; viper.damage_far = 15.0; viper.range_near = 15.0; viper.range_far = 34.0;
    viper.rpm = 850.0; viper.mag = 30; viper.reserve = 210; viper.reload_time = 1.8; viper.reload_empty = 2.4;
    viper.recoil_up = 0.0052; viper.recoil_side = 0.0030; viper.recoil_keep = 0.12;
    viper.spread_base = 0.025; viper.spread_move = 0.016; viper.ads_time = 0.17;
    viper.move_scale = 1.02; viper.ads_move_scale = 0.70; viper.penetration = 0.8;
    viper.sound_pitch = 1.16; viper.sound_body = 0.34;

    let mut shrike = base(W::Shrike, "MK-11 SHRIKE", Smg, "Heavy subgun. Hits like a rifle inside a room.", ModelShape::Smg);
    shrike.damage = 31.0; shrike.damage_far = 17.0; shrike.range_near = 14.0; shrike.range_far = 32.0;
    shrike.rpm = 700.0; shrike.mag = 25; shrike.reserve = 175; shrike.reload_time = 2.0; shrike.reload_empty = 2.7;
    shrike.recoil_up = 0.0086; shrike.recoil_side = 0.0038; shrike.recoil_keep = 0.22;
    shrike.spread_base = 0.028; shrike.ads_time = 0.19; shrike.move_scale = 0.99;
    shrike.penetration = 1.3; shrike.unlock_level = 6;
    shrike.sound_pitch = 0.96; shrike.sound_body = 0.48;

    let mut scarab = base(W::Scarab, "SCARAB", Smg, "Compact machine pistol. Absurd close up, useless past a room.", ModelShape::Smg);
    scarab.damage = 19.0; scarab.damage_far = 9.0; scarab.range_near = 9.0; scarab.range_far = 24.0;
    scarab.rpm = 1150.0; scarab.mag = 40; scarab.reserve = 240; scarab.reload_time = 1.9; scarab.reload_empty = 2.5;
    scarab.recoil_up = 0.0040; scarab.recoil_side = 0.0058; scarab.recoil_keep = 0.06;
    scarab.spread_base = 0.038; scarab.spread_per_shot = 0.0030; scarab.spread_max = 0.090;
    scarab.ads_time = 0.15; scarab.move_scale = 1.05; scarab.ads_move_scale = 0.78;
    scarab.penetration = 0.5; scarab.unlock_level = 16;
    scarab.sound_pitch = 1.42; scarab.sound_body = 0.24; scarab.flash_scale = 0.7;

    let mut hornet = base(W::HornetC, "HORNET C", Smg, "Suppressed. Quiet, invisible on the feed, and weak.", ModelShape::Smg);
    hornet.damage = 20.0; hornet.damage_far = 13.0; hornet.range_near = 13.0; hornet.range_far = 30.0;
    hornet.rpm = 780.0; hornet.mag = 30; hornet.reserve = 210;
    hornet.recoil_up = 0.0044; hornet.recoil_side = 0.0026; hornet.recoil_keep = 0.10;
    hornet.spread_base = 0.024; hornet.ads_time = 0.18; hornet.move_scale = 1.01;
    hornet.penetration = 0.6; hornet.unlock_level = 20;
    hornet.sound_pitch = 0.70; hornet.sound_bright = 0.16; hornet.sound_body = 0.20;
    hornet.flash_scale = 0.35;

    // --------------------------------------------------------- shotgun
    let mut breacher = base(W::Breacher12, "BREACHER 12", Shotgun, "Pump gun. One shell, one room.", ModelShape::Shotgun);
    breacher.damage = 15.0; breacher.damage_far = 3.0; breacher.range_near = 8.0; breacher.range_far = 20.0;
    breacher.pellets = 9; breacher.rpm = 90.0; breacher.fire_mode = FireMode::Pump; breacher.cycle_time = 0.62;
    breacher.mag = 6; breacher.reserve = 42; breacher.reload_per_round = true;
    breacher.reload_time = 0.52; breacher.reload_empty = 0.52;
    breacher.spread_base = 0.052; breacher.spread_ads = 0.62; breacher.spread_per_shot = 0.0;
    breacher.recoil_up = 0.0300; breacher.recoil_side = 0.0060; breacher.recoil_keep = 0.10;
    breacher.ads_time = 0.24; breacher.headshot_mult = 1.25; breacher.penetration = 0.4;
    breacher.move_scale = 0.97;
    breacher.sound_pitch = 0.68; breacher.sound_body = 0.86; breacher.flash_scale = 1.6;

    let mut longshore = base(W::Longshore, "LONGSHORE", Shotgun, "Semi-automatic. Tighter, faster, less brutal per shell.", ModelShape::Shotgun);
    longshore.damage = 12.0; longshore.damage_far = 3.0; longshore.range_near = 10.0; longshore.range_far = 24.0;
    longshore.pellets = 8; longshore.rpm = 220.0; longshore.fire_mode = FireMode::Semi;
    longshore.mag = 7; longshore.reserve = 49; longshore.reload_per_round = true;
    longshore.reload_time = 0.46; longshore.reload_empty = 0.46;
    longshore.spread_base = 0.042; longshore.spread_ads = 0.60;
    longshore.recoil_up = 0.0210; longshore.recoil_side = 0.0050;
    longshore.ads_time = 0.26; longshore.headshot_mult = 1.2; longshore.penetration = 0.4;
    longshore.move_scale = 0.95; longshore.unlock_level = 10;
    longshore.sound_pitch = 0.76; longshore.sound_body = 0.78; longshore.flash_scale = 1.4;

    let mut trench = base(W::TrenchM4, "TRENCH M4", Shotgun, "Sawn-off double. Two shells, then you had better run.", ModelShape::Shotgun);
    trench.damage = 19.0; trench.damage_far = 2.0; trench.range_near = 5.0; trench.range_far = 15.0;
    trench.pellets = 10; trench.rpm = 340.0; trench.fire_mode = FireMode::Semi;
    trench.mag = 2; trench.reserve = 24; trench.reload_time = 1.9; trench.reload_empty = 1.9;
    trench.spread_base = 0.078; trench.spread_ads = 0.70;
    trench.recoil_up = 0.0340; trench.recoil_side = 0.0090;
    trench.ads_time = 0.20; trench.headshot_mult = 1.2; trench.penetration = 0.3;
    trench.move_scale = 1.04; trench.ads_move_scale = 0.80; trench.unlock_level = 22;
    trench.sound_pitch = 0.62; trench.sound_body = 0.94; trench.flash_scale = 1.8;

    // ---------------------------------------------------------- sniper
    let mut longbow = base(W::Longbow, "LONGBOW .338", Sniper, "Bolt action. One shot to the chest ends it.", ModelShape::SniperLong);
    longbow.damage = 105.0; longbow.damage_far = 90.0; longbow.range_near = 80.0; longbow.range_far = 160.0;
    longbow.rpm = 55.0; longbow.fire_mode = FireMode::Bolt; longbow.cycle_time = 1.05;
    longbow.mag = 5; longbow.reserve = 30; longbow.reload_time = 2.9; longbow.reload_empty = 3.4;
    longbow.headshot_mult = 1.6; longbow.limb_mult = 0.85;
    longbow.spread_base = 0.070; longbow.spread_ads = 0.004; longbow.spread_move = 0.06;
    longbow.recoil_up = 0.0500; longbow.recoil_side = 0.0060; longbow.recoil_keep = 0.0;
    longbow.ads_time = 0.42; longbow.ads_fov_scale = 0.24; longbow.scoped = true;
    longbow.move_scale = 0.86; longbow.ads_move_scale = 0.32; longbow.penetration = 4.0;
    longbow.sound_pitch = 0.58; longbow.sound_bright = 0.80; longbow.sound_body = 1.0;
    longbow.flash_scale = 1.5; longbow.unlock_level = 5;

    let mut marksman = base(W::Marksman, "DR-7 MARKSMAN", Sniper, "Semi-automatic. Two hits, but you get them quickly.", ModelShape::SniperLong);
    marksman.damage = 68.0; marksman.damage_far = 55.0; marksman.range_near = 60.0; marksman.range_far = 130.0;
    marksman.rpm = 200.0; marksman.fire_mode = FireMode::Semi;
    marksman.mag = 10; marksman.reserve = 60; marksman.reload_time = 2.5; marksman.reload_empty = 3.0;
    marksman.headshot_mult = 1.8;
    marksman.spread_base = 0.060; marksman.spread_ads = 0.010; marksman.spread_per_shot = 0.010;
    marksman.recoil_up = 0.0290; marksman.recoil_side = 0.0040; marksman.recoil_keep = 0.20;
    marksman.ads_time = 0.34; marksman.ads_fov_scale = 0.34; marksman.scoped = true;
    marksman.move_scale = 0.90; marksman.ads_move_scale = 0.40; marksman.penetration = 3.2;
    marksman.sound_pitch = 0.72; marksman.sound_bright = 0.70; marksman.sound_body = 0.82;
    marksman.unlock_level = 14;

    let mut spectre = base(W::Spectre50, "SPECTRE .50", Sniper, "Anti-materiel. Punches through what you are hiding behind.", ModelShape::SniperLong);
    spectre.damage = 130.0; spectre.damage_far = 120.0; spectre.range_near = 100.0; spectre.range_far = 200.0;
    spectre.rpm = 45.0; spectre.fire_mode = FireMode::Bolt; spectre.cycle_time = 1.35;
    spectre.mag = 4; spectre.reserve = 20; spectre.reload_time = 3.4; spectre.reload_empty = 3.9;
    spectre.headshot_mult = 1.4; spectre.limb_mult = 0.95;
    spectre.spread_base = 0.085; spectre.spread_ads = 0.003; spectre.spread_move = 0.08;
    spectre.recoil_up = 0.0700; spectre.recoil_side = 0.0080; spectre.recoil_keep = 0.0;
    spectre.ads_time = 0.52; spectre.ads_fov_scale = 0.20; spectre.scoped = true;
    spectre.move_scale = 0.80; spectre.ads_move_scale = 0.26; spectre.penetration = 6.0;
    spectre.sound_pitch = 0.46; spectre.sound_bright = 0.92; spectre.sound_body = 1.0;
    spectre.flash_scale = 2.0; spectre.unlock_level = 26;

    // ------------------------------------------------------------- lmg
    let mut hammerhead = base(W::Hammerhead, "HAMMERHEAD", Lmg, "Belt-fed suppression. Sluggish, but it never stops.", ModelShape::Lmg);
    hammerhead.damage = 32.0; hammerhead.damage_far = 24.0; hammerhead.range_near = 34.0; hammerhead.range_far = 75.0;
    hammerhead.rpm = 620.0; hammerhead.mag = 100; hammerhead.reserve = 300;
    hammerhead.reload_time = 5.6; hammerhead.reload_empty = 6.4;
    hammerhead.recoil_up = 0.0072; hammerhead.recoil_side = 0.0050; hammerhead.recoil_keep = 0.24;
    hammerhead.spread_base = 0.034; hammerhead.spread_move = 0.040; hammerhead.spread_ads = 0.14;
    hammerhead.ads_time = 0.42; hammerhead.move_scale = 0.82; hammerhead.ads_move_scale = 0.34;
    hammerhead.penetration = 3.0; hammerhead.sprint_out = 0.34; hammerhead.unlock_level = 7;
    hammerhead.sound_pitch = 0.80; hammerhead.sound_body = 0.74; hammerhead.flash_scale = 1.35;

    let mut bulwark = base(W::Bulwark60, "BULWARK 60", Lmg, "Lighter support gun. Handles almost like a rifle.", ModelShape::Lmg);
    bulwark.damage = 28.0; bulwark.damage_far = 21.0; bulwark.range_near = 30.0; bulwark.range_far = 68.0;
    bulwark.rpm = 750.0; bulwark.mag = 60; bulwark.reserve = 240;
    bulwark.reload_time = 4.4; bulwark.reload_empty = 5.1;
    bulwark.recoil_up = 0.0060; bulwark.recoil_side = 0.0040; bulwark.recoil_keep = 0.18;
    bulwark.spread_base = 0.028; bulwark.spread_move = 0.034; bulwark.spread_ads = 0.12;
    bulwark.ads_time = 0.34; bulwark.move_scale = 0.88; bulwark.ads_move_scale = 0.42;
    bulwark.penetration = 2.4; bulwark.sprint_out = 0.28; bulwark.unlock_level = 18;
    bulwark.sound_pitch = 0.94; bulwark.sound_body = 0.62;

    let mut drumfire = base(W::Drumfire, "DRUMFIRE", Lmg, "Drum-fed and wild. Volume of fire over any kind of grace.", ModelShape::Lmg);
    drumfire.damage = 26.0; drumfire.damage_far = 16.0; drumfire.range_near = 22.0; drumfire.range_far = 55.0;
    drumfire.rpm = 950.0; drumfire.mag = 75; drumfire.reserve = 225;
    drumfire.reload_time = 5.0; drumfire.reload_empty = 5.8;
    drumfire.recoil_up = 0.0066; drumfire.recoil_side = 0.0072; drumfire.recoil_keep = 0.14;
    drumfire.spread_base = 0.042; drumfire.spread_move = 0.038; drumfire.spread_per_shot = 0.0030;
    drumfire.spread_max = 0.100; drumfire.spread_ads = 0.20;
    drumfire.ads_time = 0.36; drumfire.move_scale = 0.86; drumfire.ads_move_scale = 0.40;
    drumfire.penetration = 1.6; drumfire.sprint_out = 0.30; drumfire.unlock_level = 24;
    drumfire.sound_pitch = 1.05; drumfire.sound_body = 0.58;

    // --------------------------------------------------------- sidearm
    let mut p9 = base(W::SidearmP9, "SIDEARM P9", Pistol, "Standard issue. Always there when the rifle runs dry.", ModelShape::PistolSmall);
    p9.damage = 26.0; p9.damage_far = 16.0; p9.range_near = 16.0; p9.range_far = 38.0;
    p9.rpm = 420.0; p9.fire_mode = FireMode::Semi; p9.mag = 15; p9.reserve = 75;
    p9.reload_time = 1.5; p9.reload_empty = 2.0;
    p9.recoil_up = 0.0110; p9.recoil_side = 0.0034; p9.recoil_keep = 0.10;
    p9.spread_base = 0.022; p9.ads_time = 0.15; p9.move_scale = 1.08; p9.ads_move_scale = 0.86;
    p9.swap_out = 0.16; p9.swap_in = 0.28; p9.penetration = 0.9;
    p9.sound_pitch = 1.12; p9.sound_body = 0.42;

    let mut anvil = base(W::Anvil44, "ANVIL .44", Pistol, "Hand cannon. Two rounds is usually enough.", ModelShape::PistolHeavy);
    anvil.damage = 55.0; anvil.damage_far = 38.0; anvil.range_near = 20.0; anvil.range_far = 46.0;
    anvil.rpm = 200.0; anvil.fire_mode = FireMode::Semi; anvil.mag = 6; anvil.reserve = 36;
    anvil.reload_time = 2.3; anvil.reload_empty = 2.3;
    anvil.headshot_mult = 1.9;
    anvil.recoil_up = 0.0380; anvil.recoil_side = 0.0060; anvil.recoil_keep = 0.05;
    anvil.spread_base = 0.026; anvil.ads_time = 0.22; anvil.move_scale = 1.02; anvil.ads_move_scale = 0.70;
    anvil.swap_out = 0.20; anvil.swap_in = 0.34; anvil.penetration = 2.0; anvil.unlock_level = 9;
    anvil.sound_pitch = 0.66; anvil.sound_body = 0.90; anvil.flash_scale = 1.5;

    let mut burst = base(W::BurstP3, "BURST P3", Pistol, "Three-round machine pistol. Deceptively strong.", ModelShape::PistolSmall);
    burst.damage = 24.0; burst.damage_far = 13.0; burst.range_near = 13.0; burst.range_far = 30.0;
    burst.rpm = 1100.0; burst.fire_mode = FireMode::Burst(3); burst.cycle_time = 0.24;
    burst.mag = 18; burst.reserve = 90; burst.reload_time = 1.6; burst.reload_empty = 2.1;
    burst.recoil_up = 0.0090; burst.recoil_side = 0.0030; burst.recoil_keep = 0.34;
    burst.spread_base = 0.024; burst.ads_time = 0.16; burst.move_scale = 1.07; burst.ads_move_scale = 0.84;
    burst.swap_out = 0.17; burst.swap_in = 0.30; burst.penetration = 0.8; burst.unlock_level = 15;
    burst.sound_pitch = 1.24; burst.sound_body = 0.34;

    let mut holdout = base(W::Holdout, "HOLDOUT .380", Pistol, "Tiny, fast, and barely a weapon. You will draw it instantly.", ModelShape::PistolSmall);
    holdout.damage = 18.0; holdout.damage_far = 10.0; holdout.range_near = 10.0; holdout.range_far = 25.0;
    holdout.rpm = 520.0; holdout.fire_mode = FireMode::Semi; holdout.mag = 10; holdout.reserve = 60;
    holdout.reload_time = 1.2; holdout.reload_empty = 1.6;
    holdout.recoil_up = 0.0080; holdout.recoil_side = 0.0040; holdout.recoil_keep = 0.08;
    holdout.spread_base = 0.028; holdout.ads_time = 0.12; holdout.move_scale = 1.12; holdout.ads_move_scale = 0.92;
    holdout.swap_out = 0.10; holdout.swap_in = 0.18; holdout.penetration = 0.5; holdout.unlock_level = 28;
    holdout.sound_pitch = 1.34; holdout.sound_body = 0.28; holdout.flash_scale = 0.7;

    // ------------------------------------------------------------ melee
    let mut knife = base(W::CombatKnife, "COMBAT KNIFE", Melee, "Silent, instant, and requires you to be very close.", ModelShape::Knife);
    knife.damage = 135.0; knife.damage_far = 135.0; knife.range_near = 2.2; knife.range_far = 2.2;
    knife.headshot_mult = 1.0; knife.limb_mult = 1.0;
    knife.rpm = 130.0; knife.fire_mode = FireMode::Melee; knife.mag = 0; knife.reserve = 0;
    knife.spread_base = 0.0; knife.recoil_up = 0.0; knife.recoil_side = 0.0;
    knife.ads_time = 0.10; knife.move_scale = 1.14; knife.ads_move_scale = 1.14;
    knife.swap_out = 0.12; knife.swap_in = 0.22; knife.ejects_shells = false; knife.flash_scale = 0.0;
    knife.sound_pitch = 1.0; knife.sound_bright = 0.2; knife.sound_body = 0.1;

    let mut spade = base(W::TrenchSpade, "TRENCH SPADE", Melee, "Slower swing, wider arc, and it will not be survived either.", ModelShape::Spade);
    spade.damage = 150.0; spade.damage_far = 150.0; spade.range_near = 2.6; spade.range_far = 2.6;
    spade.headshot_mult = 1.0; spade.limb_mult = 1.0;
    spade.rpm = 90.0; spade.fire_mode = FireMode::Melee; spade.mag = 0; spade.reserve = 0;
    spade.spread_base = 0.0; spade.recoil_up = 0.0; spade.recoil_side = 0.0;
    spade.ads_time = 0.12; spade.move_scale = 1.10; spade.ads_move_scale = 1.10;
    spade.swap_out = 0.16; spade.swap_in = 0.28; spade.ejects_shells = false; spade.flash_scale = 0.0;
    spade.sound_pitch = 0.8; spade.sound_bright = 0.3; spade.sound_body = 0.2;
    spade.unlock_level = 11;

    [
        kr44, vectra, lancer, tempest, kestrel,
        wasp, viper, shrike, scarab, hornet,
        breacher, longshore, trench,
        longbow, marksman, spectre,
        hammerhead, bulwark, drumfire,
        p9, anvil, burst, holdout,
        knife, spade,
    ]
}

/// Runtime state of one carried weapon.
#[derive(Copy, Clone, Debug)]
pub struct WeaponSlot {
    pub id: WeaponId,
    pub ammo: u16,
    pub reserve: u16,
}

impl Default for WeaponSlot {
    fn default() -> Self { WeaponSlot::new(WeaponId::SidearmP9) }
}

impl WeaponSlot {
    pub fn new(id: WeaponId) -> WeaponSlot {
        let d = id.def();
        WeaponSlot { id, ammo: d.mag, reserve: d.reserve }
    }
    #[inline]
    pub fn def(&self) -> &'static WeaponDef { self.id.def() }
    #[inline]
    pub fn is_empty(&self) -> bool { self.ammo == 0 }
    #[inline]
    pub fn can_reload(&self) -> bool {
        let d = self.def();
        !d.is_melee() && self.reserve > 0 && self.ammo < d.mag
    }
    /// Refills the magazine, returning how many rounds were loaded.
    pub fn reload(&mut self) -> u16 {
        let d = self.def();
        let want = d.mag.saturating_sub(self.ammo);
        let take = want.min(self.reserve);
        self.ammo += take;
        self.reserve -= take;
        take
    }
    /// Loads a single shell, for shotguns.
    pub fn reload_one(&mut self) -> bool {
        let d = self.def();
        if self.ammo >= d.mag || self.reserve == 0 { return false; }
        self.ammo += 1;
        self.reserve -= 1;
        true
    }
    pub fn refill(&mut self) {
        let d = self.def();
        self.ammo = d.mag;
        self.reserve = d.reserve;
    }
}

/// Computes the current spread cone half-angle in radians.
#[allow(clippy::too_many_arguments)]
pub fn current_spread(d: &WeaponDef, speed_frac: f32, airborne: bool, ads_t: f32, bloom: f32, stance_mult: f32) -> f32 {
    let mut s = d.spread_base + d.spread_move * speed_frac.clamp(0.0, 1.0) + bloom;
    if airborne { s += d.spread_jump; }
    s *= stance_mult;
    // Aiming scales the whole cone down; the blend is linear so partial ADS
    // gives partial benefit, which is what makes quick-scoping feel fair.
    let aim = 1.0 + (d.spread_ads - 1.0) * ads_t.clamp(0.0, 1.0);
    (s * aim).max(0.0)
}

/// Picks a direction inside the spread cone. `shot` indexes pellets so a
/// shotgun blast is deterministic given the same seed on client and server.
pub fn spread_direction(forward: glam::Vec3, cone: f32, rng: &mut Rng) -> glam::Vec3 {
    if cone <= 1e-6 { return forward; }
    // Uniform disc, then projected onto the cone: avoids the centre-heavy
    // clustering a naive two-angle sample produces.
    let a = rng.f32() * std::f32::consts::TAU;
    let r = rng.f32().sqrt() * cone;
    let (sa, ca) = a.sin_cos();
    let up = if forward.y.abs() > 0.95 { glam::Vec3::Z } else { glam::Vec3::Y };
    let right = forward.cross(up).normalize_or_zero();
    let real_up = right.cross(forward);
    (forward + right * (ca * r) + real_up * (sa * r)).normalize_or_zero()
}
