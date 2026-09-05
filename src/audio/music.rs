//! Procedural music.
//!
//! A small step sequencer with five synth voices. Each track is a set of
//! patterns rather than an audio file, so the whole soundtrack costs a few
//! hundred bytes of source and is generated on demand into a seamless loop.
//!
//! The register is deliberate: driving industrial percussion, a low ostinato,
//! and cold sustained pads. That combination is what military shooters of the
//! era sounded like, and it stays out of the way during a firefight.

use super::synth::*;
use crate::core::Rng;
use crate::maps::MusicTrack;

/// Sixteenth notes per bar.
const STEPS: usize = 16;

struct Pattern {
    bpm: f32,
    bars: usize,
    /// One bit per sixteenth, per bar.
    kick: [u16; 4],
    snare: [u16; 4],
    hat: [u16; 4],
    /// Bass note per step, as a semitone offset; -1 is a rest.
    bass: [i8; STEPS],
    /// Chord roots for the pad, one per bar.
    pad: [i8; 4],
    /// Root note in Hz.
    root: f32,
    /// Overall level and character.
    drive: f32,
    pad_level: f32,
    lead: bool,
}

fn pattern_for(track: MusicTrack) -> Pattern {
    match track {
        // Slow, wide, mostly pad: the main menu.
        MusicTrack::Menu => Pattern {
            bpm: 84.0,
            bars: 4,
            kick: [0b1000_0000_0000_0000, 0b1000_0000_0000_0000, 0b1000_0000_0000_0000, 0b1000_0000_0010_0000],
            snare: [0, 0b0000_0000_1000_0000, 0, 0b0000_0000_1000_0000],
            hat: [0b0010_0010_0010_0010, 0b0010_0010_0010_0010, 0b0010_0010_0010_0010, 0b0010_0010_1010_1010],
            bass: [0, -1, -1, -1, -1, -1, -1, -1, 5, -1, -1, -1, -1, -1, -1, -1],
            pad: [0, 0, 5, 3],
            root: 55.0,
            drive: 0.5,
            pad_level: 1.0,
            lead: false,
        },
        // Mid-tempo, steady: between engagements.
        MusicTrack::Patrol => Pattern {
            bpm: 104.0,
            bars: 4,
            kick: [0b1000_0010_0000_1000, 0b1000_0010_0000_1000, 0b1000_0010_0000_1000, 0b1000_0010_0010_1010],
            snare: [0b0000_1000_0000_1000, 0b0000_1000_0000_1000, 0b0000_1000_0000_1000, 0b0000_1000_0010_1010],
            hat: [0b1010_1010_1010_1010, 0b1010_1010_1010_1010, 0b1010_1010_1010_1010, 0b1111_1010_1010_1110],
            bass: [0, -1, 0, -1, 3, -1, -1, -1, 0, -1, 0, -1, 7, -1, 5, -1],
            pad: [0, 0, 3, 5],
            root: 55.0,
            drive: 0.8,
            pad_level: 0.55,
            lead: false,
        },
        // Fast and loud: the match itself.
        MusicTrack::Assault => Pattern {
            bpm: 132.0,
            bars: 4,
            kick: [0b1010_0010_1000_0010, 0b1010_0010_1000_0010, 0b1010_0010_1000_0010, 0b1010_1010_1010_1110],
            snare: [0b0000_1000_0000_1000, 0b0000_1000_0000_1010, 0b0000_1000_0000_1000, 0b0000_1010_1010_1110],
            hat: [0b1111_1111_1111_1111, 0b1111_1111_1111_1111, 0b1111_1111_1111_1111, 0b1111_1111_1111_1111],
            bass: [0, 0, -1, 0, 3, -1, 0, -1, 0, 0, -1, 0, 7, -1, 5, 3],
            pad: [0, 3, 5, 3],
            root: 49.0,
            drive: 1.15,
            pad_level: 0.4,
            lead: true,
        },
        // Sparse and uneasy: search and destroy, low on the clock.
        MusicTrack::Tension => Pattern {
            bpm: 96.0,
            bars: 4,
            kick: [0b1000_0000_0000_0000, 0, 0b1000_0000_0000_0000, 0b1000_0000_1000_0000],
            snare: [0, 0, 0, 0b0000_0000_0000_1010],
            hat: [0b0000_1000_0000_1000, 0b0000_1000_0000_1000, 0b0000_1000_0000_1000, 0b0010_1000_0010_1010],
            bass: [0, -1, -1, -1, -1, -1, 1, -1, -1, -1, -1, -1, 0, -1, -1, -1],
            pad: [0, 1, 0, -2],
            root: 46.0,
            drive: 0.55,
            pad_level: 0.9,
            lead: false,
        },
        // Short, bright resolution.
        MusicTrack::Victory => Pattern {
            bpm: 110.0,
            bars: 2,
            kick: [0b1000_1000_1000_1000, 0b1000_1000_1010_1010, 0, 0],
            snare: [0b0000_1000_0000_1000, 0b0000_1010_1010_1110, 0, 0],
            hat: [0b1010_1010_1010_1010, 0b1111_1111_1111_1111, 0, 0],
            bass: [0, -1, 4, -1, 7, -1, 4, -1, 0, -1, 7, -1, 12, -1, -1, -1],
            pad: [0, 7, 0, 0],
            root: 65.0,
            drive: 0.9,
            pad_level: 0.8,
            lead: true,
        },
        // Short, sinking resolution.
        MusicTrack::Defeat => Pattern {
            bpm: 76.0,
            bars: 2,
            kick: [0b1000_0000_0000_0000, 0b1000_0000_0000_0000, 0, 0],
            snare: [0, 0b0000_0000_1000_0000, 0, 0],
            hat: [0, 0, 0, 0],
            bass: [0, -1, -1, -1, -3, -1, -1, -1, -5, -1, -1, -1, -7, -1, -1, -1],
            pad: [0, -3, 0, 0],
            root: 44.0,
            drive: 0.4,
            pad_level: 1.0,
            lead: false,
        },
    }
}

#[inline]
fn semitone(root: f32, n: i8) -> f32 {
    root * 2f32.powf(n as f32 / 12.0)
}

/// Renders one loop of a track.
pub fn render(track: MusicTrack, sample_rate: f32) -> Clip {
    let p = pattern_for(track);
    let step_dur = 60.0 / p.bpm / 4.0;
    let total = step_dur * (STEPS * p.bars) as f32;
    let n = (total * sample_rate) as usize;
    let mut out = vec![0.0f32; n];
    let mut rng = Rng::seeded(0xC0DE_0000 ^ track as u32);

    let add = |buf: &mut Vec<f32>, at: f32, clip: &[f32], gain: f32| {
        let start = (at * sample_rate) as usize;
        for (i, s) in clip.iter().enumerate() {
            let idx = start + i;
            if idx >= buf.len() { break; }
            buf[idx] += s * gain;
        }
    };

    // ------------------------------------------------------------ percussion
    let kick = render_kick(sample_rate, p.drive);
    let snare = render_snare(sample_rate, p.drive, &mut rng);
    let hat = render_hat(sample_rate, &mut rng);

    for bar in 0..p.bars {
        for step in 0..STEPS {
            let t = (bar * STEPS + step) as f32 * step_dur;
            let bit = 1u16 << (15 - step);
            if p.kick[bar % 4] & bit != 0 { add(&mut out, t, &kick, 0.95); }
            if p.snare[bar % 4] & bit != 0 { add(&mut out, t, &snare, 0.62); }
            if p.hat[bar % 4] & bit != 0 {
                // Accent the downbeats so the pattern has a pulse.
                let g = if step % 4 == 0 { 0.34 } else { 0.20 };
                add(&mut out, t, &hat, g);
            }
        }
    }

    // ------------------------------------------------------------------ bass
    let mut phase = 0.0f32;
    let mut lp = LowPass::new(0.030);
    for bar in 0..p.bars {
        for step in 0..STEPS {
            let note = p.bass[step];
            if note < 0 { continue; }
            let start = ((bar * STEPS + step) as f32 * step_dur * sample_rate) as usize;
            let len = (step_dur * 1.9 * sample_rate) as usize;
            let f = semitone(p.root, note + p.pad[bar % 4]);
            for i in 0..len {
                let idx = start + i;
                if idx >= n { break; }
                let t = i as f32 / sample_rate;
                phase += f / sample_rate;
                let env = env_ahr(t, 0.004, step_dur * 0.7, step_dur * 0.9);
                let v = saw(phase) * 0.6 + square(phase, 0.35) * 0.4;
                out[idx] += lp.run(v) * env * 0.55 * p.drive;
            }
        }
    }

    // ------------------------------------------------------------------- pad
    if p.pad_level > 0.01 {
        let bar_dur = step_dur * STEPS as f32;
        for bar in 0..p.bars {
            let root = semitone(p.root * 4.0, p.pad[bar % 4]);
            let start = (bar as f32 * bar_dur * sample_rate) as usize;
            let len = (bar_dur * 1.05 * sample_rate) as usize;
            // Three detuned voices a fifth and an octave apart.
            let voices = [
                (root, 1.0, 0.0),
                (root * 1.4983, 0.55, 0.31),
                (root * 2.0, 0.40, 0.62),
            ];
            let mut filt = LowPass::new(0.012);
            let mut phases = [0.0f32; 3];
            for i in 0..len {
                let idx = start + i;
                if idx >= n { break; }
                let t = i as f32 / sample_rate;
                let env = env_ahr(t, bar_dur * 0.28, bar_dur * 0.4, bar_dur * 0.4);
                let mut v = 0.0;
                for (vi, (f, amp, detune)) in voices.iter().enumerate() {
                    phases[vi] += (f + detune) / sample_rate;
                    v += saw(phases[vi]) * amp;
                }
                out[idx] += filt.run(v) * env * 0.10 * p.pad_level;
            }
        }
    }

    // ------------------------------------------------------------------ lead
    if p.lead {
        let bar_dur = step_dur * STEPS as f32;
        let mut phase = 0.0f32;
        let mut hp = HighPass::new(0.02);
        for bar in 0..p.bars {
            for step in (0..STEPS).step_by(2) {
                if (bar * STEPS + step) % 6 != 0 { continue; }
                let f = semitone(p.root * 8.0, p.pad[bar % 4] + if step % 8 == 0 { 0 } else { 7 });
                let start = ((bar as f32 * bar_dur + step as f32 * step_dur) * sample_rate) as usize;
                let len = (step_dur * 1.4 * sample_rate) as usize;
                for i in 0..len {
                    let idx = start + i;
                    if idx >= n { break; }
                    let t = i as f32 / sample_rate;
                    phase += f / sample_rate;
                    let env = env_pluck(t, 0.002, step_dur * 0.5);
                    out[idx] += hp.run(triangle(phase)) * env * 0.14;
                }
            }
        }
    }

    // Glue: soft saturation and a gentle tail so the loop point is soft.
    for s in out.iter_mut() { *s = soft_clip(*s * 0.9); }
    let fade = (sample_rate * 0.02) as usize;
    for i in 0..fade.min(n / 4) {
        let f = i as f32 / fade as f32;
        out[i] *= f;
        out[n - 1 - i] *= f;
    }
    finish(out, 0.62)
}

fn render_kick(sample_rate: f32, drive: f32) -> Clip {
    let n = (0.30 * sample_rate) as usize;
    let mut out = vec![0.0f32; n];
    let mut phase = 0.0f32;
    let mut click = HighPass::new(0.20);
    let mut rng = Rng::seeded(7);
    for i in 0..n {
        let t = i as f32 / sample_rate;
        // Pitch sweeps down hard, which is what makes a kick a kick.
        let f = 48.0 * (1.0 + 6.0 * (-t * 42.0).exp());
        phase += f / sample_rate;
        let body = sine(phase) * env_pluck(t, 0.001, 0.085);
        let attack = click.run(rng.signed()) * env_pluck(t, 0.0002, 0.004) * 0.5 * drive;
        out[i] = soft_clip(body * 1.3 + attack);
    }
    finish(out, 0.9)
}

fn render_snare(sample_rate: f32, drive: f32, rng: &mut Rng) -> Clip {
    let n = (0.22 * sample_rate) as usize;
    let mut out = vec![0.0f32; n];
    let mut bp = BandPass::new(1900.0, 1.1, sample_rate);
    let mut phase = 0.0f32;
    for i in 0..n {
        let t = i as f32 / sample_rate;
        phase += 185.0 / sample_rate;
        let tone = sine(phase) * env_pluck(t, 0.0008, 0.045) * 0.5;
        let noise = bp.run(rng.signed()) * env_pluck(t, 0.0005, 0.075) * (0.8 + drive * 0.4);
        out[i] = soft_clip(tone + noise);
    }
    finish(out, 0.8)
}

fn render_hat(sample_rate: f32, rng: &mut Rng) -> Clip {
    let n = (0.07 * sample_rate) as usize;
    let mut out = vec![0.0f32; n];
    let mut hp = HighPass::new(0.24);
    for i in 0..n {
        let t = i as f32 / sample_rate;
        out[i] = hp.run(rng.signed()) * env_pluck(t, 0.0003, 0.016);
    }
    finish(out, 0.55)
}
