//! Sound synthesis.
//!
//! Every sound in the game is generated from scratch at startup. That is not
//! a limitation dressed up as a feature: procedural audio means the whole
//! sound set is a few hundred lines, every weapon's report is derived from its
//! own stats so a heavier gun automatically sounds heavier, and there is no
//! sample licensing question anywhere.

use crate::core::Rng;

/// A mono clip at the device's sample rate.
pub type Clip = Vec<f32>;

// ------------------------------------------------------------------ filters

/// One-pole low pass. `cutoff` is normalised (cutoff_hz / sample_rate).
pub struct LowPass {
    a: f32,
    z: f32,
}

impl LowPass {
    pub fn new(cutoff: f32) -> LowPass {
        let x = (-std::f32::consts::TAU * cutoff.clamp(0.0001, 0.49)).exp();
        LowPass { a: 1.0 - x, z: 0.0 }
    }
    #[inline]
    pub fn run(&mut self, x: f32) -> f32 {
        self.z += self.a * (x - self.z);
        self.z
    }
}

pub struct HighPass {
    lp: LowPass,
}

impl HighPass {
    pub fn new(cutoff: f32) -> HighPass { HighPass { lp: LowPass::new(cutoff) } }
    #[inline]
    pub fn run(&mut self, x: f32) -> f32 { x - self.lp.run(x) }
}

/// A resonant band pass, used to give each surface and each weapon body its
/// own colour.
pub struct BandPass {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl BandPass {
    pub fn new(freq: f32, q: f32, sample_rate: f32) -> BandPass {
        let w = std::f32::consts::TAU * (freq / sample_rate).clamp(0.0001, 0.49);
        let alpha = w.sin() / (2.0 * q.max(0.1));
        let cos_w = w.cos();
        let a0 = 1.0 + alpha;
        BandPass {
            b0: alpha / a0,
            b1: 0.0,
            b2: -alpha / a0,
            a1: -2.0 * cos_w / a0,
            a2: (1.0 - alpha) / a0,
            x1: 0.0, x2: 0.0, y1: 0.0, y2: 0.0,
        }
    }
    #[inline]
    pub fn run(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2 - self.a1 * self.y1 - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

// ---------------------------------------------------------------- envelopes

/// Exponential decay with a short attack, which is what almost every
/// percussive sound wants.
#[inline]
pub fn env_pluck(t: f32, attack: f32, decay: f32) -> f32 {
    if t < 0.0 { return 0.0; }
    let a = if attack <= 0.0 { 1.0 } else { (t / attack).min(1.0) };
    let d = (-t / decay.max(0.0001)).exp();
    a * d
}

/// Linear attack, hold, linear release.
#[inline]
pub fn env_ahr(t: f32, attack: f32, hold: f32, release: f32) -> f32 {
    if t < 0.0 { return 0.0; }
    if t < attack { return t / attack.max(0.0001); }
    if t < attack + hold { return 1.0; }
    let r = (t - attack - hold) / release.max(0.0001);
    (1.0 - r).max(0.0)
}

#[inline]
pub fn soft_clip(x: f32) -> f32 {
    // Cheap saturating curve; keeps loud layered sounds from tearing.
    let x = x.clamp(-3.0, 3.0);
    x - x * x * x / 6.0
}

#[inline]
pub fn sine(phase: f32) -> f32 { (phase * std::f32::consts::TAU).sin() }

#[inline]
pub fn saw(phase: f32) -> f32 { phase.fract() * 2.0 - 1.0 }

#[inline]
pub fn square(phase: f32, duty: f32) -> f32 {
    if phase.fract() < duty { 1.0 } else { -1.0 }
}

#[inline]
pub fn triangle(phase: f32) -> f32 {
    let p = phase.fract();
    if p < 0.5 { p * 4.0 - 1.0 } else { 3.0 - p * 4.0 }
}

/// Normalises a clip to a target peak and applies a short fade at both ends so
/// nothing clicks when it starts or stops.
pub fn finish(mut clip: Clip, peak: f32) -> Clip {
    let mut max = 0.0f32;
    for s in clip.iter() { max = max.max(s.abs()); }
    if max > 0.0001 {
        let g = peak / max;
        for s in clip.iter_mut() { *s *= g; }
    }
    let fade = (clip.len() / 200).max(4).min(clip.len() / 2);
    let n = clip.len();
    for i in 0..fade {
        let f = i as f32 / fade as f32;
        clip[i] *= f;
        clip[n - 1 - i] *= f;
    }
    clip
}

/// A cheap reverb tail, built from a few delayed and filtered copies. Used to
/// place a gunshot in a space without a real convolution.
pub fn add_tail(clip: &mut Clip, sample_rate: f32, size: f32, wet: f32) {
    let taps = [0.031, 0.047, 0.071, 0.101, 0.139];
    let len = clip.len();
    let mut out = clip.clone();
    for (i, tap) in taps.iter().enumerate() {
        let delay = (tap * size * sample_rate) as usize;
        if delay == 0 || delay >= len { continue; }
        let gain = wet * (0.62f32).powi(i as i32 + 1);
        let mut lp = LowPass::new(0.10);
        for s in delay..len {
            out[s] += lp.run(clip[s - delay]) * gain;
        }
    }
    *clip = out;
}

// ------------------------------------------------------------------ sounds

/// A gunshot, derived from the weapon's own character.
///
/// `pitch` shifts the whole body, `bright` controls the transient crack, and
/// `body` controls how much low-end thump there is. A .50 calibre and a
/// machine pistol therefore come out of the same twenty lines sounding like
/// completely different objects.
pub fn gunshot(sample_rate: f32, pitch: f32, bright: f32, body: f32, seed: u32) -> Clip {
    let dur = 0.16 + body * 0.42;
    let n = (dur * sample_rate) as usize;
    let mut out = vec![0.0f32; n];
    let mut rng = Rng::seeded(seed);

    let mut crack_hp = HighPass::new(0.16);
    let mut body_lp = LowPass::new(0.035 + 0.05 * pitch);
    let mut mid = BandPass::new(420.0 * pitch, 1.4, sample_rate);
    let mut air = BandPass::new(3200.0 * pitch, 0.9, sample_rate);
    let mut phase = 0.0f32;

    for i in 0..n {
        let t = i as f32 / sample_rate;
        let noise = rng.signed();

        // The initial crack: very short, very bright.
        let crack = crack_hp.run(noise) * env_pluck(t, 0.0002, 0.010 + 0.006 * bright) * (0.5 + bright);

        // The body of the report: filtered noise with a slower decay.
        let body_env = env_pluck(t, 0.0008, 0.035 + body * 0.10);
        let low = body_lp.run(noise) * body_env * (0.6 + body * 1.2);

        // A pitched thump that gives the weapon its weight.
        let f = (78.0 * pitch) * (1.0 + 5.0 * (-t * 55.0).exp());
        phase += f / sample_rate;
        let thump = sine(phase) * env_pluck(t, 0.0006, 0.045 + body * 0.09) * body * 1.1;

        // Resonances that stop it sounding like a plain noise burst.
        let res = mid.run(noise) * env_pluck(t, 0.001, 0.05) * 0.45
            + air.run(noise) * env_pluck(t, 0.0004, 0.018) * bright * 0.5;

        out[i] = soft_clip(crack + low + thump + res);
    }

    add_tail(&mut out, sample_rate, 0.6 + body * 0.7, 0.16 + body * 0.14);
    finish(out, 0.92)
}

/// The mechanical part of a shot: bolt cycling, casing ejection.
pub fn action_click(sample_rate: f32, pitch: f32, seed: u32) -> Clip {
    let n = (0.09 * sample_rate) as usize;
    let mut out = vec![0.0f32; n];
    let mut rng = Rng::seeded(seed);
    let mut bp = BandPass::new(2400.0 * pitch, 3.0, sample_rate);
    let mut bp2 = BandPass::new(5200.0 * pitch, 5.0, sample_rate);
    for i in 0..n {
        let t = i as f32 / sample_rate;
        let noise = rng.signed();
        let a = bp.run(noise) * env_pluck(t, 0.0003, 0.014);
        let b = bp2.run(noise) * env_pluck(t, 0.0002, 0.006) * 0.7;
        out[i] = soft_clip(a + b);
    }
    finish(out, 0.5)
}

/// Magazine out, magazine in, bolt release: three separate clips so a reload
/// is a sequence rather than one sample.
pub fn reload_part(sample_rate: f32, kind: u8, seed: u32) -> Clip {
    let (dur, f1, f2, q) = match kind {
        0 => (0.15, 700.0, 1800.0, 2.0),   // magazine released
        1 => (0.18, 380.0, 1200.0, 1.6),   // magazine seated
        _ => (0.12, 2600.0, 5200.0, 4.0),  // bolt or charging handle
    };
    let n = (dur * sample_rate) as usize;
    let mut out = vec![0.0f32; n];
    let mut rng = Rng::seeded(seed);
    let mut a = BandPass::new(f1, q, sample_rate);
    let mut b = BandPass::new(f2, q * 1.6, sample_rate);
    for i in 0..n {
        let t = i as f32 / sample_rate;
        let noise = rng.signed();
        let hit = env_pluck(t, 0.0006, 0.030);
        let scrape = env_ahr(t, 0.004, 0.02, 0.05) * 0.35;
        out[i] = soft_clip(a.run(noise) * hit + b.run(noise) * hit * 0.6 + rng.signed() * scrape * 0.15);
    }
    finish(out, 0.45)
}

/// A footstep on a given surface.
pub fn footstep(sample_rate: f32, surface: crate::assets::materials::Surface, seed: u32) -> Clip {
    use crate::assets::materials::Surface::*;
    // Each surface gets its own filter centre, decay and grit.
    let (freq, q, decay, grit, thump) = match surface {
        Concrete => (900.0, 1.2, 0.045, 0.55, 0.35),
        Metal => (1800.0, 3.0, 0.090, 0.40, 0.20),
        Wood => (620.0, 1.8, 0.060, 0.45, 0.40),
        Dirt => (420.0, 0.9, 0.038, 0.75, 0.30),
        Sand => (1500.0, 0.6, 0.055, 1.00, 0.12),
        Gravel => (1900.0, 0.7, 0.050, 1.00, 0.22),
        Grass => (1100.0, 0.7, 0.042, 0.85, 0.18),
        Snow => (2400.0, 0.5, 0.070, 0.90, 0.10),
        Glass => (3400.0, 5.0, 0.055, 0.40, 0.08),
        Soft => (380.0, 0.8, 0.035, 0.50, 0.30),
        Water => (700.0, 0.8, 0.080, 0.80, 0.15),
    };
    let n = (0.14 * sample_rate) as usize;
    let mut out = vec![0.0f32; n];
    let mut rng = Rng::seeded(seed);
    let mut bp = BandPass::new(freq, q, sample_rate);
    let mut lp = LowPass::new(0.020);
    let mut phase = 0.0f32;
    for i in 0..n {
        let t = i as f32 / sample_rate;
        let noise = rng.signed();
        let body = bp.run(noise) * env_pluck(t, 0.001, decay) * grit;
        let low = lp.run(noise) * env_pluck(t, 0.0015, decay * 1.4) * thump;
        phase += 62.0 / sample_rate;
        let heel = sine(phase) * env_pluck(t, 0.0008, 0.026) * thump * 0.5;
        out[i] = soft_clip(body + low + heel);
    }
    finish(out, 0.36)
}

/// A bullet striking a surface.
pub fn impact(sample_rate: f32, surface: crate::assets::materials::Surface, seed: u32) -> Clip {
    use crate::assets::materials::Surface::*;
    let (freq, q, decay, ring) = match surface {
        Concrete => (1400.0, 1.0, 0.045, 0.0),
        Metal => (2600.0, 6.0, 0.130, 0.55),
        Wood => (900.0, 1.6, 0.055, 0.10),
        Dirt | Sand => (500.0, 0.7, 0.035, 0.0),
        Gravel => (1700.0, 0.8, 0.040, 0.0),
        Grass => (1000.0, 0.7, 0.030, 0.0),
        Snow => (2200.0, 0.5, 0.050, 0.0),
        Glass => (4200.0, 8.0, 0.180, 0.75),
        Soft => (360.0, 0.8, 0.030, 0.0),
        Water => (800.0, 0.9, 0.070, 0.0),
    };
    let n = (0.22 * sample_rate) as usize;
    let mut out = vec![0.0f32; n];
    let mut rng = Rng::seeded(seed);
    let mut bp = BandPass::new(freq, q, sample_rate);
    let mut hp = HighPass::new(0.05);
    let mut phase = 0.0f32;
    for i in 0..n {
        let t = i as f32 / sample_rate;
        let noise = rng.signed();
        let body = bp.run(noise) * env_pluck(t, 0.0002, decay);
        let snap = hp.run(noise) * env_pluck(t, 0.0001, 0.004) * 0.6;
        phase += freq * 1.5 / sample_rate;
        let tone = sine(phase) * env_pluck(t, 0.0004, decay * 1.6) * ring;
        out[i] = soft_clip(body + snap + tone);
    }
    finish(out, 0.5)
}

/// An explosion. Low, long and wide.
pub fn explosion(sample_rate: f32, size: f32, seed: u32) -> Clip {
    let dur = 1.0 + size * 0.8;
    let n = (dur * sample_rate) as usize;
    let mut out = vec![0.0f32; n];
    let mut rng = Rng::seeded(seed);
    let mut lp = LowPass::new(0.012);
    let mut mid = BandPass::new(240.0, 0.8, sample_rate);
    let mut phase = 0.0f32;
    for i in 0..n {
        let t = i as f32 / sample_rate;
        let noise = rng.signed();
        // A descending sub-bass sweep carries the weight.
        let f = 46.0 * (1.0 + 3.4 * (-t * 9.0).exp());
        phase += f / sample_rate;
        let sub = sine(phase) * env_pluck(t, 0.002, 0.30 * size) * 1.4;
        let blast = lp.run(noise) * env_pluck(t, 0.001, 0.16 * size) * 1.2;
        let debris = mid.run(noise) * env_pluck(t, 0.02, 0.55 * size) * 0.5;
        let crack = noise * env_pluck(t, 0.0004, 0.012) * 0.8;
        out[i] = soft_clip(sub + blast + debris + crack);
    }
    add_tail(&mut out, sample_rate, 1.4, 0.28);
    finish(out, 1.0)
}

/// The ringing left behind by a flashbang.
pub fn flash_ring(sample_rate: f32, seed: u32) -> Clip {
    let n = (2.6 * sample_rate) as usize;
    let mut out = vec![0.0f32; n];
    let mut rng = Rng::seeded(seed);
    let mut phase = 0.0;
    let mut phase2 = 0.0;
    for i in 0..n {
        let t = i as f32 / sample_rate;
        phase += 4300.0 / sample_rate;
        phase2 += 6100.0 / sample_rate;
        let env = env_pluck(t, 0.004, 1.1);
        let hiss = rng.signed() * env * 0.06;
        out[i] = soft_clip((sine(phase) * 0.7 + sine(phase2) * 0.3) * env + hiss);
    }
    finish(out, 0.55)
}

/// Grenade bouncing off something hard.
pub fn bounce(sample_rate: f32, seed: u32) -> Clip {
    let n = (0.10 * sample_rate) as usize;
    let mut out = vec![0.0f32; n];
    let mut rng = Rng::seeded(seed);
    let mut bp = BandPass::new(1600.0, 4.0, sample_rate);
    let mut phase = 0.0;
    for i in 0..n {
        let t = i as f32 / sample_rate;
        phase += 900.0 / sample_rate;
        out[i] = soft_clip(bp.run(rng.signed()) * env_pluck(t, 0.0002, 0.012)
            + sine(phase) * env_pluck(t, 0.0003, 0.020) * 0.5);
    }
    finish(out, 0.42)
}

/// A short confirmation blip. `up` gives a rising two-tone, otherwise falling.
pub fn blip(sample_rate: f32, base: f32, up: bool, dur: f32) -> Clip {
    let n = (dur * sample_rate) as usize;
    let mut out = vec![0.0f32; n];
    let mut phase = 0.0f32;
    for i in 0..n {
        let t = i as f32 / sample_rate;
        let half = if t < dur * 0.5 { 0.0 } else { 1.0 };
        let f = if up { base * (1.0 + half * 0.5) } else { base * (1.0 - half * 0.33) };
        phase += f / sample_rate;
        let env = env_pluck(t, 0.001, dur * 0.35);
        out[i] = (square(phase, 0.5) * 0.35 + sine(phase) * 0.65) * env;
    }
    finish(out, 0.5)
}

/// Radio squelch, used to top and tail every announcer cue.
pub fn squelch(sample_rate: f32, open: bool, seed: u32) -> Clip {
    let n = (0.10 * sample_rate) as usize;
    let mut out = vec![0.0f32; n];
    let mut rng = Rng::seeded(seed);
    let mut bp = BandPass::new(2000.0, 1.2, sample_rate);
    for i in 0..n {
        let t = i as f32 / sample_rate;
        let env = if open { env_pluck(t, 0.0008, 0.020) } else { env_ahr(t, 0.001, 0.01, 0.05) };
        out[i] = bp.run(rng.signed()) * env;
    }
    finish(out, 0.30)
}

/// An announcer cue: a short motif band-limited like a radio, between two
/// squelches. Different intervals read as different messages without a single
/// recorded word.
pub fn radio_cue(sample_rate: f32, notes: &[(f32, f32)], seed: u32) -> Clip {
    let total: f32 = notes.iter().map(|(_, d)| *d).sum::<f32>() + 0.22;
    let n = (total * sample_rate) as usize;
    let mut out = vec![0.0f32; n];
    let mut rng = Rng::seeded(seed);

    // Opening squelch.
    let open = squelch(sample_rate, true, seed);
    for (i, s) in open.iter().enumerate() {
        if i < n { out[i] += *s * 0.7; }
    }

    let mut cursor = 0.06;
    let mut phase = 0.0f32;
    for (freq, dur) in notes {
        let start = (cursor * sample_rate) as usize;
        let len = (*dur * sample_rate) as usize;
        for i in 0..len {
            let idx = start + i;
            if idx >= n { break; }
            let t = i as f32 / sample_rate;
            phase += freq / sample_rate;
            let env = env_ahr(t, 0.006, *dur * 0.55, *dur * 0.4);
            // A square with a little noise reads as a radio, not a synth.
            let tone = square(phase, 0.5) * 0.45 + triangle(phase) * 0.4;
            out[idx] += (tone + rng.signed() * 0.05) * env * 0.5;
        }
        cursor += dur;
    }

    // Band-limit the whole thing so it sits behind the action like comms.
    let mut hp = HighPass::new(0.012);
    let mut lp = LowPass::new(0.085);
    for s in out.iter_mut() {
        *s = soft_clip(lp.run(hp.run(*s)) * 1.4);
    }

    let close = squelch(sample_rate, false, seed ^ 0x55);
    let close_at = ((cursor + 0.01) * sample_rate) as usize;
    for (i, s) in close.iter().enumerate() {
        let idx = close_at + i;
        if idx < n { out[idx] += *s * 0.6; }
    }

    finish(out, 0.62)
}

/// Wind, rain and machinery, generated as a loopable bed.
pub fn ambience(sample_rate: f32, kind: u8, seconds: f32, seed: u32) -> Clip {
    let n = (seconds * sample_rate) as usize;
    let mut out = vec![0.0f32; n];
    let mut rng = Rng::seeded(seed);
    let mut lp = LowPass::new(0.0025);
    let mut lp2 = LowPass::new(0.05);
    let mut bp = BandPass::new(140.0, 0.6, sample_rate);
    let mut phase = 0.0f32;

    for i in 0..n {
        let t = i as f32 / sample_rate;
        let noise = rng.signed();
        // A slow modulation makes the bed breathe instead of hissing.
        let swell = 0.55 + 0.45 * ((t * 0.11).sin() * 0.5 + 0.5);
        out[i] = match kind {
            // Wind.
            0 => lp.run(noise) * swell * 2.2 + lp2.run(noise) * 0.06,
            // Industrial: a low hum with a slow mechanical pulse.
            1 => {
                phase += 50.0 / sample_rate;
                bp.run(noise) * 0.7 + sine(phase) * 0.10 + lp2.run(noise) * 0.05
                    * (1.0 + (t * 0.7).sin())
            }
            // Jungle: high insect texture over a soft bed.
            2 => {
                let chirp = ((t * 900.0).sin() * (t * 7.0).sin().max(0.0)).abs();
                lp.run(noise) * 0.8 + chirp * 0.05 + lp2.run(noise) * 0.10
            }
            // Interior: air handling.
            3 => lp.run(noise) * 1.1 + lp2.run(noise) * 0.03,
            // Coastal: slow surf.
            4 => {
                let surf = ((t * 0.23).sin() * 0.5 + 0.5).powf(3.0);
                lp2.run(noise) * surf * 0.7 + lp.run(noise) * 0.6
            }
            // Blizzard: harsher wind.
            _ => lp.run(noise) * swell * 2.6 + lp2.run(noise) * 0.16,
        };
    }

    // Cross-fade the ends so the loop is seamless.
    let fade = (sample_rate * 0.5) as usize;
    if n > fade * 2 {
        for i in 0..fade {
            let f = i as f32 / fade as f32;
            let tail = out[n - fade + i];
            out[i] = out[i] * f + tail * (1.0 - f);
        }
        out.truncate(n - fade);
    }
    finish(out, 0.30)
}
