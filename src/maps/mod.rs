//! Map data model and registry.
//!
//! A map is authored in code as a list of brushes plus gameplay markers. There
//! is no level file format and no importer: building a map is a function call,
//! so load times are dominated by mesh generation (a few milliseconds) rather
//! than IO or parsing. That is what makes map transitions feel instant.

pub mod brush;
pub mod build;
pub mod library;
pub mod nav;

use crate::assets::materials::Mat;
use crate::game::types::{Objective, PickupSpot, SpawnPoint, Team};
use brush::{Brush, CollisionWorld};
use glam::Vec3;
use nav::NavGrid;

/// Stable identifier for a map. Sent over the wire as a `u8`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum MapId {
    Ironveil = 0,
    Stormworks,
    Belvoir,
    Greenline,
    Whiteout,
    Highrise,
    Drydock,
    Foundry,
    Saltbite,
    Deepwell,
    Junction,
    Overpass,
}

pub const MAP_COUNT: usize = 12;

pub const ALL_MAPS: [MapId; MAP_COUNT] = [
    MapId::Ironveil, MapId::Stormworks, MapId::Belvoir, MapId::Greenline,
    MapId::Whiteout, MapId::Highrise, MapId::Drydock, MapId::Foundry,
    MapId::Saltbite, MapId::Deepwell, MapId::Junction, MapId::Overpass,
];

impl MapId {
    pub fn from_u8(v: u8) -> MapId {
        if (v as usize) < MAP_COUNT { ALL_MAPS[v as usize] } else { MapId::Ironveil }
    }
    pub fn index(self) -> usize { self as usize }

    pub fn name(self) -> &'static str {
        match self {
            MapId::Ironveil => "IRONVEIL",
            MapId::Stormworks => "STORMWORKS",
            MapId::Belvoir => "BELVOIR",
            MapId::Greenline => "GREENLINE",
            MapId::Whiteout => "WHITEOUT",
            MapId::Highrise => "HIGHRISE",
            MapId::Drydock => "DRYDOCK",
            MapId::Foundry => "FOUNDRY",
            MapId::Saltbite => "SALTBITE",
            MapId::Deepwell => "DEEPWELL",
            MapId::Junction => "JUNCTION",
            MapId::Overpass => "OVERPASS",
        }
    }

    pub fn theme(self) -> &'static str {
        match self {
            MapId::Ironveil => "DESERT AIRFIELD",
            MapId::Stormworks => "INDUSTRIAL WAREHOUSE",
            MapId::Belvoir => "EUROPEAN VILLAGE",
            MapId::Greenline => "JUNGLE RELAY STATION",
            MapId::Whiteout => "ARCTIC RADAR SITE",
            MapId::Highrise => "URBAN APARTMENTS",
            MapId::Drydock => "SHIPYARD",
            MapId::Foundry => "ABANDONED FOUNDRY",
            MapId::Saltbite => "COASTAL FORT",
            MapId::Deepwell => "UNDERGROUND BUNKER",
            MapId::Junction => "RAIL JUNCTION",
            MapId::Overpass => "HIGHWAY CHECKPOINT",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            MapId::Ironveil => "Hangars and revetments around a cracked runway. Long sightlines down the strip, tight fights inside the maintenance bays.",
            MapId::Stormworks => "Stacked cargo and gantry catwalks. Three floors of ambush lanes with almost no safe ground.",
            MapId::Belvoir => "A shelled market town. Rooftops overlook the square; the sewers run straight under it.",
            MapId::Greenline => "Dish arrays under heavy canopy. Foliage hides everything until it is far too late.",
            MapId::Whiteout => "A radar station buried in drift snow. Wind noise masks footsteps; the mast is a sniper's dream.",
            MapId::Highrise => "Four gutted apartment blocks around a courtyard. Verticality everywhere, cover nowhere.",
            MapId::Drydock => "A half-scrapped hull in a dry basin. Fight along the keel or up on the gantries.",
            MapId::Foundry => "Cold furnaces and pour channels. Catwalks cross above a killing floor.",
            MapId::Saltbite => "A sea fort on a tidal causeway. Casemates, ramparts and one very exposed approach.",
            MapId::Deepwell => "A command bunker three levels down. Corridor warfare with no sky and no mercy.",
            MapId::Junction => "Freight cars in a marshalling yard. Cover moves, sightlines shift, nothing is stable.",
            MapId::Overpass => "A fortified road crossing under a collapsed flyover. Fast lanes, brutal chokes.",
        }
    }

    /// Rough footprint used to sort the map list and pick sensible player
    /// counts in the lobby.
    pub fn size_class(self) -> SizeClass {
        match self {
            MapId::Deepwell | MapId::Foundry | MapId::Overpass => SizeClass::Small,
            MapId::Stormworks | MapId::Belvoir | MapId::Highrise | MapId::Junction | MapId::Saltbite => SizeClass::Medium,
            _ => SizeClass::Large,
        }
    }

    pub fn build(self) -> MapData { library::build(self) }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum SizeClass { Small, Medium, Large }

impl SizeClass {
    pub fn label(self) -> &'static str {
        match self { SizeClass::Small => "SMALL", SizeClass::Medium => "MEDIUM", SizeClass::Large => "LARGE" }
    }
    /// Suggested player count for the lobby default.
    pub fn suggested_players(self) -> u8 {
        match self { SizeClass::Small => 8, SizeClass::Medium => 12, SizeClass::Large => 14 }
    }
}

/// Per-map atmosphere. Deliberately small: a handful of colours and one fog
/// curve is all the era's look actually needs, and it costs nothing to apply.
#[derive(Clone, Copy)]
pub struct Env {
    pub fog_color: [f32; 3],
    /// Distance at which fog begins and where it saturates, in metres.
    pub fog_start: f32,
    pub fog_end: f32,
    pub sky_top: [f32; 3],
    pub sky_horizon: [f32; 3],
    /// Direction the sunlight travels (pointing away from the sun).
    pub sun_dir: Vec3,
    pub sun_color: [f32; 3],
    pub ambient_sky: [f32; 3],
    pub ambient_ground: [f32; 3],
    /// Global multiplier applied after lighting; the era's warm/cool grade.
    pub grade_warm: [f32; 3],
    pub grade_cool: [f32; 3],
    pub ambience: Ambience,
    pub track: MusicTrack,
    /// Optional light haze of falling particles (snow, ash, rain).
    pub weather: Weather,
}

impl Default for Env {
    fn default() -> Env {
        Env {
            fog_color: [0.52, 0.55, 0.58],
            fog_start: 25.0,
            fog_end: 110.0,
            sky_top: [0.28, 0.38, 0.52],
            sky_horizon: [0.62, 0.66, 0.70],
            sun_dir: Vec3::new(-0.45, -0.78, -0.44).normalize(),
            sun_color: [1.05, 1.00, 0.90],
            ambient_sky: [0.34, 0.38, 0.46],
            ambient_ground: [0.20, 0.19, 0.17],
            grade_warm: [1.05, 1.00, 0.94],
            grade_cool: [0.95, 0.99, 1.06],
            ambience: Ambience::Wind,
            track: MusicTrack::Patrol,
            weather: Weather::None,
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Ambience { Wind, Industrial, Jungle, Interior, Coastal, Urban, Blizzard, RailYard }

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Weather { None, Snow, Ash, Rain, Dust }

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum MusicTrack { Menu, Patrol, Assault, Tension, Victory, Defeat }

/// A fully built, ready-to-play level.
pub struct MapData {
    pub id: MapId,
    pub brushes: Vec<Brush>,
    pub collision: CollisionWorld,
    pub spawns: Vec<SpawnPoint>,
    /// Domination points, in order A, B, C.
    pub domination: Vec<Objective>,
    /// Bomb sites for Search & Destroy.
    pub bomb_sites: Vec<Objective>,
    pub pickups: Vec<PickupSpot>,
    pub env: Env,
    pub nav: NavGrid,
    /// Playable bounds; anything outside is a kill volume.
    pub bounds: crate::math::Aabb,
    /// Decorative, non-colliding brushes drawn but never traced against. Kept
    /// separate so collision queries never touch them.
    pub decor: Vec<Brush>,
    /// `(spawns dropped, pickups relocated)` by `bind_markers_to_navigation`.
    pub repairs: (usize, usize),
}

impl MapData {
    pub fn name(&self) -> &'static str { self.id.name() }

    /// Spawn candidates for a team, falling back to neutral spawns.
    pub fn spawns_for(&self, team: Team, initial_only: bool) -> impl Iterator<Item = &SpawnPoint> {
        self.spawns.iter().filter(move |s| {
            (!initial_only || s.initial)
                && (s.team == team || s.team == Team::None || team == Team::None)
        })
    }

    /// Snaps pickups and objectives down onto the surface beneath them.
    ///
    /// Map authors place markers at approximate heights; resolving them once at
    /// build time means a pickup is always exactly on its floor, an objective
    /// volume always sits on the ground the player stands on, and the
    /// validation pass reports genuine layout problems rather than arithmetic.
    pub fn snap_markers(&mut self) {
        let mut pickups = std::mem::take(&mut self.pickups);
        for p in pickups.iter_mut() {
            if let Some((feet, _)) = self.resolve_spawn(p.pos) {
                // Sit visibly above the surface so it reads as a pickup.
                p.pos = feet + Vec3::Y * 0.25;
            }
        }
        self.pickups = pickups;

        let snap_objective = |o: &mut Objective, world: &brush::CollisionWorld| {
            if let Some((h, _)) = world.ground_below(o.pos + Vec3::Y * 1.2, 0.3, 8.0) {
                o.pos.y = h;
            }
        };
        let mut dom = std::mem::take(&mut self.domination);
        for o in dom.iter_mut() { snap_objective(o, &self.collision); }
        self.domination = dom;
        let mut sites = std::mem::take(&mut self.bomb_sites);
        for o in sites.iter_mut() { snap_objective(o, &self.collision); }
        self.bomb_sites = sites;
    }

    /// Final safety net: guarantees that every spawn and pickup sits somewhere
    /// a character can actually reach.
    ///
    /// Spawns that resolve onto an unreachable pocket are dropped; pickups are
    /// moved onto the nearest reachable ground. Authoring a level in code means
    /// a prop can land on a marker without anyone noticing, and a player who
    /// spawns inside a sealed room has no recourse at all - so this runs on
    /// every map, every load, and the audit reports whatever it had to repair.
    pub fn bind_markers_to_navigation(&mut self) -> (usize, usize) {
        if self.nav.nodes.is_empty() { return (0, 0); }
        let (comp, main) = self.nav.main_component();

        // Nearest node that is part of the main playable region.
        let nearest_main = |p: Vec3| -> Option<(u32, f32)> {
            let mut best: Option<(u32, f32)> = None;
            for (i, n) in self.nav.nodes.iter().enumerate() {
                if comp[i] != main { continue; }
                let d = (n.pos.x - p.x).powi(2) + (n.pos.z - p.z).powi(2) + (n.pos.y - p.y).powi(2) * 4.0;
                if best.map_or(true, |(_, bd)| d < bd) { best = Some((i as u32, d)); }
            }
            best
        };
        let ok_here = |p: Vec3| -> bool {
            match self.nav.nearest(p) {
                Some(n) if comp[n as usize] == main => {
                    let np = self.nav.node(n).pos;
                    ((np.x - p.x).powi(2) + (np.z - p.z).powi(2)).sqrt() <= 2.5 && (np.y - p.y).abs() <= 2.0
                }
                _ => false,
            }
        };

        let before = self.spawns.len();
        let resolved: Vec<Option<Vec3>> = self.spawns.iter()
            .map(|s| self.resolve_spawn(s.pos).map(|(p, _)| p))
            .collect();
        let mut kept = Vec::with_capacity(before);
        for (s, r) in self.spawns.iter().zip(resolved) {
            match r {
                Some(p) if ok_here(p) => {
                    let mut s = *s;
                    s.pos = p;
                    kept.push(s);
                }
                _ => {}
            }
        }
        self.spawns = kept;
        let dropped = before - self.spawns.len();

        let mut moved = 0;
        let relocations: Vec<Option<Vec3>> = self.pickups.iter()
            .map(|p| {
                if ok_here(p.pos) { None } else { nearest_main(p.pos).map(|(n, _)| self.nav.node(n).pos) }
            })
            .collect();
        for (p, r) in self.pickups.iter_mut().zip(relocations) {
            if let Some(np) = r {
                p.pos = np + Vec3::Y * 0.25;
                moved += 1;
            }
        }
        (dropped, moved)
    }

    /// Resolves an authored spawn marker into a position a player can
    /// actually occupy: snap down to the surface, then nudge outward if a
    /// prop happens to sit on the marker. The server uses exactly this, so a
    /// crate dropped on a spawn is a non-event rather than a stuck player.
    ///
    /// Returns the resolved feet position and whether a nudge was needed.
    pub fn resolve_spawn(&self, marker: Vec3) -> Option<(Vec3, bool)> {
        const RADIUS: f32 = 0.34;
        const HEIGHT: f32 = 1.78;

        let try_at = |x: f32, z: f32| -> Option<Vec3> {
            let probe = Vec3::new(x, marker.y + 0.6, z);
            let h = self.collision.ground_below(probe, RADIUS, 4.0)?.0;
            let feet = Vec3::new(x, h + crate::maps::nav::GROUND_CLEARANCE, z);
            if self.collision.standable(feet, RADIUS, HEIGHT, crate::maps::nav::STEP_ALLOWANCE) {
                Some(feet)
            } else {
                None
            }
        };

        if let Some(p) = try_at(marker.x, marker.z) { return Some((p, false)); }
        // Spiral outward on the golden angle: even coverage, no directional bias.
        for i in 1..24 {
            let a = i as f32 * 2.399_963;
            let r = 0.5 + i as f32 * 0.11;
            if let Some(p) = try_at(marker.x + a.cos() * r, marker.z + a.sin() * r) {
                return Some((p, true));
            }
        }
        None
    }

    /// Cheap sanity report, run once after every map is built. Catches the
    /// classic authoring mistakes: spawns nothing can stand on, objectives
    /// floating in the air, navigation that failed to bake.
    pub fn validate(&self) -> Vec<String> {
        let mut issues = Vec::new();
        if self.spawns.len() < 8 {
            issues.push(format!("{}: only {} spawn points", self.name(), self.spawns.len()));
        }
        for (i, s) in self.spawns.iter().enumerate() {
            if !self.bounds.contains_point(s.pos) {
                issues.push(format!("{}: spawn {} is outside the playspace", self.name(), i));
            }
        }
        for (team, count) in [(Team::Phantom, 0), (Team::Vanguard, 0)] {
            let n = self.spawns.iter().filter(|s| s.team == team).count();
            let _ = count;
            if n < 6 { issues.push(format!("{}: {} has only {} spawns", self.name(), team.name(), n)); }
        }
        if self.domination.len() != 3 {
            issues.push(format!("{}: needs 3 domination points, has {}", self.name(), self.domination.len()));
        }
        if self.bomb_sites.len() != 2 {
            issues.push(format!("{}: needs 2 bomb sites, has {}", self.name(), self.bomb_sites.len()));
        }
        for o in self.domination.iter().chain(self.bomb_sites.iter()) {
            if self.collision.ground_below(o.pos + Vec3::Y * 0.9, 0.3, 6.0).is_none() {
                issues.push(format!("{}: objective {} floats", self.name(), o.label));
            }
        }
        for (i, p) in self.pickups.iter().enumerate() {
            if self.collision.ground_below(p.pos + Vec3::Y * 0.9, 0.3, 6.0).is_none() {
                issues.push(format!("{}: pickup {} ({:?}) floats at {:?}", self.name(), i, p.kind, p.pos));
            }
        }
        // Connectivity: everything a player must reach has to sit in the same
        // navigation component, otherwise part of the level is a dead pocket.
        if !self.nav.nodes.is_empty() {
            let (comp, main) = self.nav.main_component();
            let reachable = comp.iter().filter(|c| **c == main).count();
            let ratio = reachable as f32 / comp.len() as f32;
            if ratio < 0.80 {
                issues.push(format!("{}: only {:.0}% of navigation is one connected region",
                                    self.name(), ratio * 100.0));
            }
            // A marker is "connected" only if navigation exists close to it
            // *and* that navigation is part of the main region. Without the
            // distance test, a pickup stranded on a roof would silently pass
            // by snapping to the nearest floor node twenty metres below.
            let mut check = |label: String, p: Vec3| {
                match self.nav.nearest(p) {
                    None => issues.push(format!("{}: {} has no navigation at all", self.name(), label)),
                    Some(n) => {
                        let np = self.nav.node(n).pos;
                        let horiz = ((np.x - p.x).powi(2) + (np.z - p.z).powi(2)).sqrt();
                        let vert = (np.y - p.y).abs();
                        if horiz > 3.5 || vert > 2.5 {
                            issues.push(format!("{}: {} at {:.1},{:.1},{:.1} is stranded ({:.1} m across, {:.1} m up from nav at {:.1},{:.1},{:.1})",
                                                self.name(), label, p.x, p.y, p.z, horiz, vert, np.x, np.y, np.z));
                        } else if comp[n as usize] != main {
                            issues.push(format!("{}: {} is cut off from the main region", self.name(), label));
                        }
                    }
                }
            };
            for (i, s) in self.spawns.iter().enumerate() {
                if let Some((p, _)) = self.resolve_spawn(s.pos) { check(format!("spawn {}", i), p); }
            }
            for o in self.domination.iter() { check(format!("point {}", o.label), o.pos); }
            for o in self.bomb_sites.iter() { check(format!("site {}", o.label), o.pos); }
            for (i, p) in self.pickups.iter().enumerate() { check(format!("pickup {}", i), p.pos); }
        }
        if self.nav.walkable_count() < 500 {
            issues.push(format!("{}: navigation has only {} nodes", self.name(), self.nav.walkable_count()));
        }
        issues
    }

    /// Total triangle budget check, surfaced by the `--audit` developer flag.
    pub fn stats(&self) -> MapStats {
        MapStats {
            brushes: self.brushes.len(),
            decor: self.decor.len(),
            spawns: self.spawns.len(),
            nav_nodes: self.nav.walkable_count(),
            extent: self.bounds.size(),
        }
    }
}

pub struct MapStats {
    pub brushes: usize,
    pub decor: usize,
    pub spawns: usize,
    pub nav_nodes: usize,
    pub extent: Vec3,
}

/// Default material used when an author does not specify one.
pub const DEFAULT_MAT: Mat = Mat::Concrete;
