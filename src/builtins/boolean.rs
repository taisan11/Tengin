use alloc::rc::Rc;

use crate::error::Error;
use crate::interpreter::Engine;
use crate::value::Value;

use super::helpers::{set_primitive};

/// `Boolean.prototype` methods are non-generic: `this` must be a Boolean value
/// or a Boolean wrapper object, otherwise a `TypeError` is thrown.
fn require_boolean_this(e: &Engine, this: &Value) -> Result<bool, Error> {
    match this {
        Value::Boolean(b) => Ok(*b),
        Value::Object(o) => {
            if let Some(pv) = o.borrow().props.get("__value__") {
                if let Value::Boolean(b) = pv.value {
                    return Ok(b);
                }
            }
            Err(Error::Runtime(e.make_type_error(
                "Boolean.prototype.valueOf called on incompatible receiver",
            )))
        }
        _ => Err(Error::Runtime(e.make_type_error(
            "Boolean.prototype.valueOf called on incompatible receiver",
        ))),
    }
}

pub(crate) fn bool_to_string(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    let b = require_boolean_this(e, this)?;
    Ok(Value::String(Rc::from(if b { "true" } else { "false" })))
}

pub(crate) fn bool_value_of(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    let b = require_boolean_this(e, this)?;
    Ok(Value::Boolean(b))
}

pub(crate) fn boolean_ctor(_e: &Engine, this: &Value, a: &[Value], construct: bool) -> Result<Value, Error> {
    let b = if a.is_empty() {
        false
    } else {
        a[0].to_boolean()
    };
    if construct {
        set_primitive(this, Value::Boolean(b));
        Ok(this.clone())
    } else {
        Ok(Value::Boolean(b))
    }
}
