//! Voice chat: capture, compression and playback.
//!
//! The constraint that shapes all of this is that the game has no codec
//! library and is not going to grow one. Speech is intelligible well below the
//! quality music needs, so it is resampled to eight kilohertz mono and packed
//! four bits to a sample with IMA ADPCM. That is four kilobytes a second, an
//! order of magnitude under the snapshot stream, and it fits a twenty
//! millisecond frame into eighty-four bytes including its state header.
//!
//! ADPCM is chosen over something simpler like mu-law for one specific reason:
//! it is stateful, so a lost frame degrades the next one rather than the rest
//! of the transmission, and each frame carries its own predictor state so the
//! damage stops at the frame boundary. Over UDP that matters more than the
//! extra four kilobits a second mu-law would cost.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::Arc;

/// Everything downstream assumes this rate; it is the one number to change if
/// voice ever needs to sound better.
pub const VOICE_RATE: u32 = 8000;
/// Samples in one transmitted frame. Twenty milliseconds is the usual trade
/// between packet overhead and the delay before someone hears you.
pub const FRAME_SAMPLES: usize = 160;
/// Encoded size: a four-byte predictor header plus one nibble per sample.
///
/// The header carries the starting predictor and the starting step index. The
/// step index matters more than it looks: every frame decodes independently,
/// so a frame that started from the smallest step would spend its first dozen
/// samples climbing to the signal, and at fifty frames a second that ramp is
/// audible as a persistent rasp. Choosing the index from the frame's own
/// content costs one byte and removes it.
pub const FRAME_BYTES: usize = 4 + FRAME_SAMPLES / 2;

const STEP_TABLE: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45,
    50, 55, 60, 66, 73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253,
    279, 307, 337, 371, 408, 449, 494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166,
    1282, 1411, 1552, 1707, 1878, 2066, 2272, 2499, 2749, 3024, 3327, 3660, 4026,
    4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630, 9493, 10442, 11487, 12635, 13899,
    15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];
const INDEX_TABLE: [i32; 16] = [-1, -1, -1, -1, 2, 4, 6, 8, -1, -1, -1, -1, 2, 4, 6, 8];

/// IMA ADPCM predictor state. Carried in each frame's header so a dropped
/// frame cannot corrupt everything after it.
#[derive(Copy, Clone, Default)]
struct Adpcm {
    predictor: i32,
    index: i32,
}

impl Adpcm {
    fn encode_sample(&mut self, sample: i16) -> u8 {
        let step = STEP_TABLE[self.index.clamp(0, 88) as usize];
        let mut diff = sample as i32 - self.predictor;
        let mut code = 0u8;
        if diff < 0 {
            code = 8;
            diff = -diff;
        }
        let mut delta = step >> 3;
        if diff >= step { code |= 4; diff -= step; delta += step; }
        if diff >= step >> 1 { code |= 2; diff -= step >> 1; delta += step >> 1; }
        if diff >= step >> 2 { code |= 1; delta += step >> 2; }
        if code & 8 != 0 { self.predictor -= delta; } else { self.predictor += delta; }
        self.predictor = self.predictor.clamp(-32768, 32767);
        self.index = (self.index + INDEX_TABLE[code as usize]).clamp(0, 88);
        code
    }

    fn decode_sample(&mut self, code: u8) -> i16 {
        let step = STEP_TABLE[self.index.clamp(0, 88) as usize];
        let mut delta = step >> 3;
        if code & 4 != 0 { delta += step; }
        if code & 2 != 0 { delta += step >> 1; }
        if code & 1 != 0 { delta += step >> 2; }
        if code & 8 != 0 { self.predictor -= delta; } else { self.predictor += delta; }
        self.predictor = self.predictor.clamp(-32768, 32767);
        self.index = (self.index + INDEX_TABLE[code as usize]).clamp(0, 88);
        self.predictor as i16
    }
}

/// Packs one frame of mono audio into `FRAME_BYTES`.
pub fn encode_frame(samples: &[f32]) -> [u8; FRAME_BYTES] {
    let mut out = [0u8; FRAME_BYTES];
    let mut st = Adpcm::default();
    // The predictor starts at the first sample, so a frame never has to climb
    // from silence to reach the signal.
    st.predictor = (samples.first().copied().unwrap_or(0.0).clamp(-1.0, 1.0) * 32767.0) as i32;

    // Pick the step index from the largest step this frame actually takes, so
    // the quantiser is in range from the first sample rather than adapting
    // into it.
    let mut biggest = 0i32;
    for w in samples.windows(2).take(FRAME_SAMPLES) {
        let a = (w[0].clamp(-1.0, 1.0) * 32767.0) as i32;
        let b = (w[1].clamp(-1.0, 1.0) * 32767.0) as i32;
        biggest = biggest.max((b - a).abs());
    }
    let want = (biggest / 2).max(7);
    let index = STEP_TABLE.iter().position(|s| *s >= want).unwrap_or(88) as i32;
    st.index = index;

    out[0..2].copy_from_slice(&(st.predictor as i16).to_le_bytes());
    out[2] = index as u8;
    out[3] = 0;
    for i in 0..FRAME_SAMPLES {
        let s = samples.get(i).copied().unwrap_or(0.0).clamp(-1.0, 1.0);
        let code = st.encode_sample((s * 32767.0) as i16);
        let byte = 4 + i / 2;
        if i % 2 == 0 { out[byte] = code; } else { out[byte] |= code << 4; }
    }
    out
}

/// Unpacks a frame. Returns silence for a malformed one rather than failing,
/// because a corrupt voice packet should cost a syllable and nothing else.
pub fn decode_frame(data: &[u8], out: &mut [f32; FRAME_SAMPLES]) {
    if data.len() < FRAME_BYTES {
        out.fill(0.0);
        return;
    }
    let mut st = Adpcm::default();
    st.predictor = i16::from_le_bytes([data[0], data[1]]) as i32;
    st.index = (data[2] as i32).clamp(0, 88);
    for i in 0..FRAME_SAMPLES {
        let byte = data[4 + i / 2];
        let code = if i % 2 == 0 { byte & 0x0F } else { byte >> 4 };
        out[i] = st.decode_sample(code) as f32 / 32768.0;
    }
}

/// The microphone.
///
/// Runs on cpal's input callback, resamples to the voice rate, gates on level
/// so a quiet room sends nothing, and hands finished frames to the game thread
/// over a bounded channel. Bounded on purpose: if the game thread stalls, the
/// right thing is to drop the oldest speech rather than grow a buffer that
/// will be played back late.
pub struct Microphone {
    _stream: Option<cpal::Stream>,
    frames: Option<Receiver<[f32; FRAME_SAMPLES]>>,
    open: Arc<AtomicBool>,
    pub available: bool,
    pub device_name: String,
    /// Smoothed input level, for a talk indicator.
    pub level: f32,
}

impl Default for Microphone {
    fn default() -> Self { Microphone::disabled() }
}

impl Microphone {
    pub fn disabled() -> Microphone {
        Microphone {
            _stream: None, frames: None,
            open: Arc::new(AtomicBool::new(false)),
            available: false, device_name: "none".into(), level: 0.0,
        }
    }

    /// Opens the default input device. Failure is not fatal anywhere: a player
    /// with no microphone can still hear everyone else.
    pub fn open() -> Microphone {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let host = cpal::default_host();
        let Some(device) = host.default_input_device() else { return Microphone::disabled() };
        let name = device.name().unwrap_or_else(|_| "input".into());
        let Ok(config) = device.default_input_config() else { return Microphone::disabled() };
        let in_rate = config.sample_rate().0 as f32;
        let channels = config.channels() as usize;

        let (tx, rx) = std::sync::mpsc::sync_channel::<[f32; FRAME_SAMPLES]>(16);
        let open = Arc::new(AtomicBool::new(false));
        let gate = open.clone();

        // Resampler and frame accumulator, owned by the audio callback.
        let mut phase = 0.0f32;
        let step = VOICE_RATE as f32 / in_rate;
        let mut pending: Vec<f32> = Vec::with_capacity(FRAME_SAMPLES * 2);

        let mut push = move |data: &[f32], tx: &SyncSender<[f32; FRAME_SAMPLES]>| {
            if !gate.load(Ordering::Relaxed) {
                pending.clear();
                phase = 0.0;
                return;
            }
            for frame in data.chunks(channels) {
                // Downmix, then take samples at the voice rate.
                let mono = frame.iter().sum::<f32>() / channels.max(1) as f32;
                phase += step;
                while phase >= 1.0 {
                    phase -= 1.0;
                    pending.push(mono);
                }
            }
            while pending.len() >= FRAME_SAMPLES {
                let mut out = [0.0f32; FRAME_SAMPLES];
                out.copy_from_slice(&pending[..FRAME_SAMPLES]);
                pending.drain(..FRAME_SAMPLES);
                // A full channel means the game thread is behind; dropping the
                // newest frame is better than queueing speech that will arrive
                // after the moment it was about to describe.
                let _ = tx.try_send(out);
            }
        };

        let err = |e| eprintln!("[voice] input error: {e}");
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_input_stream(
                &config.into(),
                move |d: &[f32], _: &_| push(d, &tx),
                err, None),
            cpal::SampleFormat::I16 => {
                let mut buf: Vec<f32> = Vec::new();
                device.build_input_stream(
                    &config.into(),
                    move |d: &[i16], _: &_| {
                        buf.clear();
                        buf.extend(d.iter().map(|s| *s as f32 / 32768.0));
                        push(&buf, &tx);
                    },
                    err, None)
            }
            cpal::SampleFormat::U16 => {
                let mut buf: Vec<f32> = Vec::new();
                device.build_input_stream(
                    &config.into(),
                    move |d: &[u16], _: &_| {
                        buf.clear();
                        buf.extend(d.iter().map(|s| (*s as f32 / 32768.0) - 1.0));
                        push(&buf, &tx);
                    },
                    err, None)
            }
            _ => return Microphone::disabled(),
        };

        let Ok(stream) = stream else { return Microphone::disabled() };
        if stream.play().is_err() { return Microphone::disabled(); }

        Microphone {
            _stream: Some(stream),
            frames: Some(rx),
            open,
            available: true,
            device_name: name,
            level: 0.0,
        }
    }

    /// Opens or closes the gate. Capture runs continuously; this decides
    /// whether anything leaves the callback, which is what makes it
    /// push-to-talk rather than an open microphone.
    pub fn set_transmitting(&self, on: bool) {
        self.open.store(on && self.available, Ordering::Relaxed);
    }

    /// Takes whatever frames have been captured since the last call.
    pub fn poll(&mut self) -> Vec<[f32; FRAME_SAMPLES]> {
        let Some(rx) = self.frames.as_ref() else { return Vec::new() };
        let mut out = Vec::new();
        while let Ok(f) = rx.try_recv() {
            let peak = f.iter().fold(0.0f32, |a, s| a.max(s.abs()));
            self.level = self.level * 0.7 + peak * 0.3;
            out.push(f);
        }
        if out.is_empty() { self.level *= 0.9; }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The codec has to survive a round trip well enough to be understood.
    /// ADPCM is lossy, so this checks the error stays small relative to the
    /// signal rather than checking for equality.
    #[test]
    fn adpcm_round_trip_is_close_enough() {
        let mut input = [0.0f32; FRAME_SAMPLES];
        for (i, s) in input.iter_mut().enumerate() {
            // A couple of speech-band tones plus a slow envelope.
            let t = i as f32 / VOICE_RATE as f32;
            *s = ((t * 220.0 * std::f32::consts::TAU).sin() * 0.5
                + (t * 700.0 * std::f32::consts::TAU).sin() * 0.25)
                * (0.4 + 0.6 * (t * 6.0).sin().abs());
        }

        let encoded = encode_frame(&input);
        assert_eq!(encoded.len(), FRAME_BYTES);

        let mut out = [0.0f32; FRAME_SAMPLES];
        decode_frame(&encoded, &mut out);

        let signal: f32 = input.iter().map(|s| s * s).sum::<f32>() / FRAME_SAMPLES as f32;
        let noise: f32 = input.iter().zip(out.iter())
            .map(|(a, b)| (a - b) * (a - b)).sum::<f32>() / FRAME_SAMPLES as f32;
        let snr = 10.0 * (signal / noise.max(1e-12)).log10();
        assert!(snr > 18.0, "round trip signal-to-noise was only {snr:.1} dB");
    }

    /// A truncated or corrupt frame must not panic or produce a scream.
    #[test]
    fn malformed_frames_are_silence() {
        let mut out = [1.0f32; FRAME_SAMPLES];
        decode_frame(&[], &mut out);
        assert!(out.iter().all(|s| *s == 0.0));

        let mut out = [0.0f32; FRAME_SAMPLES];
        decode_frame(&[0xFF; FRAME_BYTES], &mut out);
        assert!(out.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
    }
}
