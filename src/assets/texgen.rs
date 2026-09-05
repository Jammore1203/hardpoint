//! Procedural texture generation.
//!
//! The game ships no image files. Every surface is generated at load into one
//! texture array, which means the whole world draws in a single call, the
//! download is a few hundred kilobytes of source, and texture quality is a
//! setting rather than a separate set of assets.
//!
//! All noise here is periodic, so every texture tiles seamlessly.

use super::materials::{Mat, MAT_COUNT};

/// One layer's pixels for every mip level.
pub struct LayerMips {
    pub mips: Vec<Vec<u8>>,
}

pub struct TextureArray {
    pub size: u32,
    pub mip_count: u32,
    pub layers: Vec<LayerMips>,
}

impl TextureArray {
    /// Total bytes, for the memory readout in the settings screen.
    pub fn bytes(&self) -> usize {
        self.layers.iter().map(|l| l.mips.iter().map(|m| m.len()).sum::<usize>()).sum()
    }
}

// ------------------------------------------------------------------- noise

#[inline]
fn hash2(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8DA6_B343)
        ^ (y as u32).wrapping_mul(0xD8163841)
        ^ seed.wrapping_mul(0xCB1AB31F);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2545_F491);
    h ^= h >> 13;
    (h & 0xFFFFFF) as f32 / 16777215.0
}

#[inline]
fn smooth(t: f32) -> f32 { t * t * (3.0 - 2.0 * t) }

/// Value noise that wraps every `period` units.
fn vnoise(x: f32, y: f32, period: i32, seed: u32) -> f32 {
    let xi = x.floor() as i32;
    let yi = y.floor() as i32;
    let xf = x - xi as f32;
    let yf = y - yi as f32;
    let wrap = |v: i32| v.rem_euclid(period.max(1));
    let (x0, y0) = (wrap(xi), wrap(yi));
    let (x1, y1) = (wrap(xi + 1), wrap(yi + 1));
    let a = hash2(x0, y0, seed);
    let b = hash2(x1, y0, seed);
    let c = hash2(x0, y1, seed);
    let d = hash2(x1, y1, seed);
    let u = smooth(xf);
    let v = smooth(yf);
    (a * (1.0 - u) + b * u) * (1.0 - v) + (c * (1.0 - u) + d * u) * v
}

/// Fractal noise, still tiling.
fn fbm(x: f32, y: f32, base_period: i32, octaves: u32, seed: u32) -> f32 {
    let mut sum = 0.0;
    let mut amp = 0.5;
    let mut total = 0.0;
    let mut period = base_period;
    let mut freq = 1.0;
    for o in 0..octaves {
        sum += vnoise(x * freq, y * freq, period, seed.wrapping_add(o * 7919)) * amp;
        total += amp;
        amp *= 0.5;
        freq *= 2.0;
        period *= 2;
    }
    if total > 0.0 { sum / total } else { 0.0 }
}

/// Cell/worley noise for gravel and rock; returns distance to the nearest of
/// a scattered set of points, normalised.
fn worley(x: f32, y: f32, period: i32, seed: u32) -> f32 {
    let xi = x.floor() as i32;
    let yi = y.floor() as i32;
    let mut best = 4.0f32;
    for dy in -1..=1 {
        for dx in -1..=1 {
            let cx = (xi + dx).rem_euclid(period.max(1));
            let cy = (yi + dy).rem_euclid(period.max(1));
            let px = (xi + dx) as f32 + hash2(cx, cy, seed);
            let py = (yi + dy) as f32 + hash2(cx, cy, seed ^ 0x1234_5678);
            let d = (px - x) * (px - x) + (py - y) * (py - y);
            if d < best { best = d; }
        }
    }
    best.sqrt().min(1.0)
}

// ----------------------------------------------------------------- helpers

struct Painter {
    size: u32,
    px: Vec<u8>,
}

impl Painter {
    fn new(size: u32) -> Painter {
        Painter { size, px: vec![0u8; (size * size * 4) as usize] }
    }

    #[inline]
    fn set(&mut self, x: u32, y: u32, r: f32, g: f32, b: f32, a: f32) {
        let i = ((y * self.size + x) * 4) as usize;
        self.px[i] = (r.clamp(0.0, 1.0) * 255.0) as u8;
        self.px[i + 1] = (g.clamp(0.0, 1.0) * 255.0) as u8;
        self.px[i + 2] = (b.clamp(0.0, 1.0) * 255.0) as u8;
        self.px[i + 3] = (a.clamp(0.0, 1.0) * 255.0) as u8;
    }

    /// Fills using a closure returning (luminance, alpha) in 0..1, tinted.
    fn shade(&mut self, tint: [u8; 3], mut f: impl FnMut(f32, f32, u32, u32) -> (f32, f32)) {
        let s = self.size as f32;
        let base = [tint[0] as f32 / 255.0, tint[1] as f32 / 255.0, tint[2] as f32 / 255.0];
        for y in 0..self.size {
            for x in 0..self.size {
                let (lum, alpha) = f(x as f32 / s, y as f32 / s, x, y);
                self.set(x, y, base[0] * lum, base[1] * lum, base[2] * lum, alpha);
            }
        }
    }
}

/// Box-filtered mip chain. Generated on the CPU because the alternative is a
/// compute pass we would only ever run once.
fn build_mips(base: Vec<u8>, size: u32) -> Vec<Vec<u8>> {
    let mut out = vec![base];
    let mut w = size;
    while w > 1 {
        let prev = out.last().unwrap();
        let nw = w / 2;
        let mut next = vec![0u8; (nw * nw * 4) as usize];
        for y in 0..nw {
            for x in 0..nw {
                for c in 0..4 {
                    let i00 = (((y * 2) * w + x * 2) * 4 + c) as usize;
                    let i10 = (((y * 2) * w + x * 2 + 1) * 4 + c) as usize;
                    let i01 = (((y * 2 + 1) * w + x * 2) * 4 + c) as usize;
                    let i11 = (((y * 2 + 1) * w + x * 2 + 1) * 4 + c) as usize;
                    let sum = prev[i00] as u32 + prev[i10] as u32 + prev[i01] as u32 + prev[i11] as u32;
                    next[((y * nw + x) * 4 + c) as usize] = (sum / 4) as u8;
                }
            }
        }
        out.push(next);
        w = nw;
    }
    out
}

// --------------------------------------------------------------- generators

/// Rough, speckled surface: concrete, plaster, stone.
fn gen_rough(p: &mut Painter, tint: [u8; 3], grain: f32, blotch: f32, seed: u32) {
    let period = 16;
    p.shade(tint, |u, v, _, _| {
        let n = fbm(u * period as f32, v * period as f32, period, 4, seed);
        let fine = vnoise(u * 64.0, v * 64.0, 64, seed ^ 0x99);
        let lum = 0.78 + (n - 0.5) * blotch + (fine - 0.5) * grain;
        (lum, 1.0)
    });
}

/// Regular brick courses with mortar.
fn gen_brick(p: &mut Painter, tint: [u8; 3], rows: f32, mortar: [u8; 3], seed: u32) {
    let mortar_l = [mortar[0] as f32 / 255.0, mortar[1] as f32 / 255.0, mortar[2] as f32 / 255.0];
    let size = p.size;
    let base = [tint[0] as f32 / 255.0, tint[1] as f32 / 255.0, tint[2] as f32 / 255.0];
    for y in 0..size {
        for x in 0..size {
            let u = x as f32 / size as f32;
            let v = y as f32 / size as f32;
            let row = (v * rows).floor();
            let offset = if (row as i32) % 2 == 0 { 0.0 } else { 0.5 };
            let bu = (u * rows * 0.5 + offset).fract();
            let bv = (v * rows).fract();
            let joint = 0.06;
            let is_mortar = bu < joint || bu > 1.0 - joint || bv < joint * 2.0 || bv > 1.0 - joint * 2.0;
            let brick_id = hash2((u * rows * 0.5 + offset) as i32, row as i32, seed);
            let n = vnoise(u * 48.0, v * 48.0, 48, seed);
            if is_mortar {
                let l = 0.82 + (n - 0.5) * 0.16;
                p.set(x, y, mortar_l[0] * l, mortar_l[1] * l, mortar_l[2] * l, 1.0);
            } else {
                let l = 0.72 + brick_id * 0.30 + (n - 0.5) * 0.14;
                p.set(x, y, base[0] * l, base[1] * l, base[2] * l, 1.0);
            }
        }
    }
}

/// Rectangular panels with recessed seams and rivets: metal, hulls, bunkers.
fn gen_panel(p: &mut Painter, tint: [u8; 3], divisions: f32, rivets: bool, rust: f32, seed: u32) {
    p.shade(tint, |u, v, _, _| {
        let pu = (u * divisions).fract();
        let pv = (v * divisions).fract();
        let seam = 0.045;
        let edge = pu < seam || pu > 1.0 - seam || pv < seam || pv > 1.0 - seam;
        let cell = hash2((u * divisions) as i32, (v * divisions) as i32, seed);
        let mut lum = 0.80 + cell * 0.12;
        if edge { lum *= 0.62; }
        // A soft highlight along the top of each panel reads as a bevel.
        if pv < seam * 3.0 && !edge { lum *= 1.12; }
        if rivets {
            let rx = (pu - 0.5).abs();
            let ry = (pv - 0.5).abs();
            if rx > 0.40 && ry > 0.40 { lum *= 1.25; }
        }
        let grain = vnoise(u * 96.0, v * 96.0, 96, seed ^ 0x55);
        lum += (grain - 0.5) * 0.06;
        if rust > 0.0 {
            let r = fbm(u * 10.0, v * 10.0, 10, 4, seed ^ 0xAB);
            if r > 0.55 { lum *= 1.0 - (r - 0.55) * rust * 2.2; }
        }
        (lum, 1.0)
    });
}

/// Vertical corrugations.
fn gen_corrugated(p: &mut Painter, tint: [u8; 3], ribs: f32, seed: u32) {
    p.shade(tint, |u, v, _, _| {
        let wave = (u * ribs * std::f32::consts::TAU).sin();
        let mut lum = 0.80 + wave * 0.20;
        let streak = vnoise(u * 8.0, v * 40.0, 40, seed);
        lum += (streak - 0.5) * 0.10;
        (lum, 1.0)
    });
}

/// Loose granular ground: gravel, sand, snow, dirt.
fn gen_granular(p: &mut Painter, tint: [u8; 3], cells: f32, contrast: f32, seed: u32) {
    p.shade(tint, |u, v, _, _| {
        let w = worley(u * cells, v * cells, cells as i32, seed);
        let n = fbm(u * 12.0, v * 12.0, 12, 3, seed ^ 0x77);
        let lum = 0.72 + (1.0 - w) * contrast + (n - 0.5) * 0.18;
        (lum, 1.0)
    });
}

/// Vegetation: clumpy, high-contrast, slightly varied in hue.
fn gen_foliage(p: &mut Painter, tint: [u8; 3], cutout: bool, seed: u32) {
    let base = [tint[0] as f32 / 255.0, tint[1] as f32 / 255.0, tint[2] as f32 / 255.0];
    let size = p.size;
    for y in 0..size {
        for x in 0..size {
            let u = x as f32 / size as f32;
            let v = y as f32 / size as f32;
            let leaf = fbm(u * 14.0, v * 14.0, 14, 4, seed);
            let detail = vnoise(u * 40.0, v * 40.0, 40, seed ^ 0x31);
            let lum = 0.55 + leaf * 0.55 + (detail - 0.5) * 0.24;
            let alpha = if cutout { if leaf > 0.44 { 1.0 } else { 0.0 } } else { 1.0 };
            // Vary the green toward yellow in the lit patches.
            let warm = (leaf - 0.5).max(0.0) * 0.5;
            p.set(x, y, base[0] * lum * (1.0 + warm), base[1] * lum, base[2] * lum * (1.0 - warm * 0.5), alpha);
        }
    }
}

/// Woven fabric or sandbags.
fn gen_woven(p: &mut Painter, tint: [u8; 3], threads: f32, lumpy: f32, seed: u32) {
    p.shade(tint, |u, v, _, _| {
        let a = (u * threads * std::f32::consts::TAU).sin();
        let b = (v * threads * std::f32::consts::TAU).sin();
        let weave = (a * b) * 0.5 + 0.5;
        let lump = fbm(u * 6.0, v * 6.0, 6, 3, seed);
        let lum = 0.70 + weave * 0.22 + (lump - 0.5) * lumpy;
        (lum, 1.0)
    });
}

/// Planks with visible grain.
fn gen_wood(p: &mut Painter, tint: [u8; 3], planks: f32, vertical: bool, seed: u32) {
    p.shade(tint, |u, v, _, _| {
        let (along, across) = if vertical { (v, u) } else { (u, v) };
        let plank = (across * planks).floor();
        let edge = (across * planks).fract();
        let id = hash2(plank as i32, 0, seed);
        let grain = fbm(along * 26.0 + id * 10.0, across * 90.0, 90, 3, seed);
        let mut lum = 0.72 + id * 0.20 + (grain - 0.5) * 0.30;
        if edge < 0.03 || edge > 0.97 { lum *= 0.62; }
        (lum, 1.0)
    });
}

/// Regular grid: tiles, diamond plate, gratings.
fn gen_grid(p: &mut Painter, tint: [u8; 3], cells: f32, thickness: f32, cutout: bool, seed: u32) {
    let base = [tint[0] as f32 / 255.0, tint[1] as f32 / 255.0, tint[2] as f32 / 255.0];
    let size = p.size;
    for y in 0..size {
        for x in 0..size {
            let u = x as f32 / size as f32;
            let v = y as f32 / size as f32;
            let cu = (u * cells).fract();
            let cv = (v * cells).fract();
            let bar = cu < thickness || cu > 1.0 - thickness || cv < thickness || cv > 1.0 - thickness;
            let n = vnoise(u * 60.0, v * 60.0, 60, seed);
            if cutout {
                let alpha = if bar { 1.0 } else { 0.0 };
                let lum = 0.75 + (n - 0.5) * 0.2;
                p.set(x, y, base[0] * lum, base[1] * lum, base[2] * lum, alpha);
            } else {
                let lum = if bar { 0.62 } else { 0.90 } + (n - 0.5) * 0.14;
                p.set(x, y, base[0] * lum, base[1] * lum, base[2] * lum, 1.0);
            }
        }
    }
}

/// Diagonal hazard stripes.
fn gen_stripes(p: &mut Painter, tint: [u8; 3], dark: [u8; 3], count: f32, seed: u32) {
    let a = [tint[0] as f32 / 255.0, tint[1] as f32 / 255.0, tint[2] as f32 / 255.0];
    let b = [dark[0] as f32 / 255.0, dark[1] as f32 / 255.0, dark[2] as f32 / 255.0];
    let size = p.size;
    for y in 0..size {
        for x in 0..size {
            let u = x as f32 / size as f32;
            let v = y as f32 / size as f32;
            let s = ((u + v) * count).fract();
            let n = vnoise(u * 50.0, v * 50.0, 50, seed);
            let wear = 0.86 + (n - 0.5) * 0.28;
            let c = if s < 0.5 { a } else { b };
            p.set(x, y, c[0] * wear, c[1] * wear, c[2] * wear, 1.0);
        }
    }
}

/// Camouflage blobs.
fn gen_camo(p: &mut Painter, tint: [u8; 3], seed: u32) {
    let base = [tint[0] as f32 / 255.0, tint[1] as f32 / 255.0, tint[2] as f32 / 255.0];
    let size = p.size;
    for y in 0..size {
        for x in 0..size {
            let u = x as f32 / size as f32;
            let v = y as f32 / size as f32;
            let a = fbm(u * 7.0, v * 7.0, 7, 3, seed);
            let b = fbm(u * 11.0, v * 11.0, 11, 3, seed ^ 0x5A5A);
            let lum = if a > 0.56 { 1.15 } else if b > 0.58 { 0.70 } else { 0.92 };
            let grain = vnoise(u * 80.0, v * 80.0, 80, seed);
            let l = lum + (grain - 0.5) * 0.08;
            p.set(x, y, base[0] * l, base[1] * l, base[2] * l, 1.0);
        }
    }
}

/// A lit surface: windows, screens, control panels.
fn gen_lit(p: &mut Painter, tint: [u8; 3], cells: f32, scanlines: bool, seed: u32) {
    p.shade(tint, |u, v, _, y| {
        let cu = (u * cells).fract();
        let cv = (v * cells).fract();
        let frame = cu < 0.08 || cu > 0.92 || cv < 0.08 || cv > 0.92;
        let on = hash2((u * cells) as i32, (v * cells) as i32, seed) > 0.35;
        let mut lum = if frame { 0.35 } else if on { 1.25 } else { 0.45 };
        if scanlines && y % 2 == 0 { lum *= 0.82; }
        let flick = vnoise(u * 20.0, v * 20.0, 20, seed ^ 0xF0);
        lum += (flick - 0.5) * 0.10;
        (lum, 1.0)
    });
}

/// Water: slow ripples, no transparency (it is decorative here).
fn gen_water(p: &mut Painter, tint: [u8; 3], seed: u32) {
    p.shade(tint, |u, v, _, _| {
        let a = fbm(u * 9.0, v * 9.0, 9, 3, seed);
        let b = (u * 22.0 + a * 4.0).sin() * 0.5 + 0.5;
        let lum = 0.72 + a * 0.30 + b * 0.16;
        (lum, 1.0)
    });
}

/// Builds every world material at the requested resolution.
pub fn generate_world_array(size: u32) -> TextureArray {
    let mut layers = Vec::with_capacity(MAT_COUNT);
    for i in 0..MAT_COUNT {
        let mat = Mat::from_index(i as u8);
        let tint = mat.tint();
        let seed = 0x1000u32.wrapping_add((i as u32).wrapping_mul(2654435761) % 100000);
        let mut p = Painter::new(size);

        use Mat::*;
        match mat {
            Concrete => gen_rough(&mut p, tint, 0.10, 0.22, seed),
            ConcreteWorn => gen_rough(&mut p, tint, 0.16, 0.34, seed),
            ConcretePanel => gen_panel(&mut p, tint, 2.0, false, 0.0, seed),
            Cinderblock => gen_brick(&mut p, tint, 6.0, [122, 120, 114], seed),
            BrickRed => gen_brick(&mut p, tint, 10.0, [168, 162, 150], seed),
            BrickPale => gen_brick(&mut p, tint, 9.0, [176, 172, 160], seed),
            Plaster => gen_rough(&mut p, tint, 0.06, 0.16, seed),
            StoneWall => gen_granular(&mut p, tint, 6.0, 0.30, seed),
            MetalPanel => gen_panel(&mut p, tint, 3.0, true, 0.0, seed),
            MetalRust => gen_panel(&mut p, tint, 2.0, true, 0.9, seed),
            MetalPlateDiamond => gen_grid(&mut p, tint, 8.0, 0.16, false, seed),
            Corrugated => gen_corrugated(&mut p, tint, 12.0, seed),
            Grating => gen_grid(&mut p, tint, 7.0, 0.20, true, seed),
            HullPainted => gen_panel(&mut p, tint, 2.0, true, 0.35, seed),
            PipeMetal => gen_corrugated(&mut p, tint, 3.0, seed),
            ShippingRed | ShippingBlue | ShippingGreen => gen_corrugated(&mut p, tint, 9.0, seed),
            Sand => gen_granular(&mut p, tint, 22.0, 0.16, seed),
            SandRock => gen_granular(&mut p, tint, 9.0, 0.28, seed),
            Dirt => gen_granular(&mut p, tint, 14.0, 0.26, seed),
            Gravel => gen_granular(&mut p, tint, 26.0, 0.34, seed),
            Grass => gen_foliage(&mut p, tint, false, seed),
            JungleFloor => gen_foliage(&mut p, tint, false, seed ^ 0x11),
            Snow => gen_granular(&mut p, tint, 18.0, 0.10, seed),
            SnowRock => gen_granular(&mut p, tint, 8.0, 0.24, seed),
            Asphalt => gen_granular(&mut p, tint, 30.0, 0.14, seed),
            ConcreteFloor => gen_rough(&mut p, tint, 0.09, 0.18, seed),
            TileFloor => gen_grid(&mut p, tint, 4.0, 0.03, false, seed),
            WoodFloor => gen_wood(&mut p, tint, 6.0, false, seed),
            Mud => gen_granular(&mut p, tint, 10.0, 0.22, seed),
            Cobble => gen_granular(&mut p, tint, 7.0, 0.36, seed),
            WoodCrate => gen_wood(&mut p, tint, 4.0, true, seed),
            WoodPlank => gen_wood(&mut p, tint, 5.0, false, seed),
            Sandbag => gen_woven(&mut p, tint, 5.0, 0.34, seed),
            Camo => gen_camo(&mut p, tint, seed),
            CamoDesert => gen_camo(&mut p, tint, seed ^ 0x22),
            CamoWinter => gen_camo(&mut p, tint, seed ^ 0x33),
            Tarp => gen_woven(&mut p, tint, 3.0, 0.26, seed),
            Canvas => gen_woven(&mut p, tint, 8.0, 0.18, seed),
            Glass => gen_lit(&mut p, tint, 2.0, false, seed),
            WindowLit => gen_lit(&mut p, tint, 3.0, false, seed),
            Screen => gen_lit(&mut p, tint, 1.0, true, seed),
            ControlPanel => gen_lit(&mut p, tint, 6.0, false, seed),
            Barrel | BarrelRust => gen_corrugated(&mut p, tint, 4.0, seed),
            Tire | Rubber => gen_granular(&mut p, tint, 20.0, 0.10, seed),
            HazardStripe => gen_stripes(&mut p, tint, [40, 38, 36], 6.0, seed),
            RedPaint | BluePaint | YellowPaint => gen_panel(&mut p, tint, 2.0, false, 0.25, seed),
            Sign => gen_panel(&mut p, tint, 1.0, false, 0.1, seed),
            RoofTile => gen_grid(&mut p, tint, 8.0, 0.06, false, seed),
            RoofMetal => gen_corrugated(&mut p, tint, 14.0, seed),
            Foliage => gen_foliage(&mut p, tint, true, seed),
            Rock => gen_granular(&mut p, tint, 5.0, 0.34, seed),
            Ice => gen_water(&mut p, tint, seed),
            WaterSurface => gen_water(&mut p, tint, seed ^ 0x9),
            Fabric => gen_woven(&mut p, tint, 10.0, 0.14, seed),
            DirtRoad => gen_granular(&mut p, tint, 16.0, 0.20, seed),
            Marble => gen_rough(&mut p, tint, 0.05, 0.30, seed),
            Bunker => gen_panel(&mut p, tint, 1.5, false, 0.15, seed),
            Duct => gen_corrugated(&mut p, tint, 8.0, seed),
            Mesh => gen_grid(&mut p, tint, 10.0, 0.10, true, seed),
        }

        layers.push(LayerMips { mips: build_mips(p.px, size) });
    }

    let mip_count = layers.first().map(|l| l.mips.len() as u32).unwrap_or(1);
    TextureArray { size, mip_count, layers }
}

// ------------------------------------------------------------------ sprites

/// Sprite slots in the effects atlas.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Sprite {
    Smoke = 0,
    Spark,
    MuzzleFlash,
    Blood,
    ImpactStar,
    Tracer,
    BulletHole,
    BlobShadow,
    Dust,
    Ember,
    Casing,
    Ring,
    Snowflake,
    Raindrop,
    Glow,
    Cross,
}

pub const SPRITE_COUNT: usize = 16;
pub const SPRITE_COLS: u32 = 4;
pub const SPRITE_CELL: u32 = 64;
pub const SPRITE_ATLAS: u32 = SPRITE_COLS * SPRITE_CELL;

impl Sprite {
    /// UV rectangle in the sprite atlas.
    pub fn uv(self) -> [f32; 4] {
        let i = self as u32;
        let x = (i % SPRITE_COLS) * SPRITE_CELL;
        let y = (i / SPRITE_COLS) * SPRITE_CELL;
        let s = SPRITE_ATLAS as f32;
        [x as f32 / s, y as f32 / s, SPRITE_CELL as f32 / s, SPRITE_CELL as f32 / s]
    }
}

/// Builds the particle and decal atlas. Everything is white so it can be
/// tinted per particle at draw time.
pub fn generate_sprite_atlas() -> (u32, Vec<u8>) {
    let size = SPRITE_ATLAS;
    let mut px = vec![0u8; (size * size * 4) as usize];

    let mut put = |sprite: Sprite, f: &mut dyn FnMut(f32, f32) -> (f32, f32)| {
        let i = sprite as u32;
        let ox = (i % SPRITE_COLS) * SPRITE_CELL;
        let oy = (i / SPRITE_COLS) * SPRITE_CELL;
        for y in 0..SPRITE_CELL {
            for x in 0..SPRITE_CELL {
                // Centred coordinates in -1..1.
                let u = (x as f32 + 0.5) / SPRITE_CELL as f32 * 2.0 - 1.0;
                let v = (y as f32 + 0.5) / SPRITE_CELL as f32 * 2.0 - 1.0;
                let (lum, alpha) = f(u, v);
                let idx = (((oy + y) * size + ox + x) * 4) as usize;
                let l = (lum.clamp(0.0, 1.0) * 255.0) as u8;
                px[idx] = l;
                px[idx + 1] = l;
                px[idx + 2] = l;
                px[idx + 3] = (alpha.clamp(0.0, 1.0) * 255.0) as u8;
            }
        }
    };

    // Soft puff with a lumpy edge.
    put(Sprite::Smoke, &mut |u, v| {
        let r = (u * u + v * v).sqrt();
        let lump = fbm(u * 2.5 + 4.0, v * 2.5 + 4.0, 8, 3, 11) * 0.35;
        let a = (1.0 - (r + lump)).clamp(0.0, 1.0);
        (0.85, a * a * 0.85)
    });
    // Hard bright dot with a short tail.
    put(Sprite::Spark, &mut |u, v| {
        let r = (u * u * 0.35 + v * v).sqrt();
        let a = (1.0 - r * 1.6).clamp(0.0, 1.0);
        (1.0, a * a * a)
    });
    // Muzzle flash: a star with a hot core.
    put(Sprite::MuzzleFlash, &mut |u, v| {
        let r = (u * u + v * v).sqrt();
        let ang = v.atan2(u);
        let petals = (ang * 4.0).cos().abs() * 0.35 + 0.65;
        let a = ((petals - r) * 2.2).clamp(0.0, 1.0);
        let core = (1.0 - r * 3.2).clamp(0.0, 1.0);
        (0.9 + core * 0.6, (a * a + core).min(1.0))
    });
    put(Sprite::Blood, &mut |u, v| {
        let r = (u * u + v * v).sqrt();
        let n = fbm(u * 3.0 + 9.0, v * 3.0 + 9.0, 8, 3, 23);
        let a = ((0.85 - r) * 3.0 + (n - 0.5) * 1.4).clamp(0.0, 1.0);
        (0.65, a)
    });
    // Impact: a small radial burst.
    put(Sprite::ImpactStar, &mut |u, v| {
        let r = (u * u + v * v).sqrt();
        let ang = v.atan2(u);
        let spikes = (ang * 6.0).sin().abs();
        let a = ((0.35 + spikes * 0.55 - r) * 3.0).clamp(0.0, 1.0);
        (1.0, a)
    });
    // Tracer: a soft horizontal streak.
    put(Sprite::Tracer, &mut |u, v| {
        let a = (1.0 - v.abs() * 2.2).clamp(0.0, 1.0) * (1.0 - u.abs() * 0.5).clamp(0.0, 1.0);
        (1.0, a * a)
    });
    put(Sprite::BulletHole, &mut |u, v| {
        let r = (u * u + v * v).sqrt();
        let n = fbm(u * 4.0 + 2.0, v * 4.0 + 2.0, 8, 3, 41);
        let a = ((0.55 - r) * 5.0).clamp(0.0, 1.0);
        let ring = ((r - 0.45).abs() * 6.0).min(1.0);
        (0.10 + ring * 0.35 + n * 0.15, a)
    });
    put(Sprite::BlobShadow, &mut |u, v| {
        let r = (u * u + v * v).sqrt();
        let a = (1.0 - r).clamp(0.0, 1.0);
        (0.0, a * a * 0.75)
    });
    put(Sprite::Dust, &mut |u, v| {
        let r = (u * u + v * v).sqrt();
        let n = fbm(u * 3.0 + 6.0, v * 3.0 + 6.0, 8, 3, 61);
        let a = ((1.0 - r) * 0.9 + (n - 0.5) * 0.5).clamp(0.0, 1.0);
        (0.95, a * 0.5)
    });
    put(Sprite::Ember, &mut |u, v| {
        let r = (u * u + v * v).sqrt();
        let a = (1.0 - r * 2.4).clamp(0.0, 1.0);
        (1.0, a)
    });
    // Shell casing: a small bright rectangle.
    put(Sprite::Casing, &mut |u, v| {
        let a = if u.abs() < 0.75 && v.abs() < 0.28 { 1.0 } else { 0.0 };
        let shade = 0.7 + (1.0 - v.abs() / 0.28).max(0.0) * 0.4;
        (shade, a)
    });
    // Expanding shockwave ring.
    put(Sprite::Ring, &mut |u, v| {
        let r = (u * u + v * v).sqrt();
        let a = (1.0 - ((r - 0.78).abs() * 9.0)).clamp(0.0, 1.0);
        (1.0, a)
    });
    put(Sprite::Snowflake, &mut |u, v| {
        let r = (u * u + v * v).sqrt();
        let a = (1.0 - r * 2.8).clamp(0.0, 1.0);
        (1.0, a * 0.9)
    });
    put(Sprite::Raindrop, &mut |u, v| {
        let a = (1.0 - u.abs() * 7.0).clamp(0.0, 1.0) * (1.0 - v.abs()).clamp(0.0, 1.0);
        (1.0, a * 0.6)
    });
    put(Sprite::Glow, &mut |u, v| {
        let r = (u * u + v * v).sqrt();
        let a = (1.0 - r).clamp(0.0, 1.0);
        (1.0, a * a * a)
    });
    // A plain cross, used by the crosshair and map markers.
    put(Sprite::Cross, &mut |u, v| {
        let a = if (u.abs() < 0.10 && v.abs() < 0.85) || (v.abs() < 0.10 && u.abs() < 0.85) { 1.0 } else { 0.0 };
        (1.0, a)
    });

    (size, px)
}
