//! Frame pacing and rate limiting.

use std::time::{Duration, Instant};

/// Tracks delta time with spike clamping plus a smoothed FPS readout for the HUD.
pub struct FrameClock {
    last: Instant,
    start: Instant,
    pub dt: f32,
    pub raw_dt: f32,
    pub elapsed: f64,
    pub frame: u64,
    fps_accum: f32,
    fps_frames: u32,
    pub fps: f32,
    /// 1% low, recomputed every second; surfaced by the perf overlay so
    /// optimisation work targets frame pacing rather than average FPS.
    pub fps_low: f32,
    worst_dt: f32,
}

impl Default for FrameClock {
    fn default() -> Self { Self::new() }
}

impl FrameClock {
    pub fn new() -> FrameClock {
        let now = Instant::now();
        FrameClock {
            last: now,
            start: now,
            dt: 1.0 / 60.0,
            raw_dt: 1.0 / 60.0,
            elapsed: 0.0,
            frame: 0,
            fps_accum: 0.0,
            fps_frames: 0,
            fps: 0.0,
            fps_low: 0.0,
            worst_dt: 0.0,
        }
    }

    pub fn tick(&mut self) {
        let now = Instant::now();
        let raw = now.duration_since(self.last).as_secs_f32();
        self.last = now;
        self.raw_dt = raw;
        // A hitch (alt-tab, map load) must not teleport the simulation.
        self.dt = raw.clamp(0.000_1, 0.1);
        self.elapsed = now.duration_since(self.start).as_secs_f64();
        self.frame += 1;

        self.fps_accum += raw;
        self.fps_frames += 1;
        self.worst_dt = self.worst_dt.max(raw);
        if self.fps_accum >= 0.5 {
            self.fps = self.fps_frames as f32 / self.fps_accum;
            self.fps_low = if self.worst_dt > 0.0 { 1.0 / self.worst_dt } else { 0.0 };
            self.fps_accum = 0.0;
            self.fps_frames = 0;
            self.worst_dt = 0.0;
        }
    }

    pub fn now_secs(&self) -> f64 { self.elapsed }
}

/// Sleeps to hold a target frame rate. Sleeps slightly short and spins the
/// remainder, which keeps frame pacing tight without burning a whole core.
pub struct RateLimiter {
    next: Instant,
    period: Duration,
    enabled: bool,
}

impl RateLimiter {
    pub fn new(hz: f32) -> RateLimiter {
        RateLimiter {
            next: Instant::now(),
            period: Duration::from_secs_f32(1.0 / hz.max(1.0)),
            enabled: hz > 0.0,
        }
    }

    pub fn set_hz(&mut self, hz: f32) {
        self.enabled = hz > 0.0;
        if self.enabled {
            self.period = Duration::from_secs_f32(1.0 / hz.max(1.0));
        }
        self.next = Instant::now();
    }

    pub fn wait(&mut self) {
        if !self.enabled { return; }
        let now = Instant::now();
        if self.next < now {
            // Fell behind; resync rather than accumulating debt.
            self.next = now + self.period;
            return;
        }
        let target = self.next;
        let slack = Duration::from_micros(900);
        if let Some(coarse) = target.checked_sub(slack) {
            let now = Instant::now();
            if coarse > now { std::thread::sleep(coarse - now); }
        }
        while Instant::now() < target { std::hint::spin_loop(); }
        self.next = target + self.period;
    }
}

/// Monotonic milliseconds since process start; used for network timestamps.
pub fn millis_since_start() -> u64 {
    use std::sync::OnceLock;
    static START: OnceLock<Instant> = OnceLock::new();
    let s = START.get_or_init(Instant::now);
    s.elapsed().as_millis() as u64
}
