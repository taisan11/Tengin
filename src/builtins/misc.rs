//! Registration of miscellaneous globals that are not part of a built-in
//! prototype: `Reflect`, `Date`, `BigInt` and the lightweight `RegExp` bundle.

use alloc::rc::Rc;
use alloc::vec::Vec;
use core::cell::RefCell;

use crate::error::Error;
use crate::interpreter::Engine;
use crate::value::{ArrayData, Object, Property, Value};

use super::helpers::{named_native, proto_of, reg_ctor, set_proto_of_val};
use super::regexp::register_regexp;
// --- misc globals (Reflect, BigInt, RegExp, Date) ---

fn date_ctor(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    // The lightweight `Date` does not track time, but the argument coercion
    // must still happen: e.g. `new Date(Object(1n))` throws a `TypeError`.
    if let Some(v) = a.first() {
        if !matches!(v, Value::Undefined) {
            e.to_number_fallible(v)?;
        }
    }
    Ok(this.clone())
}

/// `Reflect` methods, minimal set used by the test262 harness helpers.
pub(crate) fn register_reflect(engine: &mut Engine) {
    let mut r = Object::with_proto(engine.object_prototype.clone());
    r.props.insert(Rc::from("ownKeys"), Property::method(named_native("ownKeys", reflect_own_keys)));
    r.props.insert(Rc::from("get"), Property::method(named_native("get", reflect_get)));
    r.props.insert(Rc::from("set"), Property::method(named_native("set", reflect_set)));
    r.props.insert(Rc::from("apply"), Property::method(named_native("apply", reflect_apply)));
    r.props.insert(Rc::from("construct"), Property::method(named_native("construct", reflect_construct)));
    r.props.insert(Rc::from("getPrototypeOf"), Property::method(named_native("getPrototypeOf", reflect_get_proto)));
    r.props.insert(Rc::from("setPrototypeOf"), Property::method(named_native("setPrototypeOf", reflect_set_proto)));
    r.props.insert(Rc::from("defineProperty"), Property::method(named_native("defineProperty", reflect_define_prop)));
    r.props.insert(Rc::from("getOwnPropertyDescriptor"), Property::method(named_native("getOwnPropertyDescriptor", reflect_gopd)));
    r.props.insert(Rc::from("isExtensible"), Property::method(named_native("isExtensible", reflect_true)));
    engine.register_global("Reflect", Value::Object(Rc::new(RefCell::new(r))));
}

fn reflect_own_keys(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let obj = a.first().cloned().unwrap_or(Value::Undefined);
    let names: Vec<Value> = e.own_property_names(&obj).into_iter().map(Value::String).collect();
    Ok(Value::Array(Rc::new(RefCell::new(ArrayData::new(names, Some(e.array_prototype.clone()))))))
}

fn reflect_get(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let obj = a.first().cloned().unwrap_or(Value::Undefined);
    let key = super::helpers::key_arg(a.get(1).unwrap_or(&Value::Undefined));
    Ok(e.get_property(&obj, key.as_ref()))
}

fn reflect_set(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let obj = a.first().cloned().unwrap_or(Value::Undefined);
    let key = super::helpers::key_arg(a.get(1).unwrap_or(&Value::Undefined));
    let val = a.get(2).cloned().unwrap_or(Value::Undefined);
    e.set_property(&obj, key.as_ref(), val)?;
    Ok(Value::Boolean(true))
}

fn reflect_apply(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let f = a.first().cloned().unwrap_or(Value::Undefined);
    let this = a.get(1).cloned().unwrap_or(Value::Undefined);
    let args = match a.get(2) {
        Some(Value::Array(arr)) => arr.borrow().elems.clone(),
        _ => Vec::new(),
    };
    e.call_value(&f, &this, &args)
}

fn reflect_construct(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let f = a.first().cloned().unwrap_or(Value::Undefined);
    let args = match a.get(1) {
        Some(Value::Array(arr)) => arr.borrow().elems.clone(),
        _ => Vec::new(),
    };
    if !e.is_constructor(&f) {
        return Err(Error::Runtime(e.make_type_error(
            "Reflect.construct: target is not a constructor",
        )));
    }
    let new_target = a.get(2).cloned().unwrap_or_else(|| f.clone());
    if !e.is_constructor(&new_target) {
        return Err(Error::Runtime(e.make_type_error(
            "Reflect.construct: newTarget is not a constructor",
        )));
    }
    e.construct_with_new_target(&f, &args, &new_target)
}

fn reflect_get_proto(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let obj = a.first().cloned().unwrap_or(Value::Undefined);
    Ok(proto_of(&obj).map(Value::Object).unwrap_or(Value::Null))
}

fn reflect_set_proto(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let obj = a.first().cloned().unwrap_or(Value::Undefined);
    let proto = match a.get(1).cloned().unwrap_or(Value::Null) {
        Value::Null => None,
        other => proto_of(&other),
    };
    set_proto_of_val(&obj, proto);
    Ok(Value::Boolean(true))
}

fn reflect_define_prop(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let obj = a.first().cloned().unwrap_or(Value::Undefined);
    let key = super::helpers::key_arg(a.get(1).unwrap_or(&Value::Undefined));
    let desc = a.get(2).cloned().unwrap_or(Value::Undefined);
    e.define_property(&obj, key.as_ref(), &desc)?;
    Ok(Value::Boolean(true))
}

fn reflect_gopd(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let obj = a.first().cloned().unwrap_or(Value::Undefined);
    let key = super::helpers::key_arg(a.get(1).unwrap_or(&Value::Undefined));
    Ok(e.get_own_descriptor(&obj, key.as_ref()))
}

fn reflect_true(_e: &Engine, _this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Boolean(true))
}

/// Install the full `RegExp` (with prototype) and lightweight `BigInt`/`Date`
/// globals.
pub(crate) fn register_misc(engine: &mut Engine) {
    register_regexp(engine);
    let proto = engine.object_prototype.clone();
    reg_ctor(engine, "Date", date_ctor, proto.clone());
    super::bigint::register_bigint(engine);
}
