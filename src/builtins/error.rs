use alloc::format;
use alloc::rc::Rc;

use crate::error::Error;
use crate::interpreter::Engine;
use crate::value::Value;

pub(crate) fn err_to_string(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    let name = e.get_property(this, "name");
    let message = e.get_property(this, "message");
    let name = if name == Value::Undefined { Value::String(Rc::from("Error")) } else { name };
    if message == Value::Undefined || message.to_string().is_empty() {
        Ok(name)
    } else {
        Ok(Value::String(Rc::from(format!("{}: {}", name.to_string(), message.to_string()))))
    }
}

pub(crate) fn error_ctor(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let msg = a.first().cloned().unwrap_or(Value::Undefined);
    e.set_property(this, "message", msg)?;
    if e.get_property(this, "name") == Value::Undefined {
        e.set_property(this, "name", Value::String(Rc::from("Error")))?;
    }
    Ok(this.clone())
}
