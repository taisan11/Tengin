//! ECMAScript abstract operations implemented on [`Value`]: conversions
//! (`ToBoolean` / `ToNumber` / `ToString`), `typeof`, equality and error text.

use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use super::Value;
impl Value {
    /// If this value is a primitive wrapper object, return the wrapped primitive;
    /// otherwise return `self`.
    pub fn primitive_value(&self) -> Option<Value> {
        if let Value::Object(o) = self {
            if let Some(pv) = o.borrow().props.get("__value__") {
                return Some(pv.value.clone());
            }
        }
        None
    }

    /// ECMAScript `ToBoolean` abstract operation (subset).
    pub fn to_boolean(&self) -> bool {
        match self {
            Value::Undefined | Value::Null => false,
            Value::Boolean(b) => *b,
            Value::Number(n) => !n.is_nan() && *n != 0.0,
            Value::String(s) => !s.is_empty(),
            Value::BigInt(s) => s.as_ref() != "0",
            Value::Regex(_) => true,
            _ => true,
        }
    }

    /// ECMAScript `ToNumber` abstract operation (subset).
    pub fn to_number(&self) -> f64 {
        if let Some(p) = self.primitive_value() {
            return p.to_number();
        }
        match self {
            Value::Undefined => f64::NAN,
            Value::Null => 0.0,
            Value::Boolean(b) => {
                if *b {
                    1.0
                } else {
                    0.0
                }
            }
            Value::Number(n) => *n,
            Value::String(s) => string_to_number(s),
            Value::BigInt(s) => s.parse::<f64>().unwrap_or(f64::NAN),
            Value::Regex(_) => f64::NAN,
            _ => f64::NAN,
        }
    }

    /// ECMAScript `ToString` abstract operation (subset).
    pub fn to_string(&self) -> Rc<str> {
        if let Some(p) = self.primitive_value() {
            return p.to_string();
        }
        match self {
            Value::Undefined => Rc::from("undefined"),
            Value::Null => Rc::from("null"),
            Value::Boolean(b) => Rc::from(if *b { "true" } else { "false" }),
            Value::Number(n) => Rc::from(format_number(*n).as_str()),
            Value::String(s) => s.clone(),
            Value::BigInt(s) => s.clone(),
            Value::Regex(r) => {
                Rc::from(alloc::format!("/{}/{}", r.source(), r.flag_string()).as_str())
            }
            Value::Symbol(s) => {
                Rc::from(alloc::format!("Symbol({})", s.description.as_deref().unwrap_or("")).as_str())
            }
            Value::Map(_) => Rc::from("[object Map]"),
            Value::Set(_) => Rc::from("[object Set]"),
            Value::WeakMap(_) => Rc::from("[object WeakMap]"),
            Value::WeakSet(_) => Rc::from("[object WeakSet]"),
            Value::Object(_) => Rc::from("[object Object]"),
            Value::Array(a) => {
                let parts: Vec<String> =
                    a.borrow().elems.iter().map(|v| v.to_string().to_string()).collect();
                Rc::from(parts.join(",").as_str())
            }
            Value::Function(_) | Value::NativeFunction(_) => Rc::from("function"),
        }
    }

    /// ECMAScript `typeof` operator result.
    pub fn type_of(&self) -> &'static str {
        match self {
            Value::Undefined => "undefined",
            Value::Null => "object",
            Value::Boolean(_) => "boolean",
            Value::Number(_) => "number",
            Value::BigInt(_) => "bigint",
            Value::String(_) => "string",
            Value::Symbol(_) => "symbol",
            Value::Map(_) | Value::Set(_) | Value::WeakMap(_) | Value::WeakSet(_) => "object",
            Value::Regex(_) => "object",
            Value::Object(_) | Value::Array(_) => "object",
            Value::Function(_) | Value::NativeFunction(_) => "function",
        }
    }

    /// ECMAScript `SameValue` abstract operation (used by `assert.sameValue`).
    pub fn same_value(a: &Value, b: &Value) -> bool {
        match (a, b) {
            (Value::Number(x), Value::Number(y)) => {
                if x.is_nan() && y.is_nan() {
                    return true;
                }
                if *x == 0.0 && *y == 0.0 {
                    return x.is_sign_positive() == y.is_sign_positive();
                }
                x == y
            }
            (Value::BigInt(x), Value::BigInt(y)) => x == y,
            (Value::Regex(x), Value::Regex(y)) => Rc::ptr_eq(x, y),
            (Value::Symbol(x), Value::Symbol(y)) => x.id == y.id,
            (Value::Map(x), Value::Map(y)) => Rc::ptr_eq(x, y),
            (Value::Set(x), Value::Set(y)) => Rc::ptr_eq(x, y),
            (Value::WeakMap(x), Value::WeakMap(y)) => Rc::ptr_eq(x, y),
            (Value::WeakSet(x), Value::WeakSet(y)) => Rc::ptr_eq(x, y),
            (Value::Undefined, Value::Undefined) => true,
            (Value::Null, Value::Null) => true,
            (Value::Boolean(x), Value::Boolean(y)) => x == y,
            (Value::String(x), Value::String(y)) => x == y,
            (Value::Object(x), Value::Object(y)) => Rc::ptr_eq(x, y),
            (Value::Array(x), Value::Array(y)) => Rc::ptr_eq(x, y),
            (Value::Function(x), Value::Function(y)) => Rc::ptr_eq(x, y),
            (Value::NativeFunction(x), Value::NativeFunction(y)) => Rc::ptr_eq(x, y),
            _ => false,
        }
    }

    /// ECMAScript strict equality (`===`).
    pub fn strict_eq(a: &Value, b: &Value) -> bool {
        match (a, b) {
            (Value::Number(x), Value::Number(y)) => {
                if x.is_nan() || y.is_nan() {
                    false
                } else {
                    x == y
                }
            }
            (Value::BigInt(x), Value::BigInt(y)) => x == y,
            (Value::Regex(x), Value::Regex(y)) => Rc::ptr_eq(x, y),
            (Value::Symbol(x), Value::Symbol(y)) => x.id == y.id,
            (Value::Map(x), Value::Map(y)) => Rc::ptr_eq(x, y),
            (Value::Set(x), Value::Set(y)) => Rc::ptr_eq(x, y),
            (Value::WeakMap(x), Value::WeakMap(y)) => Rc::ptr_eq(x, y),
            (Value::WeakSet(x), Value::WeakSet(y)) => Rc::ptr_eq(x, y),
            (Value::Undefined, Value::Undefined) => true,
            (Value::Null, Value::Null) => true,
            (Value::Boolean(x), Value::Boolean(y)) => x == y,
            (Value::String(x), Value::String(y)) => x == y,
            (Value::Object(x), Value::Object(y)) => Rc::ptr_eq(x, y),
            (Value::Array(x), Value::Array(y)) => Rc::ptr_eq(x, y),
            (Value::Function(x), Value::Function(y)) => Rc::ptr_eq(x, y),
            (Value::NativeFunction(x), Value::NativeFunction(y)) => Rc::ptr_eq(x, y),
            _ => false,
        }
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        Value::strict_eq(self, other)
    }
}

impl Value {
    /// A human-readable description used when a value is thrown as an exception.
    /// `Error` objects render as `Name: message` when available, falling back to
    /// the ordinary `ToString` result.
    pub fn error_message(&self) -> Rc<str> {
        if let Value::Object(o) = self {
            let g = o.borrow();
            let name = g.props.get("name").map(|p| p.value.to_string());
            let msg = g.props.get("message").map(|p| p.value.to_string());
            drop(g);
            match (name, msg) {
                (Some(n), Some(m)) if !m.is_empty() => {
                    return Rc::from(alloc::format!("{}: {}", n, m).as_str());
                }
                (Some(n), _) => return n,
                (None, Some(m)) => return m,
                _ => {}
            }
        }
        self.to_string()
    }
}

/// ECMAScript `Number::toString` for the decimal (radix-10) case, i.e. the
/// shortest round-trip formatting required by the spec. Handles the special
/// values (`NaN`, `±Infinity`, `±0`) and switches to exponential notation for
/// very large / very small magnitudes, matching what `String(n)` should yield.
///
/// Rust's own `f64`"s shortest round-trip formatting agrees with JavaScript for
/// the fixed-notation range; we only add the ECMAScript thresholds for switching
/// to exponential notation (`>= 1e21` and `< 1e-6`), which differ slightly from
/// Rust's own thresholds.
pub fn format_number(n: f64) -> alloc::string::String {
    use alloc::string::{String, ToString};

    if n.is_nan() {
        return String::from("NaN");
    }
    if n == 0.0 {
        return String::from("0"); // also covers -0
    }
    if n.is_infinite() {
        return if n > 0.0 { String::from("Infinity") } else { String::from("-Infinity") };
    }

    let abs = n.abs();
    let sign = if n < 0.0 { "-" } else { "" };

    // Very large or very small values use exponential notation: `1e+21`, `1e-7`.
    if abs >= 1e21 || abs < 1e-6 {
        return format!("{sign}{}", exp_format(abs));
    }

    format!("{sign}{}", abs.to_string())
}

/// Render a positive finite magnitude in ECMAScript exponential notation
/// (e.g. `1e+21`, `1.5e-7`), using the `regex`-free shortest mantissa Rust
/// produces with the `{:e}` format.
fn exp_format(abs: f64) -> alloc::string::String {
    let s = alloc::format!("{:e}", abs);
    let (mant, exp) = s.split_once('e').unwrap_or((s.as_str(), "0"));
    let exp_num: i64 = exp.trim_start_matches('+').parse().unwrap_or(0);
    let exp_str = if exp_num >= 0 {
        alloc::format!("+{exp_num}")
    } else {
        alloc::format!("{exp_num}")
    };
    alloc::format!("{mant}e{exp_str}")
}

/// ECMAScript `ToNumber` applied to a String. This mirrors the grammar rules the
/// spec uses for string-to-number conversion: leading/trailing whitespace is
/// trimmed, `Infinity`/`+Infinity`/`-Infinity` are recognised (case-sensitively —
/// `NaN` is *not* a valid string numeric literal), and the `0x`/`0o`/`0b`
/// prefixes select hex/octal/binary parsing. The entire remainder must match the
/// numeric grammar exactly (no numeric separators, no trailing garbage), unlike
/// the more lenient prefix scan used by `parseInt`.
fn string_to_number(s: &Rc<str>) -> f64 {
    str_to_number_value(s)
}

/// Lower-level string-to-number helper (takes `&str`), also used by the parser's
/// decimal-literal path. Returns `NaN` for any string that is not a complete
/// `StringNumericLiteral`.
pub fn str_to_number_value(s: &str) -> f64 {
    use core::f64;

    let t = s.trim();
    if t.is_empty() {
        return 0.0;
    }
    // A single leading `+` or `-` introduces a sign on a *decimal* literal (or
    // `Infinity`); it is not permitted before a radix-prefixed literal.
    let (sign, body, had_sign) = match t.as_bytes()[0] {
        b'+' => (1.0, &t[1..], true),
        b'-' => (-1.0, &t[1..], true),
        _ => (1.0, t, false),
    };
    if body.is_empty() {
        return f64::NAN;
    }

    // `Infinity` case-sensitively (the spec does not allow case variants, so
    // `Number("INFINITY")` is `NaN`).
    if body == "Infinity" {
        return sign * f64::INFINITY;
    }

    // `0x`/`0X` hex, `0o`/`0O` octal, `0b`/`0B` binary prefixes. A leading sign
    // is not part of the `NonDecimalIntegerLiteral` grammar, so `+0x10` is NaN.
    let radix_lit = radix_literal(body);
    if let Some((digits, radix)) = radix_lit {
        if had_sign {
            return f64::NAN;
        }
        return strict_radix_value(digits, radix);
    }

    // Plain decimal literal; Rust's `f64::parse` already accepts the JS grammar,
    // so we only need to validate the whole body and then parse it.
    match strict_decimal_value(body) {
        Some(v) => sign * v,
        None => f64::NAN,
    }
}

/// If `body` begins with a valid `0x`/`0o`/`0b` prefix (in any case), return the
/// digit substring and the radix.
fn radix_literal(body: &str) -> Option<(&str, u32)> {
    let b = body.as_bytes();
    if b.len() >= 2 && b[0] == b'0' {
        match b[1] {
            b'x' | b'X' => return Some((&body[2..], 16)),
            b'o' | b'O' => return Some((&body[2..], 8)),
            b'b' | b'B' => return Some((&body[2..], 2)),
            _ => {}
        }
    }
    None
}

/// Parse a radix-prefixed digit string that must consist *entirely* of valid
/// digits for `radix` (numeric separators and trailing garbage are rejected).
/// An empty or invalid body yields `NaN`.
fn strict_radix_value(s: &str, radix: u32) -> f64 {
    if s.is_empty() {
        return f64::NAN;
    }
    let mut acc = 0.0;
    for c in s.chars() {
        match c.to_digit(radix) {
            Some(d) => acc = acc * (radix as f64) + d as f64,
            None => return f64::NAN,
        }
    }
    acc
}

/// Validate and parse a `StrUnsignedDecimalLiteral`: an optional digit run, an
/// optional fraction, an optional exponent, with at least one digit overall and
/// nothing trailing. Returns `None` if the body does not match exactly.
fn strict_decimal_value(s: &str) -> Option<f64> {
    let b = s.as_bytes();
    let len = b.len();
    let mut i = 0;
    while i < len && b[i].is_ascii_digit() {
        i += 1;
    }
    let int_digits = i;
    let mut frac_digits = 0;
    if i < len && b[i] == b'.' {
        i += 1;
        let frac_start = i;
        while i < len && b[i].is_ascii_digit() {
            i += 1;
        }
        frac_digits = i - frac_start;
    }
    if int_digits == 0 && frac_digits == 0 {
        return None;
    }
    if i < len && (b[i] == b'e' || b[i] == b'E') {
        i += 1;
        if i < len && (b[i] == b'+' || b[i] == b'-') {
            i += 1;
        }
        let exp_start = i;
        while i < len && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == exp_start {
            return None; // exponent requires at least one digit
        }
    }
    if i != len {
        return None; // trailing garbage
    }
    s.parse::<f64>().ok()
}
