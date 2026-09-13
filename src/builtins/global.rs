use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use core::cell::RefCell;

use crate::error::Error;
use crate::interpreter::Engine;
use crate::value::{Object, Property, Value};

use super::helpers::native;

pub(crate) fn is_nan_fn(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let n = a.first().cloned().unwrap_or(Value::Undefined).to_number();
    Ok(Value::Boolean(n.is_nan()))
}

pub(crate) fn is_finite_fn(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let n = a.first().cloned().unwrap_or(Value::Undefined).to_number();
    Ok(Value::Boolean(n.is_finite()))
}

pub(crate) fn decode_uri(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    decode_uri_impl(e, &v, ";:/?@&=+$,#")
}

pub(crate) fn encode_uri(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    encode_uri_impl(
        e,
        &v,
        "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_.!~*'();/?:@&=+$,#",
    )
}

pub(crate) fn decode_uri_component(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    decode_uri_impl(e, &v, "")
}

pub(crate) fn encode_uri_component(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    encode_uri_impl(
        e,
        &v,
        "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_.!~*'()",
    )
}

/// `decodeURI`/`decodeURIComponent` share the same `Decode` abstract operation;
/// the only difference is the reserved set.
fn decode_uri_impl(e: &Engine, v: &Value, reserved: &str) -> Result<Value, Error> {
    let s = to_uri_string(e, v)?;
    match decode_uri_string(&s, reserved) {
        Ok(out) => Ok(Value::String(Rc::from(out.as_str()))),
        Err(()) => Err(uri_error(e, "URIError: URI malformed")),
    }
}

/// `encodeURI`/`encodeURIComponent` share the same `Encode` abstract operation;
/// the only difference is the unescaped set.
fn encode_uri_impl(e: &Engine, v: &Value, unescaped: &str) -> Result<Value, Error> {
    let s = to_uri_string(e, v)?;
    match encode_uri_string(&s, unescaped) {
        Ok(out) => Ok(Value::String(Rc::from(out.as_str()))),
        Err(()) => Err(uri_error(e, "URIError: URI malformed")),
    }
}

/// ECMAScript `ToString` for the URI functions: convert an object via
/// `ToPrimitive(..., string)` (trying `toString` before `valueOf`), then
/// `ToString` the resulting primitive. A `Symbol` throws a `TypeError`.
fn to_uri_string(e: &Engine, v: &Value) -> Result<Rc<str>, Error> {
    let prim = e.to_primitive(v, true)?;
    if matches!(prim, Value::Symbol(_)) {
        return Err(Error::Runtime(e.make_type_error("Cannot convert a Symbol to a string")));
    }
    Ok(prim.to_string())
}

fn uri_error(e: &Engine, msg: &str) -> Error {
    Error::Runtime(e.make_uri_error(msg))
}

/// The `Encode` abstract operation: copy characters in `unescaped` verbatim and
/// UTF-8 percent-encode everything else, throwing `URIError` for a malformed
/// (lone) surrogate.
fn encode_uri_string(s: &str, unescaped: &str) -> core::result::Result<String, ()> {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let cu = c as u32;
        if cu < 0x80 {
            if unescaped.contains(c) {
                out.push(c);
            } else {
                out.push_str(&percent_encode(&[cu as u8]));
            }
        } else if (0xD800..=0xDFFF).contains(&cu) {
            // A surrogate code point that did not form a valid pair: the engine
            // only produces these from `String.fromCharCode` of a lone code unit.
            return Err(());
        } else {
            let mut buf = [0u8; 4];
            let bytes = c.encode_utf8(&mut buf).as_bytes();
            out.push_str(&percent_encode(bytes));
        }
    }
    Ok(out)
}

/// The `Decode` abstract operation: decode `%XX` escape sequences into UTF-8
/// characters, leaving reserved characters (in `reserved`) percent-encoded and
/// throwing `URIError` for any malformed sequence.
fn decode_uri_string(s: &str, reserved: &str) -> core::result::Result<String, ()> {
    let b = s.as_bytes();
    let n = b.len();
    let mut out = String::with_capacity(n);
    let mut k = 0;
    while k < n {
        if b[k] != b'%' {
            let ch = s[k..].chars().next().ok_or(())?;
            out.push(ch);
            k += ch.len_utf8();
            continue;
        }
        // A bare `%` must be followed by two hex digits.
        if k + 2 >= n {
            return Err(());
        }
        let d1 = hex_digit(b[k + 1]).ok_or(())?;
        let d2 = hex_digit(b[k + 2]).ok_or(())?;
        let first = (d1 << 4) | d2;

        if first < 0x80 {
            if reserved.contains(first as char) {
                out.push('%');
                out.push(b[k + 1] as char);
                out.push(b[k + 2] as char);
            } else {
                out.push(first as char);
            }
            k += 3;
            continue;
        }

        let byte_count = match first {
            0xC2..=0xDF => 2,
            0xE0..=0xEF => 3,
            0xF0..=0xF4 => 4,
            _ => return Err(()), // 0x80-0xBF lone continuation, 0xF5-0xFF invalid
        };

        let mut bytes = [0u8; 4];
        bytes[0] = first;
        let mut i = 1;
        while i < byte_count {
            let idx = k + 3 * i;
            if idx + 2 >= n {
                return Err(()); // truncated sequence
            }
            let c = hex_digit(b[idx + 1]).ok_or(())?;
            let d = hex_digit(b[idx + 2]).ok_or(())?;
            let byte = (c << 4) | d;
            if !(0x80..=0xBF).contains(&byte) {
                return Err(()); // invalid continuation byte
            }
            bytes[i] = byte;
            i += 1;
        }

        let cp = utf8_codepoint(&bytes[..byte_count]).ok_or(())?;
        out.push(char::from_u32(cp).ok_or(())?);
        k += 3 * byte_count;
    }
    Ok(out)
}

/// Interpret a well-formed leading-byte followed by continuation bytes as a
/// UTF-8 code point, enforcing the RFC-3629 constraints (no overlong encoding,
/// no surrogates, no code points above U+10FFFF).
fn utf8_codepoint(bytes: &[u8]) -> Option<u32> {
    let cp = match bytes.len() {
        2 => {
            let b0 = (bytes[0] & 0x1F) as u32;
            let b1 = (bytes[1] & 0x3F) as u32;
            (b0 << 6) | b1
        }
        3 => {
            let b0 = (bytes[0] & 0x0F) as u32;
            let b1 = (bytes[1] & 0x3F) as u32;
            let b2 = (bytes[2] & 0x3F) as u32;
            (b0 << 12) | (b1 << 6) | b2
        }
        4 => {
            let b0 = (bytes[0] & 0x07) as u32;
            let b1 = (bytes[1] & 0x3F) as u32;
            let b2 = (bytes[2] & 0x3F) as u32;
            let b3 = (bytes[3] & 0x3F) as u32;
            (b0 << 18) | (b1 << 12) | (b2 << 6) | b3
        }
        _ => return None,
    };
    // Overlong encoding: the minimal sequence leading byte encodes fewer bits
    // than the code point requires, e.g. `%C0%AF` (2-byte form of U+002F). With
    // the accepted leading-byte ranges and continuation-byte check, this reduces
    // to rejecting any sequence whose code point falls below the minimum for its
    // byte length (U+0080 / U+0800 / U+10000).
    let overlong = match bytes.len() {
        2 => cp < 0x80,     // impossible for bytes[0] in 0xC2..=0xDF, kept for clarity
        3 => cp < 0x800,
        4 => cp < 0x10000,
        _ => false,
    };
    if overlong {
        return None;
    }
    if (0xD800..=0xDFFF).contains(&cp) {
        return None; // encodes a UTF-16 surrogate
    }
    if cp > 0x10FFFF {
        return None;
    }
    Some(cp)
}

/// Render bytes as uppercase `%XX` escapes.
fn percent_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(bytes.len() * 3);
    for &b0 in bytes {
        out.push('%');
        out.push(HEX[(b0 >> 4) as usize] as char);
        out.push(HEX[(b0 & 0x0F) as usize] as char);
    }
    out
}

/// The numeric value of a single hexadecimal digit.
fn hex_digit(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

pub(crate) fn donotevaluate_fn(_e: &Engine, _this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    Err(Error::Runtime(Value::String(Rc::from(
        "Test262: This statement should not be evaluated.",
    ))))
}

/// Global `eval`: evaluate a string of source. Direct-eval lexical scoping is
/// not implemented; the program is evaluated in the global environment.
pub(crate) fn eval_fn(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let src = a.first().cloned().unwrap_or(Value::Undefined);
    if matches!(src, Value::Undefined | Value::Null) {
        return Ok(Value::Undefined);
    }
    let s = src.to_string();
    // A syntax error inside evaluated code is a runtime (catchable) error.
    match e.eval(&s) {
        Err(Error::Parse(msg)) => Err(Error::Runtime(Value::String(Rc::from(
            format!("SyntaxError: {msg}"),
        )))),
        other => other,
    }
}

pub(crate) fn make_assert(e: &Engine) -> Value {
    let mut o = Object::with_proto(e.object_prototype.clone());
    o.props.insert(Rc::from("sameValue"), Property::new(native(same_value_fn)));
    o.props.insert(Rc::from("notSameValue"), Property::new(native(not_same_value_fn)));
    o.props.insert(Rc::from("throws"), Property::new(native(throws_fn)));
    o.props.insert(Rc::from("true"), Property::new(native(assert_true_fn)));
    o.props.insert(Rc::from("false"), Property::new(native(assert_false_fn)));
    Value::Object(Rc::new(RefCell::new(o)))
}

pub(crate) fn same_value_fn(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let x = a.first().cloned().unwrap_or(Value::Undefined);
    let y = a.get(1).cloned().unwrap_or(Value::Undefined);
    if Value::same_value(&x, &y) {
        Ok(Value::Undefined)
    } else {
        let extra = if a.len() > 2 {
            format!(" {}", a[2].to_string())
        } else {
            String::new()
        };
        Err(Error::Runtime(Value::String(Rc::from(format!(
            "Expected SameValue(«{}», «{}») to be true{}",
            x.to_string(),
            y.to_string(),
            extra
        )))))
    }
}

pub(crate) fn not_same_value_fn(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let x = a.first().cloned().unwrap_or(Value::Undefined);
    let y = a.get(1).cloned().unwrap_or(Value::Undefined);
    if !Value::same_value(&x, &y) {
        Ok(Value::Undefined)
    } else {
        Err(Error::Runtime(Value::String(Rc::from(format!(
            "Expected not SameValue(«{}», «{}»)",
            x.to_string(),
            y.to_string()
        )))))
    }
}

pub(crate) fn assert_true_fn(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    if v.to_boolean() {
        Ok(Value::Undefined)
    } else {
        Err(Error::Runtime(Value::String(Rc::from("Expected true"))))
    }
}

pub(crate) fn assert_false_fn(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    if !v.to_boolean() {
        Ok(Value::Undefined)
    } else {
        Err(Error::Runtime(Value::String(Rc::from("Expected false"))))
    }
}

pub(crate) fn throws_fn(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let f = a.first().cloned().unwrap_or(Value::Undefined);
    match e.call_value(&f, &Value::Undefined, &[]) {
        Ok(_) => Err(Error::Runtime(Value::String(Rc::from(
            "Expected an exception to be thrown",
        )))),
        Err(_) => Ok(Value::Undefined),
    }
}
