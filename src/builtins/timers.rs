//! `setTimeout`/`clearTimeout`/`setInterval`/`clearInterval` on the engine's
//! virtual clock (see `Engine::drain_timers`).
//!
//! Time only advances while timers are pumped (after each top-level `eval`),
//! so timer behavior is fully deterministic: `setTimeout(fn, 100)` fires
//! after microtasks drain, and `setInterval(fn, 0)` is bounded by the pump
//! cap rather than hanging the host.

use alloc::vec::Vec;

use crate::error::Error;
use crate::interpreter::Engine;
use crate::value::Value;

use super::helpers::named_native;

/// Coerce a delay argument to whole milliseconds, clamped at zero.
fn to_delay(v: &Value) -> u64 {
    let n = v.to_number();
    if n.is_nan() || n <= 0.0 {
        0
    } else if n >= u64::MAX as f64 {
        u64::MAX / 2
    } else {
        n as u64
    }
}

fn check_callback(e: &Engine, a: &[Value]) -> Result<Value, Error> {
    let cb = a.first().cloned().unwrap_or(Value::Undefined);
    if !matches!(cb, Value::Function(_) | Value::NativeFunction(_)) {
        return Err(Error::Runtime(
            e.make_type_error("timer callback is not a function"),
        ));
    }
    Ok(cb)
}

/// `setTimeout(callback, delay = 0, ...args)` → timer id.
fn set_timeout(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let cb = check_callback(e, a)?;
    let delay = a.get(1).map(to_delay).unwrap_or(0);
    let args: Vec<Value> = a.get(2..).unwrap_or(&[]).to_vec();
    Ok(Value::Number(e.set_timer(cb, args, delay, None) as f64))
}

/// `setInterval(callback, delay = 0, ...args)` → timer id.
fn set_interval(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let cb = check_callback(e, a)?;
    let delay = a.get(1).map(to_delay).unwrap_or(0);
    let args: Vec<Value> = a.get(2..).unwrap_or(&[]).to_vec();
    Ok(Value::Number(
        e.set_timer(cb, args, delay, Some(delay)) as f64,
    ))
}

/// `clearTimeout(id)` / `clearInterval(id)` (shared namespace, no-op when
/// absent).
fn clear_timer(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let id = a.first().map(|v| v.to_number()).unwrap_or(f64::NAN);
    if id.is_nan() || id < 0.0 {
        return Ok(Value::Undefined);
    }
    e.clear_timer(id as u64);
    Ok(Value::Undefined)
}

/// Install the timer globals on the engine.
pub(crate) fn register_timers(engine: &mut Engine) {
    engine.register_global("setTimeout", named_native("setTimeout", set_timeout));
    engine.register_global("clearTimeout", named_native("clearTimeout", clear_timer));
    engine.register_global("setInterval", named_native("setInterval", set_interval));
    engine.register_global("clearInterval", named_native("clearInterval", clear_timer));
}
