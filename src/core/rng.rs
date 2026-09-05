//! Small, fast, deterministic PRNG. xoshiro128** — no dependency, no syscalls,
//! and cheap enough to call thousands of times per tick for spread/AI jitter.

#[derive(Clone)]
pub struct Rng {
    s: [u32; 4],
}

impl Default for Rng {
    fn default() -> Self { Rng::seeded(0x9E3779B9) }
}

impl Rng {
    pub fn seeded(seed: u32) -> Rng {
        // SplitMix-style expansion so nearby seeds diverge immediately.
        let mut z = seed as u64 ^ 0x9E3779B97F4A7C15;
        let mut next = || {
            z = z.wrapping_add(0x9E3779B97F4A7C15);
            let mut x = z;
            x = (x ^ (x >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
            x = (x ^ (x >> 27)).wrapping_mul(0x94D049BB133111EB);
            (x ^ (x >> 31)) as u32
        };
        let s = [next() | 1, next(), next(), next()];
        Rng { s }
    }

    /// Seed from wall-clock; used only for things that should differ per run
    /// (menu ambience, bot name shuffling), never for networked simulation.
    pub fn from_clock() -> Rng {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() ^ (d.as_secs() as u32))
            .unwrap_or(12345);
        Rng::seeded(n)
    }

    #[inline(always)]
    pub fn next_u32(&mut self) -> u32 {
        let result = self.s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = self.s[1] << 9;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(11);
        result
    }

    /// Uniform in [0, 1).
    #[inline(always)]
    pub fn f32(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 * (1.0 / 16777216.0)
    }

    /// Uniform in [lo, hi).
    #[inline(always)]
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 { lo + (hi - lo) * self.f32() }

    /// Uniform in [-1, 1).
    #[inline(always)]
    pub fn signed(&mut self) -> f32 { self.f32() * 2.0 - 1.0 }

    /// Uniform integer in [0, n).
    #[inline(always)]
    pub fn below(&mut self, n: u32) -> u32 {
        if n == 0 { return 0; }
        // Multiply-shift: unbiased enough for gameplay, one multiply.
        ((self.next_u32() as u64 * n as u64) >> 32) as u32
    }

    #[inline(always)]
    pub fn chance(&mut self, p: f32) -> bool { self.f32() < p }

    /// Approximately gaussian via the sum of three uniforms; used for recoil
    /// and bot aim error where a bell curve feels better than uniform.
    #[inline(always)]
    pub fn gaussian(&mut self) -> f32 {
        (self.signed() + self.signed() + self.signed()) * 0.5774
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        if items.is_empty() { None } else { Some(&items[self.below(items.len() as u32) as usize]) }
    }

    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.below(i as u32 + 1) as usize;
            items.swap(i, j);
        }
    }
}
