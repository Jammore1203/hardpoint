//! Loadouts: classes, perks and equipment.
//!
//! Kept deliberately small. Five classes, six perks, six pieces of equipment.
//! The point is a decision at the spawn screen, not a second game.

use super::weapons::{WeaponClass, WeaponId};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ClassId {
    Assault = 0,
    Heavy = 1,
    Scout = 2,
    Marksman = 3,
    Custom = 4,
}

pub const CLASS_COUNT: usize = 5;
pub const ALL_CLASSES: [ClassId; CLASS_COUNT] = [
    ClassId::Assault, ClassId::Heavy, ClassId::Scout, ClassId::Marksman, ClassId::Custom,
];

impl ClassId {
    pub fn from_u8(v: u8) -> ClassId {
        if (v as usize) < CLASS_COUNT { ALL_CLASSES[v as usize] } else { ClassId::Assault }
    }
    pub fn name(self) -> &'static str {
        match self {
            ClassId::Assault => "ASSAULT",
            ClassId::Heavy => "HEAVY",
            ClassId::Scout => "SCOUT",
            ClassId::Marksman => "MARKSMAN",
            ClassId::Custom => "CUSTOM",
        }
    }
    pub fn blurb(self) -> &'static str {
        match self {
            ClassId::Assault => "Rifle, frag, flash. Good everywhere, best nowhere.",
            ClassId::Heavy => "Machine gun and armour plate. Hold ground, move slowly.",
            ClassId::Scout => "Subgun, smoke and speed. Get behind them.",
            ClassId::Marksman => "Bolt rifle and a sidearm. Own one long angle.",
            ClassId::Custom => "Your own arrangement.",
        }
    }
    /// The stock loadout for a class.
    pub fn preset(self) -> Loadout {
        use Equipment::*;
        use WeaponId::*;
        match self {
            ClassId::Assault => Loadout {
                class: self, primary: Vectra5, secondary: SidearmP9, melee: CombatKnife,
                lethal: Frag, tactical: Flashbang, perk: Perk::SteadyHands,
            cosmetic: 0,
            },
            ClassId::Heavy => Loadout {
                class: self, primary: Hammerhead, secondary: Anvil44, melee: TrenchSpade,
                lethal: Incendiary, tactical: Smoke, perk: Perk::Toughness,
            cosmetic: 0,
            },
            ClassId::Scout => Loadout {
                class: self, primary: Viper, secondary: Holdout, melee: CombatKnife,
                lethal: Sticky, tactical: Smoke, perk: Perk::Lightfoot,
            cosmetic: 0,
            },
            ClassId::Marksman => Loadout {
                class: self, primary: Longbow, secondary: SidearmP9, melee: CombatKnife,
                lethal: Frag, tactical: Concussion, perk: Perk::Scavenger,
            cosmetic: 0,
            },
            ClassId::Custom => Loadout {
                class: self, primary: Kr44, secondary: SidearmP9, melee: CombatKnife,
                lethal: Frag, tactical: Flashbang, perk: Perk::FastHands,
            cosmetic: 0,
            },
        }
    }
}

/// Throwable and placeable equipment.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Equipment {
    Frag = 0,
    Sticky = 1,
    Incendiary = 2,
    Flashbang = 3,
    Smoke = 4,
    Concussion = 5,
}

pub const EQUIPMENT_COUNT: usize = 6;
pub const ALL_EQUIPMENT: [Equipment; EQUIPMENT_COUNT] = [
    Equipment::Frag, Equipment::Sticky, Equipment::Incendiary,
    Equipment::Flashbang, Equipment::Smoke, Equipment::Concussion,
];
pub const LETHAL_EQUIPMENT: [Equipment; 3] = [Equipment::Frag, Equipment::Sticky, Equipment::Incendiary];
pub const TACTICAL_EQUIPMENT: [Equipment; 3] = [Equipment::Flashbang, Equipment::Smoke, Equipment::Concussion];

impl Equipment {
    pub fn from_u8(v: u8) -> Equipment {
        if (v as usize) < EQUIPMENT_COUNT { ALL_EQUIPMENT[v as usize] } else { Equipment::Frag }
    }
    pub fn name(self) -> &'static str {
        match self {
            Equipment::Frag => "FRAG",
            Equipment::Sticky => "STICKY",
            Equipment::Incendiary => "INCENDIARY",
            Equipment::Flashbang => "FLASHBANG",
            Equipment::Smoke => "SMOKE",
            Equipment::Concussion => "CONCUSSION",
        }
    }
    pub fn blurb(self) -> &'static str {
        match self {
            Equipment::Frag => "Cooks for four seconds. Bounces.",
            Equipment::Sticky => "Adheres where it lands. Smaller radius, no roll.",
            Equipment::Incendiary => "Denies ground for ten seconds.",
            Equipment::Flashbang => "Blinds anyone facing it.",
            Equipment::Smoke => "Twelve seconds of cover.",
            Equipment::Concussion => "Slows and disorients through cover.",
        }
    }
    pub fn is_lethal(self) -> bool { matches!(self, Equipment::Frag | Equipment::Sticky | Equipment::Incendiary) }
    /// How many you spawn with.
    pub fn count(self) -> u8 {
        match self {
            Equipment::Frag | Equipment::Sticky => 1,
            Equipment::Incendiary => 1,
            Equipment::Flashbang | Equipment::Concussion => 2,
            Equipment::Smoke => 1,
        }
    }
    /// Fuse in seconds; zero means it triggers on impact.
    pub fn fuse(self) -> f32 {
        match self {
            Equipment::Frag => 3.6,
            Equipment::Sticky => 2.6,
            Equipment::Incendiary => 2.2,
            Equipment::Flashbang => 2.0,
            Equipment::Concussion => 2.0,
            Equipment::Smoke => 1.2,
        }
    }
    pub fn blast_radius(self) -> f32 {
        match self {
            Equipment::Frag => 6.2,
            Equipment::Sticky => 5.0,
            Equipment::Incendiary => 4.2,
            Equipment::Flashbang => 9.0,
            Equipment::Concussion => 8.0,
            Equipment::Smoke => 0.0,
        }
    }
    pub fn blast_damage(self) -> f32 {
        match self {
            Equipment::Frag => 130.0,
            Equipment::Sticky => 145.0,
            Equipment::Incendiary => 22.0,
            _ => 0.0,
        }
    }
    /// Bounciness and friction of the thrown object.
    pub fn physics(self) -> (f32, f32) {
        match self {
            Equipment::Sticky => (0.0, 1.0),
            Equipment::Frag => (0.36, 0.30),
            Equipment::Incendiary => (0.22, 0.45),
            Equipment::Flashbang => (0.42, 0.24),
            Equipment::Concussion => (0.40, 0.26),
            Equipment::Smoke => (0.26, 0.40),
        }
    }
}

/// Passive abilities. One per loadout.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Perk {
    SteadyHands = 0,
    Lightfoot = 1,
    Toughness = 2,
    Scavenger = 3,
    FastHands = 4,
    Bandolier = 5,
}

pub const PERK_COUNT: usize = 6;
pub const ALL_PERKS: [Perk; PERK_COUNT] = [
    Perk::SteadyHands, Perk::Lightfoot, Perk::Toughness,
    Perk::Scavenger, Perk::FastHands, Perk::Bandolier,
];

impl Perk {
    pub fn from_u8(v: u8) -> Perk {
        if (v as usize) < PERK_COUNT { ALL_PERKS[v as usize] } else { Perk::SteadyHands }
    }
    pub fn name(self) -> &'static str {
        match self {
            Perk::SteadyHands => "STEADY HANDS",
            Perk::Lightfoot => "LIGHTFOOT",
            Perk::Toughness => "TOUGHNESS",
            Perk::Scavenger => "SCAVENGER",
            Perk::FastHands => "FAST HANDS",
            Perk::Bandolier => "BANDOLIER",
        }
    }
    pub fn blurb(self) -> &'static str {
        match self {
            Perk::SteadyHands => "A quarter less recoil and tighter hip fire.",
            Perk::Lightfoot => "Eight percent faster, and your footsteps carry less.",
            Perk::Toughness => "Twenty-five extra armour and less flinch when hit.",
            Perk::Scavenger => "Take a magazine from anyone you kill.",
            Perk::FastHands => "Reload and swap weapons a third faster.",
            Perk::Bandolier => "Two extra magazines and one more grenade.",
        }
    }
    pub fn move_scale(self) -> f32 { if self == Perk::Lightfoot { 1.08 } else { 1.0 } }
    pub fn recoil_scale(self) -> f32 { if self == Perk::SteadyHands { 0.75 } else { 1.0 } }
    pub fn spread_scale(self) -> f32 { if self == Perk::SteadyHands { 0.88 } else { 1.0 } }
    pub fn handling_scale(self) -> f32 { if self == Perk::FastHands { 0.67 } else { 1.0 } }
    pub fn bonus_armor(self) -> f32 { if self == Perk::Toughness { 25.0 } else { 0.0 } }
    pub fn flinch_scale(self) -> f32 { if self == Perk::Toughness { 0.5 } else { 1.0 } }
    pub fn reserve_scale(self) -> f32 { if self == Perk::Bandolier { 1.5 } else { 1.0 } }
    pub fn extra_grenade(self) -> u8 { if self == Perk::Bandolier { 1 } else { 0 } }
    pub fn footstep_volume(self) -> f32 { if self == Perk::Lightfoot { 0.55 } else { 1.0 } }
}

/// A player's full equipment selection.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Loadout {
    pub class: ClassId,
    pub primary: WeaponId,
    pub secondary: WeaponId,
    pub melee: WeaponId,
    pub lethal: Equipment,
    pub tactical: Equipment,
    pub perk: Perk,
    /// Appearance, purely visual and replicated so everyone sees the same
    /// soldier. Never affects silhouette size or hitbox: cosmetics that change
    /// how big a target is are cosmetics that decide fights.
    pub cosmetic: u8,
}

/// The kits a player can wear.
///
/// Deliberately small and deliberately readable. Each changes headgear,
/// webbing colour and the finish on the weapon, which is enough to tell people
/// apart in a lobby and not enough to confuse a target for a teammate at
/// twenty metres, because team colour is carried by the fatigues and those do
/// not change.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Kit {
    pub name: &'static str,
    /// Headgear: 0 helmet, 1 helmet with cover, 2 patrol cap, 3 bare with a
    /// headset. Sizes vary by a couple of centimetres, never more.
    pub head: u8,
    /// Multiplier on the webbing colour.
    pub webbing: [f32; 3],
    /// Multiplier on the weapon's finish.
    pub finish: [f32; 3],
}

pub const KIT_COUNT: usize = 6;

pub const KITS: [Kit; KIT_COUNT] = [
    Kit { name: "STANDARD", head: 0, webbing: [1.00, 1.00, 1.00], finish: [1.00, 1.00, 1.00] },
    Kit { name: "COVERED",  head: 1, webbing: [0.86, 0.90, 0.78], finish: [0.92, 0.94, 0.92] },
    Kit { name: "PATROL",   head: 2, webbing: [0.78, 0.74, 0.66], finish: [1.06, 1.02, 0.94] },
    Kit { name: "HEADSET",  head: 3, webbing: [0.70, 0.72, 0.76], finish: [0.84, 0.86, 0.92] },
    Kit { name: "DESERT",   head: 1, webbing: [1.06, 0.98, 0.80], finish: [1.10, 1.04, 0.88] },
    Kit { name: "NIGHT",    head: 0, webbing: [0.62, 0.64, 0.70], finish: [0.72, 0.74, 0.80] },
];

impl Loadout {
    pub fn kit(&self) -> &'static Kit { &KITS[(self.cosmetic as usize) % KIT_COUNT] }
}

impl Default for Loadout {
    fn default() -> Self { ClassId::Assault.preset() }
}

impl Loadout {
    /// Rejects anything nonsensical: an SMG in the sidearm slot, a lethal in
    /// the tactical slot, a weapon the player has not unlocked. The server
    /// runs this on everything a client sends.
    pub fn sanitize(&mut self, max_level: u8) {
        if !self.primary.def().class.is_primary() {
            self.primary = WeaponId::Vectra5;
        }
        if self.secondary.def().class != WeaponClass::Pistol {
            self.secondary = WeaponId::SidearmP9;
        }
        if self.melee.def().class != WeaponClass::Melee {
            self.melee = WeaponId::CombatKnife;
        }
        if !self.lethal.is_lethal() { self.lethal = Equipment::Frag; }
        if self.tactical.is_lethal() { self.tactical = Equipment::Flashbang; }
        // Locked weapons fall back to the always-available option in the slot.
        if self.primary.def().unlock_level > max_level { self.primary = WeaponId::Vectra5; }
        if self.secondary.def().unlock_level > max_level { self.secondary = WeaponId::SidearmP9; }
        if self.melee.def().unlock_level > max_level { self.melee = WeaponId::CombatKnife; }
    }

    pub fn encode(&self) -> [u8; 8] {
        [
            self.class as u8,
            self.primary as u8,
            self.secondary as u8,
            self.melee as u8,
            self.lethal as u8,
            self.tactical as u8,
            self.perk as u8,
            self.cosmetic,
        ]
    }

    pub fn decode(b: [u8; 8]) -> Loadout {
        Loadout {
            class: ClassId::from_u8(b[0]),
            primary: WeaponId::from_u8(b[1]),
            secondary: WeaponId::from_u8(b[2]),
            melee: WeaponId::from_u8(b[3]),
            lethal: Equipment::from_u8(b[4]),
            tactical: Equipment::from_u8(b[5]),
            perk: Perk::from_u8(b[6]),
            cosmetic: b[7] % KIT_COUNT as u8,
        }
    }
}
