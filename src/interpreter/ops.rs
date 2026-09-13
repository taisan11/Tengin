//! Pure value operators and coercions used by the evaluator: abstract
//! equality / binary operators, integer coercions, and key/literal helpers.
//! None of these touch the engine state, so they live independently of the
//! object model and the GC.

use alloc::rc::Rc;

use crate::ast::{BinaryOp, Lit, PropKey};
use crate::value::{RegexData, Value};
pub(crate) fn lit_to_value(l: &Lit) -> Value {
    match l {
        Lit::Number(n) => Value::Number(*n),
        Lit::String(s) => Value::String(s.clone()),
        Lit::Bool(b) => Value::Boolean(*b),
        Lit::Undefined => Value::Undefined,
        Lit::Null => Value::Null,
        Lit::BigInt(s) => Value::BigInt(s.clone()),
        Lit::Regex { pattern, flags } => {
            Value::Regex(Rc::new(RegexData::new(pattern.clone(), flags.clone())))
        }
    }
}

pub(crate) fn is_nullish(v: &Value) -> bool {
    matches!(v, Value::Null | Value::Undefined)
}

/// The canonical property-key string for the well-known `Symbol.iterator`.
pub(crate) fn symbol_iterator_id() -> Rc<str> {
    Rc::from("__wk_iterator__")
}

pub(crate) fn key_of(v: &Value) -> Rc<str> {
    match v {
        Value::Number(n) => Rc::from(alloc::format!("{}", *n as i64)),
        Value::String(s) => s.clone(),
        Value::Symbol(s) => s.id.clone(),
        _ => v.to_string(),
    }
}

pub(crate) fn key_matches(key: &PropKey, k: &str) -> bool {
    match key {
        PropKey::Ident(s) | PropKey::Str(s) => s.as_ref() == k,
        PropKey::Computed(_) => false,
    }
}

/// ECMAScript `ToInt32` (used by bitwise operators). The modulo is computed
/// with an exact fmod so huge values do not saturate through integer casts.
pub(crate) fn to_int32(n: f64) -> i32 {
    if n.is_nan() || n.is_infinite() || n == 0.0 {
        return 0;
    }
    let m = n.trunc() % 4294967296.0;
    let u = if m < 0.0 { m + 4294967296.0 } else { m };
    (u as u32) as i32
}

/// ECMAScript `ToUint32` (used by unsigned right shift).
pub(crate) fn to_uint32(n: f64) -> u32 {
    if n.is_nan() || n.is_infinite() || n == 0.0 {
        return 0;
    }
    let m = n.trunc() % 4294967296.0;
    let u = if m < 0.0 { m + 4294967296.0 } else { m };
    u as u32
}

/// ECMAScript `Abstract Equality Comparison` (`==` / `!=`).
pub(crate) fn loose_eq(a: &Value, b: &Value) -> bool {
    use core::mem::discriminant;

    // Same type: fall back to strict equality.
    if discriminant(a) == discriminant(b) {
        return Value::strict_eq(a, b);
    }

    match (a, b) {
        (Value::Null, Value::Undefined) | (Value::Undefined, Value::Null) => true,
        (Value::Number(_), Value::String(_)) => loose_eq(a, &Value::Number(b.to_number())),
        (Value::String(_), Value::Number(_)) => loose_eq(&Value::Number(a.to_number()), b),
        (Value::Boolean(_), _) => loose_eq(&Value::Number(a.to_number()), b),
        (_, Value::Boolean(_)) => loose_eq(a, &Value::Number(b.to_number())),
        (
            Value::String(_) | Value::Number(_),
            Value::Object(_) | Value::Array(_) | Value::Function(_) | Value::NativeFunction(_),
        ) => loose_eq(a, &Value::String(b.to_string())),
        (
            Value::Object(_) | Value::Array(_) | Value::Function(_) | Value::NativeFunction(_),
            Value::String(_),
        ) => loose_eq(&Value::String(a.to_string()), b),
        (
            Value::Object(_) | Value::Array(_) | Value::Function(_) | Value::NativeFunction(_),
            Value::Number(_),
        ) => loose_eq(&Value::Number(a.to_number()), b),
        _ => false,
    }
}

pub(crate) fn eval_binary(op: BinaryOp, l: &Value, r: &Value) -> Value {
    match op {
        BinaryOp::Add => {
            if matches!(l, Value::String(_)) || matches!(r, Value::String(_)) {
                let s = alloc::format!("{}{}", l.to_string(), r.to_string());
                Value::String(Rc::from(s.as_str()))
            } else {
                Value::Number(l.to_number() + r.to_number())
            }
        }
        BinaryOp::Sub => Value::Number(l.to_number() - r.to_number()),
        BinaryOp::Mul => Value::Number(l.to_number() * r.to_number()),
        BinaryOp::Div => Value::Number(l.to_number() / r.to_number()),
        BinaryOp::Rem => Value::Number(l.to_number() % r.to_number()),
        BinaryOp::Exp => Value::Number(l.to_number().powf(r.to_number())),
        BinaryOp::Eq => Value::Boolean(loose_eq(l, r)),
        BinaryOp::Ne => Value::Boolean(!loose_eq(l, r)),
        BinaryOp::Seq => Value::Boolean(Value::strict_eq(l, r)),
        BinaryOp::Sne => Value::Boolean(!Value::strict_eq(l, r)),
        BinaryOp::Lt => Value::Boolean(l.to_number() < r.to_number()),
        BinaryOp::Gt => Value::Boolean(l.to_number() > r.to_number()),
        BinaryOp::Le => Value::Boolean(l.to_number() <= r.to_number()),
        BinaryOp::Ge => Value::Boolean(l.to_number() >= r.to_number()),
        BinaryOp::In => {
            let key = key_of(&l);
            Value::Boolean(has_property(&key, &r))
        }
        BinaryOp::BitAnd => {
            Value::Number((to_int32(l.to_number()) & to_int32(r.to_number())) as f64)
        }
        BinaryOp::BitOr => {
            Value::Number((to_int32(l.to_number()) | to_int32(r.to_number())) as f64)
        }
        BinaryOp::BitXor => {
            Value::Number((to_int32(l.to_number()) ^ to_int32(r.to_number())) as f64)
        }
        BinaryOp::Shl => Value::Number(
            (to_int32(l.to_number()) << (to_uint32(r.to_number()) & 0x1f)) as f64,
        ),
        BinaryOp::Shr => Value::Number(
            (to_int32(l.to_number()) >> (to_uint32(r.to_number()) & 0x1f)) as f64,
        ),
        BinaryOp::Ushr => Value::Number(
            (to_uint32(l.to_number()) >> (to_uint32(r.to_number()) & 0x1f)) as f64,
        ),
    }
}

/// ECMAScript `in` operator: whether `key` is a property of `base`.
pub(crate) fn has_property(key: &str, base: &Value) -> bool {
    match base {
        Value::Object(o) => {
            let mut cur = Some(o.clone());
            while let Some(c) = cur {
                if c.borrow().props.contains_key(key) {
                    return true;
                }
                cur = c.borrow().proto.clone();
            }
            false
        }
        Value::Array(a) => {
            key == "length"
                || key
                    .parse::<usize>()
                    .map(|i| i < a.borrow().elems.len())
                    .unwrap_or(false)
        }
        Value::String(_) => key == "length",
        Value::Function(_) | Value::NativeFunction(_) => true,
        _ => false,
    }
}
