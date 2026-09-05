//! The map library.
//!
//! Twelve original levels, each authored as a function. They share a
//! vocabulary (see `build.rs`) but not a layout: every map has its own
//! circulation, its own power positions and its own answer to them.
//!
//! House rules every map here follows:
//!   * two team spawn areas that cannot see each other,
//!   * at least one long lane, one mid lane and one interior route between them,
//!   * every power position reachable by two approaches,
//!   * three domination points forming a triangle, never a straight line,
//!   * bomb sites that attackers reach on different timings.

use super::brush::{BrushFlags, RampAxis};
use super::build::*;
use super::{Ambience, Env, MapData, MapId, MusicTrack, Weather};
use crate::assets::materials::Mat;
use crate::game::types::{PickupKind, Team};
use glam::Vec3;

pub fn build(id: MapId) -> MapData {
    let mut b = MapBuilder::new(id);
    match id {
        MapId::Ironveil => ironveil(&mut b),
        MapId::Stormworks => stormworks(&mut b),
        MapId::Belvoir => belvoir(&mut b),
        MapId::Greenline => greenline(&mut b),
        MapId::Whiteout => whiteout(&mut b),
        MapId::Highrise => highrise(&mut b),
        MapId::Drydock => drydock(&mut b),
        MapId::Foundry => foundry(&mut b),
        MapId::Saltbite => saltbite(&mut b),
        MapId::Deepwell => deepwell(&mut b),
        MapId::Junction => junction(&mut b),
        MapId::Overpass => overpass(&mut b),
    }
    b.finish()
}

// ============================================================ environments

fn env_desert() -> Env {
    Env {
        fog_color: [0.78, 0.71, 0.56],
        fog_start: 34.0,
        fog_end: 132.0,
        sky_top: [0.32, 0.50, 0.74],
        sky_horizon: [0.83, 0.79, 0.65],
        sun_dir: Vec3::new(-0.35, -0.86, -0.37).normalize(),
        sun_color: [1.20, 1.10, 0.92],
        ambient_sky: [0.40, 0.42, 0.48],
        ambient_ground: [0.30, 0.26, 0.19],
        grade_warm: [1.10, 1.02, 0.90],
        grade_cool: [0.96, 0.98, 1.02],
        ambience: Ambience::Wind,
        track: MusicTrack::Patrol,
        weather: Weather::Dust,
    }
}

fn env_industrial() -> Env {
    Env {
        fog_color: [0.30, 0.32, 0.36],
        fog_start: 18.0,
        fog_end: 95.0,
        sky_top: [0.18, 0.20, 0.25],
        sky_horizon: [0.34, 0.35, 0.38],
        sun_dir: Vec3::new(0.28, -0.90, 0.33).normalize(),
        sun_color: [0.82, 0.84, 0.92],
        ambient_sky: [0.24, 0.26, 0.32],
        ambient_ground: [0.13, 0.13, 0.14],
        grade_warm: [1.02, 1.00, 0.98],
        grade_cool: [0.92, 0.97, 1.08],
        ambience: Ambience::Industrial,
        track: MusicTrack::Assault,
        weather: Weather::None,
    }
}

fn env_village() -> Env {
    Env {
        fog_color: [0.62, 0.62, 0.60],
        fog_start: 28.0,
        fog_end: 118.0,
        sky_top: [0.34, 0.44, 0.58],
        sky_horizon: [0.70, 0.70, 0.68],
        sun_dir: Vec3::new(0.46, -0.78, -0.42).normalize(),
        sun_color: [1.06, 1.00, 0.88],
        ambient_sky: [0.36, 0.39, 0.45],
        ambient_ground: [0.24, 0.22, 0.19],
        grade_warm: [1.06, 1.01, 0.93],
        grade_cool: [0.95, 0.99, 1.05],
        ambience: Ambience::Urban,
        track: MusicTrack::Tension,
        weather: Weather::None,
    }
}

fn env_jungle() -> Env {
    Env {
        fog_color: [0.36, 0.46, 0.34],
        fog_start: 16.0,
        fog_end: 85.0,
        sky_top: [0.30, 0.44, 0.40],
        sky_horizon: [0.52, 0.60, 0.46],
        sun_dir: Vec3::new(-0.20, -0.94, 0.28).normalize(),
        sun_color: [0.98, 1.04, 0.82],
        ambient_sky: [0.26, 0.34, 0.28],
        ambient_ground: [0.16, 0.19, 0.13],
        grade_warm: [1.02, 1.04, 0.92],
        grade_cool: [0.92, 1.02, 0.96],
        ambience: Ambience::Jungle,
        track: MusicTrack::Tension,
        weather: Weather::Rain,
    }
}

fn env_arctic() -> Env {
    Env {
        fog_color: [0.80, 0.84, 0.90],
        fog_start: 12.0,
        fog_end: 78.0,
        sky_top: [0.56, 0.64, 0.76],
        sky_horizon: [0.82, 0.86, 0.92],
        sun_dir: Vec3::new(0.38, -0.66, 0.65).normalize(),
        sun_color: [0.92, 0.96, 1.08],
        ambient_sky: [0.48, 0.52, 0.60],
        ambient_ground: [0.36, 0.39, 0.44],
        grade_warm: [0.98, 1.00, 1.02],
        grade_cool: [0.90, 0.96, 1.10],
        ambience: Ambience::Blizzard,
        track: MusicTrack::Patrol,
        weather: Weather::Snow,
    }
}

fn env_urban() -> Env {
    Env {
        fog_color: [0.52, 0.52, 0.55],
        fog_start: 24.0,
        fog_end: 106.0,
        sky_top: [0.26, 0.34, 0.48],
        sky_horizon: [0.60, 0.61, 0.63],
        sun_dir: Vec3::new(-0.52, -0.74, 0.42).normalize(),
        sun_color: [1.02, 0.98, 0.92],
        ambient_sky: [0.32, 0.35, 0.42],
        ambient_ground: [0.20, 0.20, 0.20],
        grade_warm: [1.04, 1.00, 0.96],
        grade_cool: [0.94, 0.98, 1.06],
        ambience: Ambience::Urban,
        track: MusicTrack::Assault,
        weather: Weather::None,
    }
}

fn env_shipyard() -> Env {
    Env {
        fog_color: [0.56, 0.60, 0.64],
        fog_start: 26.0,
        fog_end: 112.0,
        sky_top: [0.30, 0.40, 0.54],
        sky_horizon: [0.66, 0.70, 0.74],
        sun_dir: Vec3::new(0.55, -0.72, -0.42).normalize(),
        sun_color: [1.00, 1.00, 0.98],
        ambient_sky: [0.34, 0.38, 0.46],
        ambient_ground: [0.20, 0.22, 0.24],
        grade_warm: [1.02, 1.00, 0.97],
        grade_cool: [0.93, 0.98, 1.08],
        ambience: Ambience::Coastal,
        track: MusicTrack::Patrol,
        weather: Weather::None,
    }
}

fn env_foundry() -> Env {
    Env {
        fog_color: [0.26, 0.23, 0.22],
        fog_start: 12.0,
        fog_end: 70.0,
        sky_top: [0.14, 0.13, 0.14],
        sky_horizon: [0.30, 0.26, 0.24],
        sun_dir: Vec3::new(0.18, -0.95, 0.24).normalize(),
        sun_color: [0.76, 0.72, 0.70],
        ambient_sky: [0.20, 0.19, 0.20],
        ambient_ground: [0.13, 0.11, 0.10],
        grade_warm: [1.10, 0.98, 0.88],
        grade_cool: [0.94, 0.96, 1.02],
        ambience: Ambience::Industrial,
        track: MusicTrack::Assault,
        weather: Weather::Ash,
    }
}

fn env_coastal() -> Env {
    Env {
        fog_color: [0.66, 0.70, 0.72],
        fog_start: 30.0,
        fog_end: 124.0,
        sky_top: [0.32, 0.46, 0.64],
        sky_horizon: [0.72, 0.76, 0.78],
        sun_dir: Vec3::new(-0.60, -0.70, -0.38).normalize(),
        sun_color: [1.08, 1.04, 0.96],
        ambient_sky: [0.38, 0.43, 0.50],
        ambient_ground: [0.24, 0.25, 0.24],
        grade_warm: [1.04, 1.01, 0.95],
        grade_cool: [0.94, 0.99, 1.07],
        ambience: Ambience::Coastal,
        track: MusicTrack::Tension,
        weather: Weather::None,
    }
}

fn env_bunker() -> Env {
    Env {
        fog_color: [0.14, 0.15, 0.16],
        fog_start: 8.0,
        fog_end: 48.0,
        sky_top: [0.06, 0.06, 0.07],
        sky_horizon: [0.10, 0.10, 0.11],
        sun_dir: Vec3::new(0.0, -1.0, 0.05).normalize(),
        sun_color: [0.42, 0.44, 0.48],
        ambient_sky: [0.17, 0.18, 0.21],
        ambient_ground: [0.11, 0.11, 0.12],
        grade_warm: [1.04, 1.00, 0.94],
        grade_cool: [0.90, 0.96, 1.10],
        ambience: Ambience::Interior,
        track: MusicTrack::Tension,
        weather: Weather::None,
    }
}

fn env_railyard() -> Env {
    Env {
        fog_color: [0.48, 0.48, 0.50],
        fog_start: 26.0,
        fog_end: 110.0,
        sky_top: [0.24, 0.30, 0.42],
        sky_horizon: [0.56, 0.57, 0.58],
        sun_dir: Vec3::new(0.62, -0.68, 0.39).normalize(),
        sun_color: [1.00, 0.96, 0.88],
        ambient_sky: [0.30, 0.33, 0.40],
        ambient_ground: [0.20, 0.19, 0.18],
        grade_warm: [1.06, 1.00, 0.92],
        grade_cool: [0.94, 0.98, 1.06],
        ambience: Ambience::RailYard,
        track: MusicTrack::Patrol,
        weather: Weather::None,
    }
}

fn env_overpass() -> Env {
    Env {
        fog_color: [0.44, 0.43, 0.42],
        fog_start: 20.0,
        fog_end: 105.0,
        sky_top: [0.24, 0.28, 0.36],
        sky_horizon: [0.52, 0.50, 0.47],
        sun_dir: Vec3::new(-0.44, -0.82, 0.36).normalize(),
        sun_color: [1.02, 0.96, 0.86],
        ambient_sky: [0.28, 0.30, 0.36],
        ambient_ground: [0.18, 0.17, 0.16],
        grade_warm: [1.08, 1.00, 0.90],
        grade_cool: [0.94, 0.98, 1.05],
        ambience: Ambience::Urban,
        track: MusicTrack::Assault,
        weather: Weather::Dust,
    }
}

// ================================================================ helpers

/// A parked utility truck: solid cab and bed, useful chest-to-head cover.
fn truck(b: &mut MapBuilder, cx: f32, y: f32, cz: f32, along_x: bool, body: Mat) {
    let (sx, sz) = if along_x { (5.6, 2.3) } else { (2.3, 5.6) };
    b.boxc(cx, y + 0.55, cz, sx, 1.4, sz, body).with_scale(2.2);
    b.boxc(cx, y, cz, sx * 0.94, 0.55, sz * 0.94, Mat::Tire).with_scale(1.0);
    // Cab sits at one end and is tall enough to break a sightline.
    let (ox, oz) = if along_x { (sx * 0.32, 0.0) } else { (0.0, sz * 0.32) };
    b.boxc(cx - ox, y + 1.95, cz - oz, if along_x { 1.9 } else { 2.2 }, 1.0,
           if along_x { 2.2 } else { 1.9 }, body).with_scale(1.6);
}

/// A tree: solid trunk, non-colliding canopy that breaks sightlines overhead.
fn tree(b: &mut MapBuilder, cx: f32, y: f32, cz: f32, h: f32, spread: f32) {
    b.boxc(cx, y, cz, 0.62, h, 0.62, Mat::WoodPlank).with_scale(2.0);
    let c = b.decor(cx - spread * 0.5, y + h * 0.62, cz - spread * 0.5, spread, h * 0.5, spread, Mat::Foliage);
    c.flags = BrushFlags::CUTOUT | BrushFlags::NOSHADOW | BrushFlags::NONAV;
    c.tex_scale = 3.0;
}

/// A bush: pure visual concealment, never blocks movement or bullets.
fn bush(b: &mut MapBuilder, cx: f32, y: f32, cz: f32, w: f32, h: f32) {
    let c = b.decor(cx - w * 0.5, y, cz - w * 0.5, w, h, w, Mat::Foliage);
    c.flags = BrushFlags::CUTOUT | BrushFlags::NOSHADOW | BrushFlags::NONAV;
    c.tex_scale = 2.0;
}

/// A cylindrical storage tank, approximated with a chamfered box stack.
fn tank(b: &mut MapBuilder, cx: f32, y: f32, cz: f32, r: f32, h: f32, mat: Mat) {
    b.boxc(cx, y, cz, r * 2.0, h, r * 1.42, mat).with_scale(3.0);
    b.boxc(cx, y, cz, r * 1.42, h, r * 2.0, mat).with_scale(3.0);
}

/// A short flight of steps plus the landing it serves.
fn step_up(b: &mut MapBuilder, x: f32, y: f32, z: f32, w: f32, run: f32, rise: f32, axis: RampAxis, mat: Mat) {
    b.stairs(x, y, z, w, run, rise, axis, mat);
}

/// Standard ammunition and armour placement around a contested point.
fn supply_ring(b: &mut MapBuilder, cx: f32, y: f32, cz: f32, r: f32) {
    b.pickup(cx + r, y, cz, PickupKind::Ammo);
    b.pickup(cx - r, y, cz, PickupKind::Ammo);
}

// ================================================================== IRONVEIL

/// Desert airfield. One long runway lane, two hangar interiors flanking it,
/// and a tower that sees everything but can be flushed from two staircases.
fn ironveil(b: &mut MapBuilder) {
    b.set_env(env_desert());
    b.defaults(Mat::ConcretePanel, Mat::Sand);
    b.playspace(-48.0, -38.0, 48.0, 38.0, 0.0, 34.0);
    b.apron(Mat::Sand, 0.0);

    // Ground: sand apron with an asphalt runway down the middle.
    b.floor(-48.0, -38.0, 96.0, 76.0, 0.0, Mat::Sand);
    b.floor(-48.0, -7.0, 96.0, 14.0, 0.02, Mat::Asphalt);
    b.floor(-48.0, -0.6, 96.0, 1.2, 0.04, Mat::YellowPaint);
    // Taxiways linking the runway to both hangars.
    b.floor(-24.0, -16.0, 8.0, 10.0, 0.02, Mat::Asphalt);
    b.floor(16.0, 6.0, 8.0, 10.0, 0.02, Mat::Asphalt);

    // --- North hangar (Phantom side of the map, open toward the runway).
    let (hx, hz, hw, hd, hh) = (-34.0, -35.0, 24.0, 20.0, 9.0);
    b.floor(hx, hz, hw, hd, 0.05, Mat::ConcreteFloor);
    b.wall_x(hx, hz, hw, 0.0, hh, Mat::Corrugated);
    b.wall_z(hx, hz, hd, 0.0, hh, Mat::Corrugated);
    b.wall_z(hx + hw, hz, hd, 0.0, hh, Mat::Corrugated);
    // Open face: a wide hangar door with piers either side.
    b.wall_x(hx, hz + hd, 4.5, 0.0, hh, Mat::Corrugated);
    b.wall_x(hx + hw - 4.5, hz + hd, 4.5, 0.0, hh, Mat::Corrugated);
    b.wall_x(hx, hz + hd, hw, 6.6, hh - 6.6, Mat::Corrugated);
    b.ceiling(hx, hz, hw, hd, hh, Mat::RoofMetal);
    // Interior: a maintenance mezzanine reached from either end.
    b.catwalk(hx + 0.4, 4.6, hz + 1.0, hw - 0.8, 3.2, Mat::Grating, true, &[hx + 4.0, hx + hw - 4.0]);
    b.access_stair(hx + 4.0, hz + 4.2, 0.05, 4.6, false, false, Mat::MetalPlateDiamond);
    b.access_stair(hx + hw - 4.0, hz + 4.2, 0.05, 4.6, false, false, Mat::MetalPlateDiamond);
    b.crates(hx + 5.0, 0.05, hz + 13.0, 1.3, 2, Mat::WoodCrate);
    b.crates(hx + 18.0, 0.05, hz + 12.0, 1.3, 3, Mat::WoodCrate);
    b.container(hx + 12.0, 0.05, hz + 5.0, true, Mat::ShippingGreen);
    for i in 0..3 { b.barrel(hx + 3.0 + i as f32 * 0.9, 0.05, hz + 17.0, Mat::Barrel); }

    // --- South hangar (Vanguard side, mirrored but not identical).
    let (gx, gz, gw, gd) = (10.0, 15.0, 24.0, 20.0);
    b.floor(gx, gz, gw, gd, 0.05, Mat::ConcreteFloor);
    b.wall_x(gx, gz + gd, gw, 0.0, hh, Mat::Corrugated);
    b.wall_z(gx, gz, gd, 0.0, hh, Mat::Corrugated);
    b.wall_z(gx + gw, gz, gd, 0.0, hh, Mat::Corrugated);
    b.wall_x(gx, gz, 4.5, 0.0, hh, Mat::Corrugated);
    b.wall_x(gx + gw - 4.5, gz, 4.5, 0.0, hh, Mat::Corrugated);
    b.wall_x(gx, gz, gw, 6.6, hh - 6.6, Mat::Corrugated);
    b.ceiling(gx, gz, gw, gd, hh, Mat::RoofMetal);
    b.catwalk(gx + 0.4, 4.6, gz + gd - 4.2, gw - 0.8, 3.2, Mat::Grating, true, &[gx + 4.0, gx + gw - 4.0]);
    b.access_stair(gx + 4.0, gz + gd - 4.2, 0.05, 4.6, false, true, Mat::MetalPlateDiamond);
    b.access_stair(gx + gw - 4.0, gz + gd - 4.2, 0.05, 4.6, false, true, Mat::MetalPlateDiamond);
    b.crates(gx + 6.0, 0.05, gz + 6.0, 1.3, 3, Mat::WoodCrate);
    b.container(gx + 17.0, 0.05, gz + 13.0, false, Mat::ShippingRed);
    b.container(gx + 17.0, 2.65, gz + 13.0, false, Mat::ShippingBlue);

    // --- Control tower: the map's eye, but only two ways up.
    b.tower(-4.0, -34.0, 9.0, 9.0, 0.0, 3, Mat::ConcretePanel, Mat::ConcreteFloor, Mat::ConcreteFloor);
    // Glazed observation band on the top storey, facing the runway.
    b.boxx(-4.0, 3.0 * STOREY - 1.4, -25.4, 9.0, 1.2, 0.2, Mat::Glass)
        .with_flags(BrushFlags::BULLET_CLIP | BrushFlags::CUTOUT);

    // --- Fuel depot: hard cover, tight angles, a flank onto the runway.
    tank(b, 30.0, 0.05, -26.0, 3.4, 7.0, Mat::MetalRust);
    tank(b, 30.0, 0.05, -17.0, 3.4, 7.0, Mat::MetalPanel);
    b.barrier(20.0, 0.05, -32.0, 16.0, 0.6);
    b.barrier(20.0, 0.05, -12.6, 16.0, 0.6);
    b.wall_z(38.0, -32.0, 20.0, 0.0, 3.2, Mat::Cinderblock);
    for i in 0..6 { b.barrel(21.5 + (i % 3) as f32 * 1.0, 0.05, -22.0 + (i / 3) as f32 * 1.1, Mat::BarrelRust); }
    b.floor(19.0, -33.0, 20.0, 22.0, 0.03, Mat::ConcreteFloor);

    // --- Maintenance yard: the mirrored flank, built from containers.
    b.floor(-40.0, 13.0, 22.0, 22.0, 0.03, Mat::ConcreteFloor);
    b.fence_z(-40.0, 0.0, 13.0, 22.0, 3.0);
    b.fence_x(-40.0, 0.0, 35.0, 22.0, 3.0);
    b.container(-34.0, 0.05, 18.0, true, Mat::ShippingBlue);
    b.container(-34.0, 2.65, 18.0, true, Mat::ShippingRed);
    b.container(-24.0, 0.05, 24.0, false, Mat::ShippingGreen);
    b.container(-33.0, 0.05, 30.0, true, Mat::ShippingRed);
    truck(b, -21.0, 0.05, 16.0, true, Mat::Camo);
    b.crates(-29.0, 0.05, 27.0, 1.2, 2, Mat::WoodCrate);

    // --- Mid: a maintenance hall straddling the runway.
    //
    // A ninety-six metre strip of asphalt with nothing on it is one sightline
    // and one fight. This building sits across the middle of it, so the strip
    // becomes two approaches that meet somewhere the whole map cannot see
    // into, and taking it is worth doing.
    b.room(-9.0, -10.0, 18.0, 20.0, 0.05, 6.4, DOOR_ALL, Mat::Corrugated, Mat::ConcreteFloor, false);
    b.ceiling(-9.0, -10.0, 18.0, 20.0, 6.4, Mat::RoofMetal);
    // A mezzanine over the north half, reached from inside and open to the
    // south door, which gives the hall a second storey worth contesting.
    b.catwalk(-8.4, 3.3, -9.2, 16.8, 7.0, Mat::Grating, true, &[-4.0, 3.0]);
    b.access_stair(-4.0, -2.2, 0.05, 3.3, false, false, Mat::MetalPlateDiamond);
    b.access_stair(3.0, -2.2, 0.05, 3.3, false, false, Mat::MetalPlateDiamond);
    b.crates(-5.0, 0.05, 5.0, 1.4, 2, Mat::WoodCrate);
    b.crates(5.5, 0.05, -6.0, 1.3, 1, Mat::WoodCrate);
    b.container(0.0, 0.05, 6.5, true, Mat::ShippingGreen);
    // Window slits either side let the hall watch the strip without owning it.
    b.wall_z_window(-9.0, -4.0, 8.0, 1.2, 5.2, 0.0, 1.9, Mat::Corrugated);
    b.wall_z_window(9.0, -4.0, 8.0, 1.2, 5.2, 0.0, 1.9, Mat::Corrugated);

    // --- Revetment lines: the strip is now four chambers, not one lane.
    //
    // Staggered gaps. Two openings on the same line of sight would restore
    // exactly the sightline the walls exist to remove.
    b.divider(-25.0, 0.0, -15.0, 30.0, 4.6, false, 21.0, Mat::ConcretePanel);
    b.divider(25.0, 0.0, -15.0, 30.0, 4.6, false, 9.0, Mat::ConcretePanel);
    b.half_wall(-26.6, 0.05, 6.0, 5.0, false, Mat::Concrete);
    b.half_wall(26.6, 0.05, -11.0, 5.0, false, Mat::Concrete);

    // --- Flank cover on the two remaining open runs.
    b.cover_line(-42.0, 0.05, -6.0, 16.0, true, 3, Mat::Concrete);
    b.cover_line(28.0, 0.05, 8.0, 16.0, true, 3, Mat::Concrete);
    b.container(-16.0, 0.05, -12.0, false, Mat::ShippingGreen);
    b.container(16.0, 0.05, 12.0, false, Mat::ShippingBlue);
    b.container(-16.0, 2.65, -12.0, false, Mat::ShippingRed);

    // A wrecked transport aircraft: still the landmark, now off the centre
    // line where it breaks the north-west approach instead of standing in the
    // one place the hall already covers.
    b.boxc(-26.0, 0.05, -3.0, 13.0, 3.4, 4.2, Mat::HullPainted).with_scale(3.0);
    b.boxc(-28.0, 3.45, -3.0, 7.5, 1.6, 3.0, Mat::HullPainted).with_scale(3.0);
    b.decor(-32.0, 1.6, -13.0, 3.0, 0.5, 10.0, Mat::HullPainted);
    b.decor(-23.0, 1.6, -1.0, 3.0, 0.5, 10.0, Mat::HullPainted);
    step_up(b, -20.0, 0.05, -5.1, 2.0, 3.0, 3.45, RampAxis::NegX, Mat::MetalPlateDiamond);

    // --- Quadrant walls: the apron either side of the strip was as long a
    //     run as the strip itself, just at ninety degrees to it.
    b.divider(-8.0, 0.0, -20.0, 26.0, 4.4, true, 7.0, Mat::Cinderblock);
    b.divider(-16.0, 0.0, 20.0, 24.0, 4.4, true, 17.0, Mat::Cinderblock);

    // --- Corner outbuildings, so the perimeter is architecture rather than
    //     a painted line on empty ground.
    b.room(-47.0, -31.0, 11.0, 13.0, 0.05, 3.6, DOOR_PX | DOOR_PZ, Mat::Cinderblock, Mat::ConcreteFloor, true);
    b.crates(-43.0, 0.05, -25.0, 1.2, 2, Mat::WoodCrate);
    b.room(36.0, 18.0, 11.0, 13.0, 0.05, 3.6, DOOR_NX | DOOR_NZ, Mat::Cinderblock, Mat::ConcreteFloor, true);
    b.crates(41.0, 0.05, 24.0, 1.2, 2, Mat::WoodCrate);

    // --- Perimeter revetments so the outer edges are not empty ground.
    for i in 0..7 {
        let x = -44.0 + i as f32 * 14.0;
        b.barrier(x, 0.05, -37.0, 8.0, 0.8);
        b.barrier(x, 0.05, 36.2, 8.0, 0.8);
    }

    // --- Spawns.
    b.spawn_cluster(-43.0, 0.05, -20.0, 90.0, Team::Phantom, 8, 5.0, true);
    b.spawn_cluster(-43.0, 0.05, 24.0, 90.0, Team::Phantom, 6, 4.5, false);
    b.spawn_cluster(43.0, 0.05, 20.0, -90.0, Team::Vanguard, 8, 5.0, true);
    b.spawn_cluster(43.0, 0.05, -24.0, -90.0, Team::Vanguard, 6, 4.5, false);
    b.spawn_cluster(0.0, 0.05, -20.0, 180.0, Team::None, 5, 6.0, false);
    b.spawn_cluster(0.0, 0.05, 22.0, 0.0, Team::None, 5, 6.0, false);
    b.spawn_cluster(-26.0, 0.05, 30.0, 45.0, Team::None, 4, 5.0, false);
    b.spawn_cluster(23.0, 0.05, -31.0, -135.0, Team::None, 4, 5.0, false);

    // --- Objectives: a triangle spanning both flanks and the centre.
    b.dom("A", -22.0, 0.1, -25.0, 5.0);
    b.dom("B", 0.0, 0.1, 0.0, 6.0);
    b.dom("C", 22.0, 0.1, 25.0, 5.0);
    b.site("A", 29.0, 0.1, -22.0, 4.5);
    b.site("B", -29.0, 0.1, 24.0, 4.5);
    // Both sites are enclosures with an open face and a door on a different
    // side, so the two approaches arrive on different timings.
    b.site_box(23.0, -28.0, 12.0, 12.0, 0.06, 4.4, DOOR_NX, DOOR_PZ,
               Mat::Cinderblock, Mat::ConcreteFloor, Mat::WoodCrate);
    b.site_box(-35.0, 18.0, 12.0, 12.0, 0.06, 4.4, DOOR_PX, DOOR_NZ,
               Mat::Cinderblock, Mat::ConcreteFloor, Mat::WoodCrate);

    // --- Pickups.
    supply_ring(b, 0.0, 0.15, 0.0, 8.0);
    b.pickup(-22.0, 0.15, -25.0, PickupKind::Armor);
    b.pickup(22.0, 0.15, 25.0, PickupKind::Armor);
    b.pickup(0.0, 3.0 * STOREY + 0.15, -29.5, PickupKind::Weapon);
    b.pickup(-30.0, 0.15, 24.0, PickupKind::Ammo);
    b.pickup(30.0, 0.15, -22.0, PickupKind::Ammo);
    b.pickup(-24.0, 4.75, -32.0, PickupKind::Grenade);
    b.pickup(24.0, 4.75, 32.0, PickupKind::Grenade);

    // Clutter the open ground; see MapBuilder::dress_open_ground.
    b.dress_open_ground(56, 0x1A01, &[
        CoverPiece::Container(Mat::ShippingRed),
        CoverPiece::Container(Mat::ShippingBlue),
        CoverPiece::Crates(Mat::WoodCrate, 1.5),
        CoverPiece::Barrels(Mat::BarrelRust),
        CoverPiece::Sandbags(Mat::Sandbag),
        CoverPiece::Block(Mat::Concrete, 3.4, 1.5),
        CoverPiece::Pipes(Mat::PipeMetal),
    ]);
}

// ================================================================ STORMWORKS

/// Enclosed distribution warehouse. Three usable heights, aisles that all
/// terminate in a decision, and a loading dock that lets a team reset.
fn stormworks(b: &mut MapBuilder) {
    b.set_env(env_industrial());
    b.defaults(Mat::Cinderblock, Mat::ConcreteFloor);
    b.playspace(-36.0, -36.0, 36.0, 36.0, 0.0, 30.0);
    b.apron(Mat::ConcreteFloor, 0.0);

    let h = 13.0;
    b.floor(-36.0, -36.0, 72.0, 72.0, 0.0, Mat::ConcreteFloor);
    b.ceiling(-36.0, -36.0, 72.0, 72.0, h, Mat::RoofMetal);
    // Shell.
    b.wall_x(-36.0, -36.0, 72.0, 0.0, h, Mat::Cinderblock);
    b.wall_x(-36.0, 36.0, 72.0, 0.0, h, Mat::Cinderblock);
    b.wall_z(-36.0, -36.0, 72.0, 0.0, h, Mat::Cinderblock);
    b.wall_z(36.0, -36.0, 72.0, 0.0, h, Mat::Cinderblock);
    // Roof lights: the only illumination, so the aisles read as lanes.
    for i in 0..4 {
        for j in 0..4 {
            b.decor(-30.0 + i as f32 * 18.0, h - 0.4, -30.0 + j as f32 * 18.0, 6.0, 0.3, 6.0, Mat::WindowLit);
        }
    }

    // --- Racking: four aisles running along Z, staggered so no aisle is a
    //     clean shot from end to end.
    for (i, x) in [-24.0f32, -10.0, 4.0, 18.0].iter().enumerate() {
        let gap_at = -18.0 + i as f32 * 12.0;
        let mut z = -33.0;
        while z < 33.0 {
            let len = 9.0;
            if !(z < gap_at + 5.0 && z + len > gap_at - 5.0) {
                b.boxx(*x, 0.0, z, 4.0, 5.4, len, Mat::MetalPanel).with_scale(3.0).with_top(Mat::WoodCrate);
                // Crates on top make the racks climbable cover from above.
                b.boxx(*x + 0.2, 5.4, z + 0.5, 3.6, 1.2, len - 1.0, Mat::WoodCrate).with_scale(1.6);
            }
            z += len + 3.0;
        }
    }

    // --- Perimeter mezzanine at 5.6 m, reached by four staircases.
    let mz = 5.6;
    b.catwalk(-35.0, mz, -35.0, 70.0, 3.0, Mat::Grating, true, &[-8.0, -33.5, 33.5]);
    b.catwalk(-35.0, mz, 32.0, 70.0, 3.0, Mat::Grating, true, &[12.0, -33.5, 33.5]);
    b.catwalk(-35.0, mz, -32.0, 3.0, 64.0, Mat::Grating, false, &[-20.0]);
    b.catwalk(32.0, mz, -32.0, 3.0, 64.0, Mat::Grating, false, &[20.0]);
    b.access_stair(-8.0, -32.0, 0.0, mz, false, false, Mat::MetalPlateDiamond);
    b.access_stair(12.0, 32.0, 0.0, mz, false, true, Mat::MetalPlateDiamond);
    b.access_stair(-20.0, -32.0, 0.0, mz, true, false, Mat::MetalPlateDiamond);
    b.access_stair(20.0, 32.0, 0.0, mz, true, true, Mat::MetalPlateDiamond);

    // --- Upper crane catwalk at 9.8 m: crosses the hall, exposed both ways.
    let uz = 9.8;
    // Stopped short of the walls so both ends are open landings, reached by a
    // staircase that starts on the mezzanine below.
    b.catwalk(-28.0, uz, -1.5, 56.0, 3.0, Mat::Grating, true, &[0.0]);
    b.catwalk(-1.5, uz, -28.0, 3.0, 56.0, Mat::Grating, false, &[0.0]);
    b.access_stair(0.0, -28.0, mz, uz, true, true, Mat::MetalPlateDiamond);
    b.access_stair(0.0, -28.0, mz, uz, false, true, Mat::MetalPlateDiamond);

    // --- Loading dock (north-west): trucks, a ramp, and a way onto the racks.
    b.floor(-34.0, -34.0, 16.0, 14.0, 1.2, Mat::ConcreteFloor);
    b.ramp(-34.0, 0.0, -19.0, 16.0, 1.2, 3.0, RampAxis::NegZ, Mat::ConcreteFloor);
    truck(b, -28.0, 1.25, -27.0, false, Mat::MetalPanel);
    b.crates(-22.0, 1.25, -31.0, 1.4, 3, Mat::WoodCrate);
    b.crates(-31.0, 1.25, -22.0, 1.4, 2, Mat::WoodCrate);

    // --- Offices (south-east): two small interiors and a roof to fight on.
    b.room(18.0, 20.0, 16.0, 14.0, 0.0, 3.4, DOOR_NX | DOOR_NZ, Mat::Plaster, Mat::TileFloor, false);
    b.wall_z_door(26.0, 20.0, 14.0, 0.0, 3.4, 7.0, Mat::Plaster);
    b.floor(18.0, 20.0, 16.0, 14.0, 3.4, Mat::MetalPanel);
    b.access_stair(24.0, 18.0, 0.0, 3.4, true, true, Mat::MetalPlateDiamond);
    b.decor(19.0, 0.0, 22.0, 2.4, 0.8, 1.2, Mat::ControlPanel);
    b.decor(19.0, 0.8, 22.2, 2.0, 1.1, 0.2, Mat::Screen);

    // --- Machine room (south-west): the third objective, deliberately awkward.
    b.room(-32.0, 18.0, 18.0, 16.0, 0.0, 5.0, DOOR_PX | DOOR_NZ, Mat::Cinderblock, Mat::ConcreteFloor, true);
    tank(b, -26.0, 0.0, 26.0, 2.2, 4.2, Mat::MetalRust);
    tank(b, -19.0, 0.0, 22.0, 2.2, 4.2, Mat::PipeMetal);
    for i in 0..4 { b.barrel(-29.0 + i as f32 * 1.1, 0.0, 31.0, Mat::BarrelRust); }
    b.decor(-32.0, 4.2, 18.0, 18.0, 0.4, 16.0, Mat::Duct);

    // --- Cross racking.
    //
    // The racks all ran the same way, which made every aisle a lane and every
    // gap between aisles a seventy-two metre shot across the hall. These runs
    // sit across the aisles at staggered depths, so moving down the warehouse
    // is a series of short decisions and no two aisles are open at once.
    for (i, z) in [-26.0f32, -8.0, 10.0, 28.0].iter().enumerate() {
        let start = if i % 2 == 0 { -32.0 } else { -20.0 };
        let mut x = start;
        while x < 32.0 {
            b.boxx(x, 0.0, *z, 8.0, 2.9, 3.6, Mat::MetalPanel).with_scale(2.6).with_top(Mat::WoodCrate);
            x += 20.0;
        }
    }

    // --- Mid: a boxed-in floor the whole hall can reach and nobody can see
    //     across. Four container walls with the corners left open, which
    //     makes the middle a room with four doors instead of a crossroads.
    b.container(-6.5, 0.0, -7.0, true, Mat::ShippingRed);
    b.container(6.5, 0.0, -7.0, true, Mat::ShippingBlue);
    b.container(-6.5, 0.0, 7.0, true, Mat::ShippingGreen);
    b.container(6.5, 0.0, 7.0, true, Mat::ShippingRed);
    b.container(-8.5, 0.0, 0.0, false, Mat::ShippingBlue);
    b.container(8.5, 0.0, 0.0, false, Mat::ShippingGreen);
    b.container(-6.5, 2.6, -7.0, true, Mat::ShippingGreen);
    b.container(6.5, 2.6, 7.0, true, Mat::ShippingBlue);
    step_up(b, -11.0, 0.0, -1.1, 2.2, 2.6, 2.6, RampAxis::PosX, Mat::MetalPlateDiamond);

    // --- Floor cover in the open lanes.
    for (x, z) in [(-2.0f32, -20.0f32), (10.0, -8.0), (-14.0, 8.0), (24.0, -14.0), (-24.0, -4.0), (12.0, 30.0)] {
        b.crates(x, 0.0, z, 1.4, 2, Mat::WoodCrate);
    }
    b.cover_line(-34.0, 0.0, -30.0, 26.0, false, 3, Mat::Concrete);
    b.cover_line(34.0, 0.0, 6.0, 26.0, false, 3, Mat::Concrete);

    b.spawn_cluster(-30.0, 1.3, -30.0, 45.0, Team::Phantom, 8, 4.5, true);
    b.spawn_cluster(-30.0, 0.0, 4.0, 0.0, Team::Phantom, 6, 4.5, false);
    b.spawn_cluster(29.0, 0.0, 14.0, -120.0, Team::Vanguard, 8, 4.5, true);
    b.spawn_cluster(29.0, 0.0, -14.0, -150.0, Team::Vanguard, 6, 4.5, false);
    b.spawn_cluster(0.0, 0.0, -28.0, 180.0, Team::None, 5, 5.0, false);
    b.spawn_cluster(0.0, 0.0, 28.0, 0.0, Team::None, 5, 5.0, false);
    b.spawn_cluster(-33.5, 5.7, 20.0, -90.0, Team::None, 3, 2.4, false);

    b.dom("A", -26.0, 1.3, -27.0, 5.5);
    b.dom("B", 0.0, 0.1, 0.0, 6.0);
    b.dom("C", 25.0, 0.1, 26.0, 5.0);
    b.site("A", -23.0, 0.1, 26.0, 4.5);
    b.site("B", 25.0, 0.1, 26.0, 4.5);

    supply_ring(b, 0.0, 0.1, 0.0, 7.0);
    b.pickup(-26.0, 1.35, -27.0, PickupKind::Ammo);
    b.pickup(0.0, 9.9, 0.0, PickupKind::Weapon);
    b.pickup(-33.0, 5.7, 0.0, PickupKind::Armor);
    b.pickup(33.0, 5.7, 0.0, PickupKind::Armor);
    b.pickup(25.0, 3.5, 26.0, PickupKind::Grenade);
    b.pickup(-23.0, 0.1, 26.0, PickupKind::Health);

    // Clutter the open ground; see MapBuilder::dress_open_ground.
    b.dress_open_ground(8, 0x1A0B, &[
        CoverPiece::Crates(Mat::WoodCrate, 1.4),
        CoverPiece::Barrels(Mat::BarrelRust),
        CoverPiece::Pipes(Mat::PipeMetal),
    ]);
}

// =================================================================== BELVOIR

/// Shelled market town: a square you must cross, roofs that watch it, and a
/// sewer that lets you refuse to.
fn belvoir(b: &mut MapBuilder) {
    b.set_env(env_village());
    b.defaults(Mat::Plaster, Mat::Cobble);
    b.playspace(-42.0, -38.0, 42.0, 38.0, -6.0, 34.0);
    b.apron(Mat::Cobble, 0.0);

    // The three sewer shafts are cut out of the ground slab itself; adding
    // holed slabs on top of a solid one just paves them over.
    let shafts = [
        (-4.4f32, -34.0f32, 3.0f32, 8.4f32),
        (1.2, 25.8, 3.0, 8.4),
        (11.6, -4.2, 2.8, 7.4),
    ];
    b.floor_with_holes(-42.0, -38.0, 84.0, 76.0, 0.0, Mat::Cobble,
                       &shafts.iter().map(|h| (h.0, h.1, h.2, h.3)).collect::<Vec<_>>());
    // The square itself, in a different stone so it reads as the centre.
    b.floor(-13.0, -11.0, 26.0, 22.0, 0.02, Mat::Marble);

    // --- Church: the tallest thing on the map, with a bell tower.
    b.room(-34.0, -14.0, 14.0, 22.0, 0.0, 7.0, DOOR_PX | DOOR_NZ, Mat::StoneWall, Mat::Marble, true);
    b.tower(-32.0, -30.0, 8.0, 8.0, 0.0, 3, Mat::StoneWall, Mat::Marble, Mat::RoofTile);
    // Belfry openings so the tower is shootable from the square.
    b.boxx(-32.0, 3.0 * STOREY, -30.2, 8.0, 0.2, 0.3, Mat::StoneWall);
    b.decor(-30.0, 7.0, -13.0, 10.0, 3.0, 22.0, Mat::RoofTile);

    // --- Market row along the north side: shopfronts with a shared roof walk.
    for i in 0..3 {
        let x = -8.0 + i as f32 * 13.0;
        b.tower(x, -34.0, 11.0, 12.0, 0.0, 2, Mat::BrickPale, Mat::WoodFloor, Mat::RoofTile);
    }
    // Roof plank bridges: risky but fast lateral movement up top.
    b.boxx(3.0, 2.0 * STOREY, -29.0, 2.0, 0.3, 3.0, Mat::WoodPlank).with_scale(1.5);
    b.boxx(16.0, 2.0 * STOREY, -29.0, 2.0, 0.3, 3.0, Mat::WoodPlank).with_scale(1.5);

    // --- South residential block.
    b.tower(-24.0, 14.0, 13.0, 14.0, 0.0, 2, Mat::Plaster, Mat::WoodFloor, Mat::RoofTile);
    b.tower(4.0, 16.0, 14.0, 15.0, 0.0, 2, Mat::BrickRed, Mat::WoodFloor, Mat::RoofTile);
    b.tower(24.0, 12.0, 12.0, 12.0, 0.0, 2, Mat::Plaster, Mat::WoodFloor, Mat::RoofTile);

    // --- East warehouse: the flank route, mostly interior.
    b.room(26.0, -22.0, 15.0, 18.0, 0.0, 6.0, DOOR_NX | DOOR_PZ, Mat::BrickRed, Mat::ConcreteFloor, true);
    b.catwalk(27.0, 3.4, -21.0, 13.0, 3.0, Mat::WoodPlank, true, &[33.0]);
    b.access_stair(33.0, -18.0, 0.0, 3.4, false, false, Mat::WoodPlank);
    b.crates(31.0, 0.0, -8.0, 1.3, 2, Mat::WoodCrate);

    // --- Sewer: a genuine third dimension downward, two entrances per side.
    let sy = -4.6;
    b.floor(-6.0, -34.0, 12.0, 68.0, sy, Mat::Cobble);
    b.wall_z(-6.0, -34.0, 68.0, sy, 3.2, Mat::StoneWall);
    b.wall_z(6.0, -34.0, 68.0, sy, 3.2, Mat::StoneWall);
    // Ceiling only between the stairwells, which are open shafts to the street.
    b.ceiling(-6.0, -25.6, 12.0, 51.4, sy + 3.2, Mat::StoneWall);
    // Side chamber under the square, so the tunnel is not a bare corridor.
    b.floor(6.0, -6.0, 10.0, 12.0, sy, Mat::Cobble);
    b.wall_x(6.0, -6.0, 10.0, sy, 3.2, Mat::StoneWall);
    b.wall_x(6.0, 6.0, 10.0, sy, 3.2, Mat::StoneWall);
    b.wall_z(16.0, -6.0, 12.0, sy, 3.2, Mat::StoneWall);
    b.ceiling(6.0, -6.0, 5.6, 12.0, sy + 3.2, Mat::StoneWall);
    b.ceiling(14.4, -6.0, 1.6, 12.0, sy + 3.2, Mat::StoneWall);
    b.pillar(11.0, sy, 0.0, 1.4, 3.2, Mat::StoneWall);
    // Stair entrances: north end, south end and a mid shaft into the square.
    b.stairs(-4.2, sy, -33.8, 2.6, 8.0, 4.6, RampAxis::NegZ, Mat::Cobble);
    b.stairs(1.4, sy, 26.0, 2.6, 8.0, 4.6, RampAxis::PosZ, Mat::Cobble);
    b.stairs(11.8, sy, -4.0, 2.4, 7.0, 4.6, RampAxis::PosZ, Mat::Cobble);

    // --- Sewer bulkheads: sixty-eight metres of straight tunnel is a shot
    //     from one stairwell to the other, which is not what a flank route is
    //     for. Two bulkheads with offset doors and a run of pillars.
    b.wall_x_door(-6.0, -16.0, 12.0, sy, 3.2, 3.4, Mat::StoneWall);
    b.wall_x_door(-6.0, 14.0, 12.0, sy, 3.2, 8.6, Mat::StoneWall);
    for z in [-27.0f32, -6.0, 8.0, 21.0] {
        b.pillar(-2.6, sy, z, 1.1, 3.2, Mat::StoneWall);
        b.pillar(2.6, sy, z + 4.0, 1.1, 3.2, Mat::StoneWall);
    }

    // --- Perimeter alleys. Every block stops short of the playspace edge, so
    //     the ring road around the town was the longest lane on the map in
    //     both directions. Rubble walls close it into segments.
    for x in [-30.0f32, -14.0, 2.0, 20.0, 34.0] {
        b.divider(x, 0.0, -38.5, 5.0, 4.0, false, 2.5, Mat::StoneWall);
        b.divider(x - 6.0, 0.0, 33.5, 5.0, 4.0, false, 2.5, Mat::StoneWall);
    }
    for z in [-28.0f32, -6.0, 14.0, 30.0] {
        b.divider(-39.5, 0.0, z, 5.0, 4.0, true, 2.5, Mat::StoneWall);
        b.divider(38.0, 0.0, z - 8.0, 5.0, 4.0, true, 2.5, Mat::StoneWall);
    }

    // --- Square furniture: a fountain, stalls, a burnt-out truck.
    b.boxc(0.0, 0.0, 0.0, 5.0, 1.0, 5.0, Mat::Marble).with_scale(2.0);
    b.boxc(0.0, 1.0, 0.0, 1.4, 2.2, 1.4, Mat::Marble).with_scale(1.2);
    for (x, z, ax) in [(-9.0f32, -7.0f32, true), (9.0, 6.0, true), (-9.0, 6.0, false), (9.0, -7.0, false)] {
        let (sx, sz) = if ax { (4.4, 1.6) } else { (1.6, 4.4) };
        b.boxc(x, 0.0, z, sx, 1.05, sz, Mat::WoodPlank).with_scale(1.4);
        b.decor(x - sx * 0.6, 2.1, z - sz * 0.6, sx * 1.2, 0.3, sz * 1.2, Mat::Canvas);
    }
    truck(b, -16.0, 0.0, 4.0, true, Mat::MetalRust);
    b.barrier(14.0, 0.0, -14.0, 6.0, 0.6);
    b.barrier(-20.0, 0.0, -18.0, 0.6, 6.0);

    // --- Covered market hall over the north half of the square.
    //
    // A twenty-six by twenty-two metre paved square with a fountain in it is a
    // place nobody crosses twice. Roofing half of it on columns leaves the
    // square readable from the edges and impossible to hold from any one of
    // them, which is what a centre is supposed to be.
    for i in 0..4 {
        let x = -10.0 + i as f32 * 6.4;
        b.pillar(x, 0.0, -9.0, 0.9, 4.2, Mat::StoneWall);
        b.pillar(x, 0.0, -1.0, 0.9, 4.2, Mat::StoneWall);
    }
    b.ceiling(-12.0, -10.0, 24.0, 10.0, 4.2, Mat::RoofTile);
    b.half_wall(-12.0, 0.0, -5.4, 9.0, true, Mat::StoneWall);
    b.half_wall(3.0, 0.0, -5.4, 9.0, true, Mat::StoneWall);

    // --- Terraces closing the west and east streets, which ran the full
    //     seventy-six metres of the map with nothing in them.
    b.tower(-41.0, -34.0, 8.0, 13.0, 0.0, 2, Mat::BrickPale, Mat::WoodFloor, Mat::RoofTile);
    b.tower(-41.0, 18.0, 8.0, 13.0, 0.0, 2, Mat::Plaster, Mat::WoodFloor, Mat::RoofTile);
    b.tower(33.0, 22.0, 9.0, 13.0, 0.0, 2, Mat::BrickRed, Mat::WoodFloor, Mat::RoofTile);
    b.divider(-42.0, 0.0, 6.0, 12.0, 4.0, true, 4.0, Mat::StoneWall);
    b.divider(30.0, 0.0, 6.0, 12.0, 4.0, true, 8.0, Mat::StoneWall);
    b.divider(-16.0, 0.0, 32.0, 26.0, 4.0, true, 18.0, Mat::StoneWall);
    b.divider(-8.0, 0.0, -37.0, 22.0, 4.0, true, 6.0, Mat::StoneWall);

    // --- Alleys: low walls and rubble so the streets are not bare.
    for (x, z, sx, sz) in [(-30.0f32, 6.0f32, 12.0, 0.8), (18.0, -30.0, 0.8, 10.0), (-4.0, 22.0, 10.0, 0.8)] {
        b.boxx(x, 0.0, z, sx, 1.3, sz, Mat::StoneWall).with_scale(2.0);
    }
    b.cover_line(-40.0, 0.0, -14.0, 18.0, false, 3, Mat::StoneWall);
    b.cover_line(38.0, 0.0, -8.0, 18.0, false, 3, Mat::StoneWall);

    // --- Cross walls in the two streets that ran the width of the town.
    //
    // The blocks are laid out in rows, which leaves a continuous gap between
    // each row: an eighty-metre lane at z of about minus eighteen and another
    // at plus eleven, either of which could be held from one end. These are
    // the garden walls between the properties, each with one way through.
    for (x, z, len, door) in [
        (-18.0f32, -24.0f32, 11.0f32, 7.0f32),
        (12.0, -22.0, 9.0, 3.0),
        (24.0, -34.0, 11.0, 4.0),
        (-28.0, 7.0, 8.0, 5.0),
        (-6.0, 6.0, 9.0, 3.0),
        (21.0, 7.0, 8.0, 5.0),
        (-12.0, 30.0, 9.0, 6.0),
        (14.0, 30.0, 9.0, 3.0),
        (30.0, -4.0, 9.0, 6.0),
    ] {
        b.divider(x, 0.0, z, len, 4.0, false, door, Mat::StoneWall);
    }
    for (x, z) in [(-22.0f32, -22.0f32), (20.0, 18.0), (-12.0, 30.0), (32.0, 4.0)] {
        b.crates(x, 0.0, z, 1.2, 2, Mat::WoodCrate);
    }

    b.spawn_cluster(-36.0, 0.0, 30.0, 30.0, Team::Phantom, 8, 4.5, true);
    b.spawn_cluster(-38.0, 0.0, 0.0, 90.0, Team::Phantom, 5, 4.0, false);
    b.spawn_cluster(36.0, 0.0, -32.0, -150.0, Team::Vanguard, 8, 4.5, true);
    b.spawn_cluster(38.0, 0.0, 22.0, -90.0, Team::Vanguard, 5, 4.0, false);
    b.spawn_cluster(0.0, 0.0, -24.0, 180.0, Team::None, 5, 5.0, false);
    b.spawn_cluster(-30.0, 0.0, 30.0, 0.0, Team::None, 4, 4.0, false);
    b.spawn_cluster(24.0, 0.0, 30.0, 0.0, Team::None, 4, 4.0, false);

    b.dom("A", -27.0, 0.1, -3.0, 5.5);
    b.dom("B", 0.0, 0.1, -4.0, 6.5);
    b.dom("C", 33.0, 0.1, -13.0, 5.0);
    b.site("A", 0.0, 0.1, -4.0, 5.5);
    b.site("B", 33.0, 0.1, -13.0, 4.5);

    b.pickup(0.0, 0.15, -8.0, PickupKind::Ammo);
    b.pickup(0.0, 0.15, 8.0, PickupKind::Ammo);
    b.pickup(-28.0, 3.0 * STOREY + 0.1, -26.0, PickupKind::Weapon);
    b.pickup(0.0, sy + 0.1, 0.0, PickupKind::Armor);
    b.pickup(-27.0, 0.15, -3.0, PickupKind::Health);
    b.pickup(33.0, 0.15, -13.0, PickupKind::Grenade);
    b.pickup(10.0, 2.0 * STOREY + 0.1, -28.0, PickupKind::Grenade);

    // Clutter the open ground; see MapBuilder::dress_open_ground.
    b.dress_open_ground(46, 0x1A02, &[
        CoverPiece::Crates(Mat::WoodCrate, 1.3),
        CoverPiece::Barrels(Mat::Barrel),
        CoverPiece::Planter(Mat::StoneWall, Mat::Foliage),
        CoverPiece::Block(Mat::StoneWall, 3.0, 1.3),
        CoverPiece::Sandbags(Mat::Sandbag),
    ]);
}

// ================================================================= GREENLINE

/// Jungle relay station. Sightlines are short and organic; the walkways and
/// the dish pads are the only places you can actually see across the map.
fn greenline(b: &mut MapBuilder) {
    b.set_env(env_jungle());
    b.defaults(Mat::ConcretePanel, Mat::JungleFloor);
    b.playspace(-40.0, -40.0, 40.0, 40.0, -3.0, 32.0);
    b.apron(Mat::JungleFloor, 0.0);

    b.floor(-40.0, -40.0, 80.0, 80.0, 0.0, Mat::JungleFloor);

    // --- A stream cut running east-west: a lower lane with limited exits.
    b.floor(-40.0, -4.0, 80.0, 8.0, -2.2, Mat::Mud);
    b.wall_x(-40.0, -4.0, 80.0, -2.2, 2.2, Mat::Dirt);
    b.wall_x(-40.0, 4.0, 80.0, -2.2, 2.2, Mat::Dirt);
    for x in [-30.0f32, -8.0, 14.0, 32.0] {
        b.ramp(x, -2.2, -4.0, 5.0, 2.2, 4.0, RampAxis::NegZ, Mat::Dirt);
        b.ramp(x, -2.2, 0.0, 5.0, 2.2, 4.0, RampAxis::PosZ, Mat::Dirt);
    }
    // Log bridges over the cut keep the north-south routes alive.
    for x in [-20.0f32, 4.0, 24.0] {
        b.boxc(x, 0.0, 0.0, 2.4, 0.4, 9.0, Mat::WoodPlank).with_scale(2.0);
    }

    // --- Central relay building: two storeys plus a dish on the roof.
    b.room(-9.0, -12.0, 18.0, 16.0, 0.0, 3.4, DOOR_NX | DOOR_PX | DOOR_NZ, Mat::ConcretePanel, Mat::ConcreteFloor, false);
    // Two flights in opposite corners take you ground -> first floor -> roof.
    b.floor_with_hole(-9.0, -12.0, 18.0, 16.0, 3.4, Mat::ConcreteFloor, -8.7, -11.9, 3.2, 5.4);
    b.access_stair(-7.2, -6.8, 0.0, 3.4, false, true, Mat::MetalPlateDiamond);
    b.wall_x_window(-9.0, -12.0, 18.0, 3.4, 3.0, 1.0, 2.3, Mat::ConcretePanel);
    b.wall_x_window(-9.0, 4.0, 18.0, 3.4, 3.0, 1.0, 2.3, Mat::ConcretePanel);
    b.wall_z_window(-9.0, -12.0, 16.0, 3.4, 3.0, 1.0, 2.3, Mat::ConcretePanel);
    b.wall_z_window(9.0, -12.0, 16.0, 3.4, 3.0, 1.0, 2.3, Mat::ConcretePanel);
    b.floor_with_hole(-9.0, -12.0, 18.0, 16.0, 6.4, Mat::RoofMetal, 5.5, -11.9, 3.2, 5.4);
    b.access_stair(7.2, -6.8, 3.4, 6.4, false, true, Mat::MetalPlateDiamond);
    b.decor(-3.0, 6.4, -6.0, 6.0, 0.6, 6.0, Mat::MetalPanel);
    b.decor(-2.0, 7.0, -5.0, 4.0, 4.0, 0.6, Mat::MetalPanel);
    b.decor(-1.0, 0.0, -10.0, 3.0, 1.2, 1.0, Mat::ControlPanel);
    b.decor(-1.0, 1.2, -9.8, 2.6, 1.2, 0.2, Mat::Screen);

    // --- Two dish pads: raised concrete platforms with ramps.
    for (px, pz, ax) in [(-30.0f32, -26.0f32, RampAxis::NegX), (28.0, 24.0, RampAxis::PosX)] {
        b.floor(px - 7.0, pz - 7.0, 14.0, 14.0, 2.4, Mat::ConcreteFloor);
        b.wall_x(px - 7.0, pz - 7.0, 14.0, 2.4, 1.1, Mat::ConcretePanel);
        b.wall_x(px - 7.0, pz + 7.0, 14.0, 2.4, 1.1, Mat::ConcretePanel);
        // The parapet is broken where the ramp arrives, otherwise the pad is
        // a sealed box you can see onto but never reach.
        let (rx, rz, gap_x) = match ax {
            RampAxis::PosX => (px - 12.0, pz - 2.0, px - 7.0),
            _ => (px + 7.0, pz - 2.0, px + 7.0),
        };
        for wx in [px - 7.0, px + 7.0] {
            if (wx - gap_x).abs() < 0.01 {
                b.wall_z(wx, pz - 7.0, 5.0, 2.4, 1.1, Mat::ConcretePanel);
                b.wall_z(wx, pz + 2.0, 5.0, 2.4, 1.1, Mat::ConcretePanel);
            } else {
                b.wall_z(wx, pz - 7.0, 14.0, 2.4, 1.1, Mat::ConcretePanel);
            }
        }
        b.ramp(rx, 0.0, rz, 5.0, 2.4, 4.0, ax, Mat::ConcreteFloor);
        // Dish assembly: a big silhouette and hard cover on the pad.
        b.boxc(px, 2.4, pz, 2.0, 3.0, 2.0, Mat::MetalPanel).with_scale(2.0);
        b.boxc(px, 5.4, pz, 7.0, 1.2, 7.0, Mat::MetalPanel).with_scale(4.0);
        b.decor(px - 4.0, 6.6, pz - 4.0, 8.0, 2.0, 8.0, Mat::MetalPanel);
    }

    // --- Elevated timber walkways through the canopy, at 4.5 m.
    let wy = 4.5;
    b.catwalk(-34.0, wy, 12.0, 30.0, 2.6, Mat::WoodPlank, true, &[-31.0, -4.7]);
    b.catwalk(-6.0, wy, 12.0, 2.6, 22.0, Mat::WoodPlank, false, &[13.3]);
    b.catwalk(10.0, wy, -30.0, 24.0, 2.6, Mat::WoodPlank, true, &[11.3]);
    b.catwalk(10.0, wy, -30.0, 2.6, 20.0, Mat::WoodPlank, false, &[-28.7]);
    b.access_stair(-31.0, 12.0, 0.0, wy, false, true, Mat::WoodPlank);
    b.access_stair(-4.7, 34.0, 0.0, wy, false, false, Mat::WoodPlank);
    b.access_stair(-28.7, 34.0, 0.0, wy, true, false, Mat::WoodPlank);
    b.access_stair(11.3, -10.0, 0.0, wy, false, false, Mat::WoodPlank);

    // --- Bunker at the south-west, a hard point with two mouths.
    b.room(-36.0, 24.0, 14.0, 12.0, 0.0, 3.0, DOOR_PX | DOOR_NZ, Mat::Bunker, Mat::ConcreteFloor, true);
    b.wall_x_window(-36.0, 24.0, 14.0, 0.0, 3.0, 1.1, 1.9, Mat::Bunker);
    b.sandbags(-38.0, 0.0, 21.0, 18.0, 1.2);
    b.pickup(-29.0, 0.1, 30.0, PickupKind::Ammo);

    // --- Canopy and undergrowth. Trunks are solid, leaves are not.
    let trees: [(f32, f32); 26] = [
        (-34.0, -34.0), (-24.0, -30.0), (-14.0, -34.0), (2.0, -34.0), (16.0, -36.0), (30.0, -34.0),
        (-36.0, -16.0), (-22.0, -18.0), (20.0, -18.0), (34.0, -14.0), (-34.0, 8.0), (-20.0, 10.0),
        (14.0, 10.0), (30.0, 8.0), (-32.0, 34.0), (-16.0, 30.0), (0.0, 34.0), (14.0, 32.0), (30.0, 34.0),
        (-8.0, 22.0), (8.0, 24.0), (22.0, -8.0), (-26.0, -8.0), (36.0, 20.0), (-38.0, -26.0), (36.0, -30.0),
    ];
    for (i, (x, z)) in trees.iter().enumerate() {
        tree(b, *x, 0.0, *z, 7.0 + (i % 4) as f32 * 1.6, 6.0 + (i % 3) as f32 * 1.5);
    }
    for i in 0..34 {
        let a = i as f32 * 2.399_963;
        let r = 34.0 * ((i as f32 + 0.5) / 34.0).sqrt();
        let (x, z) = (a.cos() * r, a.sin() * r);
        if x.abs() < 12.0 && z.abs() < 14.0 { continue; }
        bush(b, x, 0.0, z, 2.4 + (i % 3) as f32 * 0.8, 1.5);
    }
    // Hard cover so the fights are not purely about concealment.
    for (x, z) in [(-18.0f32, -22.0f32), (18.0, 16.0), (-12.0, 26.0), (24.0, -22.0), (-30.0, 0.0), (30.0, 2.0)] {
        b.crates(x, 0.0, z, 1.3, 2, Mat::WoodCrate);
    }
    for (x, z) in [(-6.0f32, -20.0f32), (6.0, 18.0), (-24.0, 16.0), (22.0, -12.0)] {
        b.barrier(x, 0.0, z, 5.0, 0.7);
    }

    b.spawn_cluster(-34.0, 0.0, 32.0, 30.0, Team::Phantom, 8, 4.5, true);
    b.spawn_cluster(-36.0, 0.0, -6.0, 60.0, Team::Phantom, 5, 4.0, false);
    b.spawn_cluster(34.0, 0.0, -32.0, -150.0, Team::Vanguard, 8, 4.5, true);
    b.spawn_cluster(36.0, 0.0, 6.0, -120.0, Team::Vanguard, 5, 4.0, false);
    b.spawn_cluster(-2.0, 0.0, -30.0, 180.0, Team::None, 5, 5.0, false);
    b.spawn_cluster(2.0, 0.0, 30.0, 0.0, Team::None, 5, 5.0, false);

    b.dom("A", -30.0, 2.5, -22.0, 6.0);
    b.dom("B", 0.0, 0.1, -4.0, 6.0);
    b.dom("C", 28.0, 2.5, 20.0, 6.0);
    b.site("A", 0.0, 0.1, -4.0, 5.0);
    b.site("B", -29.0, 0.1, 30.0, 4.5);

    b.pickup(0.0, 6.5, -4.0, PickupKind::Weapon);
    b.pickup(-33.0, 2.5, -22.0, PickupKind::Armor);
    b.pickup(31.0, 2.5, 20.0, PickupKind::Armor);
    b.pickup(-20.0, -2.1, 0.0, PickupKind::Ammo);
    b.pickup(20.0, -2.1, 0.0, PickupKind::Ammo);
    b.pickup(-6.0, 4.6, 22.0, PickupKind::Grenade);
    b.pickup(11.0, 4.6, -22.0, PickupKind::Health);

    // Clutter the open ground; see MapBuilder::dress_open_ground.
    b.dress_open_ground(72, 0x1A03, &[
        CoverPiece::Crates(Mat::WoodCrate, 1.4),
        CoverPiece::Barrels(Mat::BarrelRust),
        CoverPiece::Planter(Mat::Rock, Mat::Foliage),
        CoverPiece::Block(Mat::Rock, 3.2, 1.4),
        CoverPiece::Sandbags(Mat::Sandbag),
        CoverPiece::Pipes(Mat::PipeMetal),
    ]);
}

// ================================================================== WHITEOUT

/// Arctic radar site. Short fog and blowing snow make the mast the only place
/// with real information, so everything else is fought at closing range.
fn whiteout(b: &mut MapBuilder) {
    b.set_env(env_arctic());
    b.defaults(Mat::CamoWinter, Mat::Snow);
    b.playspace(-44.0, -36.0, 44.0, 36.0, -5.0, 40.0);
    b.apron(Mat::Snow, 0.0);

    // Ground, with the three service-tunnel shafts cut out of it.
    let shafts = [(-30.4f32, -1.0f32, 5.9f32, 3.0f32), (-0.4, -1.0, 5.9, 3.0), (14.5, -1.0, 5.9, 3.0)];
    b.floor_with_holes(-44.0, -36.0, 88.0, 72.0, 0.0, Mat::Snow,
                       &shafts.iter().map(|h| (h.0, h.1, h.2, h.3)).collect::<Vec<_>>());
    // Frozen pond: flat, open, and the fastest way across the middle.
    b.floor(-11.0, 8.0, 22.0, 18.0, 0.01, Mat::Ice);

    // --- Snow berms: the map's primary cover, plough-heaped into long ridges.
    let berms: [(f32, f32, f32, f32); 12] = [
        (-40.0, -22.0, 16.0, 1.6), (-18.0, -28.0, 1.6, 12.0), (4.0, -24.0, 14.0, 1.6),
        (26.0, -30.0, 1.6, 14.0), (-38.0, 4.0, 1.6, 14.0), (-14.0, -6.0, 12.0, 1.6),
        (14.0, -8.0, 1.6, 12.0), (30.0, 2.0, 12.0, 1.6), (-30.0, 24.0, 14.0, 1.6),
        (-4.0, 30.0, 1.6, 10.0), (18.0, 26.0, 12.0, 1.6), (36.0, 18.0, 1.6, 12.0),
    ];
    for (x, z, sx, sz) in berms {
        b.boxx(x, 0.0, z, sx, 1.35, sz, Mat::Snow).with_scale(3.0).with_top(Mat::Snow);
    }

    // --- Radar mast: four storeys, the map's power position, two ways up.
    b.tower(-4.0, -6.0, 8.0, 8.0, 0.0, 4, Mat::MetalPanel, Mat::MetalPlateDiamond, Mat::MetalPlateDiamond);
    // An external gantry rings the mast at the second storey and is reached by
    // one long, exposed staircase. Holding the internal stair is never enough
    // on its own, but taking the mast from outside costs you the approach.
    b.catwalk(4.0, 6.8, -7.0, 2.8, 10.0, Mat::Grating, false, &[-2.0]);
    b.access_stair(5.4, 3.0, 0.0, 6.8, false, false, Mat::MetalPlateDiamond);
    // Rotating array on top: a big silhouette visible through the weather.
    b.decor(-6.0, 4.0 * STOREY + 1.0, -1.5, 12.0, 5.0, 3.0, Mat::Mesh);
    b.decor(-1.0, 4.0 * STOREY + 1.0, -6.0, 2.0, 5.0, 12.0, Mat::Mesh);

    // --- Two habitat blocks, one per side, each a small interior loop.
    for (hx, hz, mat) in [(-38.0f32, -14.0f32, Mat::CamoWinter), (22.0, 6.0, Mat::MetalPanel)] {
        b.room(hx, hz, 18.0, 13.0, 0.8, 3.2, DOOR_NX | DOOR_PX | DOOR_NZ, mat, Mat::MetalPlateDiamond, false);
        b.floor(hx, hz, 18.0, 13.0, 4.0, Mat::RoofMetal);
        b.wall_x(hx, hz, 18.0, 4.0, 0.9, Mat::RoofMetal);
        b.wall_x(hx, hz + 13.0, 18.0, 4.0, 0.9, Mat::RoofMetal);
        b.wall_z(hx, hz, 13.0, 4.0, 0.9, Mat::RoofMetal);
        b.wall_z(hx + 18.0, hz, 13.0, 4.0, 0.9, Mat::RoofMetal);
        // The ramp lands on the -X doorway; the roof stair climbs the +X face.
        b.ramp(hx - 4.0, 0.0, hz + 5.0, 4.0, 0.8, 4.0, RampAxis::PosX, Mat::MetalPlateDiamond);
        b.access_stair(hz + 3.0, hx + 18.0, 0.8, 4.0, true, false, Mat::MetalPlateDiamond);
        b.decor(hx + 1.0, 0.8, hz + 1.0, 3.0, 1.0, 1.4, Mat::ControlPanel);
        b.decor(hx + 1.0, 1.8, hz + 1.2, 2.6, 1.0, 0.2, Mat::Screen);
        b.crates(hx + 14.0, 0.8, hz + 9.0, 1.2, 2, Mat::WoodCrate);
    }

    // --- Buried service tunnel connecting the two habitats under the pond.
    let ty = -3.4;
    b.floor(-30.0, -3.0, 54.0, 7.0, ty, Mat::ConcreteFloor);
    b.wall_x(-30.0, -3.0, 54.0, ty, 3.0, Mat::Bunker);
    b.wall_x(-30.0, 4.0, 54.0, ty, 3.0, Mat::Bunker);
    // Ceiling in segments, leaving the three shafts open to the surface.
    for (a, bx) in [(-30.0f32, -30.4f32), (-24.5, -0.4), (5.5, 14.5), (20.4, 24.0)] {
        if bx > a + 0.05 { b.ceiling(a, -3.0, bx - a, 7.0, ty + 3.0, Mat::Bunker); }
    }
    for i in 0..6 { b.decor(-27.0 + i as f32 * 9.0, ty + 2.6, -2.0, 1.6, 0.3, 1.6, Mat::WindowLit); }
    // Three staircases climb out of the tunnel along its length, leaving the
    // walkway beside them clear.
    b.access_stair(0.5, -24.5, ty, 0.0, true, true, Mat::ConcreteFloor);
    b.access_stair(0.5, 5.5, ty, 0.0, true, false, Mat::ConcreteFloor);
    b.access_stair(0.5, 20.4, ty, 0.0, true, true, Mat::ConcreteFloor);

    // --- Generator shed and fuel farm: the third contested pocket.
    b.room(20.0, -30.0, 16.0, 14.0, 0.0, 4.2, DOOR_NX | DOOR_PX | DOOR_PZ, Mat::Corrugated, Mat::ConcreteFloor, true);
    tank(b, 27.0, 0.0, -24.0, 2.6, 5.0, Mat::MetalRust);
    tank(b, 32.0, 0.0, -19.0, 2.6, 5.0, Mat::MetalPanel);
    for i in 0..4 { b.barrel(22.0 + i as f32 * 1.1, 0.0, -27.0, Mat::BarrelRust); }
    b.pickup(23.0, 0.1, -20.0, PickupKind::Ammo);

    // --- Vehicles and scattered cover in the open ground.
    truck(b, -22.0, 0.0, 20.0, true, Mat::CamoWinter);
    truck(b, 8.0, 0.0, -28.0, false, Mat::CamoWinter);
    b.container(-16.0, 0.0, -18.0, true, Mat::ShippingBlue);
    b.container(12.0, 0.0, 20.0, false, Mat::ShippingRed);
    for (x, z) in [(-8.0f32, -20.0f32), (16.0, -14.0), (-26.0, 6.0), (32.0, 28.0), (0.0, 26.0)] {
        b.crates(x, 0.0, z, 1.3, 2, Mat::WoodCrate);
    }

    b.spawn_cluster(-38.0, 0.0, 26.0, 30.0, Team::Phantom, 8, 4.5, true);
    b.spawn_cluster(-40.0, 0.0, -28.0, 60.0, Team::Phantom, 5, 4.0, false);
    b.spawn_cluster(38.0, 0.0, -28.0, -150.0, Team::Vanguard, 8, 4.5, true);
    b.spawn_cluster(40.0, 0.0, 26.0, -120.0, Team::Vanguard, 5, 4.0, false);
    b.spawn_cluster(0.0, 0.0, 30.0, 0.0, Team::None, 5, 5.0, false);
    b.spawn_cluster(-2.0, 0.0, -30.0, 180.0, Team::None, 5, 5.0, false);

    b.dom("A", -29.0, 0.9, -7.0, 5.5);
    b.dom("B", 0.0, 0.1, -2.0, 6.0);
    b.dom("C", 24.0, 0.1, -20.0, 5.5);
    b.site("A", 24.0, 0.1, -20.0, 4.5);
    b.site("B", 31.0, 0.9, 12.0, 4.5);

    b.pickup(0.0, 0.1, 8.0, PickupKind::Ammo);
    b.pickup(0.0, 4.0 * STOREY + 0.1, -2.0, PickupKind::Weapon);
    b.pickup(-29.0, 0.9, -7.0, PickupKind::Armor);
    b.pickup(31.0, 0.9, 12.0, PickupKind::Armor);
    b.pickup(0.0, ty + 0.1, 0.0, PickupKind::Health);
    b.pickup(-16.0, 0.15, -21.0, PickupKind::Grenade);
    b.pickup(5.4, 7.0, -2.0, PickupKind::Grenade);

    // Clutter the open ground; see MapBuilder::dress_open_ground.
    b.dress_open_ground(26, 0x1A04, &[
        CoverPiece::Crates(Mat::WoodCrate, 1.4),
        CoverPiece::Barrels(Mat::BarrelRust),
        CoverPiece::Block(Mat::SnowRock, 3.2, 1.5),
        CoverPiece::Sandbags(Mat::Sandbag),
        CoverPiece::Container(Mat::ShippingGreen),
    ]);
}

// ================================================================== HIGHRISE

/// Four gutted apartment blocks around a courtyard. Every window is an angle
/// and every roof connects to another, so the fight moves vertically.
fn highrise(b: &mut MapBuilder) {
    b.set_env(env_urban());
    b.defaults(Mat::ConcretePanel, Mat::Asphalt);
    b.playspace(-38.0, -38.0, 38.0, 38.0, -4.5, 42.0);
    b.apron(Mat::Asphalt, 0.0);

    // Ground, holed where the car park's ramp and two stairwells surface.
    let shafts = [(-30.0f32, -13.5f32, 6.6f32, 3.0f32), (5.4, -6.5, 6.7, 3.0), (23.4, -13.5, 6.6, 3.0)];
    b.floor_with_holes(-38.0, -38.0, 76.0, 76.0, 0.0, Mat::Asphalt,
                       &shafts.iter().map(|h| (h.0, h.1, h.2, h.3)).collect::<Vec<_>>());
    b.floor(-14.0, -14.0, 28.0, 28.0, 0.02, Mat::ConcreteFloor);

    // --- Four blocks. Three storeys each; the pairs are offset so no two
    //     roofs look at each other along the same axis.
    let blocks: [(f32, f32, f32, f32, u32, Mat); 4] = [
        (-34.0, -34.0, 18.0, 16.0, 3, Mat::BrickRed),
        (16.0, -32.0, 18.0, 16.0, 3, Mat::Plaster),
        (-32.0, 18.0, 17.0, 16.0, 3, Mat::Plaster),
        (16.0, 16.0, 18.0, 18.0, 3, Mat::BrickPale),
    ];
    for (x, z, sx, sz, st, mat) in blocks {
        b.tower(x, z, sx, sz, 0.0, st, mat, Mat::ConcreteFloor, Mat::ConcreteFloor);
    }

    // --- Roof crossings: two planks and one collapsed slab. Fast, exposed.
    b.boxx(-16.0, 3.0 * STOREY, -28.0, 32.0, 0.35, 2.6, Mat::WoodPlank).with_scale(2.0);
    b.boxx(-15.0, 3.0 * STOREY, 24.0, 31.0, 0.35, 2.6, Mat::WoodPlank).with_scale(2.0);
    b.boxx(-26.0, 3.0 * STOREY, -18.0, 3.0, 0.35, 36.0, Mat::ConcreteFloor).with_scale(3.0);

    // --- Courtyard: planters, burnt cars, a fountain slab, a bus shelter.
    for (x, z) in [(-9.0f32, -9.0f32), (7.0, -9.0), (-9.0, 7.0), (7.0, 7.0)] {
        b.boxc(x, 0.0, z, 4.4, 0.95, 4.4, Mat::Concrete).with_scale(2.0).with_top(Mat::Dirt);
        bush(b, x, 0.95, z, 3.4, 1.6);
    }
    truck(b, 0.0, 0.0, -18.0, true, Mat::MetalRust);
    truck(b, -18.0, 0.0, 2.0, false, Mat::BluePaint);
    truck(b, 18.0, 0.0, -2.0, false, Mat::RedPaint);
    b.barrier(-6.0, 0.0, 16.0, 12.0, 0.7);
    b.barrier(-6.0, 0.0, -17.0, 12.0, 0.7);
    b.boxc(0.0, 0.0, 0.0, 6.0, 0.6, 6.0, Mat::Marble).with_scale(2.5);
    b.boxc(0.0, 0.6, 0.0, 1.2, 1.8, 1.2, Mat::Marble);
    // Bus shelter: a roof you can stand on to reach a first-floor window.
    b.boxc(24.0, 0.0, 0.0, 1.0, 2.8, 6.0, Mat::PipeMetal);
    b.boxc(24.0, 2.8, 0.0, 4.0, 0.3, 7.0, Mat::MetalPanel).with_scale(2.0);
    step_up(b, 20.5, 0.0, -1.0, 1.6, 3.0, 2.8, RampAxis::PosX, Mat::Concrete);

    // --- Underground car park beneath the north side: a full flank route.
    let py = -3.6;
    b.floor(-30.0, -16.0, 60.0, 14.0, py, Mat::ConcreteFloor);
    b.wall_x(-30.0, -16.0, 60.0, py, 3.0, Mat::Concrete);
    b.wall_x(-30.0, -2.0, 60.0, py, 3.0, Mat::Concrete);
    b.wall_z(-30.0, -16.0, 14.0, py, 3.0, Mat::Concrete);
    b.wall_z(30.0, -16.0, 14.0, py, 3.0, Mat::Concrete);
    // The street slab above is the car park's ceiling; a second one here would
    // only fight with it.
    for i in 0..7 { b.pillar(-24.0 + i as f32 * 8.0, py, -9.0, 1.2, 3.0, Mat::Concrete); }
    for i in 0..5 { b.decor(-22.0 + i as f32 * 11.0, py + 2.7, -9.6, 2.0, 0.25, 1.2, Mat::WindowLit); }
    truck(b, -14.0, py, -12.0, true, Mat::MetalPanel);
    truck(b, 12.0, py, -13.0, true, Mat::MetalRust);
    // Three stairwells surface into the courtyard and the two side streets.
    b.access_stair(-12.0, -24.0, py, 0.0, true, true, Mat::ConcreteFloor);
    b.access_stair(-5.0, 6.0, py, 0.0, true, false, Mat::ConcreteFloor);
    b.access_stair(-12.0, 24.0, py, 0.0, true, false, Mat::ConcreteFloor);

    // --- Perimeter: rubble walls keep the outer ring from being a racetrack.
    for (x, z, sx, sz) in [(-38.0f32, -8.0f32, 2.0, 16.0), (36.0, -6.0, 2.0, 16.0),
                           (-8.0, 36.0, 16.0, 2.0), (-6.0, -38.0, 16.0, 2.0)] {
        b.boxx(x, 0.0, z, sx, 2.2, sz, Mat::Cinderblock).with_scale(2.5);
    }

    b.spawn_cluster(-32.0, 0.0, 6.0, 90.0, Team::Phantom, 7, 4.0, true);
    b.spawn_cluster(-6.0, 0.0, 32.0, 0.0, Team::Phantom, 6, 4.5, false);
    b.spawn_cluster(32.0, 0.0, -6.0, -90.0, Team::Vanguard, 7, 4.0, true);
    b.spawn_cluster(4.0, 0.0, -32.0, 180.0, Team::Vanguard, 6, 4.5, false);
    b.spawn_cluster(-30.0, 0.0, -20.0, 45.0, Team::None, 4, 4.0, false);
    b.spawn_cluster(30.0, 0.0, 22.0, -135.0, Team::None, 4, 4.0, false);

    b.dom("A", -25.0, 0.1, -25.0, 5.5);
    b.dom("B", 0.0, 0.1, 0.0, 6.5);
    b.dom("C", 25.0, 0.1, 25.0, 5.5);
    b.site("A", 0.0, 0.1, 0.0, 5.5);
    b.site("B", 0.0, py + 0.1, -9.0, 5.5);

    b.pickup(-12.0, 0.15, 0.0, PickupKind::Ammo);
    b.pickup(12.0, 0.15, 0.0, PickupKind::Ammo);
    b.pickup(-25.0, 3.0 * STOREY + 0.1, -26.0, PickupKind::Weapon);
    b.pickup(25.0, 3.0 * STOREY + 0.1, 25.0, PickupKind::Armor);
    b.pickup(0.0, py + 0.1, -9.0, PickupKind::Armor);
    b.pickup(20.0, 0.15, 6.0, PickupKind::Grenade);
    b.pickup(-20.0, 0.15, 20.0, PickupKind::Health);

    // Clutter the open ground; see MapBuilder::dress_open_ground.
    b.dress_open_ground(58, 0x1A05, &[
        CoverPiece::Crates(Mat::WoodCrate, 1.3),
        CoverPiece::Planter(Mat::ConcretePanel, Mat::Foliage),
        CoverPiece::Block(Mat::Concrete, 3.0, 1.2),
        CoverPiece::Barrels(Mat::Barrel),
        CoverPiece::Sandbags(Mat::Sandbag),
    ]);
}

// =================================================================== DRYDOCK

/// A half-scrapped hull in a drained basin. Three heights: basin floor,
/// quayside, and the gantries that run the length of the dock.
fn drydock(b: &mut MapBuilder) {
    b.set_env(env_shipyard());
    b.defaults(Mat::Concrete, Mat::ConcreteFloor);
    b.playspace(-48.0, -34.0, 48.0, 34.0, -7.0, 40.0);
    b.apron(Mat::ConcreteFloor, 0.0);

    // Quayside at 0, basin floor at -6.
    // The quay is four slabs framing the basin, rather than one slab with a
    // hole, because the basin walls want to be their own brushes anyway.
    b.floor(-48.0, -34.0, 96.0, 16.0, 0.0, Mat::ConcreteFloor);
    b.floor(-48.0, 18.0, 96.0, 16.0, 0.0, Mat::ConcreteFloor);
    b.floor(-48.0, -18.0, 12.0, 36.0, 0.0, Mat::ConcreteFloor);
    b.floor(36.0, -18.0, 12.0, 36.0, 0.0, Mat::ConcreteFloor);
    // The basin itself.
    b.floor(-36.0, -18.0, 72.0, 36.0, -6.0, Mat::Concrete);
    for (x, z, sx, sz) in [(-36.0f32, -18.0f32, 72.0, 0.4), (-36.0, 17.6, 72.0, 0.4),
                           (-36.4, -18.0, 0.4, 36.0), (36.0, -18.0, 0.4, 36.0)] {
        b.boxx(x, -6.0, z, sx, 6.0, sz, Mat::Concrete).with_scale(4.0);
    }
    // Stepped sides: the basin is enterable in six places, not two.
    for x in [-28.0f32, -4.0, 20.0] {
        b.stairs(x, -6.0, -18.0, 3.0, 6.0, 6.0, RampAxis::NegZ, Mat::Concrete);
        b.stairs(x + 8.0, -6.0, 12.0, 3.0, 6.0, 6.0, RampAxis::PosZ, Mat::Concrete);
    }

    // --- The hull: a long, tall structure with an interior deck.
    let (hx, hz, hw, hd) = (-20.0f32, -8.0f32, 42.0, 16.0);
    b.floor(hx, hz, hw, hd, -6.0, Mat::HullPainted);
    // The hull sides are pierced twice each so the interior is a route, not a pit.
    for wz in [hz, hz + hd] {
        let mut cursor = hx;
        for gap in [hx + 9.0f32, hx + 27.0] {
            b.boxx(cursor, -6.0, wz - WALL * 0.5, gap - cursor, 5.4, WALL, Mat::HullPainted);
            b.boxx(gap, -6.0 + 2.6, wz - WALL * 0.5, 5.0, 2.8, WALL, Mat::HullPainted);
            cursor = gap + 5.0;
        }
        b.boxx(cursor, -6.0, wz - WALL * 0.5, hx + hw - cursor, 5.4, WALL, Mat::HullPainted);
    }
    b.wall_z(hx, hz, hd, -6.0, 5.4, Mat::HullPainted);
    b.wall_z(hx + hw, hz, hd, -6.0, 5.4, Mat::HullPainted);
    // Hull deck at -0.6, with three cargo hatches dropping into the interior.
    b.floor_with_holes(hx, hz, hw, hd, -0.6, Mat::MetalPlateDiamond, &[
        (hx + 5.0, hz + 5.0, 5.0, 6.0),
        (hx + 18.5, hz + 5.0, 5.0, 6.0),
        (hx + 32.0, hz + 5.0, 5.0, 6.0),
    ]);
    // Ribs sticking up out of the deck: cover along an otherwise bare run,
    // broken amidships so the deck is a route rather than a set of pens.
    for i in 0..7 {
        let x = hx + 3.0 + i as f32 * 6.0;
        if x > hx + hw - 1.0 { break; }
        b.boxx(x, -0.6, hz + 2.0, 1.0, 1.6, 4.0, Mat::MetalRust).with_scale(2.0);
        b.boxx(x, -0.6, hz + 10.0, 1.0, 1.6, 4.0, Mat::MetalRust).with_scale(2.0);
    }
    // Stairs up the outside of the hull at both ends, arriving on the deck.
    b.access_stair(hz + 8.0, hx, -6.0, -0.6, true, true, Mat::MetalPlateDiamond);
    b.access_stair(hz + 8.0, hx + hw, -6.0, -0.6, true, false, Mat::MetalPlateDiamond);
    // Superstructure amidships: the hull's own power position.
    b.room(-2.0, -5.0, 10.0, 10.0, -0.6, 3.4, DOOR_NX | DOOR_PZ, Mat::HullPainted, Mat::MetalPlateDiamond, false);
    b.floor(-2.0, -5.0, 10.0, 10.0, 2.8, Mat::MetalPlateDiamond);
    b.wall_x(-2.0, -5.0, 10.0, 2.8, 0.9, Mat::HullPainted);
    b.wall_x(-2.0, 5.0, 10.0, 2.8, 0.9, Mat::HullPainted);
    b.wall_z(-2.0, -5.0, 10.0, 2.8, 0.9, Mat::HullPainted);
    // Parapet broken where the stair arrives on the superstructure roof.
    b.wall_z(8.0, -5.0, 3.6, 2.8, 0.9, Mat::HullPainted);
    b.wall_z(8.0, 1.4, 3.6, 2.8, 0.9, Mat::HullPainted);
    b.access_stair(0.0, 8.0, -0.6, 2.8, true, false, Mat::MetalPlateDiamond);

    // --- Gantry cranes spanning the dock at 9 m.
    for cx in [-26.0f32, 26.0] {
        // The gantry stops at the basin lip so both of its ends are open
        // landings sitting over solid quay, where the stairs can reach them.
        b.catwalk(cx, 9.0, -18.0, 3.2, 36.0, Mat::Grating, false, &[5.3]);
        b.pillar(cx + 1.6, 0.0, -17.0, 1.6, 9.0, Mat::PipeMetal);
        b.pillar(cx + 1.6, 0.0, 16.6, 1.6, 9.0, Mat::PipeMetal);
        b.access_stair(cx + 1.6, -18.0, 0.0, 9.0, false, true, Mat::MetalPlateDiamond);
        b.access_stair(cx + 1.6, 18.0, 0.0, 9.0, false, false, Mat::MetalPlateDiamond);
    }
    // Crossing walkway joining the two cranes over the middle of the hull.
    b.catwalk(-24.0, 9.0, 4.0, 52.0, 2.6, Mat::Grating, true, &[]);

    // --- Quayside yards.
    b.container(-40.0, 0.0, -26.0, true, Mat::ShippingRed);
    b.container(-40.0, 2.65, -26.0, true, Mat::ShippingBlue);
    b.container(-32.0, 0.0, -28.0, false, Mat::ShippingGreen);
    b.container(40.0, 0.0, 26.0, true, Mat::ShippingBlue);
    b.container(40.0, 2.65, 26.0, true, Mat::ShippingRed);
    b.container(32.0, 0.0, 28.0, false, Mat::ShippingGreen);
    for (x, z) in [(-14.0f32, -26.0f32), (10.0, 26.0), (-8.0, 27.0), (16.0, -27.0)] {
        b.crates(x, 0.0, z, 1.4, 2, Mat::WoodCrate);
    }
    truck(b, 0.0, 0.0, -28.0, true, Mat::MetalPanel);
    truck(b, -4.0, 0.0, 28.0, true, Mat::MetalRust);
    for i in 0..8 { b.barrel(-44.0 + i as f32 * 1.0, 0.0, 4.0, Mat::BarrelRust); }

    // --- Workshop building on the west quay.
    b.room(-46.0, -12.0, 14.0, 24.0, 0.0, 6.0, DOOR_PX | DOOR_PZ, Mat::Corrugated, Mat::ConcreteFloor, true);
    b.catwalk(-45.0, 3.4, -11.0, 12.0, 2.8, Mat::Grating, true, &[-39.0]);
    b.access_stair(-39.0, -8.2, 0.0, 3.4, false, false, Mat::MetalPlateDiamond);
    tank(b, -40.0, 0.0, 6.0, 2.0, 4.0, Mat::MetalRust);

    b.spawn_cluster(-43.0, 0.0, 26.0, 20.0, Team::Phantom, 8, 4.5, true);
    b.spawn_cluster(-43.0, 0.0, -6.0, 90.0, Team::Phantom, 5, 4.0, false);
    b.spawn_cluster(43.0, 0.0, -26.0, -160.0, Team::Vanguard, 8, 4.5, true);
    b.spawn_cluster(43.0, 0.0, 6.0, -90.0, Team::Vanguard, 5, 4.0, false);
    b.spawn_cluster(-2.0, 0.0, -28.0, 180.0, Team::None, 5, 5.0, false);
    b.spawn_cluster(2.0, 0.0, 28.0, 0.0, Team::None, 5, 5.0, false);

    b.dom("A", -30.0, -5.9, 0.0, 5.5);
    b.dom("B", 6.0, -0.5, 0.0, 6.0);
    b.dom("C", 30.0, -5.9, 0.0, 5.5);
    b.site("A", 6.0, -0.5, 0.0, 5.0);
    b.site("B", -39.0, 0.1, 0.0, 5.0);

    b.pickup(-30.0, -5.85, 0.0, PickupKind::Ammo);
    b.pickup(30.0, -5.85, 0.0, PickupKind::Ammo);
    b.pickup(3.0, 3.0, 0.0, PickupKind::Weapon);
    b.pickup(-26.0, 9.1, 0.0, PickupKind::Armor);
    b.pickup(26.0, 9.1, 0.0, PickupKind::Armor);
    b.pickup(0.0, -0.5, 0.0, PickupKind::Grenade);
    b.pickup(-39.0, 0.1, -6.0, PickupKind::Health);

    // Clutter the open ground; see MapBuilder::dress_open_ground.
    b.dress_open_ground(28, 0x1A06, &[
        CoverPiece::Container(Mat::ShippingGreen),
        CoverPiece::Crates(Mat::WoodCrate, 1.5),
        CoverPiece::Barrels(Mat::BarrelRust),
        CoverPiece::Pipes(Mat::PipeMetal),
        CoverPiece::Block(Mat::HullPainted, 3.4, 1.4),
    ]);
}

// =================================================================== FOUNDRY

/// Small, dark and vertical. Two catwalk levels cross a killing floor, and
/// the pour channels below give a low route nobody can see into.
fn foundry(b: &mut MapBuilder) {
    b.set_env(env_foundry());
    b.defaults(Mat::MetalRust, Mat::ConcreteFloor);
    b.playspace(-32.0, -32.0, 32.0, 32.0, -4.0, 28.0);
    b.apron(Mat::ConcreteFloor, 0.0);

    let h = 15.0;
    b.floor(-32.0, -32.0, 64.0, 64.0, 0.0, Mat::ConcreteFloor);
    b.ceiling(-32.0, -32.0, 64.0, 64.0, h, Mat::RoofMetal);
    b.wall_x(-32.0, -32.0, 64.0, 0.0, h, Mat::Cinderblock);
    b.wall_x(-32.0, 32.0, 64.0, 0.0, h, Mat::Cinderblock);
    b.wall_z(-32.0, -32.0, 64.0, 0.0, h, Mat::Cinderblock);
    b.wall_z(32.0, -32.0, 64.0, 0.0, h, Mat::Cinderblock);
    // Clerestory windows: the only daylight, and they streak the floor.
    for i in 0..6 {
        b.decor(-30.0 + i as f32 * 11.0, h - 3.0, -31.6, 7.0, 2.2, 0.3, Mat::WindowLit);
        b.decor(-30.0 + i as f32 * 11.0, h - 3.0, 31.3, 7.0, 2.2, 0.3, Mat::WindowLit);
    }

    // --- Pour channels: shallow trenches crossing the floor.
    let cy = -2.2;
    for (x, z, sx, sz) in [(-30.0f32, -3.0f32, 60.0, 6.0), (-3.0, -30.0, 6.0, 60.0)] {
        b.floor(x, z, sx, sz, cy, Mat::Concrete);
        if sx > sz {
            b.wall_x(x, z, sx, cy, 2.2, Mat::Concrete);
            b.wall_x(x, z + sz, sx, cy, 2.2, Mat::Concrete);
        } else {
            b.wall_z(x, z, sz, cy, 2.2, Mat::Concrete);
            b.wall_z(x + sx, z, sz, cy, 2.2, Mat::Concrete);
        }
    }
    // Crossing plates so the trenches do not sever the ground floor.
    for (x, z) in [(-18.0f32, -3.0f32), (14.0, -3.0)] {
        b.boxx(x, -0.3, z, 4.0, 0.3, 6.0, Mat::MetalPlateDiamond).with_scale(2.0);
    }
    for (x, z) in [(-3.0f32, -18.0f32), (-3.0, 14.0)] {
        b.boxx(x, -0.3, z, 6.0, 0.3, 4.0, Mat::MetalPlateDiamond).with_scale(2.0);
    }
    for (x, z, ax) in [(-30.0f32, -2.6f32, RampAxis::NegX), (26.0, -2.6, RampAxis::PosX),
                       (-2.6, -30.0, RampAxis::NegZ), (-2.6, 26.0, RampAxis::PosZ)] {
        let (sx, sz) = match ax { RampAxis::NegX | RampAxis::PosX => (4.0, 5.2), _ => (5.2, 4.0) };
        b.ramp(x, cy, z, sx, 2.2, sz, ax, Mat::Concrete);
    }

    // --- Four furnaces: enormous hard cover in the quadrants.
    for (fx, fz) in [(-20.0f32, -20.0f32), (14.0, -20.0), (-20.0, 14.0), (14.0, 14.0)] {
        b.boxx(fx, 0.0, fz, 8.0, 7.0, 8.0, Mat::MetalRust).with_scale(3.5);
        b.decor(fx - 1.0, 7.0, fz - 1.0, 10.0, 1.2, 10.0, Mat::MetalPanel);
        b.decor(fx + 3.0, 8.2, fz + 3.0, 2.0, 6.8, 2.0, Mat::Duct);
        // No way onto a furnace roof: they are cover and silhouette, not a
        // position. The catwalks above are where height is fought over.
    }

    // --- Two catwalk levels.
    let l1 = 5.4;
    // A square ring of walkways, entered at four points around the hall.
    // Railings are broken at all four corners of the ring, or the two pairs of
    // walkways would fence each other off where they cross.
    b.catwalk(-31.0, l1, 8.0, 62.0, 2.6, Mat::Grating, true, &[-24.0, -9.3, 9.3]);
    b.catwalk(-31.0, l1, -10.6, 62.0, 2.6, Mat::Grating, true, &[24.0, -9.3, 9.3]);
    b.catwalk(8.0, l1, -31.0, 2.6, 62.0, Mat::Grating, false, &[-24.0, -9.3, 9.3]);
    b.catwalk(-10.6, l1, -31.0, 2.6, 62.0, Mat::Grating, false, &[24.0, -9.3, 9.3]);
    b.access_stair(-24.0, 10.6, 0.0, l1, false, false, Mat::MetalPlateDiamond);
    b.access_stair(24.0, -10.6, 0.0, l1, false, true, Mat::MetalPlateDiamond);
    b.access_stair(-24.0, 10.6, 0.0, l1, true, false, Mat::MetalPlateDiamond);
    b.access_stair(24.0, -10.6, 0.0, l1, true, true, Mat::MetalPlateDiamond);

    let l2 = 9.6;
    // The high cross reaches over the furnaces; its two staircases start on
    // the ring below and block it where they land, so the ring is a loop with
    // two pinch points rather than a free racetrack.
    b.catwalk(-31.0, l2, -1.3, 62.0, 2.6, Mat::Grating, true, &[9.3, -9.3, 0.0]);
    b.catwalk(-1.3, l2, -31.0, 2.6, 62.0, Mat::Grating, false, &[0.0]);
    b.access_stair(9.3, 1.3, l1, l2, false, false, Mat::MetalPlateDiamond);
    b.access_stair(-9.3, -1.3, l1, l2, false, true, Mat::MetalPlateDiamond);
    let _ = l2;

    // --- Control room overlooking the hall from the north wall.
    b.floor(-10.0, -31.0, 20.0, 8.0, l1, Mat::ConcreteFloor);
    // Window band with a doorway in the middle, where the stair arrives.
    b.wall_x_window(-10.0, -23.0, 6.7, l1, 3.6, 1.0, 2.3, Mat::ConcretePanel);
    b.wall_x_window(3.3, -23.0, 6.7, l1, 3.6, 1.0, 2.3, Mat::ConcretePanel);
    b.wall_x(-3.4, -23.0, 6.8, l1 + 2.3, 1.3, Mat::ConcretePanel);
    b.access_stair(0.0, -23.0, 0.0, l1, false, false, Mat::MetalPlateDiamond);
    b.wall_z(-10.0, -31.0, 8.0, l1, 3.6, Mat::ConcretePanel);
    b.wall_z(10.0, -31.0, 8.0, l1, 3.6, Mat::ConcretePanel);
    b.ceiling(-10.0, -31.0, 20.0, 8.0, l1 + 3.6, Mat::ConcretePanel);
    for i in 0..4 {
        b.decor(-8.0 + i as f32 * 4.5, l1, -24.4, 3.0, 1.1, 1.2, Mat::ControlPanel);
        b.decor(-8.0 + i as f32 * 4.5, l1 + 1.1, -24.2, 2.6, 1.1, 0.2, Mat::Screen);
    }

    // --- Ore hoppers and scrap heaps for floor-level cover.
    for (x, z) in [(-8.0f32, 6.0f32), (6.0, -8.0), (-26.0, 4.0), (24.0, -4.0), (4.0, 24.0), (-6.0, -26.0)] {
        b.crates(x, 0.0, z, 1.5, 2, Mat::MetalRust);
    }
    for i in 0..10 { b.barrel(-29.0 + (i % 5) as f32 * 1.1, 0.0, 27.0 + (i / 5) as f32 * 1.1, Mat::BarrelRust); }

    b.spawn_cluster(-27.0, 0.0, 27.0, 30.0, Team::Phantom, 8, 3.6, true);
    b.spawn_cluster(-28.0, 0.0, -6.0, 90.0, Team::Phantom, 4, 3.0, false);
    b.spawn_cluster(27.0, 0.0, -27.0, -150.0, Team::Vanguard, 8, 3.6, true);
    b.spawn_cluster(28.0, 0.0, 6.0, -90.0, Team::Vanguard, 4, 3.0, false);
    b.spawn_cluster(-27.0, 0.0, -27.0, 45.0, Team::None, 4, 3.6, false);
    b.spawn_cluster(27.0, 0.0, 27.0, -135.0, Team::None, 4, 3.6, false);

    b.dom("A", -24.0, 0.1, -8.0, 5.0);
    b.dom("B", 0.0, cy + 0.1, 0.0, 5.5);
    b.dom("C", 24.0, 0.1, 8.0, 5.0);
    b.site("A", -24.0, 0.1, 8.0, 4.5);
    b.site("B", 24.0, 0.1, -8.0, 4.5);

    b.pickup(0.0, cy + 0.15, 0.0, PickupKind::Weapon);
    b.pickup(-24.0, 0.15, -8.0, PickupKind::Armor);
    b.pickup(24.0, 0.15, 8.0, PickupKind::Armor);
    b.pickup(0.0, l1 + 0.1, 9.3, PickupKind::Ammo);
    b.pickup(0.0, l1 + 0.1, -9.3, PickupKind::Ammo);
    b.pickup(0.0, l1 + 0.1, -27.0, PickupKind::Grenade);
    b.pickup(-24.0, 0.15, 8.0, PickupKind::Health);

    // Clutter the open ground; see MapBuilder::dress_open_ground.
    b.dress_open_ground(40, 0x1A07, &[
        CoverPiece::Crates(Mat::WoodCrate, 1.4),
        CoverPiece::Barrels(Mat::BarrelRust),
        CoverPiece::Pipes(Mat::PipeMetal),
        CoverPiece::Block(Mat::MetalRust, 3.0, 1.3),
    ]);
}

// ================================================================== SALTBITE

/// A sea fort on a tidal causeway. Ramparts ring an inner courtyard, casemates
/// tunnel through the walls, and the causeway itself is a deliberate trap.
fn saltbite(b: &mut MapBuilder) {
    b.set_env(env_coastal());
    b.defaults(Mat::StoneWall, Mat::Cobble);
    b.playspace(-40.0, -36.0, 40.0, 36.0, -2.0, 40.0);
    b.apron(Mat::WaterSurface, -1.2);

    b.floor(-40.0, -36.0, 80.0, 72.0, -1.4, Mat::Sand);
    // Shallow water ring outside the fort; walkable but slow and exposed.
    b.floor(-40.0, -36.0, 80.0, 72.0, -1.2, Mat::WaterSurface);
    b.brushes.last_mut().unwrap().flags = BrushFlags::NOSHADOW | BrushFlags::NONAV;
    // Rock shelf the fort stands on.
    b.floor(-26.0, -24.0, 52.0, 48.0, 0.0, Mat::Rock);

    // --- Outer curtain wall: a walkable rampart at 6 m with a parapet.
    let (fx0, fz0, fx1, fz1) = (-24.0f32, -22.0f32, 24.0f32, 22.0f32);
    let wt = 4.0;
    let rh = 6.0;
    // The north and south walls are built in segments, leaving four casemate
    // mouths punched through them at ground level with a lintel over each.
    let casemates = [(-15.0f32, 6.0f32), (9.0, 6.0)];
    for (x, z, sx, sz, pierced) in [
        (fx0, fz0, fx1 - fx0, wt, true),
        (fx0, fz1 - wt, fx1 - fx0, wt, true),
        (fx0, fz0 + wt, wt, fz1 - fz0 - wt * 2.0, false),
        (fx1 - wt, fz0 + wt, wt, fz1 - fz0 - wt * 2.0, false),
    ] {
        if !pierced {
            // The two side walls carry the gates, offset from each other so
            // the fort is never a straight shot end to end.
            let gate = if x < 0.0 { -3.0f32 } else { 6.0f32 };
            b.boxx(x, 0.0, z, sx, rh, gate - z, Mat::StoneWall).with_scale(4.0).with_top(Mat::Cobble);
            b.boxx(x, 0.0, gate + 6.0, sx, rh, z + sz - gate - 6.0, Mat::StoneWall).with_scale(4.0).with_top(Mat::Cobble);
            b.boxx(x, 3.4, gate, sx, rh - 3.4, 6.0, Mat::StoneWall).with_scale(4.0).with_top(Mat::Cobble);
            b.floor(x, gate, sx, 6.0, 0.0, Mat::Cobble);
            continue;
        }
        // Solid piers between the mouths.
        let mut cursor = x;
        for (cx, cw) in casemates {
            if cx > cursor {
                b.boxx(cursor, 0.0, z, cx - cursor, rh, sz, Mat::StoneWall).with_scale(4.0).with_top(Mat::Cobble);
            }
            // Lintel spanning the mouth, keeping the rampart above walkable.
            b.boxx(cx, 2.9, z, cw, rh - 2.9, sz, Mat::StoneWall).with_scale(4.0).with_top(Mat::Cobble);
            cursor = cx + cw;
        }
        if x + sx > cursor {
            b.boxx(cursor, 0.0, z, x + sx - cursor, rh, sz, Mat::StoneWall).with_scale(4.0).with_top(Mat::Cobble);
        }
        // The casemate floor, so the tunnel does not open onto the rock ledge.
        for (cx, cw) in casemates {
            b.floor(cx, z, cw, sz, 0.0, Mat::Cobble);
        }
    }
    // Parapet: chest high on the outside, waist high inside.
    b.wall_x(fx0, fz0 + 0.2, fx1 - fx0, rh, 1.3, Mat::StoneWall);
    b.wall_x(fx0, fz1 - 0.2, fx1 - fx0, rh, 1.3, Mat::StoneWall);
    b.wall_z(fx0 + 0.2, fz0, fz1 - fz0, rh, 1.3, Mat::StoneWall);
    b.wall_z(fx1 - 0.2, fz0, fz1 - fz0, rh, 1.3, Mat::StoneWall);

    // --- Gate on the west face plus stairs up to the rampart at each corner.
    step_up(b, fx0 + wt + 0.2, 0.0, fz0 + wt + 0.2, 2.2, 7.0, rh, RampAxis::PosZ, Mat::Cobble);
    step_up(b, fx1 - wt - 2.4, 0.0, fz1 - wt - 7.2, 2.2, 7.0, rh, RampAxis::NegZ, Mat::Cobble);
    step_up(b, fx0 + wt + 6.0, 0.0, fz1 - wt - 2.4, 2.2, 7.0, rh, RampAxis::PosX, Mat::Cobble);
    step_up(b, fx1 - wt - 13.0, 0.0, fz0 + wt + 0.2, 2.2, 7.0, rh, RampAxis::NegX, Mat::Cobble);

    // --- Inner courtyard: a keep, a well, and stacked stores.
    b.tower(-6.0, -7.0, 12.0, 13.0, 0.0, 2, Mat::StoneWall, Mat::Marble, Mat::Cobble);
    b.boxc(12.0, 0.0, 10.0, 3.2, 1.1, 3.2, Mat::StoneWall).with_scale(1.6);
    b.crates(-14.0, 0.0, 12.0, 1.4, 2, Mat::WoodCrate);
    b.crates(14.0, 0.0, -12.0, 1.4, 3, Mat::WoodCrate);
    b.sandbags(-18.0, 0.0, -4.0, 1.4, 10.0);
    b.sandbags(16.0, 0.0, -6.0, 1.4, 10.0);
    for i in 0..5 { b.barrel(-16.0 + i as f32 * 1.1, 0.0, 16.0, Mat::Barrel); }

    // --- Lighthouse on the seaward rock: the map's long angle.
    b.tower(28.0, -32.0, 7.0, 7.0, 0.0, 4, Mat::Plaster, Mat::Cobble, Mat::Cobble);
    b.floor(24.0, -34.0, 14.0, 12.0, 0.0, Mat::Rock);
    b.ramp(20.0, -1.4, -30.0, 4.0, 1.4, 5.0, RampAxis::PosX, Mat::Rock);
    b.decor(29.0, 4.0 * STOREY + 1.0, -29.0, 5.0, 2.2, 5.0, Mat::WindowLit);

    // --- The causeway: the long exposed approach from the mainland.
    b.floor(-40.0, -4.0, 16.0, 8.0, 0.0, Mat::Cobble);
    b.boxx(-40.0, 0.0, -4.4, 16.0, 0.9, 0.4, Mat::StoneWall).with_scale(2.0);
    b.boxx(-40.0, 0.0, 4.0, 16.0, 0.9, 0.4, Mat::StoneWall).with_scale(2.0);
    // Wrecked boats give the causeway the cover it desperately needs.
    b.boxc(-32.0, 0.0, -8.0, 8.0, 2.2, 3.0, Mat::WoodPlank).with_scale(2.5);
    b.boxc(-30.0, 0.0, 9.0, 3.0, 2.0, 7.0, Mat::WoodPlank).with_scale(2.5);
    b.crates(-36.0, 0.0, 0.0, 1.3, 2, Mat::WoodCrate);

    // --- Beach rocks on the seaward side keep the flank playable.
    for (i, (x, z)) in [(-30.0f32, 26.0f32), (-14.0, 30.0), (4.0, 32.0), (20.0, 28.0),
                        (30.0, 14.0), (32.0, -4.0), (-32.0, -20.0), (-18.0, -30.0),
                        (2.0, -32.0)].iter().enumerate() {
        let s = 2.6 + (i % 3) as f32 * 1.1;
        b.boxc(*x, -1.4, *z, s, 1.4 + s * 0.5, s * 0.9, Mat::Rock).with_scale(3.0);
    }

    b.spawn_cluster(-34.0, -1.2, 22.0, 20.0, Team::Phantom, 8, 4.5, true);
    b.spawn_cluster(-36.0, 0.0, 0.0, 90.0, Team::Phantom, 5, 3.5, false);
    b.spawn_cluster(30.0, -1.2, 24.0, -140.0, Team::Vanguard, 8, 4.5, true);
    b.spawn_cluster(32.0, 0.0, -22.0, -140.0, Team::Vanguard, 5, 3.5, false);
    b.spawn_cluster(0.0, 0.0, 16.0, 0.0, Team::None, 5, 4.0, false);
    b.spawn_cluster(0.0, 0.0, -16.0, 180.0, Team::None, 5, 4.0, false);

    b.dom("A", -18.0, 0.1, 0.0, 5.0);
    b.dom("B", 0.0, 0.1, 12.0, 5.5);
    b.dom("C", 16.0, 0.1, -12.0, 5.0);
    b.site("A", 0.0, 0.1, 12.0, 5.0);
    b.site("B", 28.0, 0.1, -30.0, 4.5);

    b.pickup(-12.0, 0.15, 14.0, PickupKind::Ammo);
    b.pickup(12.0, 0.15, -14.0, PickupKind::Ammo);
    b.pickup(31.0, 4.0 * STOREY + 0.1, -29.0, PickupKind::Weapon);
    b.pickup(0.0, 2.0 * STOREY + 0.1, 0.0, PickupKind::Armor);
    b.pickup(-22.0, 6.1, 0.0, PickupKind::Grenade);
    b.pickup(22.0, 6.1, 0.0, PickupKind::Grenade);
    b.pickup(-32.0, 0.1, 0.0, PickupKind::Health);

    // Clutter the open ground; see MapBuilder::dress_open_ground.
    b.dress_open_ground(20, 0x1A08, &[
        CoverPiece::Sandbags(Mat::Sandbag),
        CoverPiece::Barrels(Mat::BarrelRust),
        CoverPiece::Crates(Mat::WoodCrate, 1.4),
        CoverPiece::Block(Mat::Rock, 3.2, 1.4),
    ]);
}

// ================================================================== DEEPWELL

/// A command bunker three levels down. No sky, no long angles, and corners
/// every eight metres. The most claustrophobic map in the rotation.
fn deepwell(b: &mut MapBuilder) {
    b.set_env(env_bunker());
    b.defaults(Mat::Bunker, Mat::ConcreteFloor);
    b.playspace(-30.0, -30.0, 30.0, 30.0, -8.0, 14.0);
    b.apron(Mat::ConcreteFloor, 0.0);

    // Two decks: 0 and -4.2, joined by four stairwells.
    let lower = -4.2f32;
    let ch = 3.4f32;

    // Helper closures are avoided here so the layout reads as literal geometry.
    // ---- Upper deck shell. The four stairwells are cut out of the slab here,
    // which is what actually connects the two decks.
    // The wells sit in the corridors between blocks. Putting one inside a
    // room means that room's own floor slab paves the shaft over.
    let wells = [(-11.5f32, -11.5f32), (2.0, -11.5), (-11.5, 7.0), (2.0, 7.0)];
    let holes: Vec<(f32, f32, f32, f32)> = wells.iter().map(|(x, z)| (*x, *z, 4.4, 4.4)).collect();
    b.floor_with_holes(-30.0, -30.0, 60.0, 60.0, 0.0, Mat::ConcreteFloor, &holes);
    b.ceiling(-30.0, -30.0, 60.0, 60.0, ch, Mat::Bunker);
    b.wall_x(-30.0, -30.0, 60.0, 0.0, ch, Mat::Bunker);
    b.wall_x(-30.0, 30.0, 60.0, 0.0, ch, Mat::Bunker);
    b.wall_z(-30.0, -30.0, 60.0, 0.0, ch, Mat::Bunker);
    b.wall_z(30.0, -30.0, 60.0, 0.0, ch, Mat::Bunker);

    // ---- Upper deck: a ring corridor with rooms hung off it.
    // Internal blocks carve the corridors; everything between them is walkable.
    let blocks: [(f32, f32, f32, f32); 9] = [
        (-24.0, -24.0, 12.0, 10.0), (-6.0, -24.0, 12.0, 10.0), (12.0, -24.0, 12.0, 10.0),
        (-24.0, -6.0, 12.0, 12.0), (12.0, -6.0, 12.0, 12.0),
        (-24.0, 12.0, 12.0, 12.0), (-6.0, 14.0, 12.0, 10.0), (12.0, 12.0, 12.0, 12.0),
        (-5.0, -5.0, 10.0, 10.0),
    ];
    for (i, (x, z, sx, sz)) in blocks.iter().enumerate() {
        // Every block is a room with two doors, so the plan is a real maze
        // rather than a set of solid pillars.
        let doors = match i % 4 {
            0 => DOOR_NZ | DOOR_PX,
            1 => DOOR_PZ | DOOR_NX,
            2 => DOOR_NZ | DOOR_NX,
            _ => DOOR_PZ | DOOR_PX,
        };
        b.room(*x, *z, *sx, *sz, 0.0, ch, doors, Mat::Bunker, Mat::TileFloor, false);
    }
    // Command centre in the middle block: screens, a table, hard cover.
    b.decor(-3.0, 0.0, -4.0, 6.0, 1.0, 3.0, Mat::ControlPanel);
    b.boxc(0.0, 0.0, 0.0, 3.4, 1.0, 2.2, Mat::ControlPanel).with_scale(1.4);
    for i in 0..3 { b.decor(-4.0 + i as f32 * 3.0, 1.4, -4.6, 2.4, 1.4, 0.2, Mat::Screen); }

    // ---- Lower deck: generator hall, server rows, storage.
    b.floor(-26.0, -26.0, 52.0, 52.0, lower, Mat::ConcreteFloor);
    b.wall_x(-26.0, -26.0, 52.0, lower, ch, Mat::Bunker);
    b.wall_x(-26.0, 26.0, 52.0, lower, ch, Mat::Bunker);
    b.wall_z(-26.0, -26.0, 52.0, lower, ch, Mat::Bunker);
    b.wall_z(26.0, -26.0, 52.0, lower, ch, Mat::Bunker);
    // Server rows: long low cover, three parallel aisles.
    for i in 0..3 {
        let x = -16.0 + i as f32 * 12.0;
        b.boxx(x, lower, -20.0, 4.0, 2.4, 16.0, Mat::ControlPanel).with_scale(2.0);
        b.boxx(x, lower, 6.0, 4.0, 2.4, 16.0, Mat::ControlPanel).with_scale(2.0);
        b.decor(x + 0.1, lower + 0.4, -20.2, 3.8, 1.6, 0.2, Mat::Screen);
        b.decor(x + 0.1, lower + 0.4, 21.8, 3.8, 1.6, 0.2, Mat::Screen);
    }
    // Generator hall in the south-west with real machinery to fight around.
    tank(b, -19.0, lower, 19.0, 2.6, 3.0, Mat::MetalRust);
    tank(b, -12.0, lower, 21.0, 2.2, 3.0, Mat::PipeMetal);
    b.boxc(-18.0, lower, 12.0, 7.0, 2.6, 3.0, Mat::MetalPanel).with_scale(2.4);
    for i in 0..6 { b.barrel(-24.0 + (i % 3) as f32 * 1.1, lower, 24.0 + (i / 3) as f32 * 1.1, Mat::BarrelRust); }
    b.crates(20.0, lower, 20.0, 1.3, 2, Mat::WoodCrate);
    b.crates(20.0, lower, -20.0, 1.3, 3, Mat::WoodCrate);
    // Lighting strips so the lower deck is legible without being bright.
    for i in 0..5 {
        for j in 0..5 {
            b.decor(-22.0 + i as f32 * 11.0, lower + ch - 0.35, -22.0 + j as f32 * 11.0, 3.0, 0.25, 0.5, Mat::WindowLit);
        }
    }

    // ---- Four stairwells linking the decks, one per quadrant.
    for (i, (sx, sz)) in wells.iter().enumerate() {
        let ax = [RampAxis::PosZ, RampAxis::NegZ, RampAxis::PosZ, RampAxis::NegZ][i];
        b.stairs(*sx + 0.2, lower, *sz + 0.2, 4.0, 4.0, 4.24, ax, Mat::ConcreteFloor);
    }

    // ---- Surface access shafts at the two spawn ends, so the map has a
    //      recognisable "in" and "out" even though it is fully enclosed.
    for (ex, ez) in [(-27.0f32, -27.0f32), (27.0, 27.0)] {
        b.decor(ex - 1.6, 2.6, ez - 1.6, 3.2, 0.7, 3.2, Mat::Mesh);
        b.decor(ex - 1.4, 2.9, ez - 1.4, 2.8, 0.2, 2.8, Mat::WindowLit);
    }

    b.spawn_cluster(-27.0, 0.0, 0.0, 90.0, Team::Phantom, 7, 2.0, true);
    b.spawn_cluster(-21.0, lower, 6.0, 90.0, Team::Phantom, 5, 2.4, false);
    b.spawn_cluster(27.0, 0.0, 0.0, -90.0, Team::Vanguard, 7, 2.0, true);
    b.spawn_cluster(21.0, lower, -6.0, -90.0, Team::Vanguard, 5, 2.4, false);
    b.spawn_cluster(0.0, 0.0, -27.0, 180.0, Team::None, 4, 2.0, false);
    b.spawn_cluster(0.0, 0.0, 27.0, 0.0, Team::None, 4, 2.0, false);
    b.spawn_cluster(0.0, lower, -23.0, 180.0, Team::None, 4, 2.0, false);

    b.dom("A", 0.0, 0.1, 0.0, 5.0);
    b.dom("B", -20.0, lower + 0.1, 16.0, 5.0);
    b.dom("C", 21.0, lower + 0.1, -14.0, 5.0);
    b.site("A", 0.0, 0.1, 0.0, 4.5);
    b.site("B", -20.0, lower + 0.1, 16.0, 4.5);

    b.pickup(0.0, 0.1, -9.0, PickupKind::Ammo);
    b.pickup(0.0, 0.1, 9.0, PickupKind::Ammo);
    b.pickup(0.0, lower + 0.1, 0.0, PickupKind::Weapon);
    b.pickup(-22.0, lower + 0.1, -24.0, PickupKind::Armor);
    b.pickup(22.0, lower + 0.1, 24.0, PickupKind::Armor);
    b.pickup(-18.0, 0.1, -18.0, PickupKind::Grenade);
    b.pickup(18.0, 0.1, 18.0, PickupKind::Health);

    // Clutter the open ground; see MapBuilder::dress_open_ground.
    b.dress_open_ground(6, 0x1A0C, &[
        CoverPiece::Crates(Mat::WoodCrate, 1.3),
        CoverPiece::Barrels(Mat::BarrelRust),
    ]);
}

// ================================================================== JUNCTION

/// A marshalling yard. Freight cars form the cover, and because they are
/// arranged in long rakes the sightlines are all parallel until you cross one.
fn junction(b: &mut MapBuilder) {
    b.set_env(env_railyard());
    b.defaults(Mat::Corrugated, Mat::Gravel);
    b.playspace(-46.0, -32.0, 46.0, 32.0, 0.0, 34.0);
    b.apron(Mat::Gravel, 0.0);

    b.floor(-46.0, -32.0, 92.0, 64.0, 0.0, Mat::Gravel);

    // --- Five tracks running along X, with sleepers picked out in the floor.
    let tracks = [-22.0f32, -11.0, 0.0, 11.0, 22.0];
    for tz in tracks {
        b.floor(-46.0, tz - 1.6, 92.0, 3.2, 0.03, Mat::DirtRoad);
        b.decor(-46.0, 0.03, tz - 0.75, 92.0, 0.16, 0.16, Mat::PipeMetal);
        b.decor(-46.0, 0.03, tz + 0.6, 92.0, 0.16, 0.16, Mat::PipeMetal);
    }

    // --- Rakes of freight cars. Deliberately gapped so each track has two or
    //     three crossing points, and the gaps do not line up between tracks.
    let rakes: [(f32, f32, f32, Mat); 14] = [
        (-42.0, -22.0, 14.0, Mat::ShippingRed), (-20.0, -22.0, 16.0, Mat::ShippingBlue), (10.0, -22.0, 20.0, Mat::ShippingGreen),
        (-38.0, -11.0, 18.0, Mat::ShippingGreen), (-8.0, -11.0, 14.0, Mat::ShippingRed), (18.0, -11.0, 16.0, Mat::ShippingBlue),
        (-44.0, 0.0, 12.0, Mat::ShippingBlue), (-16.0, 0.0, 14.0, Mat::ShippingGreen), (22.0, 0.0, 18.0, Mat::ShippingRed),
        (-40.0, 11.0, 16.0, Mat::ShippingRed), (-10.0, 11.0, 18.0, Mat::ShippingBlue), (20.0, 11.0, 14.0, Mat::ShippingGreen),
        (-30.0, 22.0, 20.0, Mat::ShippingGreen), (4.0, 22.0, 22.0, Mat::ShippingRed),
    ];
    for (x, z, len, mat) in rakes {
        // Flatbed underframe, then the box body: two useful cover heights.
        b.boxx(x, 0.0, z - 1.5, len, 1.1, 3.0, Mat::MetalRust).with_scale(2.0);
        b.boxx(x + 0.3, 1.1, z - 1.4, len - 0.6, 2.9, 2.8, mat).with_scale(2.8);
        // A stepped ladder at one end so the roofs are a real, earned route
        // rather than decoration. Shallow enough that bots will use it too.
        b.stairs(x - 3.0, 0.0, z - 0.9, 1.6, 3.0, 4.0, RampAxis::PosX, Mat::MetalPlateDiamond);
    }

    // --- Signal tower: the tallest structure, overlooking the whole yard.
    b.tower(-4.0, -30.0, 8.0, 8.0, 0.0, 3, Mat::BrickRed, Mat::WoodFloor, Mat::RoofMetal);
    b.boxx(-4.0, 3.0 * STOREY - 1.5, -22.4, 8.0, 1.3, 0.2, Mat::Glass)
        .with_flags(BrushFlags::BULLET_CLIP | BrushFlags::CUTOUT);

    // --- Water tower: a mid-height perch reached by one exposed stair.
    for (px, pz) in [(30.0f32, -27.0f32)] {
        for (ox, oz) in [(-3.0f32, -3.0f32), (3.0, -3.0), (-3.0, 3.0), (3.0, 3.0)] {
            b.pillar(px + ox, 0.0, pz + oz, 0.7, 7.0, Mat::PipeMetal);
        }
        b.floor(px - 4.0, pz - 4.0, 8.0, 8.0, 7.0, Mat::MetalPlateDiamond);
        b.wall_x(px - 4.0, pz - 4.0, 8.0, 7.0, 1.0, Mat::PipeMetal);
        b.wall_x(px - 4.0, pz + 4.0, 8.0, 7.0, 1.0, Mat::PipeMetal);
        b.wall_z(px - 4.0, pz - 4.0, 8.0, 7.0, 1.0, Mat::PipeMetal);
        b.wall_z(px + 4.0, pz - 4.0, 2.7, 7.0, 1.0, Mat::PipeMetal);
        b.wall_z(px + 4.0, pz + 1.3, 2.7, 7.0, 1.0, Mat::PipeMetal);
        tank(b, px, 9.3, pz, 3.0, 4.5, Mat::MetalRust);
        b.access_stair(pz, px + 4.0, 0.0, 7.0, true, false, Mat::MetalPlateDiamond);
    }

    // --- Depot shed: the only substantial interior, spanning three tracks.
    b.floor(-44.0, 14.0, 22.0, 16.0, 0.05, Mat::ConcreteFloor);
    b.wall_x(-44.0, 30.0, 22.0, 0.0, 8.0, Mat::Corrugated);
    b.wall_z(-44.0, 14.0, 16.0, 0.0, 8.0, Mat::Corrugated);
    b.wall_x(-44.0, 14.0, 6.0, 0.0, 8.0, Mat::Corrugated);
    b.wall_x(-32.0, 14.0, 10.0, 0.0, 8.0, Mat::Corrugated);
    b.wall_z(-22.0, 14.0, 16.0, 0.0, 8.0, Mat::Corrugated);
    b.ceiling(-44.0, 14.0, 22.0, 16.0, 8.0, Mat::RoofMetal);
    b.catwalk(-43.0, 4.4, 15.0, 20.0, 2.6, Mat::Grating, true, &[-33.0]);
    b.access_stair(-33.0, 17.6, 0.05, 4.4, false, false, Mat::MetalPlateDiamond);
    b.crates(-38.0, 0.05, 24.0, 1.4, 2, Mat::WoodCrate);
    truck(b, -28.0, 0.05, 22.0, false, Mat::MetalPanel);

    // --- Loading platform on the east side: raised, long, contested.
    b.floor(28.0, 6.0, 16.0, 22.0, 1.2, Mat::ConcreteFloor);
    b.ramp(24.0, 0.0, 12.0, 4.0, 1.2, 6.0, RampAxis::PosX, Mat::ConcreteFloor);
    b.ramp(28.0, 0.0, 2.0, 6.0, 1.2, 4.0, RampAxis::PosZ, Mat::ConcreteFloor);
    b.crates(32.0, 1.2, 12.0, 1.4, 3, Mat::WoodCrate);
    b.crates(38.0, 1.2, 22.0, 1.4, 2, Mat::WoodCrate);
    b.container(34.0, 1.2, 18.0, true, Mat::ShippingBlue);
    b.sandbags(28.0, 1.2, 5.0, 16.0, 1.2);

    // --- Yard clutter to break the parallel lanes at the ends.
    for (x, z) in [(-44.0f32, -30.0f32), (40.0, -8.0), (-16.0, 28.0), (14.0, -30.0)] {
        b.crates(x, 0.0, z, 1.4, 2, Mat::WoodCrate);
    }
    for i in 0..8 { b.barrel(-6.0 + (i % 4) as f32 * 1.1, 0.0, 27.0 + (i / 4) as f32 * 1.1, Mat::BarrelRust); }
    b.fence_x(-46.0, 0.0, -31.0, 92.0, 2.6);
    b.fence_x(-46.0, 0.0, 31.0, 92.0, 2.6);

    b.spawn_cluster(-41.0, 0.0, -27.0, 30.0, Team::Phantom, 8, 4.5, true);
    b.spawn_cluster(-41.0, 0.05, 22.0, 60.0, Team::Phantom, 5, 4.0, false);
    b.spawn_cluster(41.0, 1.25, 20.0, -150.0, Team::Vanguard, 8, 4.0, true);
    b.spawn_cluster(41.0, 0.0, -22.0, -120.0, Team::Vanguard, 5, 4.0, false);
    b.spawn_cluster(0.0, 0.0, -28.0, 180.0, Team::None, 5, 5.0, false);
    b.spawn_cluster(-2.0, 0.0, 28.0, 0.0, Team::None, 5, 5.0, false);

    b.dom("A", -33.0, 0.1, 22.0, 5.5);
    b.dom("B", 0.0, 0.1, 5.5, 6.0);
    b.dom("C", 34.0, 1.3, 16.0, 5.5);
    b.site("A", -33.0, 0.1, 22.0, 4.5);
    b.site("B", 34.0, 1.3, 16.0, 4.5);

    b.pickup(0.0, 0.15, -5.5, PickupKind::Ammo);
    b.pickup(0.0, 0.15, 16.5, PickupKind::Ammo);
    b.pickup(0.0, 3.0 * STOREY + 0.1, -26.0, PickupKind::Weapon);
    b.pickup(30.0, 7.1, -27.0, PickupKind::Armor);
    b.pickup(-33.0, 4.5, 22.0, PickupKind::Armor);
    b.pickup(-20.0, 0.15, 0.0, PickupKind::Grenade);
    b.pickup(20.0, 0.15, -11.0, PickupKind::Health);

    // Clutter the open ground; see MapBuilder::dress_open_ground.
    b.dress_open_ground(22, 0x1A09, &[
        CoverPiece::Container(Mat::ShippingRed),
        CoverPiece::Crates(Mat::WoodCrate, 1.5),
        CoverPiece::Barrels(Mat::BarrelRust),
        CoverPiece::Block(Mat::Concrete, 3.2, 1.4),
        CoverPiece::Pipes(Mat::PipeMetal),
    ]);
}

// ================================================================== OVERPASS

/// A fortified crossing under a collapsed flyover. Small, fast, and built
/// almost entirely from chokepoints and the ways around them.
fn overpass(b: &mut MapBuilder) {
    b.set_env(env_overpass());
    b.defaults(Mat::Concrete, Mat::Asphalt);
    b.playspace(-34.0, -28.0, 34.0, 28.0, 0.0, 30.0);
    b.apron(Mat::Asphalt, 0.0);

    b.floor(-34.0, -28.0, 68.0, 56.0, 0.0, Mat::Asphalt);
    b.floor(-34.0, -7.0, 68.0, 14.0, 0.02, Mat::DirtRoad);
    b.floor(-34.0, -0.4, 68.0, 0.8, 0.04, Mat::YellowPaint);

    // --- The flyover deck at 8 m, broken in the middle so it is two perches
    //     rather than one continuous sniper run.
    let dy = 6.8;
    b.floor(-34.0, 8.0, 26.0, 9.0, dy, Mat::Concrete);
    b.floor(12.0, 8.0, 22.0, 9.0, dy, Mat::Concrete);
    b.wall_x(-34.0, 8.0, 26.0, dy, 1.1, Mat::Concrete);
    b.wall_x(-34.0, 17.0, 26.0, dy, 1.1, Mat::Concrete);
    b.wall_x(12.0, 8.0, 22.0, dy, 1.1, Mat::Concrete);
    b.wall_x(12.0, 17.0, 22.0, dy, 1.1, Mat::Concrete);
    // Collapsed span: a rubble ramp climbing from the road onto the west deck.
    b.ramp(-8.0, 0.0, 9.0, 14.0, dy, 7.0, RampAxis::NegX, Mat::Rock);
    b.decor(-13.0, 0.0, 6.0, 4.0, 2.4, 2.6, Mat::Rock);
    // Piers holding the deck up: hard cover along the road.
    for x in [-28.0f32, -16.0, 18.0, 28.0] {
        b.boxc(x, 0.0, 12.5, 3.0, dy, 3.0, Mat::Concrete).with_scale(3.0);
    }
    // A long open stair climbs to the east deck's open western end.
    b.access_stair(12.5, 12.0, 0.0, dy, true, true, Mat::Concrete);

    // --- Checkpoint bunkers straddling the road: the central choke.
    for (bx, bz) in [(-6.0f32, -18.0f32), (0.0, 2.0)] {
        b.room(bx, bz, 10.0, 8.0, 0.0, 3.2, DOOR_NX | DOOR_PX, Mat::Bunker, Mat::ConcreteFloor, true);
        b.wall_x_window(bx, bz, 10.0, 0.0, 3.2, 1.1, 1.9, Mat::Bunker);
        b.wall_x_window(bx, bz + 8.0, 10.0, 0.0, 3.2, 1.1, 1.9, Mat::Bunker);
        b.floor(bx, bz, 10.0, 8.0, 3.2, Mat::ConcreteFloor);
        b.wall_x(bx, bz, 10.0, 3.2, 0.9, Mat::Bunker);
        b.wall_x(bx, bz + 8.0, 10.0, 3.2, 0.9, Mat::Bunker);
        b.wall_z(bx, bz, 8.0, 3.2, 0.9, Mat::Bunker);
        b.wall_z(bx + 10.0, bz, 8.0, 3.2, 0.9, Mat::Bunker);
        step_up(b, bx + 10.2, 0.0, bz + 1.0, 1.8, 5.0, 3.2, RampAxis::NegZ, Mat::MetalPlateDiamond);
        b.sandbags(bx - 2.5, 0.0, bz - 1.5, 15.0, 1.2);
    }

    // --- Toll booth row: a line of small hard covers across the road.
    for i in 0..4 {
        let x = -18.0 + i as f32 * 12.0;
        b.boxc(x, 0.0, -3.0, 2.4, 2.8, 3.2, Mat::Plaster).with_scale(1.8);
        b.decor(x - 1.0, 1.2, -4.7, 2.0, 1.2, 0.2, Mat::Glass);
        b.boxc(x, 2.8, -3.0, 4.0, 0.3, 5.0, Mat::MetalPanel).with_scale(2.0);
    }

    // --- Wrecked traffic: the road's cover, arranged to stagger sightlines.
    truck(b, -24.0, 0.0, -1.0, true, Mat::MetalRust);
    truck(b, -8.0, 0.0, 4.0, true, Mat::BluePaint);
    truck(b, 8.0, 0.0, -4.0, true, Mat::RedPaint);
    truck(b, 24.0, 0.0, 2.0, true, Mat::MetalPanel);
    // Kept clear of the bunker footprints so the road cover does not end up
    // inside a building.
    for (x, z) in [(-30.0f32, 4.0f32), (-14.0, -6.0), (12.0, 5.0), (18.0, -6.0), (30.0, 4.0)] {
        b.barrier(x, 0.0, z, 5.0, 0.7);
    }

    // --- South flank: a service yard that bypasses the road entirely.
    b.floor(-30.0, -26.0, 26.0, 14.0, 0.0, Mat::ConcreteFloor);
    b.fence_x(-30.0, 0.0, -26.0, 26.0, 2.8);
    b.container(-26.0, 0.0, -22.0, true, Mat::ShippingGreen);
    b.container(-26.0, 2.65, -22.0, true, Mat::ShippingRed);
    b.container(-14.0, 0.0, -24.0, false, Mat::ShippingBlue);
    b.crates(-20.0, 0.0, -16.0, 1.4, 2, Mat::WoodCrate);
    b.room(6.0, -26.0, 14.0, 12.0, 0.0, 4.0, DOOR_PZ | DOOR_NX, Mat::Corrugated, Mat::ConcreteFloor, true);
    b.floor(6.0, -26.0, 14.0, 12.0, 4.0, Mat::RoofMetal);
    step_up(b, 20.2, 0.0, -25.0, 1.8, 5.6, 4.0, RampAxis::NegZ, Mat::MetalPlateDiamond);
    for i in 0..6 { b.barrel(22.0 + (i % 3) as f32 * 1.1, 0.0, -20.0 + (i / 3) as f32 * 1.1, Mat::BarrelRust); }

    // --- North flank: embankment behind the flyover, reached by two ramps.
    b.floor(-32.0, 20.0, 64.0, 8.0, 2.2, Mat::Dirt);
    b.ramp(-32.0, 0.0, 16.0, 8.0, 2.2, 4.0, RampAxis::PosZ, Mat::Dirt);
    b.ramp(24.0, 0.0, 16.0, 8.0, 2.2, 4.0, RampAxis::PosZ, Mat::Dirt);
    b.sandbags(-30.0, 2.2, 25.0, 12.0, 1.2);
    b.sandbags(18.0, 2.2, 25.0, 12.0, 1.2);
    b.crates(0.0, 2.2, 24.0, 1.3, 2, Mat::WoodCrate);

    b.spawn_cluster(-30.0, 0.0, -2.0, 90.0, Team::Phantom, 8, 4.0, true);
    b.spawn_cluster(-27.0, 0.0, -20.0, 60.0, Team::Phantom, 5, 3.5, false);
    b.spawn_cluster(30.0, 0.0, 2.0, -90.0, Team::Vanguard, 8, 4.0, true);
    b.spawn_cluster(26.0, 2.3, 24.0, -120.0, Team::Vanguard, 5, 3.5, false);
    b.spawn_cluster(0.0, 0.0, -22.0, 180.0, Team::None, 4, 4.0, false);
    b.spawn_cluster(0.0, 2.3, 24.0, 0.0, Team::None, 4, 4.0, false);

    b.dom("A", -20.0, 0.1, -20.0, 5.0);
    b.dom("B", 5.0, 0.1, 6.0, 5.5);
    b.dom("C", 13.0, 0.1, -20.0, 5.0);
    b.site("A", 5.0, 0.1, 6.0, 5.0);
    b.site("B", -20.0, 0.1, -20.0, 4.5);

    b.pickup(-10.0, 0.15, 0.0, PickupKind::Ammo);
    b.pickup(10.0, 0.15, 0.0, PickupKind::Ammo);
    b.pickup(-24.0, dy + 0.1, 12.5, PickupKind::Weapon);
    b.pickup(22.0, dy + 0.1, 12.5, PickupKind::Armor);
    b.pickup(-20.0, 0.15, -19.0, PickupKind::Grenade);
    b.pickup(13.0, 4.2, -20.0, PickupKind::Health);
    b.pickup(0.0, 2.3, 24.0, PickupKind::Grenade);

    // Clutter the open ground; see MapBuilder::dress_open_ground.
    b.dress_open_ground(30, 0x1A0A, &[
        CoverPiece::Block(Mat::Concrete, 3.4, 1.3),
        CoverPiece::Sandbags(Mat::Sandbag),
        CoverPiece::Barrels(Mat::Barrel),
        CoverPiece::Crates(Mat::WoodCrate, 1.4),
        CoverPiece::Container(Mat::ShippingBlue),
    ]);
}
