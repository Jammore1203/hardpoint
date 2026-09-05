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
    p.shade(tint, |u, v, _, _| {
        let n = fbm(u * period as f32, v * period as f32, period, 5, seed);
        let mid = fbm(u * 40.0, v * 40.0, 40, 3, seed ^ 0x1D);
        let fine = vnoise(u * 96.0, v * 96.0, 96, seed ^ 0x99);
        // Hairline cracks from a worley boundary. Faint, and at a period well
        // off the pattern's own, because a crack network the eye can trace is
        // a crack network the eye can see repeating every three metres.
        let (f1, f2) = worley2(u * 11.0, v * 11.0, 11, seed ^ 0xC4);
        let crack = (1.0 - smoothstep(tx * 4.0, tx * 14.0, f2 - f1))
            * smoothstep(0.35, 0.60, fbm(u * 3.0, v * 3.0, 3, 2, seed ^ 0x6F));
        // Small pits: the pockmarking that reads as concrete rather than paper.
        let (pf, _) = worley2(u * 40.0, v * 40.0, 40, seed ^ 0x2E);
        let pit = smoothstep(0.26, 0.02, pf) * 0.07;
        let mut lum = 0.80 + (n - 0.5) * blotch + (mid - 0.5) * blotch * 0.45
            + (fine - 0.5) * grain;
        lum -= crack * 0.07 + pit;
        (lum, 1.0)
    });
    p.grime(0.5, seed);
}

/// Regular brick courses with mortar.
fn gen_brick(p: &mut Painter, tint: [u8; 3], rows: f32, mortar: [u8; 3], seed: u32) {
    let mortar_l = [mortar[0] as f32 / 255.0, mortar[1] as f32 / 255.0, mortar[2] as f32 / 255.0];
    let base = [tint[0] as f32 / 255.0, tint[1] as f32 / 255.0, tint[2] as f32 / 255.0];
    let tx = p.texel;
    // Joints get an antialiased shoulder; anything thinner than about three
    // texels crawls at distance no matter how good the mip chain is.
    let soft = (tx * rows * 1.6).max(0.006);
    p.shade_rgb(|u, v| {
        let row = (v * rows).floor();
        let offset = if (row as i32) % 2 == 0 { 0.0 } else { 0.5 };
        let bu = (u * rows * 0.5 + offset).fract();
        let bv = (v * rows).fract();
        let joint_u = 0.05;
        let joint_v = 0.10;
        // Distance into the brick face, 0 at the joint centreline.
        let du = smoothstep(joint_u - soft, joint_u + soft, cell_edge(bu));
        let dv = smoothstep(joint_v - soft * 2.0, joint_v + soft * 2.0, cell_edge(bv));
        let face = du.min(dv);

        let id = hash2((u * rows * 0.5 + offset) as i32, row as i32, seed);
        let id2 = hash2((u * rows * 0.5 + offset) as i32, row as i32, seed ^ 0xBEEF);
        let n = fbm(u * 60.0, v * 60.0, 60, 3, seed);
        let coarse = fbm(u * 9.0, v * 9.0, 9, 3, seed ^ 0x4C);

        // Mortar: paler, rougher, and recessed, so the courses catch light.
        let mortar_lum = 0.80 + (n - 0.5) * 0.20;
        // Brick faces vary in both value and hue; a wall of identical bricks
        // is the tell of a generated texture.
        let brick_lum = 0.66 + id * 0.34 + (n - 0.5) * 0.16 + (coarse - 0.5) * 0.12;
        // Chipped corners: bite into the face near the joints on some bricks.
        let chip = if id2 > 0.72 {
            smoothstep(0.16, 0.0, cell_edge(bu).min(cell_edge(bv))) * (id2 - 0.72) * 2.6
        } else { 0.0 };
        let lum = mortar_lum + (brick_lum - mortar_lum) * face - chip * 0.18;

        // A shadow line along the bottom of each course sells the relief.
        let shadow = (1.0 - smoothstep(joint_v, joint_v + 0.07, bv)) * 0.10;
        let value = (lum - shadow).max(0.0);

        // Fired brick varies in hue as well as value: some run red, some tan.
        let warm = (id - 0.5) * 0.16 * face;
        let hue = [1.0 + warm, 1.0, 1.0 - warm * 0.7];
        let mixed = [
            (mortar_l[0] + (base[0] * hue[0] - mortar_l[0]) * face) * value,
            (mortar_l[1] + (base[1] * hue[1] - mortar_l[1]) * face) * value,
            (mortar_l[2] + (base[2] * hue[2] - mortar_l[2]) * face) * value,
        ];
        (mixed, 1.0)
    });
    p.grime(0.55, seed);
}

/// Rectangular panels with recessed seams and rivets: metal, hulls, bunkers.
fn gen_panel(p: &mut Painter, tint: [u8; 3], divisions: f32, rivets: bool, rust: f32, seed: u32) {
    let tx = p.texel;
    let soft = (tx * divisions * 1.8).max(0.008);
    p.shade(tint, |u, v, _, _| {
        let pu = (u * divisions).fract();
        let pv = (v * divisions).fract();
        let seam = 0.040;
        let edge_u = cell_edge(pu);
        let edge_v = cell_edge(pv);
        let inside = smoothstep(seam - soft, seam + soft, edge_u.min(edge_v));

        let cell = hash2((u * divisions) as i32, (v * divisions) as i32, seed);
        let mut lum = 0.80 + cell * 0.14;
        // Recessed seam, with a lit shoulder on the upper lip: a bevel drawn
        // in value, which is all a texture can do without a normal map.
        let bevel = (1.0 - smoothstep(seam, seam + 0.030, edge_v)) * (if pv < 0.5 { 0.14 } else { -0.10 });
        lum = lum * (0.60 + 0.40 * inside) + bevel * inside;

        if rivets {
            // Round heads inset from each corner, antialiased and lit from
            // above so they read as domes rather than dots.
            let r = 0.055f32;
            let inset = 0.10f32;
            let dx = (pu - inset).min(pu - (1.0 - inset)).abs().min((pu - (1.0 - inset)).abs());
            let dy = (pv - inset).abs().min((pv - (1.0 - inset)).abs());
            let d = (dx * dx + dy * dy).sqrt();
            let head = 1.0 - smoothstep(r - soft, r + soft, d);
            let lit = ((r - d) / r).clamp(0.0, 1.0);
            lum += head * (0.10 + lit * 0.16) - head * smoothstep(0.4, 1.0, dy / r.max(1e-4)) * 0.05;
        }

        let grain = vnoise(u * 110.0, v * 110.0, 110, seed ^ 0x55);
        let brushed = vnoise(u * 6.0, v * 150.0, 150, seed ^ 0x71);
        lum += (grain - 0.5) * 0.05 + (brushed - 0.5) * 0.05;

        if rust > 0.0 {
            // Rust blooms from the seams and runs downward, which is where
            // water actually sits on a panel.
            let bloom = fbm(u * 10.0, v * 10.0, 10, 4, seed ^ 0xAB);
            let run = fbm(u * 26.0, v * 5.0, 26, 3, seed ^ 0xD3);
            let near_seam = 1.0 - inside;
            let amount = ((bloom - 0.50).max(0.0) * 2.0 + near_seam * 0.35 + (run - 0.6).max(0.0) * 1.2)
                .clamp(0.0, 1.0);
            lum *= 1.0 - amount * rust * 0.45;
        }
        (lum, 1.0)
    });
    p.grime(0.45, seed);
}

/// Vertical corrugations.
fn gen_corrugated(p: &mut Painter, tint: [u8; 3], ribs: f32, seed: u32) {
    let tx = p.texel;
    // A rib narrower than four texels is a moire generator. Cap the count so
    // the profile is always resolvable at this resolution.
    let ribs = ribs.min(1.0 / (tx * 6.0));
    p.shade(tint, |u, v, _, _| {
        let phase = u * ribs * std::f32::consts::TAU;
        let wave = phase.sin();
        // Asymmetric: a bright crest and a darker, wider trough, which is what
        // a rolled sheet lit from above actually looks like.
        let mut lum = 0.78 + wave * 0.16 + (wave * wave - 0.5) * 0.06;
        // Horizontal fixing seams every so often.
        let band = (v * 4.0).fract();
        lum *= 1.0 - (1.0 - smoothstep(0.0, tx * 5.0, cell_edge(band))) * 0.22;
        // Weathering runs down the troughs.
        let streak = fbm(u * ribs * 0.5, v * 6.0, 12, 3, seed);
        lum += (streak - 0.5) * 0.10 * (0.6 - wave * 0.4);
        let grain = vnoise(u * 90.0, v * 90.0, 90, seed ^ 0x3B);
        lum += (grain - 0.5) * 0.04;
        (lum, 1.0)
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
    // The coarse layer only exists for genuinely stony ground, and even then
    // at a period chosen not to echo the fine layer.
    let big = (cells * 0.34).max(5.0);
    let cells = cells.min(1.0 / (tx * 3.0));
    p.shade(tint, |u, v, _, _| {
        let (f1, f2) = worley2(u * cells, v * cells, cells as i32, seed);
        let drift = fbm(u * 3.0, v * 3.0, 3, 4, seed ^ 0x77);
        let mid = fbm(u * 11.0, v * 11.0, 11, 3, seed ^ 0x21);
        let fine = vnoise(u * 128.0, v * 128.0, 128, seed ^ 0x4D);

        // Each cell lit like a small dome, with a shadow in the gap between.
        let dome = (1.0 - f1) * contrast;
        let gap = (1.0 - smoothstep(tx * 3.0, 0.10, f2 - f1)) * contrast * 0.55 * stony;

        let mut lum = 0.72 + dome + (drift - 0.5) * 0.20 + (mid - 0.5) * 0.13
            + (fine - 0.5) * 0.09 - gap;

        if stony > 0.01 {
            // Faint. A coarse cell layer at readable strength is a honeycomb
            // the eye locks onto, and it repeats every tile.
            let (b1, b2) = worley2(u * big, v * big, big as i32, seed ^ 0x9E1);
            let boulder = ((1.0 - b1) - 0.5) * contrast * 0.20 * stony;
            let seam = (1.0 - smoothstep(tx * 3.0, 0.10, b2 - b1)) * contrast * 0.16 * stony;
            lum += boulder - seam;
        }
        (lum, 1.0)
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

/// Woven fabric or sandbags.
fn gen_woven(p: &mut Painter, tint: [u8; 3], threads: f32, lumpy: f32, seed: u32) {
    let tx = p.texel;
    let threads = threads.min(1.0 / (tx * 8.0));
    p.shade(tint, |u, v, _, _| {
        let a = (u * threads * std::f32::consts::TAU).sin();
        let b = (v * threads * std::f32::consts::TAU).sin();
        // Over-under weave rather than a product, so the warp and weft cross.
        let over = if a > b { a } else { b };
        let weave = over * 0.5 + 0.5;
        let lump = fbm(u * 5.0, v * 5.0, 5, 4, seed);
        let fray = vnoise(u * 64.0, v * 64.0, 64, seed ^ 0x8A);
        let lum = 0.70 + weave * 0.20 + (lump - 0.5) * lumpy + (fray - 0.5) * 0.06;
        (lum, 1.0)
    });
    p.grime(0.4, seed);
}

/// Planks with visible grain.
fn gen_wood(p: &mut Painter, tint: [u8; 3], planks: f32, vertical: bool, seed: u32) {
    let tx = p.texel;
    let soft = (tx * planks * 2.0).max(0.010);
    p.shade(tint, |u, v, _, _| {
        let (along, across) = if vertical { (v, u) } else { (u, v) };
        let plank = (across * planks).floor();
        let edge = (across * planks).fract();
        let id = hash2(plank as i32, 0, seed);
        let id2 = hash2(plank as i32, 7, seed);

        // Growth rings: ridged noise stretched hard along the plank.
        let rings = ridged(along * 3.0 + id * 8.0, across * planks * 6.0, 24, 4, seed);
        let fibre = vnoise(along * 120.0, across * planks * 20.0, 120, seed ^ 0x2B);
        // A knot or two per plank.
        let kx = hash2(plank as i32, 3, seed);
        let kd = ((along - kx) * (along - kx) * 6.0
            + (edge - 0.5) * (edge - 0.5)).sqrt();
        let knot = smoothstep(0.16, 0.0, kd) * if id2 > 0.55 { 1.0 } else { 0.0 };

        let mut lum = 0.70 + id * 0.22 + rings * 0.26 + (fibre - 0.5) * 0.10;
        lum -= knot * 0.34;
        // Gap between planks, plus a lit top lip.
        let gap = 1.0 - smoothstep(0.0, soft * 2.0, cell_edge(edge));
        lum = lum * (1.0 - gap * 0.55);
        (lum, 1.0)
    });
    p.grime(0.45, seed);
}

/// Regular grid: tiles, diamond plate, gratings.
fn gen_grid(p: &mut Painter, tint: [u8; 3], cells: f32, thickness: f32, cutout: bool, seed: u32) {
    let tx = p.texel;
    // Never ask for more cells than the resolution can hold a bar in.
    let cells = cells.min(1.0 / (tx * 10.0));
    let soft = (tx * cells * 1.5).max(0.010);
    p.shade(tint, |u, v, _, _| {
        let cu = (u * cells).fract();
        let cv = (v * cells).fract();
        let du = cell_edge(cu);
        let dv = cell_edge(cv);
        // `bar` is 1 on the frame, 0 in the hole, with a soft shoulder.
        let bar = (1.0 - smoothstep(thickness - soft, thickness + soft, du))
            .max(1.0 - smoothstep(thickness - soft, thickness + soft, dv));
        let n = vnoise(u * 80.0, v * 80.0, 80, seed);
        if cutout {
            // The hole is transparent, the bar is lit along its upper edge.
            let lit = 1.0 - smoothstep(0.0, thickness, dv.min(du));
            let lum = 0.72 + lit * 0.22 + (n - 0.5) * 0.14;
            (lum, bar)
        } else {
            // A tile floor: grout recessed, tile face slightly domed, and the
            // odd cracked or discoloured tile.
            let id = hash2((u * cells) as i32, (v * cells) as i32, seed);
            let dome = (du.min(dv) * 4.0).min(1.0);
            let lum = (0.60 + (n - 0.5) * 0.10) * bar
                + (0.86 + id * 0.16 + dome * 0.06 + (n - 0.5) * 0.08) * (1.0 - bar);
            (lum, 1.0)
        }
    });
    if !cutout { p.grime(0.4, seed); }
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
    p.shade_rgb(|u, v| {
        let a = fbm(u * 6.0, v * 6.0, 6, 4, seed);
        let b = fbm(u * 10.0, v * 10.0, 10, 4, seed ^ 0x5A5A);
        let c3 = fbm(u * 16.0, v * 16.0, 16, 3, seed ^ 0xA13);
        // Three overlapping blob layers, edges softened by a few texels.
        let m1 = smoothstep(0.54 - soft, 0.54 + soft, a);
        let m2 = smoothstep(0.57 - soft, 0.57 + soft, b) * (1.0 - m1);
        let m3 = smoothstep(0.60 - soft, 0.60 + soft, c3) * (1.0 - m1) * (1.0 - m2);
        let lum = 0.86 + m1 * 0.34 - m2 * 0.24 - m3 * 0.10;
        // Fabric weave under the print.
        let weave = ((u * 160.0).sin() * (v * 160.0).sin()) * 0.5 + 0.5;
        let grain = vnoise(u * 70.0, v * 70.0, 70, seed);
        let l = lum + (grain - 0.5) * 0.07 + (weave - 0.5) * 0.035;
        // The dark blobs shift toward green, the light ones toward tan.
        let cc = [
            base[0] * l * (1.0 + m1 * 0.06 - m2 * 0.10),
            base[1] * l * (1.0 + m2 * 0.06),
            base[2] * l * (1.0 - m1 * 0.10 - m2 * 0.06),
        ];
        (cc, 1.0)
    });
    p.grime(0.35, seed);
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

/// Water: slow ripples, no transparency (it is decorative here).
fn gen_water(p: &mut Painter, tint: [u8; 3], seed: u32) {
    p.shade(tint, |u, v, _, _| {
        let swell = fbm(u * 4.0, v * 4.0, 4, 4, seed);
        let chop = ridged(u * 14.0, v * 14.0, 14, 4, seed ^ 0x2C);
        let glint = ridged(u * 34.0, v * 34.0, 34, 2, seed ^ 0x91);
        let lum = 0.62 + swell * 0.30 + chop * 0.24 + smoothstep(0.72, 0.95, glint) * 0.34;
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
        .min(MAT_COUNT);

    let mut layers: Vec<Option<LayerMips>> = (0..MAT_COUNT).map(|_| None).collect();
    {
        let chunk = MAT_COUNT.div_ceil(workers);
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

fn generate_layer(i: usize, size: u32) -> LayerMips {
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
        StoneWall => gen_granular(&mut p, tint, 15.0, 0.26, seed),
        MetalPanel => gen_panel(&mut p, tint, 3.0, true, 0.0, seed),
        MetalRust => gen_panel(&mut p, tint, 2.0, true, 0.9, seed),
        MetalPlateDiamond => gen_grid(&mut p, tint, 8.0, 0.16, false, seed),
        Corrugated => gen_corrugated(&mut p, tint, 12.0, seed),
        Grating => gen_grid(&mut p, tint, 7.0, 0.20, true, seed),
        HullPainted => gen_panel(&mut p, tint, 2.0, true, 0.35, seed),
        PipeMetal => gen_corrugated(&mut p, tint, 3.0, seed),
        ShippingRed | ShippingBlue | ShippingGreen => gen_corrugated(&mut p, tint, 9.0, seed),
        Sand => gen_granular(&mut p, tint, 22.0, 0.16, seed),
        SandRock => gen_granular(&mut p, tint, 17.0, 0.24, seed),
        Dirt => gen_granular(&mut p, tint, 21.0, 0.22, seed),
        Gravel => gen_granular(&mut p, tint, 30.0, 0.30, seed),
        Grass => gen_foliage(&mut p, tint, false, seed),
        JungleFloor => gen_foliage(&mut p, tint, false, seed ^ 0x11),
        Snow => gen_granular(&mut p, tint, 18.0, 0.10, seed),
        SnowRock => gen_granular(&mut p, tint, 16.0, 0.21, seed),
        Asphalt => gen_granular(&mut p, tint, 30.0, 0.14, seed),
        ConcreteFloor => gen_rough(&mut p, tint, 0.09, 0.18, seed),
        TileFloor => gen_grid(&mut p, tint, 4.0, 0.03, false, seed),
        WoodFloor => gen_wood(&mut p, tint, 6.0, false, seed),
        Mud => gen_granular(&mut p, tint, 18.0, 0.19, seed),
        Cobble => gen_granular(&mut p, tint, 13.0, 0.30, seed),
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
        Rock => gen_granular(&mut p, tint, 12.0, 0.30, seed),
        Ice => gen_water(&mut p, tint, seed),
        WaterSurface => gen_water(&mut p, tint, seed ^ 0x9),
        Fabric => gen_woven(&mut p, tint, 10.0, 0.14, seed),
        DirtRoad => gen_granular(&mut p, tint, 16.0, 0.20, seed),
        Marble => gen_rough(&mut p, tint, 0.05, 0.30, seed),
        Bunker => gen_panel(&mut p, tint, 1.5, false, 0.15, seed),
        Duct => gen_corrugated(&mut p, tint, 8.0, seed),
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
