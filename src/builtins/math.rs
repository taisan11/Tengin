//! The `Math` namespace object: every abstract operation defined by the
//! specification (`abs`, trigonometry, logarithms, `pow`, …) plus the exact
//! summation helper `Math.sumPrecise`.
//!
//! All methods apply `ToNumber` (which can run user `valueOf`/`toString` code
//! and therefore fail) before their numeric work.

use libm::{
    acos, acosh, asin, asinh, atan, atan2, atanh, cbrt, ceil, cos, cosh, exp, expm1, floor, log,
    log10, log1p, log2, pow, sin, sinh, sqrt, tan, tanh, trunc,
};

use crate::error::Error;
use crate::interpreter::Engine;
use crate::value::Value;

/// `ToNumber` with error propagation (spec-correct `ToNumber`): user code may
/// throw during `valueOf`/`toString` coercion, and `Symbol`/`BigInt` throw a
/// `TypeError`.
fn num(e: &Engine, v: Option<&Value>) -> Result<f64, Error> {
    match v {
        None => Ok(f64::NAN),
        Some(v) => e.to_number_fallible(v),
    }
}

pub(crate) fn math_abs(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(libm::fabs(num(e, a.first())?)))
}
pub(crate) fn math_floor(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(floor(num(e, a.first())?)))
}
pub(crate) fn math_ceil(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(ceil(num(e, a.first())?)))
}

/// ECMAScript `Math.round`: half-way cases round toward +∞ and `-0.5` yields
/// `-0` (libm's `round` rounds half away from zero, which the spec forbids).
pub(crate) fn math_round(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let x = num(e, a.first())?;
    if x.is_nan() || x.is_infinite() || x == 0.0 {
        return Ok(Value::Number(x));
    }
    let fx = floor(x);
    let y = if x - fx < 0.5 { fx } else { fx + 1.0 };
    let y = if y == 0.0 && x < 0.0 { -0.0 } else { y };
    Ok(Value::Number(y))
}

pub(crate) fn math_trunc(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(trunc(num(e, a.first())?)))
}

/// `Math.max`: `-Infinity` with no arguments, any `NaN` argument poisons the
/// result, and `+0`/`-0` selection follows the spec's `>` comparison. Every
/// argument is coerced *before* the NaN check (spec step order).
pub(crate) fn math_max(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let mut nums = Vec::with_capacity(a.len());
    for v in a {
        nums.push(num(e, Some(v))?);
    }
    if nums.iter().any(|n| n.is_nan()) {
        return Ok(Value::Number(f64::NAN));
    }
    let mut m = f64::NEG_INFINITY;
    for n in nums {
        if n > m || (n == 0.0 && m == 0.0 && n.is_sign_positive() && m.is_sign_negative()) {
            m = n;
        }
    }
    Ok(Value::Number(m))
}

/// `Math.min`: mirror image of `Math.max`.
pub(crate) fn math_min(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let mut nums = Vec::with_capacity(a.len());
    for v in a {
        nums.push(num(e, Some(v))?);
    }
    if nums.iter().any(|n| n.is_nan()) {
        return Ok(Value::Number(f64::NAN));
    }
    let mut m = f64::INFINITY;
    for n in nums {
        if n < m || (n == 0.0 && m == 0.0 && n.is_sign_negative() && m.is_sign_positive()) {
            m = n;
        }
    }
    Ok(Value::Number(m))
}

pub(crate) fn math_sqrt(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(sqrt(num(e, a.first())?)))
}

/// ECMAScript `Math.pow` (which mirrors the `**` exponentiation edge cases):
/// `±1 ** ±∞` is `NaN` and a zero exponent is `1`, beyond what IEEE 754 `pow`
/// guarantees on its own.
pub(crate) fn math_pow(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let x = num(e, a.first())?;
    let y = num(e, a.get(1))?;
    Ok(Value::Number(ecma_pow(x, y)))
}

/// The ECMAScript exponentiation edge cases layered over IEEE 754 `pow`.
pub(crate) fn ecma_pow(x: f64, y: f64) -> f64 {
    if y.is_nan() {
        return f64::NAN;
    }
    if y == 0.0 {
        return 1.0;
    }
    if x.is_nan() {
        return f64::NAN;
    }
    let y_odd_int = {
        let t = y.trunc();
        // Only integral exponents can be odd; `t % 2` stays exact for huge t.
        t == y && (t % 2.0).abs() == 1.0
    };
    if x.is_infinite() {
        if x > 0.0 {
            return if y > 0.0 { f64::INFINITY } else { 0.0 };
        }
        // x = -Infinity
        return if y > 0.0 {
            if y_odd_int {
                f64::NEG_INFINITY
            } else {
                f64::INFINITY
            }
        } else if y_odd_int {
            -0.0
        } else {
            0.0
        };
    }
    if x == 0.0 {
        return if y > 0.0 {
            if y_odd_int {
                x
            } else {
                0.0
            }
        } else if y_odd_int {
            if x.is_sign_negative() {
                f64::NEG_INFINITY
            } else {
                f64::INFINITY
            }
        } else {
            f64::INFINITY
        };
    }
    if (x == 1.0 || x == -1.0) && y.is_infinite() {
        return f64::NAN;
    }
    pow(x, y)
}

/// `Math.sign`: `±0` keeps its sign, everything else maps to `±1`.
pub(crate) fn math_sign(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let n = num(e, a.first())?;
    Ok(Value::Number(if n > 0.0 {
        1.0
    } else if n < 0.0 {
        -1.0
    } else {
        n
    }))
}

pub(crate) fn math_exp(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(exp(num(e, a.first())?)))
}
pub(crate) fn math_log(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(log(num(e, a.first())?)))
}
pub(crate) fn math_sin(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(sin(num(e, a.first())?)))
}
pub(crate) fn math_cos(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(cos(num(e, a.first())?)))
}
pub(crate) fn math_tan(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(tan(num(e, a.first())?)))
}
pub(crate) fn math_acos(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(acos(num(e, a.first())?)))
}
pub(crate) fn math_asin(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(asin(num(e, a.first())?)))
}
pub(crate) fn math_atan(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(atan(num(e, a.first())?)))
}
pub(crate) fn math_atan2(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let y = num(e, a.first())?;
    let x = num(e, a.get(1))?;
    Ok(Value::Number(atan2(y, x)))
}
pub(crate) fn math_acosh(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(acosh(num(e, a.first())?)))
}
pub(crate) fn math_asinh(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(asinh(num(e, a.first())?)))
}
pub(crate) fn math_atanh(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(atanh(num(e, a.first())?)))
}
pub(crate) fn math_cbrt(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(cbrt(num(e, a.first())?)))
}
pub(crate) fn math_cosh(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(cosh(num(e, a.first())?)))
}
pub(crate) fn math_sinh(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(sinh(num(e, a.first())?)))
}
pub(crate) fn math_tanh(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(tanh(num(e, a.first())?)))
}
pub(crate) fn math_expm1(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(expm1(num(e, a.first())?)))
}
pub(crate) fn math_log1p(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(log1p(num(e, a.first())?)))
}
pub(crate) fn math_log10(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(log10(num(e, a.first())?)))
}
pub(crate) fn math_log2(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(log2(num(e, a.first())?)))
}

/// `Math.clz32`: leading zero bits of `ToUint32(x)`.
pub(crate) fn math_clz32(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let n = num(e, a.first())?;
    let u = if n.is_nan() || n.is_infinite() || n == 0.0 {
        0u32
    } else {
        let int = n.trunc();
        (int as i64 as u64 & 0xFFFF_FFFF) as u32
    };
    Ok(Value::Number(u.leading_zeros() as f64))
}

/// `Math.imul`: 32-bit wrapping integer multiplication.
pub(crate) fn math_imul(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let x = num(e, a.first())?;
    let y = num(e, a.get(1))?;
    let ix = crate::interpreter::ops::to_int32(x);
    let iy = crate::interpreter::ops::to_int32(y);
    Ok(Value::Number(ix.wrapping_mul(iy) as f64))
}

/// `Math.fround`: round to the nearest IEEE binary32 value.
pub(crate) fn math_fround(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(num(e, a.first())? as f32 as f64))
}

/// Convert an IEEE binary64 value to the nearest binary16 value
/// (round-half-to-even), returning the raw 16-bit pattern.
fn f64_to_f16_bits(x: f64) -> u16 {
    let bits = x.to_bits();
    let sign = ((bits >> 63) as u16) << 15;
    let exp = ((bits >> 52) & 0x7FF) as i32;
    let frac52 = bits & 0x000F_FFFF_FFFF_FFFF;

    if exp == 0x7FF {
        if frac52 == 0 {
            return sign | 0x7C00; // ±Infinity
        }
        return sign | 0x7E00; // NaN (canonical quiet NaN)
    }

    let e16 = exp - 1023 + 15;

    if e16 >= 0x1F {
        return sign | 0x7C00; // overflow → ±Infinity
    }
    if e16 >= 1 {
        // Normal binary16: keep the top 10 mantissa bits, round-half-to-even.
        let keep = (frac52 >> 42) as u16;
        let rest = frac52 & ((1u64 << 42) - 1);
        let half = 1u64 << 41;
        let mut m = keep;
        if rest > half || (rest == half && (m & 1) == 1) {
            m += 1;
            if m == 0x0400 {
                // Mantissa carry bumps the exponent.
                return sign | (((e16 + 1) as u16) << 10);
            }
        }
        return sign | ((e16 as u16) << 10) | m;
    }
    // Subnormal binary16 (or underflow to zero): `e16 <= 0`.
    let s = (43 - e16) as u32; // right-shift of the 53-bit significand
    if s >= 54 {
        return sign; // rounds to zero (the smallest subnormal is 2^-24)
    }
    let m_full = frac52 | (1u64 << 52);
    let mut m = (m_full >> s) as u16;
    let rest = m_full & ((1u64 << s) - 1);
    let half = 1u64 << (s - 1);
    if rest > half || (rest == half && (m & 1) == 1) {
        m += 1;
        // A carry into bit 10 lands in the exponent field, which is exactly
        // the encoding of the smallest normal (exp=1, mantissa=0).
    }
    sign | m
}

/// Rebuild an IEEE binary64 value from a binary16 bit pattern.
fn f16_bits_to_f64(h: u16) -> f64 {
    let sign = ((h & 0x8000) as u64) << 48;
    let exp = ((h >> 10) & 0x1F) as i32;
    let mant = (h & 0x03FF) as u64;
    if exp == 0 {
        if mant == 0 {
            return f64::from_bits(sign);
        }
        // Subnormal: normalize the leading bit.
        let mut e = -14i32;
        let mut m = mant;
        while m & 0x0400 == 0 {
            m <<= 1;
            e -= 1;
        }
        m &= 0x03FF;
        return f64::from_bits(sign | (((e + 1023) as u64) << 52) | (m << 42));
    }
    if exp == 0x1F {
        let inf_nan = 0x7FF0_0000_0000_0000u64 | (mant << 42);
        return f64::from_bits(sign | inf_nan);
    }
    f64::from_bits(sign | (((exp as u64) - 15 + 1023) << 52) | (mant << 42))
}

/// `Math.f16round`: round to the nearest IEEE binary16 (half precision) value.
pub(crate) fn math_f16round(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let x = num(e, a.first())?;
    Ok(Value::Number(f16_bits_to_f64(f64_to_f16_bits(x))))
}

/// `Math.hypot`: `sqrt(Σxᵢ²)` without intermediate overflow. Any infinite
/// argument yields `+Infinity`; `NaN` yields `NaN` only when no argument was
/// infinite.
pub(crate) fn math_hypot(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let mut maxabs = 0.0f64;
    let mut coerced = Vec::with_capacity(a.len());
    for v in a {
        let n = num(e, Some(v))?;
        coerced.push(n);
        let a0 = n.abs();
        if a0 > maxabs {
            maxabs = a0;
        }
    }
    if coerced.iter().any(|n| n.is_infinite()) {
        return Ok(Value::Number(f64::INFINITY));
    }
    if coerced.iter().any(|n| n.is_nan()) {
        return Ok(Value::Number(f64::NAN));
    }
    // Scale by an exact power of two so that `len * (max·2^-shift)²` stays
    // safely inside the binary64 range before squaring/summing.
    let len = coerced.len() as f64;
    let target = (0.25 * f64::MAX / len).sqrt();
    let mut shift = 0i32;
    if maxabs > target {
        shift = ((maxabs / target).log2().ceil() as i32).max(0);
    }
    let scale = 2f64.powi(-shift);
    let mut acc = 0.0f64;
    for n in &coerced {
        let s = n * scale;
        acc += s * s;
    }
    Ok(Value::Number(sqrt(acc) * 2f64.powi(shift)))
}

/// `Math.sumPrecise` (ES2025): maximally precise summation of an iterable of
/// Numbers — the exact sum rounded once to binary64. Elements are processed
/// one at a time (so the iterator is advanced lazily and closed on an abrupt
/// completion), and every element must be a Number.
pub(crate) fn math_sumprecise(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let items = match a.first() {
        Some(v) => v,
        None => {
            return Err(Error::Runtime(e.make_type_error(
                "Math.sumPrecise requires an iterable of Numbers",
            )))
        }
    };

    let not_number_err =
        || Error::Runtime(e.make_type_error("Math.sumPrecise requires an iterable of Numbers"));

    enum Source {
        Protocol(Value),          // an iterator object
        Values,                   // pre-collected fallback in `pre`
    }
    let (source, pre) = {
        let iter_key = crate::value::SymbolData::well_known_key("iterator");
        let iter_fn = e.get_property(items, iter_key.as_ref());
        if Engine::is_callable_value(&iter_fn) {
            (Source::Protocol(e.call_value(&iter_fn, items, &[])?), None)
        } else {
            (Source::Values, Some(e.iterable_values(items)?))
        }
    };

    let mut acc = SuperAcc::new();
    let mut has_nan = false;
    let mut pos_inf = false;
    let mut neg_inf = false;
    let mut saw_zero = false;
    let mut all_neg_zero = true;
    let mut saw_any = false;
    let mut idx = 0usize;

    loop {
        // Advance one element; `None` = done.
        let item = match (&source, &pre) {
            (Source::Protocol(iter), _) => {
                let next_fn = e.get_property(iter, "next");
                let res = e.call_value(&next_fn, iter, &[])?;
                if e.get_property(&res, "done").to_boolean() {
                    None
                } else {
                    Some(e.get_property(&res, "value"))
                }
            }
            (Source::Values, Some(values)) => values.get(idx).cloned(),
            _ => None,
        };
        let Some(item) = item else { break };
        idx += 1;
        saw_any = true;

        if !matches!(item, Value::Number(_)) {
            // Abrupt completion: close the iterator, then throw.
            if let Source::Protocol(iter) = &source {
                let ret = e.get_property(iter, "return");
                if Engine::is_callable_value(&ret) {
                    let _ = e.call_value(&ret, iter, &[]);
                }
            }
            return Err(not_number_err());
        }
        let n = item.to_number();
        if n.is_nan() {
            has_nan = true;
            continue;
        }
        if n.is_infinite() {
            if n > 0.0 {
                pos_inf = true;
            } else {
                neg_inf = true;
            }
            continue;
        }
        if n == 0.0 {
            saw_zero = true;
            if n.is_sign_positive() {
                all_neg_zero = false;
            }
            continue;
        }
        all_neg_zero = false;
        acc.add(n);
    }

    if has_nan {
        return Ok(Value::Number(f64::NAN));
    }
    if pos_inf && neg_inf {
        return Ok(Value::Number(f64::NAN));
    }
    if pos_inf {
        return Ok(Value::Number(f64::INFINITY));
    }
    if neg_inf {
        return Ok(Value::Number(f64::NEG_INFINITY));
    }
    if !saw_any {
        return Ok(Value::Number(-0.0)); // empty iterable → -0
    }
    if acc.is_empty() {
        return Ok(Value::Number(if saw_zero && all_neg_zero {
            -0.0 // only -0 values
        } else {
            0.0
        }));
    }
    Ok(Value::Number(acc.finish(!saw_zero || all_neg_zero)))
}

/// Fixed-point accumulator holding a non-negative integer multiple of
/// `2^-1074`, limb-wise little-endian. A [`SuperAcc`] keeps the positive and
/// negative contributions separately so no signed arithmetic is needed.
struct SuperAcc {
    pos: FixedSum,
    neg: FixedSum,
}

struct FixedSum {
    limbs: [u64; 40], // 2560 bits ≫ the 2098 bits any double can contribute
    overflow: bool,
}

impl FixedSum {
    fn new() -> Self {
        FixedSum {
            limbs: [0; 40],
            overflow: false,
        }
    }

    /// Add (or subtract) `mant << shift` bits into the accumulator.
    fn add_shifted(&mut self, mant: u64, shift: u32, subtract: bool) {
        let start = (shift / 64) as usize;
        let off = shift % 64;
        let mut cur = mant as u128;
        let mut i = start;
        // Spread `cur` (already shifted) over limbs starting at `start`.
        if off > 0 {
            // `cur` is 53 bits; shifting by up to 63 keeps it in u128.
            cur <<= off;
        }
        if i >= self.limbs.len() {
            self.overflow = true;
            return;
        }
        if !subtract {
            let mut carry = cur;
            while carry != 0 {
                if i >= self.limbs.len() {
                    self.overflow = true;
                    return;
                }
                let s = self.limbs[i] as u128 + (carry & 0xFFFF_FFFF_FFFF_FFFF);
                self.limbs[i] = s as u64;
                carry = (carry >> 64) + (s >> 64);
                i += 1;
            }
        } else {
            // Subtract `cur` with borrow; the caller guarantees the total is
            // non-negative (per-sign accumulators), so a borrow escaping the
            // top limb cannot happen unless the accumulator overflowed.
            let mut borrow = 0u128;
            while (cur != 0 || borrow != 0) && i < self.limbs.len() {
                let take = (cur & 0xFFFF_FFFF_FFFF_FFFF) + borrow;
                let li = self.limbs[i] as u128;
                if li >= take {
                    self.limbs[i] = (li - take) as u64;
                    borrow = 0;
                } else {
                    self.limbs[i] = (li + (1u128 << 64) - take) as u64;
                    borrow = 1;
                }
                cur >>= 64;
                i += 1;
            }
        }
    }

    fn cmp(&self, other: &FixedSum) -> core::cmp::Ordering {
        for i in (0..self.limbs.len()).rev() {
            match self.limbs[i].cmp(&other.limbs[i]) {
                core::cmp::Ordering::Equal => continue,
                o => return o,
            }
        }
        core::cmp::Ordering::Equal
    }

    fn is_zero(&self) -> bool {
        self.limbs.iter().all(|&l| l == 0)
    }

    /// Round the accumulated value (`limbs × 2^-1074`) to the nearest binary64.
    fn to_f64(&self, negative: bool) -> f64 {
        let sign = (negative as u64) << 63;
        if self.overflow {
            return f64::from_bits(sign | 0x7FF0_0000_0000_0000); // ±Infinity
        }
        // Bit length of the magnitude.
        let mut l = 0usize;
        for i in (0..self.limbs.len()).rev() {
            if self.limbs[i] != 0 {
                l = i * 64 + (64 - self.limbs[i].leading_zeros() as usize);
                break;
            }
        }
        if l == 0 {
            return f64::from_bits(sign); // exact zero
        }
        // Denormal range (value < 2^-1022): exact, mantissa in limb 0.
        if l <= 52 {
            return f64::from_bits(sign | self.limbs[0]);
        }
        // Normal: unbiased exponent of the leading bit.
        let mut e = l as i32 - 1075;
        // Top 53 bits of the magnitude: f = mag >> (l - 53).
        let e2 = l - 53;
        let mut f = 0u64;
        for j in 0..53u32 {
            if bit_at(self, e2 as u32 + j) {
                f |= 1u64 << j;
            }
        }
        // Round the dropped bits ([0, e2)) half-to-even.
        if e2 > 0 {
            let half_set = bit_at(self, e2 as u32 - 1);
            let mut greater = false;
            if half_set {
                for b in 0..e2 - 1 {
                    if bit_at(self, b as u32) {
                        greater = true;
                        break;
                    }
                }
            }
            let round_up = if half_set {
                if greater {
                    true
                } else {
                    f & 1 == 1 // tie: round to even
                }
            } else {
                false
            };
            if round_up {
                f += 1;
                if f == 1u64 << 53 {
                    f = 1u64 << 52;
                    e += 1;
                }
            }
        }
        if e > 1023 {
            return f64::from_bits(sign | 0x7FF0_0000_0000_0000); // ±Infinity
        }
        f64::from_bits(sign | (((e + 1023) as u64) << 52) | (f & ((1u64 << 52) - 1)))
    }
}

/// The value of bit `idx` of the accumulated magnitude.
fn bit_at(acc: &FixedSum, idx: u32) -> bool {
    let limb = (idx / 64) as usize;
    if limb >= acc.limbs.len() {
        return false;
    }
    acc.limbs[limb] & (1u64 << (idx % 64)) != 0
}

impl SuperAcc {
    fn new() -> Self {
        SuperAcc {
            pos: FixedSum::new(),
            neg: FixedSum::new(),
        }
    }

    fn is_empty(&self) -> bool {
        self.pos.is_zero() && self.neg.is_zero()
    }

    /// Add a finite, non-zero double to the exact sum.
    fn add(&mut self, n: f64) {
        let bits = n.to_bits();
        let negative = bits >> 63 == 1;
        let exp_field = ((bits >> 52) & 0x7FF) as u32;
        let frac = bits & 0x000F_FFFF_FFFF_FFFF;
        let (mant, shift) = if exp_field == 0 {
            if frac == 0 {
                return; // ±0: zero-sign handled by the caller
            }
            (frac, 0u32)
        } else {
            (frac | (1u64 << 52), exp_field - 1)
        };
        let target = if negative { &mut self.neg } else { &mut self.pos };
        target.add_shifted(mant, shift, false);
    }

    /// Reduce the two accumulators to a single binary64 value. `neg_zero`
    /// selects the sign of an exactly-zero result.
    fn finish(&mut self, neg_zero: bool) -> f64 {
        if self.pos.is_zero() && self.neg.is_zero() {
            return if neg_zero { -0.0 } else { 0.0 };
        }
        let ordering = self.pos.cmp(&self.neg);
        let (negative, mag) = match ordering {
            core::cmp::Ordering::Less => (true, &self.neg),
            _ => (false, &self.pos),
        };
        if ordering == core::cmp::Ordering::Equal {
            // Exact cancellation → +0.
            return 0.0;
        }
        let mut diff = FixedSum::new();
        diff.overflow = mag.overflow;
        for i in 0..mag.limbs.len() {
            diff.limbs[i] = mag.limbs[i];
        }
        let sub = match ordering {
            core::cmp::Ordering::Less => &self.pos,
            _ => &self.neg,
        };
        for i in 0..sub.limbs.len() {
            let v = sub.limbs[i];
            if v != 0 {
                diff.add_shifted(v, (i * 64) as u32, true);
            }
        }
        diff.to_f64(negative)
    }
}

pub(crate) fn math_random(_e: &Engine, _this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(0.0))
}
