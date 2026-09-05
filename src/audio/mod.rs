//! Audio: the sound bank, the mixer, and the device.
//!
//! Sounds are generated once into a bank of shared clips. Playing one sends a
//! small command to the audio thread, which owns the mixer and does nothing
//! but add samples together. Spatialisation is computed on the game thread at
//! the moment a sound starts, so the callback stays a tight loop with no
//! locks, no allocation and no chance of stalling the device.

pub mod music;
pub mod synth;

use crate::assets::materials::Surface;
use crate::core::Rng;
use crate::game::events::AnnounceLine;
use crate::game::weapons::{WeaponId, ALL_WEAPONS};
use crate::maps::{Ambience, MusicTrack};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use glam::Vec3;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use synth::Clip;

/// Mixer categories, so the player's volume sliders mean something.
pub const CAT_SFX: u8 = 0;
pub const CAT_MUSIC: u8 = 1;
pub const CAT_VOICE: u8 = 2;
pub const CAT_AMBIENT: u8 = 3;
const CATEGORIES: usize = 4;

/// Maximum simultaneous voices. Beyond this the quietest is replaced, which
/// during a firefight is inaudible and keeps the callback bounded.
const MAX_VOICES: usize = 48;

/// Distance at which a sound is at full volume.
const REFERENCE_DISTANCE: f32 = 6.0;
/// Beyond this a sound is not worth playing at all.
pub const MAX_AUDIBLE: f32 = 130.0;

enum Command {
    Play {
        clip: Arc<Clip>,
        gain_l: f32,
        gain_r: f32,
        rate: f32,
        lowpass: f32,
        category: u8,
        looping: bool,
        handle: u32,
    },
    SetCategory(u8, f32),
    Stop(u32),
    StopCategory(u8),
}

struct Voice {
    clip: Arc<Clip>,
    position: f32,
    rate: f32,
    gain_l: f32,
    gain_r: f32,
    lowpass: f32,
    lp_l: f32,
    lp_r: f32,
    category: u8,
    looping: bool,
    handle: u32,
    active: bool,
}

impl Voice {
    fn silent() -> Voice {
        Voice {
            clip: Arc::new(Vec::new()),
            position: 0.0,
            rate: 1.0,
            gain_l: 0.0,
            gain_r: 0.0,
            lowpass: 0.0,
            lp_l: 0.0,
            lp_r: 0.0,
            category: CAT_SFX,
            looping: false,
            handle: 0,
            active: false,
        }
    }
}

struct Mixer {
    voices: Vec<Voice>,
    category_gain: [f32; CATEGORIES],
    rx: Receiver<Command>,
    channels: usize,
}

impl Mixer {
    fn drain(&mut self) {
        while let Ok(cmd) = self.rx.try_recv() {
            match cmd {
                Command::Play { clip, gain_l, gain_r, rate, lowpass, category, looping, handle } => {
                    if clip.is_empty() { continue; }
                    let slot = self.pick_slot(gain_l + gain_r);
                    if let Some(v) = slot {
                        let voice = &mut self.voices[v];
                        voice.clip = clip;
                        voice.position = 0.0;
                        voice.rate = rate.clamp(0.25, 4.0);
                        voice.gain_l = gain_l;
                        voice.gain_r = gain_r;
                        voice.lowpass = lowpass.clamp(0.0, 0.98);
                        voice.lp_l = 0.0;
                        voice.lp_r = 0.0;
                        voice.category = category;
                        voice.looping = looping;
                        voice.handle = handle;
                        voice.active = true;
                    }
                }
                Command::SetCategory(c, v) => {
                    if (c as usize) < CATEGORIES { self.category_gain[c as usize] = v.clamp(0.0, 1.5); }
                }
                Command::Stop(handle) => {
                    for v in self.voices.iter_mut() {
                        if v.handle == handle { v.active = false; }
                    }
                }
                Command::StopCategory(c) => {
                    for v in self.voices.iter_mut() {
                        if v.category == c { v.active = false; }
                    }
                }
            }
        }
    }

    /// Finds a free voice, or steals the quietest one.
    fn pick_slot(&self, incoming: f32) -> Option<usize> {
        if let Some(i) = self.voices.iter().position(|v| !v.active) {
            return Some(i);
        }
        let mut worst = 0usize;
        let mut worst_gain = f32::MAX;
        for (i, v) in self.voices.iter().enumerate() {
            if v.looping { continue; }
            let g = v.gain_l + v.gain_r;
            if g < worst_gain { worst_gain = g; worst = i; }
        }
        if worst_gain < incoming { Some(worst) } else { None }
    }

    fn fill(&mut self, out: &mut [f32]) {
        self.drain();
        for s in out.iter_mut() { *s = 0.0; }
        let ch = self.channels.max(1);
        let frames = out.len() / ch;

        for v in self.voices.iter_mut() {
            if !v.active { continue; }
            let cat = self.category_gain[v.category as usize % CATEGORIES];
            if cat <= 0.0001 {
                if !v.looping { continue; }
            }
            let len = v.clip.len();
            if len == 0 { v.active = false; continue; }

            for f in 0..frames {
                let idx = v.position as usize;
                if idx + 1 >= len {
                    if v.looping {
                        v.position -= len as f32;
                        continue;
                    }
                    v.active = false;
                    break;
                }
                // Linear interpolation: cheap, and at these rates inaudible.
                let frac = v.position - idx as f32;
                let s = v.clip[idx] * (1.0 - frac) + v.clip[idx + 1] * frac;

                let mut l = s * v.gain_l * cat;
                let mut r = s * v.gain_r * cat;
                if v.lowpass > 0.001 {
                    // One-pole per channel, which is what distance sounds like.
                    v.lp_l += (l - v.lp_l) * (1.0 - v.lowpass);
                    v.lp_r += (r - v.lp_r) * (1.0 - v.lowpass);
                    l = v.lp_l;
                    r = v.lp_r;
                }
                let base = f * ch;
                out[base] += l;
                if ch > 1 { out[base + 1] += r; }
                for extra in 2..ch { out[base + extra] += (l + r) * 0.5; }

                v.position += v.rate;
            }
        }

        // A final limiter so a dozen simultaneous explosions cannot clip.
        for s in out.iter_mut() {
            *s = synth::soft_clip(*s * 0.85);
        }
    }
}

/// Every generated sound.
pub struct SoundBank {
    pub sample_rate: f32,
    pub shots: Vec<Arc<Clip>>,
    pub actions: Vec<Arc<Clip>>,
    pub dry_fire: Arc<Clip>,
    pub reload: [Arc<Clip>; 3],
    pub shell: Arc<Clip>,
    pub footsteps: Vec<Vec<Arc<Clip>>>,
    pub impacts: Vec<Vec<Arc<Clip>>>,
    pub explosion: Arc<Clip>,
    pub explosion_small: Arc<Clip>,
    pub flash: Arc<Clip>,
    pub bounce: Arc<Clip>,
    pub hitmarker: Arc<Clip>,
    pub hitmarker_kill: Arc<Clip>,
    pub headshot: Arc<Clip>,
    pub melee_swing: Arc<Clip>,
    pub melee_hit: Arc<Clip>,
    pub pickup: Arc<Clip>,
    pub spawn: Arc<Clip>,
    pub death: Arc<Clip>,
    pub land: Arc<Clip>,
    pub whoosh: Arc<Clip>,
    pub tick: Arc<Clip>,
    pub ui_move: Arc<Clip>,
    pub ui_select: Arc<Clip>,
    pub ui_back: Arc<Clip>,
    pub ui_error: Arc<Clip>,
    pub radio: Vec<Arc<Clip>>,
}

impl SoundBank {
    /// Generates the whole bank. Takes a few hundred milliseconds, which is
    /// why it runs on a worker thread during the splash.
    pub fn generate(sample_rate: f32) -> SoundBank {
        let sr = sample_rate;
        let mut rng = Rng::seeded(0xA0D_10);

        let shots: Vec<Arc<Clip>> = ALL_WEAPONS
            .iter()
            .map(|w| {
                let d = w.def();
                Arc::new(synth::gunshot(sr, d.sound_pitch, d.sound_bright, d.sound_body, 100 + *w as u32))
            })
            .collect();

        let actions: Vec<Arc<Clip>> = ALL_WEAPONS
            .iter()
            .map(|w| Arc::new(synth::action_click(sr, w.def().sound_pitch, 400 + *w as u32)))
            .collect();

        let mut footsteps = Vec::with_capacity(Surface::COUNT);
        let mut impacts = Vec::with_capacity(Surface::COUNT);
        for s in [
            Surface::Concrete, Surface::Metal, Surface::Wood, Surface::Dirt, Surface::Sand,
            Surface::Gravel, Surface::Grass, Surface::Snow, Surface::Glass, Surface::Soft, Surface::Water,
        ] {
            // Three variants each, so repeated steps do not machine-gun.
            footsteps.push((0..3).map(|i| Arc::new(synth::footstep(sr, s, 900 + s.index() as u32 * 7 + i))).collect());
            impacts.push((0..3).map(|i| Arc::new(synth::impact(sr, s, 1300 + s.index() as u32 * 11 + i))).collect());
        }

        let radio = ANNOUNCE_MOTIFS
            .iter()
            .enumerate()
            .map(|(i, notes)| Arc::new(synth::radio_cue(sr, notes, 5000 + i as u32 * 31)))
            .collect();

        SoundBank {
            sample_rate: sr,
            shots,
            actions,
            dry_fire: Arc::new(synth::action_click(sr, 1.6, 77)),
            reload: [
                Arc::new(synth::reload_part(sr, 0, 201)),
                Arc::new(synth::reload_part(sr, 1, 202)),
                Arc::new(synth::reload_part(sr, 2, 203)),
            ],
            shell: Arc::new(synth::bounce(sr, 311)),
            footsteps,
            impacts,
            explosion: Arc::new(synth::explosion(sr, 1.0, 601)),
            explosion_small: Arc::new(synth::explosion(sr, 0.55, 602)),
            flash: Arc::new(synth::flash_ring(sr, 603)),
            bounce: Arc::new(synth::bounce(sr, 604)),
            hitmarker: Arc::new(synth::blip(sr, 1750.0, true, 0.055)),
            hitmarker_kill: Arc::new(synth::blip(sr, 1300.0, false, 0.16)),
            headshot: Arc::new(synth::blip(sr, 2400.0, true, 0.10)),
            melee_swing: Arc::new(synth::footstep(sr, Surface::Soft, 707)),
            melee_hit: Arc::new(synth::impact(sr, Surface::Soft, 708)),
            pickup: Arc::new(synth::blip(sr, 900.0, true, 0.12)),
            spawn: Arc::new(synth::blip(sr, 420.0, true, 0.30)),
            death: Arc::new(synth::blip(sr, 260.0, false, 0.55)),
            land: Arc::new(synth::footstep(sr, Surface::Concrete, 811)),
            whoosh: Arc::new({
                // A thrown object passing the ear.
                let n = (0.35 * sr) as usize;
                let mut out = vec![0.0f32; n];
                let mut lp = synth::LowPass::new(0.05);
                for i in 0..n {
                    let t = i as f32 / sr;
                    out[i] = lp.run(rng.signed()) * synth::env_ahr(t, 0.08, 0.05, 0.20);
                }
                synth::finish(out, 0.30)
            }),
            tick: Arc::new(synth::blip(sr, 2100.0, false, 0.035)),
            ui_move: Arc::new(synth::blip(sr, 1150.0, true, 0.045)),
            ui_select: Arc::new(synth::blip(sr, 1500.0, true, 0.10)),
            ui_back: Arc::new(synth::blip(sr, 700.0, false, 0.09)),
            ui_error: Arc::new(synth::blip(sr, 320.0, false, 0.18)),
            radio,
        }
    }

    pub fn shot(&self, w: WeaponId) -> Arc<Clip> {
        self.shots.get(w.index()).cloned().unwrap_or_else(|| self.dry_fire.clone())
    }
    pub fn action(&self, w: WeaponId) -> Arc<Clip> {
        self.actions.get(w.index()).cloned().unwrap_or_else(|| self.dry_fire.clone())
    }
    pub fn footstep(&self, s: Surface, variant: usize) -> Arc<Clip> {
        let list = &self.footsteps[s.index().min(self.footsteps.len() - 1)];
        list[variant % list.len()].clone()
    }
    pub fn impact(&self, s: Surface, variant: usize) -> Arc<Clip> {
        let list = &self.impacts[s.index().min(self.impacts.len() - 1)];
        list[variant % list.len()].clone()
    }
    pub fn announce(&self, line: AnnounceLine) -> Arc<Clip> {
        self.radio.get(line as usize).cloned().unwrap_or_else(|| self.tick.clone())
    }

    pub fn memory_bytes(&self) -> usize {
        let mut n = 0;
        for c in &self.shots { n += c.len() * 4; }
        for c in &self.actions { n += c.len() * 4; }
        for l in &self.footsteps { for c in l { n += c.len() * 4; } }
        for l in &self.impacts { for c in l { n += c.len() * 4; } }
        for c in &self.radio { n += c.len() * 4; }
        n + self.explosion.len() * 4 + self.flash.len() * 4
    }
}

/// Two-note motifs, one per announcer line. Distinct intervals make each
/// message recognisable without a single spoken word.
const ANNOUNCE_MOTIFS: [&[(f32, f32)]; 24] = [
    &[(392.0, 0.10), (523.0, 0.16)],                    // match starting
    &[(523.0, 0.08), (659.0, 0.08), (784.0, 0.18)],     // fight
    &[(587.0, 0.12), (587.0, 0.12)],                    // one minute
    &[(659.0, 0.09), (659.0, 0.09), (659.0, 0.14)],     // thirty seconds
    &[(440.0, 0.14), (554.0, 0.14), (440.0, 0.18)],     // overtime
    &[(523.0, 0.12), (659.0, 0.12), (784.0, 0.24)],     // victory
    &[(392.0, 0.14), (330.0, 0.14), (262.0, 0.26)],     // defeat
    &[(440.0, 0.16), (440.0, 0.20)],                    // draw
    &[(494.0, 0.10), (392.0, 0.16)],                    // losing lead
    &[(392.0, 0.10), (494.0, 0.16)],                    // taking lead
    &[(523.0, 0.09), (698.0, 0.15)],                    // point captured
    &[(523.0, 0.09), (392.0, 0.15)],                    // point lost
    &[(587.0, 0.07), (587.0, 0.07), (494.0, 0.14)],     // point contested
    &[(330.0, 0.10), (330.0, 0.10), (392.0, 0.20)],     // bomb planted
    &[(392.0, 0.10), (523.0, 0.10), (392.0, 0.18)],     // bomb defused
    &[(294.0, 0.14), (247.0, 0.20)],                    // bomb dropped
    &[(659.0, 0.10), (523.0, 0.10), (440.0, 0.22)],     // last man standing
    &[(494.0, 0.09), (494.0, 0.13)],                    // enemies remaining
    &[(523.0, 0.07), (659.0, 0.07), (784.0, 0.12)],     // five confirmed
    &[(523.0, 0.06), (659.0, 0.06), (784.0, 0.06), (1046.0, 0.16)], // ten confirmed
    &[(1046.0, 0.06), (784.0, 0.10)],                   // headshot
    &[(440.0, 0.08), (587.0, 0.08), (440.0, 0.14)],     // revenge
    &[(784.0, 0.08), (1046.0, 0.14)],                   // first blood
    &[(523.0, 0.08), (659.0, 0.08), (880.0, 0.18)],     // promotion
];

/// Where the listener is, used to place sounds.
#[derive(Copy, Clone, Debug)]
pub struct Listener {
    pub position: Vec3,
    pub forward: Vec3,
    pub right: Vec3,
}

impl Default for Listener {
    fn default() -> Self {
        Listener { position: Vec3::ZERO, forward: Vec3::NEG_Z, right: Vec3::X }
    }
}

pub struct AudioEngine {
    _stream: Option<cpal::Stream>,
    tx: Option<Sender<Command>>,
    pub bank: Option<Arc<SoundBank>>,
    pub sample_rate: f32,
    pub listener: Listener,
    next_handle: u32,
    music_handle: Option<u32>,
    ambient_handle: Option<u32>,
    current_track: Option<MusicTrack>,
    current_ambience: Option<Ambience>,
    music_cache: Vec<(MusicTrack, Arc<Clip>)>,
    ambience_cache: Vec<(Ambience, Arc<Clip>)>,
    rng: Rng,
    /// Set when the device could not be opened; the game still runs.
    pub available: bool,
    pub device_name: String,
}

impl AudioEngine {
    /// Opens the default output device. Failure is not fatal: the game plays
    /// silently rather than refusing to start.
    pub fn new() -> AudioEngine {
        let mut engine = AudioEngine {
            _stream: None,
            tx: None,
            bank: None,
            sample_rate: 48000.0,
            listener: Listener::default(),
            next_handle: 1,
            music_handle: None,
            ambient_handle: None,
            current_track: None,
            current_ambience: None,
            music_cache: Vec::new(),
            ambience_cache: Vec::new(),
            rng: Rng::from_clock(),
            available: false,
            device_name: "NONE".to_string(),
        };

        let host = cpal::default_host();
        let Some(device) = host.default_output_device() else {
            eprintln!("[audio] no output device; running silent");
            return engine;
        };
        engine.device_name = device.name().unwrap_or_else(|_| "OUTPUT".into());

        let config = match device.default_output_config() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[audio] no usable output configuration ({e}); running silent");
                return engine;
            }
        };
        let sample_rate = config.sample_rate().0 as f32;
        let channels = config.channels() as usize;
        engine.sample_rate = sample_rate;

        let (tx, rx) = std::sync::mpsc::channel();
        let mut mixer = Mixer {
            voices: (0..MAX_VOICES).map(|_| Voice::silent()).collect(),
            category_gain: [1.0, 0.6, 1.0, 0.8],
            rx,
            channels,
        };

        let err_fn = |e| eprintln!("[audio] stream error: {e}");
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_output_stream(
                &config.into(),
                move |data: &mut [f32], _| mixer.fill(data),
                err_fn,
                None,
            ),
            cpal::SampleFormat::I16 => {
                let mut scratch: Vec<f32> = Vec::new();
                device.build_output_stream(
                    &config.into(),
                    move |data: &mut [i16], _| {
                        scratch.resize(data.len(), 0.0);
                        mixer.fill(&mut scratch);
                        for (o, s) in data.iter_mut().zip(scratch.iter()) {
                            *o = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
                        }
                    },
                    err_fn,
                    None,
                )
            }
            cpal::SampleFormat::U16 => {
                let mut scratch: Vec<f32> = Vec::new();
                device.build_output_stream(
                    &config.into(),
                    move |data: &mut [u16], _| {
                        scratch.resize(data.len(), 0.0);
                        mixer.fill(&mut scratch);
                        for (o, s) in data.iter_mut().zip(scratch.iter()) {
                            *o = ((s.clamp(-1.0, 1.0) * 0.5 + 0.5) * 65535.0) as u16;
                        }
                    },
                    err_fn,
                    None,
                )
            }
            other => {
                eprintln!("[audio] unsupported sample format {other:?}; running silent");
                return engine;
            }
        };

        match stream {
            Ok(s) => {
                if let Err(e) = s.play() {
                    eprintln!("[audio] could not start the stream ({e}); running silent");
                    return engine;
                }
                engine._stream = Some(s);
                engine.tx = Some(tx);
                engine.available = true;
            }
            Err(e) => eprintln!("[audio] could not open the stream ({e}); running silent"),
        }
        engine
    }

    pub fn set_bank(&mut self, bank: Arc<SoundBank>) { self.bank = Some(bank); }
    pub fn ready(&self) -> bool { self.available && self.bank.is_some() }

    fn send(&self, cmd: Command) {
        if let Some(tx) = &self.tx { let _ = tx.send(cmd); }
    }

    fn handle(&mut self) -> u32 {
        self.next_handle = self.next_handle.wrapping_add(1).max(1);
        self.next_handle
    }

    pub fn set_volumes(&self, master: f32, sfx: f32, music: f32, voice: f32) {
        self.send(Command::SetCategory(CAT_SFX, master * sfx));
        self.send(Command::SetCategory(CAT_MUSIC, master * music));
        self.send(Command::SetCategory(CAT_VOICE, master * voice));
        self.send(Command::SetCategory(CAT_AMBIENT, master * sfx * 0.75));
    }

    /// Plays a sound without any spatialisation: interface, music stings.
    pub fn play_ui(&mut self, clip: Arc<Clip>, gain: f32, rate: f32) {
        let handle = self.handle();
        self.send(Command::Play {
            clip, gain_l: gain, gain_r: gain, rate, lowpass: 0.0,
            category: CAT_SFX, looping: false, handle,
        });
    }

    pub fn play_voice(&mut self, clip: Arc<Clip>, gain: f32) {
        let handle = self.handle();
        self.send(Command::Play {
            clip, gain_l: gain, gain_r: gain, rate: 1.0, lowpass: 0.0,
            category: CAT_VOICE, looping: false, handle,
        });
    }

    /// Plays a sound at a world position, relative to the listener.
    pub fn play_at(&mut self, clip: Arc<Clip>, pos: Vec3, gain: f32, rate: f32) {
        let to = pos - self.listener.position;
        let dist = to.length();
        if dist > MAX_AUDIBLE { return; }

        // Inverse-distance falloff with a soft knee near the listener.
        let atten = REFERENCE_DISTANCE / (REFERENCE_DISTANCE + dist.max(0.0));
        let atten = atten * atten.sqrt();
        let g = gain * atten;
        if g < 0.002 { return; }

        let dir = if dist > 0.001 { to / dist } else { self.listener.forward };
        let pan = dir.dot(self.listener.right).clamp(-1.0, 1.0);
        // Constant-power panning, narrowed for close sounds so they do not
        // jump between ears when the player turns.
        let width = (dist / 8.0).clamp(0.0, 1.0);
        let p = pan * width;
        let angle = (p * 0.5 + 0.5) * std::f32::consts::FRAC_PI_2;
        let gain_l = g * angle.cos();
        let gain_r = g * angle.sin();

        // Distance rolls off the highs, which is most of what sells distance.
        let lowpass = (dist / MAX_AUDIBLE).clamp(0.0, 1.0).powf(0.6) * 0.86;

        let handle = self.handle();
        self.send(Command::Play {
            clip, gain_l, gain_r, rate, lowpass,
            category: CAT_SFX, looping: false, handle,
        });
    }

    /// A slight random pitch on repeated sounds stops them sounding cloned.
    pub fn vary(&mut self, amount: f32) -> f32 {
        1.0 + self.rng.signed() * amount
    }

    pub fn random_variant(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { self.rng.below(n as u32) as usize }
    }

    // ---------------------------------------------------------------- music

    pub fn play_music(&mut self, track: MusicTrack) {
        if self.current_track == Some(track) { return; }
        if !self.available { self.current_track = Some(track); return; }
        self.stop_music();
        let clip = self.music_clip(track);
        let handle = self.handle();
        self.music_handle = Some(handle);
        self.current_track = Some(track);
        self.send(Command::Play {
            clip, gain_l: 1.0, gain_r: 1.0, rate: 1.0, lowpass: 0.0,
            category: CAT_MUSIC, looping: true, handle,
        });
    }

    pub fn stop_music(&mut self) {
        if let Some(h) = self.music_handle.take() { self.send(Command::Stop(h)); }
        self.current_track = None;
    }

    fn music_clip(&mut self, track: MusicTrack) -> Arc<Clip> {
        if let Some((_, c)) = self.music_cache.iter().find(|(t, _)| *t == track) {
            return c.clone();
        }
        let clip = Arc::new(music::render(track, self.sample_rate));
        // Keep only a couple of tracks resident; a loop is a few megabytes.
        if self.music_cache.len() >= 2 { self.music_cache.remove(0); }
        self.music_cache.push((track, clip.clone()));
        clip
    }

    // ------------------------------------------------------------- ambience

    pub fn play_ambience(&mut self, kind: Ambience) {
        if self.current_ambience == Some(kind) { return; }
        if !self.available { self.current_ambience = Some(kind); return; }
        self.stop_ambience();
        let clip = self.ambience_clip(kind);
        let handle = self.handle();
        self.ambient_handle = Some(handle);
        self.current_ambience = Some(kind);
        self.send(Command::Play {
            clip, gain_l: 1.0, gain_r: 1.0, rate: 1.0, lowpass: 0.0,
            category: CAT_AMBIENT, looping: true, handle,
        });
    }

    pub fn stop_ambience(&mut self) {
        if let Some(h) = self.ambient_handle.take() { self.send(Command::Stop(h)); }
        self.current_ambience = None;
    }

    fn ambience_clip(&mut self, kind: Ambience) -> Arc<Clip> {
        if let Some((_, c)) = self.ambience_cache.iter().find(|(k, _)| *k == kind) {
            return c.clone();
        }
        let index = match kind {
            Ambience::Wind => 0,
            Ambience::Industrial | Ambience::RailYard => 1,
            Ambience::Jungle => 2,
            Ambience::Interior => 3,
            Ambience::Coastal => 4,
            Ambience::Blizzard | Ambience::Urban => 5,
        };
        let clip = Arc::new(synth::ambience(self.sample_rate, index, 8.0, 3000 + index as u32));
        if self.ambience_cache.len() >= 2 { self.ambience_cache.remove(0); }
        self.ambience_cache.push((kind, clip.clone()));
        clip
    }

    pub fn stop_all_world_sound(&mut self) {
        self.send(Command::StopCategory(CAT_SFX));
        self.stop_ambience();
    }
}
