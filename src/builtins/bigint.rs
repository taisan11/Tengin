//! The `BigInt` constructor: `BigInt(value)`, `BigInt.asIntN`, `BigInt.asUintN`
//! and `BigInt.prototype` (`valueOf`, `toString`, `toLocaleString`,
//! `Symbol.toStringTag`).

use alloc::rc::Rc;
use core::cell::RefCell;

use crate::builtins::bigint_num::Big;
use crate::builtins::helpers::{named_native_len, native, proto_data, proto_method, proto_method_len};
use crate::error::Error;
use crate::interpreter::Engine;
use crate::value::{Object, Property, Value};

/// `BigInt(value)`:
/// 1. throw a `TypeError` when invoked via `new`
/// 2. `prim = ? ToPrimitive(value, number)`
/// 3. Number → `NumberToBigInt` (`RangeError` for non-integral values)
/// 4. otherwise → `ToBigInt(prim)` (strings parse, everything else `TypeError`)
pub(crate) fn bigint_ctor(e: &Engine, _this: &Value, a: &[Value], construct: bool) -> Result<Value, Error> {
    if construct {
        return Err(Error::Runtime(e.make_type_error(
            "BigInt is not a constructor",
        )));
    }
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    let prim = e.to_primitive(&v, false)?;
    match prim {
        Value::Number(n) => number_to_bigint(e, n),
        other => e.to_bigint_value(&other),
    }
}

/// `NumberToBigInt(number)`: exact conversion; `-0` becomes `0n`.
fn number_to_bigint(e: &Engine, n: f64) -> Result<Value, Error> {
    if !n.is_finite() || n.fract() != 0.0 {
        return Err(Error::Runtime(e.make_range_error(
            "Cannot convert a Number to a BigInt: not an integer",
        )));
    }
    // `{:.0}` prints the exact decimal value of the double.
    let s = alloc::format!("{:.0}", n);
    let s = s.trim_start_matches('-');
    let neg = n < 0.0;
    let text = if neg {
        alloc::format!("-{s}")
    } else {
        s.to_string()
    };
    Ok(Value::BigInt(Rc::from(text.as_str())))
}

/// `ToBigInt(argument)` for non-Number primitives and objects (the object case
/// is handled by `ToPrimitive` in the constructor; this entry implements the
/// primitive switch used by `asIntN`/`asUintN`).
impl Engine {
    /// `ToBigInt(argument)` → a `Value::BigInt`.
    pub(crate) fn to_bigint_value(&self, v: &Value) -> Result<Value, Error> {
        // ToPrimitive (number hint) for every object kind: a BigInt wrapper
        // exposes its primitive via `valueOf`.
        if Engine::is_object_value(v) {
            let prim = self.to_primitive(v, false)?;
            return self.to_bigint_value(&prim);
        }
        match v {
            Value::BigInt(s) => Ok(Value::BigInt(s.clone())),
            Value::String(s) => match string_to_bigint(s) {
                Some(b) => Ok(Value::BigInt(Rc::from(b.to_decimal_string().as_str()))),
                None => Err(Error::Runtime(self.make_syntax_error(
                    "Cannot convert a string to a BigInt: invalid syntax",
                ))),
            },
            Value::Boolean(b) => Ok(Value::BigInt(Rc::from(if *b { "1" } else { "0" }))),
            // Numbers, undefined, null and symbols cannot be converted by
            // `ToBigInt` (the constructor handles Numbers before calling this,
            // via `NumberToBigInt`).
            _ => Err(Error::Runtime(self.make_type_error(
                "Cannot convert the given value to a BigInt",
            ))),
        }
    }
}

/// `StringToBigInt`: the BigInt subset of the string numeric grammar —
/// optional sign (decimal only), then decimal digits or a `0x`/`0o`/`0b`
/// radix prefix. Underscores, decimal points, exponents and `Infinity` are
/// invalid. Leading/trailing whitespace is skipped; empty is `0`.
pub(crate) fn string_to_bigint(s: &str) -> Option<Big> {
    let t = s.trim();
    if t.is_empty() {
        return Some(Big::from_radix_str("0", 10).unwrap());
    }
    let first = t.as_bytes()[0];
    let (neg, body) = match first {
        b'+' => (false, &t[1..]),
        b'-' => (true, &t[1..]),
        _ => (false, t),
    };
    if body.is_empty() {
        return None;
    }
    // A sign may not precede a radix prefix.
    if let Some((digits, radix)) = radix_prefix(body) {
        if first == b'+' || first == b'-' {
            return None;
        }
        if digits.is_empty() {
            return None;
        }
        return Big::from_radix_str(digits, radix);
    }
    if !body.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let b = Big::from_radix_str(body, 10)?;
    Some(if neg { b.negated() } else { b })
}

/// Recognize a `0x`/`0X`/`0o`/`0O`/`0b`/`0B` prefix; returns the digit run and
/// the radix.
fn radix_prefix(body: &str) -> Option<(&str, u32)> {
    let b = body.as_bytes();
    if b.len() >= 2 && b[0] == b'0' {
        return match b[1] {
            b'x' | b'X' => Some((&body[2..], 16)),
            b'o' | b'O' => Some((&body[2..], 8)),
            b'b' | b'B' => Some((&body[2..], 2)),
            _ => None,
        };
    }
    None
}

/// `ToIndex(bits)` → u64, or a `RangeError` when out of `[0, 2^53-1]`.
fn to_index(e: &Engine, v: &Value) -> Result<u64, Error> {
    let n = e.to_integer_fallible(v)?;
    if n.is_infinite() {
        return Err(Error::Runtime(e.make_range_error(
            "Index is out of range",
        )));
    }
    if n < 0.0 || n > 9007199254740991.0 {
        return Err(Error::Runtime(e.make_range_error(
            "Index is out of range",
        )));
    }
    Ok(n as u64)
}

/// `thisBigIntValue`: the primitive of a BigInt or of a BigInt wrapper object.
fn this_bigint_value(e: &Engine, this: &Value) -> Result<Rc<str>, Error> {
    match this {
        Value::BigInt(s) => Ok(s.clone()),
        Value::Object(_) => match e.get_property(this, "__value__") {
            Value::BigInt(s) => Ok(s),
            _ => Err(Error::Runtime(e.make_type_error(
                "BigInt.prototype method called on a non-BigInt",
            ))),
        },
        _ => Err(Error::Runtime(e.make_type_error(
            "BigInt.prototype method called on a non-BigInt",
        ))),
    }
}

// --- BigInt.prototype methods ---

pub(crate) fn bigint_value_of(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::BigInt(this_bigint_value(e, this)?))
}

/// `BigInt.prototype.toString(radix)` — radix defaults to 10 and must be an
/// integer in `[2, 36]`.
pub(crate) fn bigint_to_string(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let x = this_bigint_value(e, this)?;
    let radix = match a.first() {
        None | Some(Value::Undefined) => 10,
        Some(v) => {
            let r = e.to_integer_fallible(v)?;
            if r.is_infinite() || r < 2.0 || r > 36.0 {
                return Err(Error::Runtime(e.make_range_error(
                    "radix must be an integer at least 2 and no greater than 36",
                )));
            }
            r as u32
        }
    };
    let big = Big::from_decimal(&x).ok_or_else(|| {
        Error::Runtime(e.make_type_error("invalid BigInt internal representation"))
    })?;
    Ok(Value::String(Rc::from(big.to_string_radix(radix).as_str())))
}

pub(crate) fn bigint_to_locale_string(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    // The engine performs no locale-aware formatting: same digits as toString(10).
    let x = this_bigint_value(e, this)?;
    Ok(Value::String(x))
}

// --- BigInt.asIntN / BigInt.asUintN ---

pub(crate) fn bigint_as_uint_n(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let bits = to_index(e, a.first().unwrap_or(&Value::Undefined))?;
    let arg = e.to_bigint_value(&a.get(1).cloned().unwrap_or(Value::Undefined))?;
    as_n(e, bits, &arg, false)
}

pub(crate) fn bigint_as_int_n(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let bits = to_index(e, a.first().unwrap_or(&Value::Undefined))?;
    let arg = e.to_bigint_value(&a.get(1).cloned().unwrap_or(Value::Undefined))?;
    as_n(e, bits, &arg, true)
}

/// Shared implementation of `asIntN`/`asUintN`.
fn as_n(e: &Engine, bits: u64, arg: &Value, signed: bool) -> Result<Value, Error> {
    let s = match arg {
        Value::BigInt(s) => s.clone(),
        _ => return Err(Error::Runtime(e.make_type_error("not a BigInt"))),
    };
    let big = Big::from_decimal(&s).ok_or_else(|| {
        Error::Runtime(e.make_type_error("invalid BigInt internal representation"))
    })?;
    let result = if bits > 1u64 << 26 {
        // Masking cannot materialize for absurd bit counts; a non-negative
        // value is its own residue, a negative one would need 2^bits.
        if big.neg && !big.is_zero() {
            return Err(Error::Runtime(e.make_range_error(
                "BigInt too large for asIntN/asUintN with this bit count",
            )));
        }
        big
    } else if signed {
        let m = big.mod_pow2(bits);
        if bits > 0 && m.bit_set(bits - 1) {
            m.sub(&pow2_for(bits))
        } else {
            m
        }
    } else {
        big.mod_pow2(bits)
    };
    Ok(Value::BigInt(Rc::from(result.to_decimal_string().as_str())))
}

fn pow2_for(bits: u64) -> Big {
    // 2^bits — `bits` is capped well below memory-safe limits here.
    let mut mag = alloc::vec![0u32; (bits / 32) as usize + 1];
    mag[(bits / 32) as usize] = 1u32 << (bits % 32);
    Big { neg: false, mag }
}

// --- registration ---

/// Install the `BigInt` global: the constructor, its statics and prototype.
pub(crate) fn register_bigint(e: &mut Engine) {
    let proto = Rc::new(RefCell::new(Object::with_proto(e.object_prototype.clone())));

    // The constructor is callable (not new-able) yet is a constructor object
    // for `Reflect.construct(fn, args, BigInt)`'s `Get(newTarget, "prototype")`.
    let ctor = native(bigint_ctor);
    if let Value::NativeFunction(nf) = &ctor {
        let mut b = nf.borrow_mut();
        b.constructable = true;
        b.proto = Some(e.function_prototype.clone());
        b.realm = Some(e.realm.clone());
        b.intrinsic_proto = Some(crate::value::RealmProto::Object);
        b.props.insert(
            Rc::from("prototype"),
            Property::constant(Value::Object(proto.clone())),
        );
        b.props.insert(
            Rc::from("name"),
            Property::config(Value::String(Rc::from("BigInt"))),
        );
        b.props
            .insert(Rc::from("length"), Property::config(Value::Number(1.0)));
    }

    proto_data(&proto, "constructor", ctor.clone());

    // BigInt.prototype.valueOf / toString / toLocaleString.
    proto_method_len(&proto, "toString", bigint_to_string, 0);
    proto_method(&proto, "valueOf", bigint_value_of);
    proto_method_len(&proto, "toLocaleString", bigint_to_locale_string, 0);
    // `Symbol.toStringTag` = "BigInt" ({ Writable: false, Enumerable: false,
    // Configurable: true }).
    proto.borrow_mut().props.insert(
        Rc::from(crate::value::SymbolData::well_known_key("toStringTag").as_ref()),
        Property::config(Value::String(Rc::from("BigInt"))),
    );

    // Statics: asIntN / asUintN (both length 2, not constructors).
    if let Value::NativeFunction(nf) = &ctor {
        let mut b = nf.borrow_mut();
        b.props.insert(
            Rc::from("asIntN"),
            Property::method(named_native_len("asIntN", bigint_as_int_n, 2)),
        );
        b.props.insert(
            Rc::from("asUintN"),
            Property::method(named_native_len("asUintN", bigint_as_uint_n, 2)),
        );
    }

    e.bigint_prototype = proto.clone();
    e.register_global("BigInt", ctor);
}
