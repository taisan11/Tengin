//! Arbitrary-precision integer arithmetic backing `BigInt` values.
//!
//! The engine stores BigInt primitives as their decimal text (`Value::BigInt`
//! is an `Rc<str>`, optionally signed); this module converts to a binary-limb
//! representation (`Big`, little-endian `u32` limbs) for arithmetic and back.

use alloc::vec::Vec;

/// A signed arbitrary-precision integer: sign + magnitude limbs (base 2³²,
/// little-endian, no trailing zero limbs; an empty magnitude is zero).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Big {
    pub neg: bool,
    pub mag: Vec<u32>,
}

impl Big {
    pub fn zero() -> Self {
        Big { neg: false, mag: Vec::new() }
    }

    pub fn is_zero(&self) -> bool {
        self.mag.is_empty()
    }

    /// Normalize: drop trailing zero limbs and the sign of zero.
    fn normalize(mut self) -> Self {
        while self.mag.last() == Some(&0) {
            self.mag.pop();
        }
        if self.mag.is_empty() {
            self.neg = false;
        }
        self
    }

    pub fn from_sign_abs(neg: bool, mag: Vec<u32>) -> Self {
        Big { neg, mag }.normalize()
    }

    pub fn negated(&self) -> Self {
        if self.is_zero() {
            self.clone()
        } else {
            Big { neg: !self.neg, mag: self.mag.clone() }
        }
    }

    pub fn abs(&self) -> Self {
        Big { neg: false, mag: self.mag.clone() }
    }

    // --- parsing / formatting ---

    /// Parse a big integer from decimal digits with an optional leading sign.
    /// Returns `None` for anything that is not `[+-]?[0-9]+`.
    pub fn from_decimal(s: &str) -> Option<Self> {
        Self::from_radix_str(s, 10)
    }

    /// Parse from digits in the given radix (2, 8, 10 or 16), allowing an
    /// optional leading `+`/`-` (the caller applies the ECMAScript grammar
    /// restrictions separately).
    pub fn from_radix_str(s: &str, radix: u32) -> Option<Self> {
        let (neg, body) = match s.as_bytes().first() {
            Some(b'+') => (false, &s[1..]),
            Some(b'-') => (true, &s[1..]),
            _ => (false, s),
        };
        if body.is_empty() {
            return None;
        }
        let mut mag: Vec<u32> = Vec::new(); // accumulates in base 2^32
        let base = radix as u64;
        let mut chunk_mul: u64 = 1; // base^k, kept below 2^32
        let mut chunk: u64 = 0;
        for c in body.chars() {
            let d = match c.to_digit(radix) {
                Some(d) => d as u64,
                None => return None,
            };
            chunk = chunk * base + d;
            chunk_mul *= base;
            if chunk_mul * base >= (1u64 << 32) {
                // Fold the chunk into the accumulator: mag = mag * chunk_mul + chunk.
                mul_small_in_place(&mut mag, chunk_mul);
                add_small_in_place(&mut mag, chunk);
                chunk_mul = 1;
                chunk = 0;
            }
        }
        if chunk_mul > 1 {
            mul_small_in_place(&mut mag, chunk_mul);
            add_small_in_place(&mut mag, chunk);
        }
        Some(Big { neg, mag }.normalize())
    }

    /// Render in decimal (the canonical `Value::BigInt` text form).
    pub fn to_decimal_string(&self) -> alloc::string::String {
        self.to_string_radix(10)
    }

    /// Render in radix 2..=36 (lowercase digits), with a `-` sign when negative.
    pub fn to_string_radix(&self, radix: u32) -> alloc::string::String {
        use alloc::string::String;
        if self.is_zero() {
            return String::from("0");
        }
        // Extract chunks of `k` digits at a time via repeated small division.
        let (k, divisor) = max_digit_chunk(radix);
        let mut chunks: Vec<u32> = Vec::new();
        let mut mag = self.mag.clone();
        while !mag.is_empty() {
            let r = divmod_small(&mut mag, divisor);
            chunks.push(r);
        }
        let mut out = String::new();
        let last = chunks.len() - 1;
        for (i, c) in chunks.iter().enumerate().rev() {
            if i == last {
                out.push_str(&format_chunk(*c, radix, k, false));
            } else {
                out.push_str(&format_chunk(*c, radix, k, true));
            }
        }
        if self.neg {
            out.insert(0, '-');
        }
        out
    }

    // --- comparisons ---

    pub fn cmp(&self, other: &Big) -> core::cmp::Ordering {
        use core::cmp::Ordering;
        if self.neg != other.neg {
            return if self.neg { Ordering::Less } else { Ordering::Greater };
        }
        let c = cmp_mag(&self.mag, &other.mag);
        if self.neg {
            c.reverse()
        } else {
            c
        }
    }

    // --- arithmetic ---

    pub fn add(&self, other: &Big) -> Big {
        if self.neg == other.neg {
            let mag = add_mag(&self.mag, &other.mag);
            Big::from_sign_abs(self.neg, mag)
        } else {
            let (a, b) = (self, other);
            match cmp_mag(&a.mag, &b.mag) {
                core::cmp::Ordering::Equal => Big::zero(),
                core::cmp::Ordering::Greater => {
                    Big::from_sign_abs(a.neg, sub_mag(&a.mag, &b.mag))
                }
                core::cmp::Ordering::Less => Big::from_sign_abs(b.neg, sub_mag(&b.mag, &a.mag)),
            }
        }
    }

    pub fn sub(&self, other: &Big) -> Big {
        self.add(&other.negated())
    }

    pub fn mul(&self, other: &Big) -> Big {
        if self.is_zero() || other.is_zero() {
            return Big::zero();
        }
        let mut mag = vec![0u32; self.mag.len() + other.mag.len()];
        for (i, &a) in self.mag.iter().enumerate() {
            let mut carry: u64 = 0;
            for (j, &b) in other.mag.iter().enumerate() {
                let idx = i + j;
                let cur = mag[idx] as u64 + (a as u64) * (b as u64) + carry;
                mag[idx] = cur as u32;
                carry = cur >> 32;
            }
            let mut idx = i + other.mag.len();
            while carry > 0 {
                let cur = mag[idx] as u64 + carry;
                mag[idx] = cur as u32;
                carry = cur >> 32;
                idx += 1;
            }
        }
        Big::from_sign_abs(self.neg != other.neg, mag)
    }

    /// Truncated division and remainder (sign of the remainder follows the
    /// dividend, per ECMAScript `/` and `%`).
    pub fn divmod(&self, other: &Big) -> (Big, Big) {
        assert!(!other.is_zero(), "division by zero");
        if self.is_zero() {
            return (Big::zero(), Big::zero());
        }
        // Binary long division on magnitudes.
        let mut quot = vec![0u32; self.mag.len().max(other.mag.len()) + 1];
        let mut rem = Big { neg: false, mag: Vec::new() };
        let divisor = other.abs();
        let n = (self.mag.len() * 32) as u64;
        for bit in (0..n).rev() {
            // rem = rem << 1 | self_bit
            shl1_in_place(&mut rem.mag);
            if bit_at(&self.mag, bit) {
                if rem.mag.is_empty() {
                    rem.mag.push(1);
                } else {
                    rem.mag[0] |= 1;
                }
            }
            rem = Big { neg: false, mag: rem.mag }.normalize();
            if cmp_mag(&rem.mag, &divisor.mag) != core::cmp::Ordering::Less {
                rem = Big { neg: false, mag: sub_mag(&rem.mag, &divisor.mag) };
                set_bit(&mut quot, bit);
            }
        }
        let q = Big::from_sign_abs(self.neg != other.neg, quot);
        let r = Big::from_sign_abs(self.neg, rem.mag);
        (q, r)
    }

    /// `self ** exp` for a non-negative exponent.
    pub fn pow(&self, exp: u64) -> Big {
        let mut result = Big::from_radix_str("1", 10).unwrap();
        let mut base = self.clone();
        let mut e = exp;
        while e > 0 {
            if e & 1 == 1 {
                result = result.mul(&base);
            }
            base = base.mul(&base);
            e >>= 1;
        }
        result
    }

    // --- shifts / masks ---

    /// Logical left shift of the magnitude-based value (sign preserved).
    pub fn shl(&self, k: u64) -> Big {
        if self.is_zero() {
            return Big::zero();
        }
        let limbs = (k / 32) as usize;
        let off = (k % 32) as u32;
        let mut mag = alloc::vec![0u32; limbs];
        if off == 0 {
            mag.extend_from_slice(&self.mag);
        } else {
            let mut carry: u32 = 0;
            for &l in &self.mag {
                mag.push((l << off) | carry);
                carry = l >> (32 - off);
            }
            if carry != 0 {
                mag.push(carry);
            }
        }
        Big::from_sign_abs(self.neg, mag)
    }

    /// Arithmetic right shift (`floor` semantics for negative values).
    pub fn shr(&self, k: u64) -> Big {
        if self.is_zero() {
            return Big::zero();
        }
        let limbs = (k / 32) as usize;
        let off = (k % 32) as u32;
        let mut lost = false;
        // Whole limbs shifted out.
        for i in 0..limbs.min(self.mag.len()) {
            if self.mag[i] != 0 {
                lost = true;
                break;
            }
        }
        let mut src: Vec<u32> = if limbs >= self.mag.len() {
            Vec::new()
        } else {
            self.mag[limbs..].to_vec()
        };
        if off > 0 && !src.is_empty() {
            if src[0] & ((1u32 << off) - 1) != 0 {
                lost = true;
            }
            for i in (0..src.len()).rev() {
                let next = if i + 1 < src.len() { src[i + 1] } else { 0 };
                src[i] = (src[i] >> off) | (next << (32 - off));
            }
        }
        let dropped = Big { neg: false, mag: src }.normalize();
        if self.neg && lost {
            // Floor semantics: -⌈|x|/2^k⌉.
            let one = Big::from_radix_str("1", 10).unwrap();
            dropped.add(&one).negated()
        } else if self.neg {
            dropped.negated()
        } else {
            dropped
        }
    }

    /// `self mod 2^bits` as an unsigned value (for `asUintN`).
    pub fn mod_pow2(&self, bits: u64) -> Big {
        if bits == 0 || self.is_zero() {
            return Big::zero();
        }
        let masked = self.masked(bits);
        if self.neg {
            // r = 2^bits - |x| mod 2^bits  (0 when |x| ≡ 0)
            if masked.is_zero() {
                Big::zero()
            } else {
                pow2(bits).sub(&masked)
            }
        } else {
            masked
        }
    }

    /// The low `bits` bits of the absolute value.
    fn masked(&self, bits: u64) -> Big {
        let full = (bits / 32) as usize;
        let off = (bits % 32) as u32;
        let mut mag: Vec<u32> = Vec::with_capacity(full + 1);
        for i in 0..full {
            mag.push(*self.mag.get(i).unwrap_or(&0));
        }
        if off > 0 {
            let last = (*self.mag.get(full).unwrap_or(&0) & ((1u32 << off) - 1)) as u64;
            mag.push(last as u32);
        }
        Big { neg: false, mag }.normalize()
    }

    /// Number of significant bits in the magnitude.
    pub fn bit_length(&self) -> u64 {
        match self.mag.last() {
            None => 0,
            Some(&top) => (self.mag.len() as u64 - 1) * 32 + (32 - top.leading_zeros() as u64),
        }
    }

    /// Whether bit `k` of the magnitude is set.
    pub fn bit_set(&self, k: u64) -> bool {
        let limb = (k / 32) as usize;
        match self.mag.get(limb) {
            Some(&l) => l & (1u32 << (k % 32)) != 0,
            None => false,
        }
    }
}

// --- helpers ---

fn cmp_mag(a: &[u32], b: &[u32]) -> core::cmp::Ordering {
    let sa = a.iter().rposition(|&l| l != 0).map_or(0, |i| i + 1);
    let sb = b.iter().rposition(|&l| l != 0).map_or(0, |i| i + 1);
    if sa != sb {
        return sa.cmp(&sb);
    }
    for i in (0..sa).rev() {
        if a[i] != b[i] {
            return a[i].cmp(&b[i]);
        }
    }
    core::cmp::Ordering::Equal
}

fn add_mag(a: &[u32], b: &[u32]) -> Vec<u32> {
    let (long, short) = if a.len() >= b.len() { (a, b) } else { (b, a) };
    let mut out = Vec::with_capacity(long.len() + 1);
    let mut carry: u64 = 0;
    for i in 0..long.len() {
        let mut cur = long[i] as u64 + carry;
        if i < short.len() {
            cur += short[i] as u64;
        }
        out.push(cur as u32);
        carry = cur >> 32;
    }
    if carry > 0 {
        out.push(carry as u32);
    }
    out
}

fn sub_mag(a: &[u32], b: &[u32]) -> Vec<u32> {
    // Requires |a| >= |b|.
    let mut out = Vec::with_capacity(a.len());
    let mut borrow: i64 = 0;
    for i in 0..a.len() {
        let mut cur = a[i] as i64 - borrow;
        if i < b.len() {
            cur -= b[i] as i64;
        }
        if cur < 0 {
            cur += 1i64 << 32;
            borrow = 1;
        } else {
            borrow = 0;
        }
        out.push(cur as u32);
    }
    while out.last() == Some(&0) {
        out.pop();
    }
    out
}

fn mul_small_in_place(mag: &mut Vec<u32>, m: u64) {
    let mut carry: u64 = 0;
    for l in mag.iter_mut() {
        let cur = (*l as u64) * m + carry;
        *l = cur as u32;
        carry = cur >> 32;
    }
    while carry > 0 {
        mag.push(carry as u32);
        carry >>= 32;
    }
}

fn add_small_in_place(mag: &mut Vec<u32>, v: u64) {
    let mut carry = v;
    let mut i = 0;
    while carry > 0 {
        if i == mag.len() {
            mag.push(0);
        }
        let cur = mag[i] as u64 + (carry & 0xFFFF_FFFF);
        mag[i] = cur as u32;
        carry = (carry >> 32) + (cur >> 32);
        i += 1;
    }
}

/// Divide the magnitude by a single-limb divisor in place (the magnitude
/// becomes the quotient); returns the remainder.
fn divmod_small(mag: &mut Vec<u32>, divisor: u32) -> u32 {
    let mut q = vec![0u32; mag.len()];
    let mut rem: u64 = 0;
    for i in (0..mag.len()).rev() {
        let cur = (rem << 32) | mag[i] as u64;
        q[i] = (cur / divisor as u64) as u32;
        rem = cur % divisor as u64;
    }
    while q.last() == Some(&0) {
        q.pop();
    }
    *mag = q;
    rem as u32
}

fn shl1_in_place(mag: &mut Vec<u32>) {
    let mut carry: u32 = 0;
    for l in mag.iter_mut() {
        let new_carry = *l >> 31;
        *l = (*l << 1) | carry;
        carry = new_carry;
    }
    if carry != 0 {
        mag.push(carry);
    }
}

fn bit_at(mag: &[u32], bit: u64) -> bool {
    let limb = (bit / 32) as usize;
    match mag.get(limb) {
        Some(&l) => l & (1u32 << (bit % 32)) != 0,
        None => false,
    }
}

fn set_bit(mag: &mut [u32], bit: u64) {
    let limb = (bit / 32) as usize;
    if limb < mag.len() {
        mag[limb] |= 1u32 << (bit % 32);
    }
}

/// `2^bits` as a `Big`.
fn pow2(bits: u64) -> Big {
    let mut mag = alloc::vec![0u32; (bits / 32) as usize + 1];
    mag[(bits / 32) as usize] = 1u32 << (bits % 32);
    Big { neg: false, mag }
}

/// The largest `radix^k` below 2^32 and its exponent `k`.
fn max_digit_chunk(radix: u32) -> (u32, u32) {
    let mut mul: u64 = radix as u64;
    let mut k = 1u32;
    while mul * (radix as u64) < (1u64 << 32) {
        mul *= radix as u64;
        k += 1;
    }
    (k, mul as u32)
}

/// Render a chunk of `k` radix digits, optionally zero-padded.
fn format_chunk(mut c: u32, radix: u32, k: u32, pad: bool) -> alloc::string::String {
    use alloc::string::String;
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut buf = [0u8; 32];
    let mut n = 0;
    loop {
        buf[n] = DIGITS[(c % radix) as usize];
        c /= radix;
        n += 1;
        if c == 0 {
            break;
        }
    }
    let mut out = String::new();
    while n > 0 {
        n -= 1;
        out.push(buf[n] as char);
    }
    while pad && out.len() < k as usize {
        out.insert(0, '0');
    }
    out
}
