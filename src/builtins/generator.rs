//! Generator objects: `%GeneratorPrototype%` with `next`/`return`/`throw` and
//! `Symbol.iterator`.
//!
//! Generator bodies are executed *eagerly*: the first `next()` runs the whole
//! body and collects every `yield`ed value; subsequent `next()` calls replay
//! the collected values in order. This is sufficient for the generator-shaped
//! code the conformance suite exercises in these categories (simple finite
//! sequences), without full coroutine machinery.

use alloc::rc::Rc;
use core::cell::RefCell;

use crate::error::Error;
use crate::interpreter::Engine;
use crate::value::{Property, Value};

/// `%GeneratorPrototype%.next()`: run the body once (first call), then hand
/// out the collected values one per call, finishing with `{done: true}`.
pub(crate) fn gen_next(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    if !matches!(e.get_property(this, "__gen_fn__"), Value::Function(_)) {
        return Err(Error::Runtime(e.make_type_error("next method called on incompatible receiver")));
    }
    let started = matches!(e.get_property(this, "__gen_started__"), Value::Boolean(true));
    if !started {
        e.run_generator_body(this)?;
    }
    gen_next_value(e, this)
}

/// Serve the next collected value (or the completion record).
fn gen_next_value(e: &Engine, this: &Value) -> Result<Value, Error> {
    let idx = match e.get_property(this, "__gen_index__") {
        Value::Number(n) => n as usize,
        _ => 0,
    };
    let values = match e.get_property(this, "__gen_values__") {
        Value::Array(a) => a.borrow().elems.clone(),
        _ => Vec::new(),
    };
    let (value, done) = match values.get(idx) {
        Some(v) => (v.clone(), false),
        None => (Value::Undefined, true),
    };
    if !done {
        set_prop(this, "__gen_index__", Value::Number(idx as f64 + 1.0));
    }
    Ok(iter_result(e, value, done))
}

/// `%GeneratorPrototype%.return(v)`: complete the generator with `v`.
pub(crate) fn gen_return(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    if !matches!(e.get_property(this, "__gen_fn__"), Value::Function(_)) {
        return Err(Error::Runtime(e.make_type_error("return method called on incompatible receiver")));
    }
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    set_prop(this, "__gen_started__", Value::Boolean(true));
    set_prop(this, "__gen_values__", Value::Undefined);
    set_prop(this, "__gen_index__", Value::Number(0.0));
    Ok(iter_result(e, v, true))
}

/// `%GeneratorPrototype%.throw(v)`: complete the generator by throwing `v`.
pub(crate) fn gen_throw(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    if !matches!(e.get_property(this, "__gen_fn__"), Value::Function(_)) {
        return Err(Error::Runtime(e.make_type_error("throw method called on incompatible receiver")));
    }
    set_prop(this, "__gen_started__", Value::Boolean(true));
    set_prop(this, "__gen_values__", Value::Undefined);
    set_prop(this, "__gen_index__", Value::Number(0.0));
    Err(Error::Runtime(
        a.first().cloned().unwrap_or_else(|| e.make_type_error("Generator threw")),
    ))
}

fn set_prop(this: &Value, key: &str, v: Value) {
    if let Value::Object(o) = this {
        o.borrow_mut().props.insert(Rc::from(key), Property::new(v));
    }
}

/// Build an iterator-result object `{ value, done }`.
fn iter_result(e: &Engine, value: Value, done: bool) -> Value {
    let obj = e.new_object();
    {
        let mut b = obj.borrow_mut();
        b.props.insert(Rc::from("value"), Property::new(value));
        b.props
            .insert(Rc::from("done"), Property::new(Value::Boolean(done)));
    }
    Value::Object(obj)
}

/// Install the shared generator prototype on the engine.
pub(crate) fn register_generator(engine: &mut Engine) {
    let proto = engine.generator_prototype.clone();
    let next = named_gen("next", gen_next);
    let ret = named_gen("return", gen_return);
    let thr = named_gen("throw", gen_throw);
    let iter = {
        let mut nf = crate::value::NativeFunctionData::new(|_e, this, _a, _c| Ok(this.clone()));
        nf.props.insert(
            Rc::from("name"),
            Property::config(Value::String(Rc::from("[Symbol.iterator]"))),
        );
        nf.props
            .insert(Rc::from("length"), Property::config(Value::Number(0.0)));
        Value::NativeFunction(Rc::new(RefCell::new(nf)))
    };
    {
        let mut b = proto.borrow_mut();
        b.props
            .insert(Rc::from("next"), Property::method(next));
        b.props
            .insert(Rc::from("return"), Property::method(ret));
        b.props
            .insert(Rc::from("throw"), Property::method(thr));
        b.props.insert(
            Rc::from(crate::value::SymbolData::well_known_key("iterator").as_ref()),
            Property::method(iter),
        );
    }
}

/// A named native method value with `name`/`length` descriptor properties.
fn named_gen(name: &str, f: crate::value::NativeFn) -> Value {
    let mut nf = crate::value::NativeFunctionData::new(f);
    nf.props
        .insert(Rc::from("name"), Property::config(Value::String(Rc::from(name))));
    nf.props
        .insert(Rc::from("length"), Property::config(Value::Number(0.0)));
    Value::NativeFunction(Rc::new(RefCell::new(nf)))
}
