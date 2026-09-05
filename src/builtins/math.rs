use libm::{ceil, exp, floor, fabs, log, pow, round, sin, cos, tan, sqrt, trunc};

use crate::error::Error;
use crate::interpreter::Engine;
use crate::value::Value;

pub(crate) fn math_abs(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(fabs(a.first().map(|v| v.to_number()).unwrap_or(f64::NAN))))
}
pub(crate) fn math_floor(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(floor(a.first().map(|v| v.to_number()).unwrap_or(f64::NAN))))
}
pub(crate) fn math_ceil(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(ceil(a.first().map(|v| v.to_number()).unwrap_or(f64::NAN))))
}
pub(crate) fn math_round(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(round(a.first().map(|v| v.to_number()).unwrap_or(f64::NAN))))
}
pub(crate) fn math_trunc(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(trunc(a.first().map(|v| v.to_number()).unwrap_or(f64::NAN))))
}
pub(crate) fn math_max(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let mut m = f64::NEG_INFINITY;
    for v in a {
        m = m.max(v.to_number());
    }
    Ok(Value::Number(m))
}
pub(crate) fn math_min(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    if a.is_empty() {
        return Ok(Value::Number(f64::INFINITY));
    }
    let mut m = f64::INFINITY;
    for v in a {
        m = m.min(v.to_number());
    }
    Ok(Value::Number(m))
}
pub(crate) fn math_sqrt(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(sqrt(a.first().map(|v| v.to_number()).unwrap_or(f64::NAN))))
}
pub(crate) fn math_pow(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let x = a.first().map(|v| v.to_number()).unwrap_or(f64::NAN);
    let y = a.get(1).map(|v| v.to_number()).unwrap_or(f64::NAN);
    Ok(Value::Number(pow(x, y)))
}
pub(crate) fn math_sign(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let n = a.first().map(|v| v.to_number()).unwrap_or(f64::NAN);
    Ok(Value::Number(if n > 0.0 { 1.0 } else if n < 0.0 { -1.0 } else { n }))
}
pub(crate) fn math_exp(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(exp(a.first().map(|v| v.to_number()).unwrap_or(f64::NAN))))
}
pub(crate) fn math_log(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(log(a.first().map(|v| v.to_number()).unwrap_or(f64::NAN))))
}
pub(crate) fn math_sin(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(sin(a.first().map(|v| v.to_number()).unwrap_or(f64::NAN))))
}
pub(crate) fn math_cos(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(cos(a.first().map(|v| v.to_number()).unwrap_or(f64::NAN))))
}
pub(crate) fn math_tan(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(tan(a.first().map(|v| v.to_number()).unwrap_or(f64::NAN))))
}
pub(crate) fn math_random(_e: &Engine, _this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(0.0))
}
