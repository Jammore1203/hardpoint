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

// ----------------------------------------------------------------- helpers

/// Ridged noise: folds value noise about its midpoint, which turns smooth
/// blobs into creases. Rock, bark and cracked ground all want this rather
/// than plain fbm, which only ever looks like fog.
fn ridged(x: f32, y: f32, period: i32, octaves: u32, seed: u32) -> f32 {
    let mut sum = 0.0;
    let mut amp = 0.5;
    let mut total = 0.0;
    let mut period = period;
    let mut freq = 1.0;
    for o in 0..octaves {
        let n = vnoise(x * freq, y * freq, period, seed.wrapping_add(o * 7919));
        sum += (1.0 - (n * 2.0 - 1.0).abs()) * amp;
        total += amp;
        amp *= 0.55;
        freq *= 2.0;
        period *= 2;
    }
    if total > 0.0 { sum / total } else { 0.0 }
}

/// Worley returning the two nearest distances, so a cell's interior and its
/// boundary can be shaded separately. `f2 - f1` is the classic crack mask.
fn worley2(x: f32, y: f32, period: i32, seed: u32) -> (f32, f32) {
    let xi = x.floor() as i32;
    let yi = y.floor() as i32;
    let mut f1 = 8.0f32;
    let mut f2 = 8.0f32;
    for dy in -1..=1 {
        for dx in -1..=1 {
            let cx = (xi + dx).rem_euclid(period.max(1));
            let cy = (yi + dy).rem_euclid(period.max(1));
            let px = (xi + dx) as f32 + hash2(cx, cy, seed);
            let py = (yi + dy) as f32 + hash2(cx, cy, seed ^ 0x1234_5678);
            let d = ((px - x) * (px - x) + (py - y) * (py - y)).sqrt();
            if d < f1 { f2 = f1; f1 = d; } else if d < f2 { f2 = d; }
        }
    }
    (f1.min(1.0), f2.min(2.0))
}

#[inline]
fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    if (b - a).abs() < 1e-6 { return if x < a { 0.0 } else { 1.0 }; }
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Distance from `v` to the nearest edge of a unit cell, in cell units.
#[inline]
fn cell_edge(v: f32) -> f32 { v.min(1.0 - v) }

struct Painter {
    size: u32,
    /// One texel expressed in 0..1 texture space. Every hard edge in this file
    /// is softened over at least this much, because an edge narrower than a
    /// texel is aliasing that no amount of mip filtering can undo: it is
    /// already wrong in the image the mips are built from. Getting this right
    /// is the single largest difference between a surface that shimmers as you
    /// walk past it and one that sits still.
    texel: f32,
    px: Vec<u8>,
}

impl Painter {
    fn new(size: u32) -> Painter {
        Painter { size, texel: 1.0 / size.max(1) as f32, px: vec![0u8; (size * size * 4) as usize] }
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

    /// Fills with a full colour, for materials that shift hue as well as value.
    fn shade_rgb(&mut self, mut f: impl FnMut(f32, f32) -> ([f32; 3], f32)) {
        let s = self.size as f32;
        for y in 0..self.size {
            for x in 0..self.size {
                let (c, alpha) = f(x as f32 / s, y as f32 / s);
                self.set(x, y, c[0], c[1], c[2], alpha);
            }
        }
    }

    /// Paints from a height field as well as a colour.
    ///
    /// This is the single largest thing that separates these materials from
    /// flat patterns. The closure returns an albedo and a height; the height
    /// field is differenced into a normal and lit from a fixed direction, so
    /// mortar courses recess, panel seams have a lit lip and a corrugated
    /// sheet is actually round. Nothing at run time knows about any of it -
    /// the world shader has no normals and no lights - which is exactly why it
    /// has to be baked in here.
    ///
    /// `relief` scales the apparent depth. It is in the same units as the
    /// height the closure returns, which is nominally 0..1 across the tile.
    fn shade_relief(
        &mut self,
        tint: [u8; 3],
        relief: f32,
        mut f: impl FnMut(f32, f32) -> (f32, f32),
    ) {
        let n = self.size as usize;
        let sf = self.size as f32;
        let mut albedo = vec![0.0f32; n * n];
        let mut height = vec![0.0f32; n * n];
        for y in 0..n {
            for x in 0..n {
                let (a, h) = f(x as f32 / sf, y as f32 / sf);
                albedo[y * n + x] = a;
                height[y * n + x] = h;
            }
        }

        // Light from the upper left, which is the convention every texture in
        // every game of this kind is lit from, and which agrees with the
        // baked sun direction closely enough not to fight it.
        let lx = -0.55f32;
        let ly = -0.62f32;
        let lz = 0.56f32;
        let base = [tint[0] as f32 / 255.0, tint[1] as f32 / 255.0, tint[2] as f32 / 255.0];
        let scale = relief * sf * 0.02;

        for y in 0..n {
            for x in 0..n {
                // Central differences, wrapping, so the relief tiles.
                let xm = height[y * n + (x + n - 1) % n];
                let xp = height[y * n + (x + 1) % n];
                let ym = height[((y + n - 1) % n) * n + x];
                let yp = height[((y + 1) % n) * n + x];
                let dx = (xp - xm) * scale;
                let dy = (yp - ym) * scale;
                // Normal of the height field, normalised.
                let inv = 1.0 / (dx * dx + dy * dy + 1.0).sqrt();
                let (nx, ny, nz) = (-dx * inv, -dy * inv, inv);
                let diffuse = (nx * lx + ny * ly + nz * lz).max(0.0);
                // A little ambient so a face turned away is dark, not black.
                let lit = 0.62 + 0.72 * diffuse;
                let lum = albedo[y * n + x] * lit;
                self.set(x as u32, y as u32,
                         base[0] * lum, base[1] * lum, base[2] * lum, 1.0);
            }
        }
    }

    /// Multiplies in large-scale dirt and a hint of colour drift.
    ///
    /// Applied on top of every opaque material. Tiling is what makes a
    /// procedural world read as cheap - the eye finds the repeat instantly
    /// when every tile is identical at every scale - and a low-frequency
    /// stain layer at a different period from the pattern underneath is the
    /// cheapest way to break it up.
    fn grime(&mut self, amount: f32, seed: u32) {
        if amount <= 0.0 { return; }
        let s = self.size as f32;
        for y in 0..self.size {
            for x in 0..self.size {
                let u = x as f32 / s;
                let v = y as f32 / s;
                let large = fbm(u * 3.0, v * 3.0, 3, 3, seed ^ 0x51A1);
                let patch = fbm(u * 7.0, v * 7.0, 7, 2, seed ^ 0x7BC3);
                let k = 1.0 - amount * (0.55 - large) .max(0.0) * 1.7 - amount * (patch - 0.62).max(0.0) * 0.9;
                let i = ((y * self.size + x) * 4) as usize;
                for c in 0..3 {
                    self.px[i + c] = ((self.px[i + c] as f32 * k).clamp(0.0, 255.0)) as u8;
                }
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
    let tx = p.texel;
    p.shade_relief(tint, 0.22, |u, v| {
        let n = fbm(u * period as f32, v * period as f32, period, 5, seed);
        let mid = fbm(u * 40.0, v * 40.0, 40, 3, seed ^ 0x1D);
        let fine = vnoise(u * 96.0, v * 96.0, 96, seed ^ 0x99);

        // Hairline cracks, faint and at a period well off the pattern's own.
        let (f1, f2) = worley2(u * 11.0, v * 11.0, 11, seed ^ 0xC4);
        let crack = (1.0 - smoothstep(tx * 4.0, tx * 14.0, f2 - f1))
            * smoothstep(0.35, 0.60, fbm(u * 3.0, v * 3.0, 3, 2, seed ^ 0x6F));
        // Pits: the pockmarking that reads as poured concrete rather than paper.
        let (pf, _) = worley2(u * 40.0, v * 40.0, 40, seed ^ 0x2E);
        let pit = smoothstep(0.26, 0.02, pf);

        let albedo = 0.94 + (n - 0.5) * blotch + (mid - 0.5) * blotch * 0.45
            + (fine - 0.5) * grain - crack * 0.07 - pit * 0.07;
        // Relief carries the cracks and pits, so they read as cut into the
        // surface from any angle rather than as dark paint.
        let height = (n - 0.5) * 0.5 + (mid - 0.5) * 0.35 + (fine - 0.5) * 0.7
            - crack * 1.4 - pit * 0.9;
        (albedo, height)
    });
    p.grime(0.5, seed);
}

/// Regular brick courses with mortar.
fn gen_brick(p: &mut Painter, tint: [u8; 3], rows: f32, mortar: [u8; 3], seed: u32) {
    let mortar_l = (mortar[0] as f32 + mortar[1] as f32 + mortar[2] as f32) / (3.0 * 255.0);
    let base_l = (tint[0] as f32 + tint[1] as f32 + tint[2] as f32) / (3.0 * 255.0);
    let tx = p.texel;
    let soft = (tx * rows * 1.6).max(0.006);
    p.shade_relief(tint, 0.30, |u, v| {
        let row = (v * rows).floor();
        let offset = if (row as i32) % 2 == 0 { 0.0 } else { 0.5 };
        let bu = (u * rows * 0.5 + offset).fract();
        let bv = (v * rows).fract();
        let joint_u = 0.05;
        let joint_v = 0.10;
        let du = smoothstep(joint_u - soft, joint_u + soft, cell_edge(bu));
        let dv = smoothstep(joint_v - soft * 2.0, joint_v + soft * 2.0, cell_edge(bv));
        let face = du.min(dv);

        let id = hash2((u * rows * 0.5 + offset) as i32, row as i32, seed);
        let id2 = hash2((u * rows * 0.5 + offset) as i32, row as i32, seed ^ 0xBEEF);
        let grain = fbm(u * 70.0, v * 70.0, 70, 3, seed);
        let coarse = fbm(u * 9.0, v * 9.0, 9, 3, seed ^ 0x4C);

        // A brick that is slightly proud or slightly sunk, which is what makes
        // a real wall read as laid by hand rather than printed.
        let sit = (id2 - 0.5) * 0.25;
        let chip = if id2 > 0.74 {
            smoothstep(0.16, 0.0, cell_edge(bu).min(cell_edge(bv))) * (id2 - 0.74) * 3.0
        } else { 0.0 };

        let height = face * (1.0 + sit) - chip * 0.7 + (grain - 0.5) * 0.10;

        let albedo = if face > 0.5 {
            // Fired brick varies in value and hue between units.
            base_l * (0.80 + id * 0.42) + (grain - 0.5) * 0.10 + (coarse - 0.5) * 0.08
        } else {
            mortar_l * (0.94 + (grain - 0.5) * 0.22)
        };
        (albedo / base_l.max(0.02), height)
    });
    p.grime(0.5, seed);
}

/// Rectangular panels with recessed seams and rivets: metal, hulls, bunkers.
fn gen_panel(p: &mut Painter, tint: [u8; 3], divisions: f32, rivets: bool, rust: f32, seed: u32) {
    let tx = p.texel;
    let soft = (tx * divisions * 1.8).max(0.008);
    p.shade_relief(tint, 0.42, |u, v| {
        let pu = (u * divisions).fract();
        let pv = (v * divisions).fract();
        let seam = 0.040;
        let edge_u = cell_edge(pu);
        let edge_v = cell_edge(pv);
        let inside = smoothstep(seam - soft, seam + soft, edge_u.min(edge_v));

        let cell = hash2((u * divisions) as i32, (v * divisions) as i32, seed);
        let mut height = inside * (1.0 + (cell - 0.5) * 0.18);
        let mut albedo = 0.86 + cell * 0.14;

        if rivets {
            // Round heads inset from each corner. Relief does the lighting, so
            // they no longer need a hand-painted highlight to read as domes.
            let inset = 0.11f32;
            let dx = (pu - inset).abs().min((pu - (1.0 - inset)).abs());
            let dy = (pv - inset).abs().min((pv - (1.0 - inset)).abs());
            let d = (dx * dx + dy * dy).sqrt();
            let r = 0.055f32;
            let head = 1.0 - smoothstep(r - soft, r + soft, d);
            // A hemisphere, not a cylinder: height falls off toward the rim.
            height += head * (1.0 - (d / r.max(1e-4)).min(1.0).powi(2)) * 0.55;
        }

        let grain = vnoise(u * 110.0, v * 110.0, 110, seed ^ 0x55);
        let brushed = vnoise(u * 6.0, v * 150.0, 150, seed ^ 0x71);
        albedo += (grain - 0.5) * 0.05 + (brushed - 0.5) * 0.06;
        height += (grain - 0.5) * 0.04;

        if rust > 0.0 {
            let bloom = fbm(u * 10.0, v * 10.0, 10, 4, seed ^ 0xAB);
            let run = fbm(u * 26.0, v * 5.0, 26, 3, seed ^ 0xD3);
            let near_seam = 1.0 - inside;
            let amount = ((bloom - 0.50).max(0.0) * 2.0 + near_seam * 0.35
                          + (run - 0.6).max(0.0) * 1.2).clamp(0.0, 1.0);
            albedo *= 1.0 - amount * rust * 0.45;
            // Rust eats the surface as well as staining it.
            height -= amount * rust * 0.18;
        }
        (albedo, height)
    });
    p.grime(0.45, seed);
}

/// Vertical corrugations.
fn gen_corrugated(p: &mut Painter, tint: [u8; 3], ribs: f32, seed: u32) {
    let tx = p.texel;
    // A rib narrower than six texels is a moire generator whatever else is
    // done to it; cap the count to what the resolution can resolve.
    let ribs = ribs.min(1.0 / (tx * 6.0));
    p.shade_relief(tint, 0.55, |u, v| {
        let phase = u * ribs * std::f32::consts::TAU;
        // The profile is the height now, so the sheet is genuinely round and
        // its highlight moves with the surface rather than being painted on.
        let height = phase.sin() * 0.5 + 0.5;

        let mut albedo = 0.90;
        // Horizontal fixing seams every so often, pressed in.
        let band = (v * 4.0).fract();
        let seam = 1.0 - smoothstep(0.0, tx * 5.0, cell_edge(band));
        albedo *= 1.0 - seam * 0.18;

        let streak = fbm(u * ribs * 0.5, v * 6.0, 12, 3, seed);
        albedo += (streak - 0.5) * 0.12 * (1.0 - height * 0.5);
        let grain = vnoise(u * 90.0, v * 90.0, 90, seed ^ 0x3B);
        albedo += (grain - 0.5) * 0.05;
        (albedo, height - seam * 0.25)
    });
    p.grime(0.5, seed);
}

/// Loose granular ground: gravel, sand, snow, dirt.
///
/// `contrast` is really "how stony is this": sand and snow want grain and
/// almost no structure, gravel and cobble want individual readable stones.
/// Driving the whole generator off that one number is what stops a beach from
/// coming out as a honeycomb of boulders.
fn gen_granular(p: &mut Painter, tint: [u8; 3], cells: f32, contrast: f32, seed: u32) {
    let tx = p.texel;
    let stony = smoothstep(0.14, 0.34, contrast);
    let big = (cells * 0.46).max(9.0);
    let cells = cells.min(1.0 / (tx * 3.0));
    // Relief scales with stoniness: sand is nearly flat, gravel is a bed of
    // separate stones. Doing this by lighting a height field rather than by
    // painting light and dark is what stops gravel reading as static.
    let relief = 0.10 + 0.40 * stony;
    p.shade_relief(tint, relief, |u, v| {
        let (f1, f2) = worley2(u * cells, v * cells, cells as i32, seed);
        let drift = fbm(u * 3.0, v * 3.0, 3, 4, seed ^ 0x77);
        let mid = fbm(u * 11.0, v * 11.0, 11, 3, seed ^ 0x21);
        let fine = vnoise(u * 128.0, v * 128.0, 128, seed ^ 0x4D);

        // Each cell is a stone: domed in the middle, with a gap around it.
        let dome = (1.0 - f1).powf(0.7);
        let gap = 1.0 - smoothstep(tx * 3.0, 0.10, f2 - f1);

        let mut height = dome * (0.5 + 0.5 * stony) - gap * 0.55 * stony
            + (drift - 0.5) * 0.30 + (mid - 0.5) * 0.20 + (fine - 0.5) * 0.16;

        if stony > 0.01 {
            let (b1, b2) = worley2(u * big, v * big, big as i32, seed ^ 0x9E1);
            let boulder = ((1.0 - b1) - 0.5) * 0.30 * stony;
            let seam = (1.0 - smoothstep(tx * 3.0, 0.10, b2 - b1)) * 0.22 * stony;
            height += boulder - seam;
        }

        // The albedo stays nearly flat: the lighting is doing the work now, so
        // painting contrast in as well would double it and give back the noise
        // this is meant to remove.
        let albedo = 0.96 + (drift - 0.5) * 0.14 + (mid - 0.5) * 0.09
            + (fine - 0.5) * 0.05
            + ((1.0 - f1) - 0.5) * contrast * 0.30;
        (albedo, height)
    });
    // Ground tiles across enormous areas, so it gets the least stain of
    // anything: a low-frequency blotch here is a visible repeat out there.
    p.grime(0.18, seed);
}

/// Vegetation: clumpy, high-contrast, slightly varied in hue.
fn gen_foliage(p: &mut Painter, tint: [u8; 3], cutout: bool, seed: u32) {
    let base = [tint[0] as f32 / 255.0, tint[1] as f32 / 255.0, tint[2] as f32 / 255.0];
    let tx = p.texel;
    p.shade_rgb(|u, v| {
        let clump = fbm(u * 6.0, v * 6.0, 6, 4, seed);
        let leaf = fbm(u * 18.0, v * 18.0, 18, 4, seed ^ 0x77);
        let vein = ridged(u * 30.0, v * 30.0, 30, 3, seed ^ 0x31);
        let detail = vnoise(u * 70.0, v * 70.0, 70, seed ^ 0x5C);

        let mass = clump * 0.55 + leaf * 0.45;
        let lum = 0.46 + mass * 0.72 + vein * 0.14 + (detail - 0.5) * 0.16;
        // Cutout foliage still needs a soft-ish edge in the source image; the
        // alpha test snaps it back to a hard line, but the mips no longer
        // carry a stack of half-transparent fringes.
        let alpha = if cutout { smoothstep(0.45 - tx * 3.0, 0.45 + tx * 3.0, mass) } else { 1.0 };
        // Sunlit tips go yellow, shaded interiors go blue-green.
        let warm = (mass - 0.55).max(0.0) * 1.2;
        let cool = (0.45 - mass).max(0.0) * 0.8;
        let c = [
            base[0] * lum * (1.0 + warm * 0.55),
            base[1] * lum * (1.0 + warm * 0.12),
            base[2] * lum * (1.0 - warm * 0.35 + cool * 0.45),
        ];
        (c, alpha)
    });
    if !cutout { p.grime(0.3, seed); }
}

/// Polished stone with veining.
///
/// Marble was `gen_rough`, which is the concrete generator with the grain
/// turned down: pale, speckled, and indistinguishable from plaster. What
/// makes marble marble is the veins.
fn gen_marble(p: &mut Painter, tint: [u8; 3], seed: u32) {
    p.shade_relief(tint, 0.06, |u, v| {
        // Domain-warped bands: a smooth gradient folded back on itself, which
        // is the standard way to draw a vein and still the cheapest. Two
        // families at different angles and scales, because a slab with one
        // set of parallel veins reads as a contour map.
        let warp = fbm(u * 2.4, v * 2.4, 2, 4, seed) - 0.5;
        let warp2 = fbm(u * 6.0, v * 6.0, 6, 3, seed ^ 0x2D) - 0.5;
        let vein_of = |x: f32, sharp: f32| -> (f32, f32) {
            let b = (x * std::f32::consts::TAU).sin().abs();
            // The core is a soft dark line; the halo around it is the wash of
            // colour that makes the vein look like it is in the stone rather
            // than drawn on it.
            ((1.0 - b).powf(sharp), (1.0 - b).powf(1.1))
        };
        let (a_core, a_halo) = vein_of(u * 1.7 + v * 0.8 + warp * 3.4 + warp2 * 1.1, 3.2);
        let (b_core, b_halo) = vein_of(u * -0.9 + v * 2.2 + warp * 2.6 - warp2 * 1.5, 4.4);

        // Some of the slab is nearly clean; the veining runs in seams.
        let seam = 0.35 + fbm(u * 1.6, v * 1.6, 1, 3, seed ^ 0x5B) * 1.15;
        let core = (a_core + b_core * 0.7).min(1.4) * seam;
        let halo = (a_halo + b_halo * 0.6) * 0.5 * seam;

        let grain = vnoise(u * 140.0, v * 140.0, 140, seed ^ 0x63) - 0.5;
        let albedo = 1.05 - core * 0.30 - halo * 0.14 + grain * 0.04 + warp * 0.05;
        // Almost flat: polished stone has no relief to speak of, and the
        // little there is comes from the softer vein material wearing back.
        let height = -core * 0.28 + grain * 0.10;
        (albedo, height)
    });
}

/// Ground cover: turf with bare earth showing through it.
///
/// Grass shared a generator with tree canopy, which is a mass of leaves seen
/// from outside and reads, when you lay it flat over eighty metres, as one
/// saturated green carpet. What ground looks like is grass in some places and
/// soil in others, and the boundary between them is most of the visual
/// information a lawn has.
fn gen_grass(p: &mut Painter, tint: [u8; 3], soil: [u8; 3], seed: u32) {
    let base = [tint[0] as f32 / 255.0, tint[1] as f32 / 255.0, tint[2] as f32 / 255.0];
    let earth = [soil[0] as f32 / 255.0, soil[1] as f32 / 255.0, soil[2] as f32 / 255.0];
    p.shade_rgb(|u, v| {
        let patch = fbm(u * 2.0, v * 2.0, 2, 3, seed ^ 0x4D);
        let clump = fbm(u * 6.0, v * 6.0, 6, 4, seed);
        let blade = ridged(u * 44.0, v * 44.0, 44, 3, seed ^ 0x77);
        let fine = vnoise(u * 120.0, v * 120.0, 120, seed ^ 0x11);

        // Where the slow field dips, the turf is worn through.
        let bare = smoothstep(0.60, 0.34, patch);

        let lum = 0.50 + clump * 0.52 + blade * 0.24 + (fine - 0.5) * 0.20;
        // Sunlit tips yellow, the shade under them blue-green: the same trick
        // the canopy uses, at a scale that suits a lawn.
        let warm = (clump - 0.55).max(0.0) * 1.1;
        let green = [
            base[0] * lum * (1.0 + warm * 0.60),
            base[1] * lum * (1.0 + warm * 0.10),
            base[2] * lum * (1.0 - warm * 0.30),
        ];
        let dirt_lum = 0.72 + (fine - 0.5) * 0.34 + blade * 0.12;
        let dirt = [earth[0] * dirt_lum, earth[1] * dirt_lum, earth[2] * dirt_lum];

        let k = bare * 0.80;
        ([
            green[0] + (dirt[0] - green[0]) * k,
            green[1] + (dirt[1] - green[1]) * k,
            green[2] + (dirt[2] - green[2]) * k,
        ], 1.0)
    });
    p.grime(0.35, seed);
}

/// Woven fabric or sandbags.
fn gen_woven(p: &mut Painter, tint: [u8; 3], threads: f32, lumpy: f32, seed: u32) {
    let tx = p.texel;
    let threads = threads.min(1.0 / (tx * 8.0));
    p.shade_relief(tint, 0.30, |u, v| {
        let a = (u * threads * std::f32::consts::TAU).sin();
        let b = (v * threads * std::f32::consts::TAU).sin();
        // Over-under rather than a product, so the warp and weft cross and the
        // relief has a thread passing over a thread rather than a grid of pits.
        let over = if a > b { a } else { b };
        let weave = over * 0.5 + 0.5;
        let lump = fbm(u * 5.0, v * 5.0, 5, 4, seed);
        let fray = vnoise(u * 64.0, v * 64.0, 64, seed ^ 0x8A);
        let height = weave * 0.55 + (lump - 0.5) * lumpy * 2.4 + (fray - 0.5) * 0.20;
        let albedo = 0.92 + (lump - 0.5) * lumpy * 0.5 + (fray - 0.5) * 0.08;
        (albedo, height)
    });
    p.grime(0.4, seed);
}

/// Planks with visible grain.
fn gen_wood(p: &mut Painter, tint: [u8; 3], planks: f32, vertical: bool, seed: u32) {
    let tx = p.texel;
    let soft = (tx * planks * 2.0).max(0.010);
    p.shade_relief(tint, 0.20, |u, v| {
        let (along, across) = if vertical { (v, u) } else { (u, v) };
        let plank = (across * planks).floor();
        let edge = (across * planks).fract();
        let id = hash2(plank as i32, 0, seed);
        let id2 = hash2(plank as i32, 7, seed);

        // Growth rings stretched hard along the plank.
        let rings = ridged(along * 3.0 + id * 8.0, across * planks * 6.0, 24, 4, seed);
        let fibre = vnoise(along * 120.0, across * planks * 20.0, 120, seed ^ 0x2B);
        let kx = hash2(plank as i32, 3, seed);
        let kd = ((along - kx) * (along - kx) * 6.0 + (edge - 0.5) * (edge - 0.5)).sqrt();
        let knot = smoothstep(0.16, 0.0, kd) * if id2 > 0.55 { 1.0 } else { 0.0 };

        let albedo = 0.86 + id * 0.24 + rings * 0.26 + (fibre - 0.5) * 0.10 - knot * 0.34;
        // Gap between planks, and planks that do not all sit at one level.
        let gap = 1.0 - smoothstep(0.0, soft * 2.0, cell_edge(edge));
        let height = (id2 - 0.5) * 0.30 + rings * 0.22 + (fibre - 0.5) * 0.22
            - gap * 2.6 - knot * 0.5;
        (albedo * (1.0 - gap * 0.45), height)
    });
    p.grime(0.45, seed);
}

/// Regular grid: tiles, diamond plate, gratings.
fn gen_grid(p: &mut Painter, tint: [u8; 3], cells: f32, thickness: f32, cutout: bool, seed: u32) {
    let tx = p.texel;
    let cells = cells.min(1.0 / (tx * 10.0));
    let soft = (tx * cells * 1.5).max(0.010);
    if cutout {
        // A grating is a cut-out, so it has an alpha channel and cannot go
        // through the relief path; it is lit by the bar's own edge instead.
        p.shade(tint, |u, v, _, _| {
            let du = cell_edge((u * cells).fract());
            let dv = cell_edge((v * cells).fract());
            let bar = (1.0 - smoothstep(thickness - soft, thickness + soft, du))
                .max(1.0 - smoothstep(thickness - soft, thickness + soft, dv));
            let n = vnoise(u * 80.0, v * 80.0, 80, seed);
            let lit = 1.0 - smoothstep(0.0, thickness, dv.min(du));
            let lum = 0.72 + lit * 0.22 + (n - 0.5) * 0.14;
            (lum, bar)
        });
        return;
    }
    p.shade_relief(tint, 0.40, |u, v| {
        let cu = (u * cells).fract();
        let cv = (v * cells).fract();
        let du = cell_edge(cu);
        let dv = cell_edge(cv);
        let grout = (1.0 - smoothstep(thickness - soft, thickness + soft, du))
            .max(1.0 - smoothstep(thickness - soft, thickness + soft, dv));
        let n = vnoise(u * 80.0, v * 80.0, 80, seed);
        let id = hash2((u * cells) as i32, (v * cells) as i32, seed);

        // Each tile domes very slightly and sits at its own level, which is
        // what stops a tiled floor looking like a printed grid.
        let dome = (du.min(dv) * 5.0).min(1.0);
        let albedo = 0.70 * grout + (0.96 + id * 0.16 + (n - 0.5) * 0.08) * (1.0 - grout);
        let height = (1.0 - grout) * (0.72 + dome * 0.28 + (id - 0.5) * 0.22)
            + (n - 0.5) * 0.06;
        (albedo, height)
    });
    p.grime(0.4, seed);
}

/// Diagonal hazard stripes.
fn gen_stripes(p: &mut Painter, tint: [u8; 3], dark: [u8; 3], count: f32, seed: u32) {
    let a = [tint[0] as f32 / 255.0, tint[1] as f32 / 255.0, tint[2] as f32 / 255.0];
    let b = [dark[0] as f32 / 255.0, dark[1] as f32 / 255.0, dark[2] as f32 / 255.0];
    let tx = p.texel;
    let soft = (tx * count * 2.4).max(0.012);
    p.shade_rgb(|u, v| {
        let s = ((u + v) * count).fract();
        let t = smoothstep(0.5 - soft, 0.5 + soft, s) - smoothstep(1.0 - soft, 1.0, s);
        let n = vnoise(u * 70.0, v * 70.0, 70, seed);
        // Paint wears off the high points; the metal beneath shows through.
        let wear = fbm(u * 14.0, v * 14.0, 14, 3, seed ^ 0x6D);
        let scuff = smoothstep(0.58, 0.78, wear);
        let l = 0.88 + (n - 0.5) * 0.22 - scuff * 0.22;
        let c = [
            (a[0] + (b[0] - a[0]) * t) * l,
            (a[1] + (b[1] - a[1]) * t) * l,
            (a[2] + (b[2] - a[2]) * t) * l,
        ];
        (c, 1.0)
    });
    p.grime(0.5, seed);
}

/// Camouflage blobs.
fn gen_camo(p: &mut Painter, tint: [u8; 3], seed: u32) {
    let base = [tint[0] as f32 / 255.0, tint[1] as f32 / 255.0, tint[2] as f32 / 255.0];
    let tx = p.texel;
    let soft = (tx * 6.0).max(0.006);
    // The print is flat - it is dye, not relief - so the height field is
    // purely the weave and the creases of the cloth underneath it.
    let mut heights: Vec<f32> = Vec::new();
    let n = p.size as usize;
    for y in 0..n {
        for x in 0..n {
            let u = x as f32 / n as f32;
            let v = y as f32 / n as f32;
            let weave = ((u * 150.0).sin() * (v * 150.0).sin()) * 0.5 + 0.5;
            let crease = fbm(u * 5.0, v * 5.0, 5, 3, seed ^ 0x2C1);
            let grain = vnoise(u * 70.0, v * 70.0, 70, seed);
            heights.push(weave * 0.45 + (crease - 0.5) * 1.5 + (grain - 0.5) * 0.25);
        }
    }
    let mut i = 0usize;
    p.shade_relief(tint, 0.16, |u, v| {
        let a = fbm(u * 6.0, v * 6.0, 6, 4, seed);
        let b = fbm(u * 10.0, v * 10.0, 10, 4, seed ^ 0x5A5A);
        let c3 = fbm(u * 16.0, v * 16.0, 16, 3, seed ^ 0xA13);
        let m1 = smoothstep(0.54 - soft, 0.54 + soft, a);
        let m2 = smoothstep(0.57 - soft, 0.57 + soft, b) * (1.0 - m1);
        let m3 = smoothstep(0.60 - soft, 0.60 + soft, c3) * (1.0 - m1) * (1.0 - m2);
        let albedo = 0.98 + m1 * 0.34 - m2 * 0.26 - m3 * 0.12;
        let h = heights[i.min(heights.len() - 1)];
        i += 1;
        (albedo, h)
    });
    let _ = base;
    p.grime(0.30, seed);
}


/// A lit surface: windows, screens, control panels.
fn gen_lit(p: &mut Painter, tint: [u8; 3], cells: f32, scanlines: bool, seed: u32) {
    let tx = p.texel;
    let soft = (tx * cells * 2.0).max(0.010);
    p.shade(tint, |u, v, _, y| {
        let cu = (u * cells).fract();
        let cv = (v * cells).fract();
        let frame = 1.0 - smoothstep(0.07 - soft, 0.07 + soft, cell_edge(cu).min(cell_edge(cv)));
        let on = hash2((u * cells) as i32, (v * cells) as i32, seed) > 0.35;
        let mut lum = if on { 1.28 } else { 0.44 };
        // A soft vertical falloff inside each pane reads as glass.
        lum *= 1.0 + (0.5 - cv) * 0.18;
        lum = lum * (1.0 - frame) + 0.30 * frame;
        if scanlines && y % 2 == 0 { lum *= 0.84; }
        let flick = vnoise(u * 24.0, v * 24.0, 24, seed ^ 0xF0);
        let dust = fbm(u * 8.0, v * 8.0, 8, 3, seed ^ 0x3A);
        lum += (flick - 0.5) * 0.08 - (dust - 0.55).max(0.0) * 0.25;
        (lum, 1.0)
    });
}

/// A block of flats at four hundred metres: storeys of windows in concrete.
///
/// Backdrop blocks were plain wall textures scaled up, which gives a
/// silhouette but no sense of size -- a brick tile on a forty-metre tower
/// reads as a forty-metre brick. Windows are how the eye counts storeys and
/// works out how far away a building is, and it is the only detail that
/// survives the haze.
fn gen_facade(p: &mut Painter, tint: [u8; 3], seed: u32) {
    // Three storeys and three bays to a tile; the backdrop draws this at nine
    // metres a tile, so a window lands every three metres in both directions,
    // which is what a storey is.
    let cols = 3.0f32;
    let rows = 3.0f32;
    p.shade_relief(tint, 0.30, |u, v| {
        let cu = (u * cols).fract();
        let cv = (v * rows).fract();
        let ix = (u * cols) as i32;
        let iy = (v * rows) as i32;

        // Window opening: a tall rectangle inset in its cell.
        // Taller than wide, and sitting in the upper part of its storey.
        let inx = smoothstep(0.26, 0.31, cu) * (1.0 - smoothstep(0.69, 0.74, cu));
        let iny = smoothstep(0.13, 0.18, cv) * (1.0 - smoothstep(0.62, 0.67, cv));
        let win = inx * iny;

        // Spandrel band under each row of windows, and a pilaster between
        // each column: the two things that make concrete read as panelled.
        let band = 1.0 - smoothstep(0.72, 0.80, cv);
        let stain = fbm(u * 5.0, v * 9.0, 5, 3, seed ^ 0x2F) - 0.5;
        let grain = vnoise(u * 90.0, v * 90.0, 90, seed) - 0.5;

        // Most windows are dark; a few catch the sky, fewer still are lit.
        let k = hash2(ix, iy, seed);
        let glass = if k > 0.90 { 1.35 } else if k > 0.62 { 0.62 } else { 0.22 };

        let wall = 0.92 + stain * 0.18 + grain * 0.08 - (1.0 - band) * 0.06;
        let albedo = wall * (1.0 - win) + glass * win;
        // Windows are recessed; the wall between them stands proud.
        let height = (1.0 - win) * 0.75 + band * 0.10 + stain * 0.10;
        (albedo, height)
    });
    p.grime(0.55, seed);
}

/// Wind-worked ground: sand ripples, snow sastrugi.
///
/// `gen_granular` treats every loose surface as a bed of stones, which is
/// right for gravel and wrong for the two materials that carpet whole maps.
/// Sand and snow are not stones; they are a fluid the wind has left ridges
/// in, and without those ridges a hundred metres of either is one flat colour
/// with a bit of grain on it.
///
/// `ripple` is ridges per tile, `depth` how sharply they stand, `sparkle`
/// the strength of the bright specks that snow has and sand does not.
fn gen_drift(p: &mut Painter, tint: [u8; 3], ripple: f32, depth: f32,
             sparkle: f32, seed: u32) {
    p.shade_relief(tint, 0.30 + depth * 0.55, |u, v| {
        // The ripple lines are dragged about by a slow field, so they curve
        // and fork the way a dune surface does rather than running as a comb.
        let warp = fbm(u * 2.4, v * 2.4, 2, 4, seed) - 0.5;
        let bend = fbm(u * 6.0, v * 6.0, 6, 3, seed ^ 0x35) - 0.5;
        let phase = (v * ripple + warp * 5.0 + bend * 1.4) * std::f32::consts::TAU;
        // Asymmetric: a wind ridge has a long windward slope and a short lee
        // face, and squaring the sine is the cheapest way to say so.
        let wave = phase.sin() * 0.5 + 0.5;
        let ridge = wave * wave;

        // Where the drift is deep the ripples smooth out; where it is thin
        // they bite. That variation is what stops the pattern reading as a
        // printed texture.
        let deep = fbm(u * 1.7, v * 1.7, 1, 3, seed ^ 0x9C);
        let strength = 0.35 + deep * 0.65;

        let grain = vnoise(u * 150.0, v * 150.0, 150, seed ^ 0x4D) - 0.5;
        let mid = fbm(u * 14.0, v * 14.0, 14, 3, seed ^ 0x21) - 0.5;

        let height = ridge * depth * strength + mid * 0.34 + grain * 0.13
            + (deep - 0.5) * 0.5;
        let mut albedo = 0.90 + ridge * 0.10 * strength + mid * 0.13 + grain * 0.07;
        if sparkle > 0.001 {
            // Ice crystals: a few isolated very bright texels, which is what
            // makes snow read as snow rather than as white paper.
            let s = vnoise(u * 210.0, v * 210.0, 210, seed ^ 0xBE);
            albedo += smoothstep(0.86, 0.99, s) * sparkle;
        }
        (albedo, height)
    });
    p.grime(0.30, seed);
}

/// Chequer plate: raised lozenges in staggered pairs, the way real tread
/// plate is rolled.
///
/// This used to be `gen_grid`, which is a grid of squares -- fine for a tiled
/// floor and nothing like tread plate, and on a large surface it read as a
/// chessboard. The studs are the whole point of the material: they are what
/// says "you can walk on this" from across a room.
fn gen_chequer(p: &mut Painter, tint: [u8; 3], cells: f32, seed: u32) {
    p.shade_relief(tint, 0.52, |u, v| {
        let cu = (u * cells).fract();
        // Every other row is offset half a cell, which is how the pattern is
        // actually laid out and what stops it reading as a lattice.
        let row = (v * cells).floor();
        let cv = (v * cells).fract();
        let su = (cu + if (row as i32) % 2 == 0 { 0.0 } else { 0.5 }).fract();

        // Two lozenges per cell, crossed, one leaning each way.
        let a = ((su - 0.30) + (cv - 0.30)).abs() + ((su - 0.30) - (cv - 0.30)).abs() * 0.34;
        let bb = ((su - 0.72) - (cv - 0.70)).abs() + ((su - 0.72) + (cv - 0.70)).abs() * 0.34;
        let stud = (1.0 - smoothstep(0.14, 0.24, a)).max(1.0 - smoothstep(0.14, 0.24, bb));

        let grit = vnoise(u * 110.0, v * 110.0, 110, seed) - 0.5;
        let wear = fbm(u * 8.0, v * 8.0, 8, 3, seed ^ 0x63) - 0.5;
        // The plate between the studs is scuffed dull; the studs themselves
        // are polished by boots.
        let albedo = 0.80 + stud * 0.26 + wear * 0.14 + grit * 0.08;
        let height = stud * 0.80 + wear * 0.12 + grit * 0.06;
        (albedo, height)
    });
    p.grime(0.45, seed);
}

/// Blued steel: fine lengthwise machining, a few wear marks on the high spots.
///
/// A weapon is the one object the player looks at for the whole match, at
/// twenty centimetres, so its material is the one that has to survive being
/// looked at closely. The frequency here is deliberately high -- the tile
/// covers a few centimetres of a receiver, not two metres of wall.
fn gen_gunmetal(p: &mut Painter, tint: [u8; 3], seed: u32) {
    p.shade_relief(tint, 0.18, |u, v| {
        // Machining runs along the part. Two frequencies so it does not read
        // as a ruled grating.
        let cut = vnoise(u * 6.0, v * 180.0, 180, seed) * 0.6
            + vnoise(u * 3.0, v * 74.0, 74, seed ^ 0x2B) * 0.4;
        let mottle = fbm(u * 7.0, v * 7.0, 7, 3, seed ^ 0x5F);
        let pit = vnoise(u * 46.0, v * 46.0, 46, seed ^ 0x91);
        // Wear: the raised edges of a blued part go bright before anything
        // else does, and that is most of what says "used" about a weapon.
        let wear = smoothstep(0.74, 0.94, mottle) * 0.55;
        let albedo = 0.82 + (cut - 0.5) * 0.10 + (mottle - 0.5) * 0.16 + wear;
        let height = (cut - 0.5) * 0.55 + (mottle - 0.5) * 0.35 - (pit - 0.72).max(0.0) * 1.2;
        (albedo, height)
    });
}

/// Matte black furniture: a fine moulded stipple and nothing else.
fn gen_polymer(p: &mut Painter, tint: [u8; 3], seed: u32) {
    p.shade_relief(tint, 0.26, |u, v| {
        let stipple = vnoise(u * 96.0, v * 96.0, 96, seed);
        let coarse = fbm(u * 11.0, v * 11.0, 11, 3, seed ^ 0x17);
        let scuff = smoothstep(0.80, 0.97, coarse) * 0.30;
        let albedo = 0.90 + (stipple - 0.5) * 0.12 + (coarse - 0.5) * 0.10 + scuff;
        let height = (stipple - 0.5) * 0.9 + (coarse - 0.5) * 0.25;
        (albedo, height)
    });
}

/// Tyre tread: chevron blocks with a circumferential groove.
///
/// Rubber and tyre shared a granular noise, which on a truck wheel read as a
/// black rectangle. Tread is what makes a wheel a wheel at ten metres.
fn gen_tread(p: &mut Painter, tint: [u8; 3], seed: u32) {
    p.shade_relief(tint, 0.55, |u, v| {
        // Two shoulder ribs and a centre groove running around the tyre.
        let across = (v - 0.5).abs() * 2.0;
        let groove = 1.0 - smoothstep(0.06, 0.20, (across - 0.16).abs());
        // Chevron blocks, mirrored either side of the centre line.
        let skew = u + (v - 0.5).abs() * 0.55;
        let block = (skew * 7.0).fract();
        let gap = smoothstep(0.02, 0.13, block) * (1.0 - smoothstep(0.76, 0.90, block));
        let wear = fbm(u * 9.0, v * 9.0, 9, 3, seed) - 0.5;
        let grain = vnoise(u * 90.0, v * 90.0, 90, seed ^ 0x4D) - 0.5;

        let mut height = gap * 0.72 + 0.14;
        height *= 1.0 - groove * 0.85;
        height += wear * 0.10 + grain * 0.06;
        // Rubber is nearly matte and nearly black; the pattern has to come
        // from the relief, not from the albedo.
        let albedo = 0.88 + wear * 0.12 + grain * 0.10 - groove * 0.10;
        (albedo, height)
    });
    p.grime(0.5, seed);
}

/// An instrument panel.
///
/// This was `gen_lit` at six cells, which is a six-by-six grid of windows lit
/// at random: on a shed it reads as a shed, and on a console it reads as a
/// chessboard. Every control room in the game was panelled in chessboard.
///
/// What a panel of this period actually was: a dark brushed face inside a
/// bezel, a recessed screen, two gauges, and rows of small indicator lamps in
/// amber and green with a red one to worry about.
fn gen_control_panel(p: &mut Painter, tint: [u8; 3], seed: u32) {
    let base = [tint[0] as f32 / 255.0, tint[1] as f32 / 255.0, tint[2] as f32 / 255.0];
    let tx = p.texel;
    let soft = (tx * 2.0).max(0.004);

    // A rounded rectangle's distance to its own edge, positive inside.
    let slab = |u: f32, v: f32, x0: f32, y0: f32, x1: f32, y1: f32| -> f32 {
        (u - x0).min(x1 - u).min(v - y0).min(y1 - v)
    };

    p.shade_rgb(|u, v| {
        // Brushed steel face: fine vertical streaks, a slow horizontal drift.
        let brush = vnoise(u * 220.0, v * 3.0, 220, seed) * 0.16
            + fbm(u * 6.0, v * 6.0, 6, 3, seed ^ 0x21) * 0.12;
        let mut c = [
            base[0] * (0.72 + brush),
            base[1] * (0.72 + brush),
            base[2] * (0.74 + brush),
        ];

        // Bezel around the tile, so a wall of these reads as a rack of
        // separate units rather than one continuous surface.
        let edge = u.min(1.0 - u).min(v).min(1.0 - v);
        let bez = 1.0 - smoothstep(0.038, 0.038 + soft, edge);
        // Lit on the top and left, in shadow on the bottom and right: the
        // whole trick of a bevel, and the relief path cannot do it here
        // because this material needs colour, not just value.
        let lip = if u < 0.5 && v < 0.5 { 1.34 } else { 0.68 };
        for i in 0..3 { c[i] = c[i] * (1.0 - bez) + base[i] * 0.86 * lip * bez; }

        // Screen, recessed, upper left.
        let scr = slab(u, v, 0.085, 0.075, 0.615, 0.435);
        if scr > 0.0 {
            let inner = smoothstep(0.0, 0.016, scr);
            let scan = if ((v * 96.0) as i32) % 2 == 0 { 0.72 } else { 1.0 };
            let trace = smoothstep(0.55, 0.95, ridged(u * 9.0, v * 5.0, 9, 2, seed ^ 0x7C));
            let glow = 0.10 + trace * 0.75;
            let lit = [0.16 + glow * 0.30, 0.30 + glow * 0.86, 0.22 + glow * 0.42];
            for i in 0..3 {
                c[i] = c[i] * (1.0 - inner) + lit[i] * scan * inner;
            }
        }

        // Two gauges on the right, each a pale dial with a dark needle.
        for (gi, (gx, gy)) in [(0.735f32, 0.175f32), (0.885, 0.175)].into_iter().enumerate() {
            let d = ((u - gx).powi(2) + (v - gy).powi(2)).sqrt();
            let face = 1.0 - smoothstep(0.062, 0.062 + soft, d);
            if face > 0.0 {
                let rim = smoothstep(0.050, 0.056, d);
                let ang = (v - gy).atan2(u - gx);
                let needle = 1.0 - smoothstep(0.035, 0.075,
                    (ang - (-2.0 + gi as f32 * 0.9)).abs().min(6.283 - (ang - (-2.0 + gi as f32 * 0.9)).abs()));
                let dial = 0.86 - rim * 0.45 - needle * 0.62 * (1.0 - rim);
                for i in 0..3 { c[i] = c[i] * (1.0 - face) + dial * face; }
            }
        }

        // Indicator lamps: two rows of six.
        for row in 0..2 {
            let ly = 0.575 + row as f32 * 0.180;
            for i in 0..6 {
                let lx = 0.115 + i as f32 * 0.154;
                let d = slab(u, v, lx - 0.040, ly - 0.030, lx + 0.040, ly + 0.030);
                if d <= 0.0 { continue; }
                let k = hash2(i, row + 7, seed);
                let on = k > 0.42;
                let hue = hash2(i + 11, row, seed ^ 0x5B);
                let col = if hue < 0.16 { [1.00f32, 0.28, 0.20] }
                          else if hue < 0.58 { [1.00, 0.72, 0.24] }
                          else { [0.42, 1.00, 0.46] };
                let m = if on { 1.30 } else { 0.26 };
                let body = smoothstep(0.0, 0.010, d);
                for j in 0..3 { c[j] = c[j] * (1.0 - body) + col[j] * m * body; }
            }
        }

        (c, 1.0)
    });
    p.grime(0.5, seed);
}

/// Water: slow ripples, no transparency (it is decorative here).
fn gen_water(p: &mut Painter, tint: [u8; 3], seed: u32) {
    p.shade(tint, |u, v, _, _| {
        let swell = fbm(u * 4.0, v * 4.0, 4, 4, seed);
        let chop = ridged(u * 14.0, v * 14.0, 14, 4, seed ^ 0x2C);
        let glint = ridged(u * 34.0, v * 34.0, 34, 2, seed ^ 0x91);
        // Water is dark with bright things on it, not bright throughout. The
        // old balance sat everything in the top half of the range and came
        // out as a flat sheet of cyan; most of the contrast now lives in the
        // glints, which the sun's own highlight adds to on top.
        let lum = 0.42 + swell * 0.26 + chop * 0.20 + smoothstep(0.68, 0.96, glint) * 0.62;
        (lum, 1.0)
    });
}

/// Builds every world material at the requested resolution.
///
/// Generation is spread over the machine's cores. At the highest tier this is
/// eighty half-megapixel images of multi-octave noise, which is several
/// seconds on one thread and a blink on eight.
pub fn generate_world_array(size: u32) -> TextureArray {
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, 16)
        .min(MAT_COUNT + 1);

    // One layer past the materials holds the shared detail noise. See
    // `DETAIL_LAYER`: it is sampled at a much higher frequency than the
    // material underneath, which is what stops a surface going flat and
    // featureless as you walk up to it.
    let mut layers: Vec<Option<LayerMips>> = (0..MAT_COUNT + 1).map(|_| None).collect();
    {
        let chunk = (MAT_COUNT + 1).div_ceil(workers);
        let mut slices: Vec<&mut [Option<LayerMips>]> = layers.chunks_mut(chunk).collect();
        std::thread::scope(|s| {
            for (ci, slot) in slices.iter_mut().enumerate() {
                let first = ci * chunk;
                s.spawn(move || {
                    for (k, out) in slot.iter_mut().enumerate() {
                        *out = Some(generate_layer(first + k, size));
                    }
                });
            }
        });
    }

    let layers: Vec<LayerMips> = layers.into_iter().map(|l| l.expect("layer generated")).collect();
    let mip_count = layers.first().map(|l| l.mips.len() as u32).unwrap_or(1);
    TextureArray { size, mip_count, layers }
}

/// Index of the detail layer within the world texture array.
pub const DETAIL_LAYER: u32 = MAT_COUNT as u32;

/// Fine grey noise, centred on mid-grey so it modulates without tinting.
fn gen_detail(p: &mut Painter, seed: u32) {
    p.shade([255, 255, 255], |u, v, _, _| {
        let a = vnoise(u * 24.0, v * 24.0, 24, seed);
        let b = vnoise(u * 61.0, v * 61.0, 61, seed ^ 0x2F);
        let c = vnoise(u * 149.0, v * 149.0, 149, seed ^ 0x71);
        // Three octaves, weighted so the finest dominates: this layer exists
        // to add the frequencies the material lost, not to add another blotch.
        let n = 0.5 + ((a - 0.5) * 0.22 + (b - 0.5) * 0.34 + (c - 0.5) * 0.44);
        (n, 1.0)
    });
}

fn generate_layer(i: usize, size: u32) -> LayerMips {
    if i == MAT_COUNT {
        let mut p = Painter::new(size);
        gen_detail(&mut p, 0x0D_E7A1);
        return LayerMips { mips: build_mips(p.px, size) };
    }
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
        StoneWall => gen_brick(&mut p, tint, 7.0, [148, 144, 136], seed),
        MetalPanel => gen_panel(&mut p, tint, 3.0, true, 0.0, seed),
        MetalRust => gen_panel(&mut p, tint, 2.0, true, 0.9, seed),
        MetalPlateDiamond => gen_chequer(&mut p, tint, 15.0, seed),
        Corrugated => gen_corrugated(&mut p, tint, 12.0, seed),
        Grating => gen_grid(&mut p, tint, 7.0, 0.20, true, seed),
        HullPainted => gen_panel(&mut p, tint, 2.0, true, 0.35, seed),
        PipeMetal => gen_corrugated(&mut p, tint, 3.0, seed),
        ShippingRed | ShippingBlue | ShippingGreen => gen_corrugated(&mut p, tint, 9.0, seed),
        Sand => gen_drift(&mut p, tint, 7.0, 0.55, 0.0, seed),
        SandRock => gen_granular(&mut p, tint, 22.0, 0.20, seed),
        Dirt => gen_granular(&mut p, tint, 21.0, 0.22, seed),
        Gravel => gen_granular(&mut p, tint, 30.0, 0.32, seed),
        Grass => gen_grass(&mut p, tint, [118, 96, 62], seed),
        JungleFloor => gen_grass(&mut p, tint, [92, 74, 48], seed ^ 0x11),
        Snow => gen_drift(&mut p, tint, 4.0, 0.42, 0.16, seed),
        SnowRock => gen_granular(&mut p, tint, 22.0, 0.18, seed),
        Asphalt => gen_granular(&mut p, tint, 30.0, 0.14, seed),
        ConcreteFloor => gen_rough(&mut p, tint, 0.09, 0.18, seed),
        TileFloor => gen_grid(&mut p, tint, 4.0, 0.03, false, seed),
        WoodFloor => gen_wood(&mut p, tint, 6.0, false, seed),
        Mud => gen_granular(&mut p, tint, 18.0, 0.19, seed),
        Cobble => gen_granular(&mut p, tint, 24.0, 0.27, seed),
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
        ControlPanel => gen_control_panel(&mut p, tint, seed),
        Barrel | BarrelRust => gen_corrugated(&mut p, tint, 4.0, seed),
        Tire => gen_tread(&mut p, tint, seed),
        Rubber => gen_granular(&mut p, tint, 34.0, 0.07, seed),
        HazardStripe => gen_stripes(&mut p, tint, [40, 38, 36], 6.0, seed),
        RedPaint | BluePaint | YellowPaint => gen_panel(&mut p, tint, 2.0, false, 0.25, seed),
        Sign => gen_panel(&mut p, tint, 1.0, false, 0.1, seed),
        RoofTile => gen_grid(&mut p, tint, 8.0, 0.06, false, seed),
        RoofMetal => gen_corrugated(&mut p, tint, 14.0, seed),
        Foliage => gen_foliage(&mut p, tint, true, seed),
        Rock => gen_granular(&mut p, tint, 16.0, 0.26, seed),
        Ice => gen_water(&mut p, tint, seed),
        WaterSurface => gen_water(&mut p, tint, seed ^ 0x9),
        Fabric => gen_woven(&mut p, tint, 10.0, 0.14, seed),
        DirtRoad => gen_granular(&mut p, tint, 16.0, 0.20, seed),
        Marble => gen_marble(&mut p, tint, seed),
        Bunker => gen_panel(&mut p, tint, 1.5, false, 0.15, seed),
        Duct => gen_corrugated(&mut p, tint, 8.0, seed),
        GunMetal => gen_gunmetal(&mut p, tint, seed),
        GunPolymer => gen_polymer(&mut p, tint, seed),
        Facade => gen_facade(&mut p, tint, seed),
        Mesh => gen_grid(&mut p, tint, 10.0, 0.10, true, seed),
    }

    LayerMips { mips: build_mips(p.px, size) }
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
/// Particles are drawn large and close -- a smoke puff can be half the
/// screen -- and there are no mips on this atlas, so the cell size is the
/// only thing standing between a plume and a staircase.
pub const SPRITE_CELL: u32 = 128;
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
    // Shell casing.
    //
    // This was a hard-edged rectangle of flat brightness, which at the size
    // it is drawn is a yellow domino tumbling out of the gun. A case is a
    // little brass cylinder: rounded ends, a rim at the base, dark where it
    // turns away and a hot line along the top where the light runs down it.
    put(Sprite::Casing, &mut |u, v| {
        let body = 0.80f32;
        let radius = 0.30f32;
        // Distance to the axis segment, so the ends are round.
        let dx = (u.abs() - (body - radius)).max(0.0);
        let d = (dx * dx + v * v).sqrt();
        let a = ((radius - d) * 9.0).clamp(0.0, 1.0);
        // Cylindrical shading across the short axis.
        let across = (v / radius).clamp(-1.0, 1.0);
        let round = (1.0 - across * across).sqrt();
        let mut shade = 0.34 + round * 0.62;
        // Specular line, high on the lit side.
        shade += (1.0 - ((across + 0.45) * 3.4).abs()).max(0.0) * 0.45;
        // Extractor rim at the base.
        if u < -(body - radius) - 0.06 { shade *= 0.72; }
        (shade.min(1.0), a)
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
