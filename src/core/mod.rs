//! Core utilities shared by every subsystem: RNG, timing, key/value config IO,
//! and small fixed-capacity containers used in hot loops to avoid allocation.

pub mod kv;
pub mod rng;
pub mod time;

pub use rng::Rng;
#[allow(unused_imports)]
pub use time::{FrameClock, RateLimiter};

/// A fixed-capacity ring buffer used for input/state history in the netcode.
/// Never allocates after construction.
#[derive(Clone)]
pub struct Ring<T, const N: usize> {
    items: [T; N],
    head: usize,
    len: usize,
}

impl<T: Copy + Default, const N: usize> Default for Ring<T, N> {
    fn default() -> Self {
        Self { items: [T::default(); N], head: 0, len: 0 }
    }
}

impl<T: Copy + Default, const N: usize> Ring<T, N> {
    pub fn new() -> Self { Self::default() }

    pub fn push(&mut self, v: T) {
        self.items[self.head] = v;
        self.head = (self.head + 1) % N;
        if self.len < N { self.len += 1; }
    }

    pub fn len(&self) -> usize { self.len }
    pub fn is_empty(&self) -> bool { self.len == 0 }
    pub fn capacity(&self) -> usize { N }

    /// `back(0)` is the most recently pushed item.
    pub fn back(&self, n: usize) -> Option<&T> {
        if n >= self.len { return None; }
        Some(&self.items[(self.head + N - 1 - n) % N])
    }

    pub fn back_mut(&mut self, n: usize) -> Option<&mut T> {
        if n >= self.len { return None; }
        Some(&mut self.items[(self.head + N - 1 - n) % N])
    }

    pub fn iter_newest(&self) -> impl Iterator<Item = &T> {
        (0..self.len).map(move |i| &self.items[(self.head + N - 1 - i) % N])
    }

    pub fn clear(&mut self) { self.head = 0; self.len = 0; }
}

/// Clamp helper that reads better than nested min/max at call sites.
#[inline(always)]
pub fn clampf(v: f32, lo: f32, hi: f32) -> f32 {
    if v < lo { lo } else if v > hi { hi } else { v }
}

#[inline(always)]
pub fn lerp(a: f32, b: f32, t: f32) -> f32 { a + (b - a) * t }

/// Frame-rate independent exponential approach. `rate` is the fraction of the
/// remaining distance covered per second.
#[inline(always)]
pub fn approach_exp(cur: f32, target: f32, rate: f32, dt: f32) -> f32 {
    target + (cur - target) * (-rate * dt).exp()
}

#[inline(always)]
pub fn smoothstep(t: f32) -> f32 {
    let t = clampf(t, 0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Shortest signed angular difference in radians, result in (-PI, PI].
#[inline(always)]
pub fn angle_delta(from: f32, to: f32) -> f32 {
    let mut d = (to - from) % std::f32::consts::TAU;
    if d > std::f32::consts::PI { d -= std::f32::consts::TAU; }
    if d < -std::f32::consts::PI { d += std::f32::consts::TAU; }
    d
}
