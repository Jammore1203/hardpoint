//! Byte-level serialisation for the network protocol.
//!
//! Every read is bounds checked and returns an `Option`. Nothing in the
//! protocol may panic on malformed input: a hostile or simply broken peer must
//! only ever cause a packet to be discarded.

pub struct Writer<'a> {
    buf: &'a mut [u8],
    pos: usize,
    overflow: bool,
}

impl<'a> Writer<'a> {
    pub fn new(buf: &'a mut [u8]) -> Writer<'a> {
        Writer { buf, pos: 0, overflow: false }
    }

    #[inline]
    pub fn len(&self) -> usize { self.pos }
    #[inline]
    pub fn is_empty(&self) -> bool { self.pos == 0 }
    #[inline]
    pub fn remaining(&self) -> usize { self.buf.len().saturating_sub(self.pos) }
    /// True if any write did not fit. The caller must check this before
    /// sending: an overflowed buffer holds a truncated, invalid message.
    #[inline]
    pub fn overflowed(&self) -> bool { self.overflow }

    #[inline]
    fn put(&mut self, bytes: &[u8]) {
        if self.pos + bytes.len() > self.buf.len() {
            self.overflow = true;
            return;
        }
        self.buf[self.pos..self.pos + bytes.len()].copy_from_slice(bytes);
        self.pos += bytes.len();
    }

    #[inline] pub fn u8(&mut self, v: u8) { self.put(&[v]); }
    #[inline] pub fn i8(&mut self, v: i8) { self.put(&[v as u8]); }
    #[inline] pub fn u16(&mut self, v: u16) { self.put(&v.to_le_bytes()); }
    #[inline] pub fn i16(&mut self, v: i16) { self.put(&v.to_le_bytes()); }
    #[inline] pub fn u32(&mut self, v: u32) { self.put(&v.to_le_bytes()); }
    #[inline] pub fn u64(&mut self, v: u64) { self.put(&v.to_le_bytes()); }
    #[inline] pub fn f32(&mut self, v: f32) { self.put(&v.to_le_bytes()); }
    #[inline] pub fn bool(&mut self, v: bool) { self.u8(v as u8); }
    #[inline] pub fn bytes(&mut self, b: &[u8]) { self.put(b); }

    /// Length-prefixed UTF-8, capped so a peer cannot make us allocate.
    pub fn string(&mut self, s: &str, max: usize) {
        let bytes = s.as_bytes();
        let n = bytes.len().min(max).min(255);
        // Never split a multi-byte character.
        let mut n = n;
        while n > 0 && !s.is_char_boundary(n) { n -= 1; }
        self.u8(n as u8);
        self.put(&bytes[..n]);
    }

    /// Reserves a byte and returns its index, for back-patching a count.
    pub fn reserve_u8(&mut self) -> usize {
        let at = self.pos;
        self.u8(0);
        at
    }

    pub fn patch_u8(&mut self, at: usize, v: u8) {
        if at < self.buf.len() { self.buf[at] = v; }
    }

    pub fn reserve_u16(&mut self) -> usize {
        let at = self.pos;
        self.u16(0);
        at
    }

    pub fn patch_u16(&mut self, at: usize, v: u16) {
        if at + 2 <= self.buf.len() {
            self.buf[at..at + 2].copy_from_slice(&v.to_le_bytes());
        }
    }

    pub fn finish(self) -> usize { self.pos }
}

pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Reader<'a> { Reader { buf, pos: 0 } }

    #[inline] pub fn remaining(&self) -> usize { self.buf.len().saturating_sub(self.pos) }
    #[inline] pub fn is_empty(&self) -> bool { self.remaining() == 0 }
    #[inline] pub fn pos(&self) -> usize { self.pos }

    #[inline]
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.pos + n > self.buf.len() { return None; }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Some(s)
    }

    #[inline] pub fn u8(&mut self) -> Option<u8> { self.take(1).map(|b| b[0]) }
    #[inline] pub fn i8(&mut self) -> Option<i8> { self.u8().map(|b| b as i8) }
    #[inline]
    pub fn u16(&mut self) -> Option<u16> { self.take(2).map(|b| u16::from_le_bytes([b[0], b[1]])) }
    #[inline]
    pub fn i16(&mut self) -> Option<i16> { self.u16().map(|v| v as i16) }
    #[inline]
    pub fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    #[inline]
    pub fn u64(&mut self) -> Option<u64> {
        self.take(8).map(|b| u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
    }
    #[inline]
    pub fn f32(&mut self) -> Option<f32> {
        let v = self.take(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))?;
        // A NaN or infinity arriving from the network would poison the
        // simulation, so it is rejected at the door.
        if v.is_finite() { Some(v) } else { None }
    }
    #[inline] pub fn bool(&mut self) -> Option<bool> { self.u8().map(|v| v != 0) }

    pub fn bytes(&mut self, n: usize) -> Option<&'a [u8]> { self.take(n) }

    /// Reads a length-prefixed string, rejecting invalid UTF-8.
    pub fn string(&mut self, max: usize) -> Option<String> {
        let n = self.u8()? as usize;
        if n > max { return None; }
        let b = self.take(n)?;
        std::str::from_utf8(b).ok().map(|s| s.to_string())
    }

    /// Reads a string, replacing anything invalid with a placeholder rather
    /// than failing the whole packet. Used for cosmetic fields.
    pub fn lossy_string(&mut self, max: usize) -> Option<String> {
        let n = self.u8()? as usize;
        if n > max { return None; }
        let b = self.take(n)?;
        Some(String::from_utf8_lossy(b).into_owned())
    }
}

// ------------------------------------------------------------ quantisation

/// World positions are quantised to 1/256 m over a +/-128 m range, which
/// comfortably covers every map and costs six bytes per player.
pub const POS_SCALE: f32 = 256.0;
pub const POS_LIMIT: f32 = 127.0;

#[inline]
pub fn quantize_pos(v: f32) -> i16 {
    (v.clamp(-POS_LIMIT, POS_LIMIT) * POS_SCALE) as i16
}

#[inline]
pub fn dequantize_pos(v: i16) -> f32 { v as f32 / POS_SCALE }

/// Velocities are quantised to 1/64 m/s over +/-512 m/s.
pub const VEL_SCALE: f32 = 64.0;

#[inline]
pub fn quantize_vel(v: f32) -> i16 { (v.clamp(-511.0, 511.0) * VEL_SCALE) as i16 }
#[inline]
pub fn dequantize_vel(v: i16) -> f32 { v as f32 / VEL_SCALE }

/// Yaw uses the full unsigned range, giving about 0.005 degrees of precision.
#[inline]
pub fn quantize_yaw(rad: f32) -> u16 {
    let t = rad.rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU;
    (t * 65535.0) as u16
}

#[inline]
pub fn dequantize_yaw(v: u16) -> f32 {
    v as f32 / 65535.0 * std::f32::consts::TAU
}

/// Pitch is limited to +/- 90 degrees so it can use the signed range fully.
#[inline]
pub fn quantize_pitch(rad: f32) -> i16 {
    let t = (rad / std::f32::consts::FRAC_PI_2).clamp(-1.0, 1.0);
    (t * 32767.0) as i16
}

#[inline]
pub fn dequantize_pitch(v: i16) -> f32 {
    v as f32 / 32767.0 * std::f32::consts::FRAC_PI_2
}

/// A unit vector packed into three bytes. Used for shot directions in events,
/// where a fraction of a degree either way is invisible.
#[inline]
pub fn quantize_dir(d: glam::Vec3) -> [i8; 3] {
    let n = d.normalize_or_zero();
    [(n.x * 127.0) as i8, (n.y * 127.0) as i8, (n.z * 127.0) as i8]
}

#[inline]
pub fn dequantize_dir(v: [i8; 3]) -> glam::Vec3 {
    glam::Vec3::new(v[0] as f32, v[1] as f32, v[2] as f32).normalize_or_zero()
}

/// Player collision height, 0.5 m to 2.0 m in 256 steps.
#[inline]
pub fn quantize_height(h: f32) -> u8 {
    (((h - 0.5) / 1.5).clamp(0.0, 1.0) * 255.0) as u8
}

#[inline]
pub fn dequantize_height(v: u8) -> f32 { 0.5 + v as f32 / 255.0 * 1.5 }
