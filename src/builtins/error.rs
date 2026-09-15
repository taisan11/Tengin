//! `Error.prototype.toString` and the shared implementation behind the `Error`
//! and NativeError constructors (`message` handling, `options`/`cause`).

use alloc::format;
use alloc::rc::Rc;

use crate::error::Error;
use crate::interpreter::Engine;
use crate::value::{Property, Value};

/// `Error.prototype.toString`:
/// `name`/`message` are read through `Get` (with defaults `"Error"`/`""`)
/// and stringified with `ToString` (which can run user code and throw).
pub(crate) fn err_to_string(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    if !Engine::is_object_value(this) {
        return Err(Error::Runtime(e.make_type_error(
            "Error.prototype.toString called on a non-object",
        )));
    }
    let mut name = e.get_property(this, "name");
    if matches!(name, Value::Undefined) {
        name = Value::String(Rc::from("Error"));
    }
    let mut msg = e.get_property(this, "message");
    if matches!(msg, Value::Undefined) {
        msg = Value::String(Rc::from(""));
    }
    let name_s = e.to_string_fallible(&name)?;
    let msg_s = e.to_string_fallible(&msg)?;
    let s = if name_s.is_empty() {
        msg_s.to_string()
    } else if msg_s.is_empty() {
        name_s.to_string()
    } else {
        format!("{}: {}", name_s, msg_s)
    };
    Ok(Value::String(Rc::from(s.as_str())))
}

/// The body of `Error(message, options)` and every
/// `NativeError(message, options)` constructor.
pub(crate) fn error_ctor(e: &Engine, this: &Value, a: &[Value], construct: bool) -> Result<Value, Error> {
    // The active constructor identifies which prototype a plain (non-`new`)
    // call must use; `new` calls already carry the correct instance in `this`
    // (allocated with the new-target's prototype by `construct`, including
    // subclass `super()` chains).
    let (target, fresh) = if construct && Engine::is_object_value(this) {
        (this.clone(), false)
    } else {
        let ctor = e.active_native_ctor.borrow().clone();
        let proto = ctor.as_ref().and_then(|c| e.proto_of(c));
        let proto = proto.unwrap_or_else(|| e.error_prototype.clone());
        (Value::Object(e.make_object(proto)), true)
    };

    // Tag the instance with the `[[ErrorData]]` internal slot (prototypes and
    // ordinary objects don't carry it).
    if let Value::Object(o) = &target {
        o.borrow_mut().props.insert(
            Rc::from("__error_data__"),
            Property::new(Value::Boolean(true)),
        );
    }
    let _ = fresh;

    let msg = a.first().cloned().unwrap_or(Value::Undefined);
    if !matches!(msg, Value::Undefined) {
        let s = e.to_string_fallible(&msg)?;
        define_own_non_enumerable(&target, "message", Value::String(s));
    }
    let options = a.get(1).cloned().unwrap_or(Value::Undefined);
    if Engine::is_object_value(&options) {
        // InstallErrorCause: only when `options` has a `cause` property.
        if crate::interpreter::ops::has_property("cause", &options) {
            let cause = e.get_property(&options, "cause");
            define_own_non_enumerable(&target, "cause", cause);
        }
    }
    Ok(target)
}

/// `CreateNonEnumerableDataPropertyOrThrow(O, key, value)`.
fn define_own_non_enumerable(target: &Value, key: &str, value: Value) {
    if let Value::Object(o) = target {
        o.borrow_mut().props.insert(
            Rc::from(key),
            Property::data(value, true, false, true),
        );
    }
}
