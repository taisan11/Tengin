//! Shared registration helpers for installing built-in constructors, prototype
//! methods and static properties. These are used by [`super::register_builtins`]
//! and the various built-in submodules.

use alloc::rc::Rc;
use core::cell::RefCell;

use crate::interpreter::Engine;
use crate::value::{CtorRef, NativeFn, Object, Property, RealmProto, Value};
/// Fractional part of a float (no_std-compatible).
pub(super) fn fract(x: f64) -> f64 {
    x - libm::floor(x)
}

/// Create a `Value::NativeFunction` from a Rust implementation.
pub(super) fn native(f: NativeFn) -> Value {
    crate::value::native(f)
}

/// The `length` (formal parameter count) of a well-known built-in function.
///
/// The `NativeFn` signature does not carry arity, so it is supplied here.
/// Defaults to `1`; the common/important entries are listed explicitly.
pub(super) fn arity_of(name: &str) -> usize {
    match name {
        "call" | "apply" | "bind" => 1,
        "valueOf" | "toString" => 0,
        "concat" => 1,
        "push" | "unshift" => 1,
        "shifts" | "shift" | "pop" => 0,
        "indexOf" | "lastIndexOf" | "includes" | "find" | "findIndex" | "some" | "every"
        | "forEach" | "map" | "filter" | "reduce" | "at" => 1,
        "slice" | "splice" | "fill" => 2,
        "join" | "reverse" => 1,
        "charAt" | "charCodeAt" | "codePointAt" | "startsWith" | "endsWith" | "repeat"
        | "trim" | "trimStart" | "trimEnd" | "toUpperCase" | "toLowerCase"
        | "toLocaleLowerCase" | "toLocaleUpperCase" => 1,
        "substring" | "padStart" | "padEnd" => 2,
        "split" => 2,
        "replace" => 2,
        "localeCompare" | "substr" => 1,
        "isWellFormed" | "toWellFormed" => 0,
        "keys" | "values" | "entries" | "is" => 1,
        "assign" => 2,
        "create" | "defineProperty" | "getOwnPropertyDescriptor" | "getPrototypeOf"
        | "setPrototypeOf" | "has" | "ownKeys" => 2,
        "getOwnPropertyNames" | "getOwnPropertySymbols" | "getOwnPropertyDescriptors"
        | "defineProperties" | "freeze" | "seal" | "preventExtensions" | "isFrozen"
        | "isSealed" | "isExtensible" => 1,
        "isArray" | "from" | "of" => 1,
        "isNaN" | "isFinite" | "isInteger" | "isSafeInteger" | "parseInt" | "parseFloat" => 1,
        "fromCharCode" | "fromCodePoint" | "raw" => 1,
        "max" | "min" => 2,
        "stringify" | "parse" => 2,
        "toFixed" | "toExponential" | "toPrecision" | "toLocaleString" => 1,
        _ => 1,
    }
}

/// Build a native function value with spec-accurate `name` and `length`
/// properties (non-writable, non-enumerable, configurable).
pub(super) fn named_native(name: &str, f: NativeFn) -> Value {
    let v = native(f);
    if let Value::NativeFunction(nf) = &v {
        let mut b = nf.borrow_mut();
        props_on(&mut b.props, name, arity_of(name));
    }
    v
}

/// Like [`named_native`], but with an explicit `length` (for built-ins whose
/// arity differs from the generic `arity_of` table, e.g.
/// `Number.prototype.toString`).
pub(super) fn named_native_len(name: &str, f: NativeFn, len: usize) -> Value {
    let v = native(f);
    if let Value::NativeFunction(nf) = &v {
        let mut b = nf.borrow_mut();
        props_on(&mut b.props, name, len);
    }
    v
}

fn props_on(props: &mut alloc::collections::BTreeMap<Rc<str>, Property>, name: &str, len: usize) {
    props.insert(Rc::from("name"), Property::config(Value::String(Rc::from(name))));
    props.insert(Rc::from("length"), Property::config(Value::Number(len as f64)));
}

/// Register a native constructor (e.g. `Object`, `Array`) together with its
/// `.prototype` and `.name` properties, and expose it as a global.
pub(super) fn reg_ctor(engine: &mut Engine, name: &str, f: NativeFn, proto: Rc<RefCell<Object>>) -> Value {
    let v = native(f);
    if let Value::NativeFunction(nf) = &v {
        nf.borrow_mut().constructable = true;
        nf.borrow_mut().proto = Some(engine.function_prototype.clone());
        nf.borrow_mut().realm = Some(engine.realm.clone());
        nf.borrow_mut().intrinsic_proto = Some(ctor_intrinsic(name));
        nf.borrow_mut()
            .props
            .insert(Rc::from("prototype"), Property::constant(Value::Object(proto.clone())));
        nf.borrow_mut()
            .props
            .insert(Rc::from("name"), Property::config(Value::String(Rc::from(name))));
        nf.borrow_mut()
            .props
            .insert(Rc::from("length"), Property::config(Value::Number(arity_of(name) as f64)));
    }
    engine.register_global(name, v.clone());
    v
}

/// The intrinsic default prototype produced by a named constructor.
fn ctor_intrinsic(name: &str) -> RealmProto {
    match name {
        "Number" => RealmProto::Number,
        "String" => RealmProto::String,
        "Boolean" => RealmProto::Boolean,
        _ => RealmProto::Object,
    }
}

/// Install a method on a prototype object.
pub(super) fn proto_method(proto: &Rc<RefCell<Object>>, name: &str, f: NativeFn) {
    proto
        .borrow_mut()
        .props
        .insert(Rc::from(name), Property::method(named_native(name, f)));
}

/// Install a method on a prototype object with an explicit `length`.
pub(super) fn proto_method_len(proto: &Rc<RefCell<Object>>, name: &str, f: NativeFn, len: usize) {
    proto
        .borrow_mut()
        .props
        .insert(Rc::from(name), Property::method(named_native_len(name, f, len)));
}

/// Install a data property on a prototype object.
///
/// The `constructor` property is stored as an *own* non-enumerable data
/// property (`{ writable: true, enumerable: false, configurable: true }`), as
/// required by `Number.prototype.hasOwnProperty("constructor")` and by
/// `verifyProperty` checks. A weak `ctor` back-reference is also retained so the
/// constructor still resolves even if the property is later removed.
pub(super) fn proto_data(proto: &Rc<RefCell<Object>>, name: &str, val: Value) {
    if name == "constructor" {
        let ctor = match &val {
            Value::NativeFunction(nf) => Some(CtorRef::Native(alloc::rc::Rc::downgrade(nf))),
            Value::Function(f) => Some(CtorRef::Func(alloc::rc::Rc::downgrade(f))),
            _ => None,
        };
        if let Some(c) = ctor {
            proto.borrow_mut().ctor = Some(c);
        }
        proto
            .borrow_mut()
            .props
            .insert(Rc::from(name), Property::method(val));
        return;
    }
    proto
        .borrow_mut()
        .props
        .insert(Rc::from(name), Property::new(val));
}

pub(super) fn engine_global(engine: &Engine, name: &str) -> Value {
    engine
        .global_env
        .borrow()
        .get(name)
        .unwrap_or(Value::Undefined)
}

pub(super) fn set_static(engine: &Engine, ctor: &Value, items: &[(&str, NativeFn)]) {
    if let Value::NativeFunction(nf) = ctor {
        for (n, f) in items {
            nf.borrow_mut()
                .props
                .insert(Rc::from(*n), Property::method(named_native(n, *f)));
        }
    }
    let _ = engine;
}

pub(super) fn set_static_value(ctor: &Value, items: &[(&str, Value)]) {
    if let Value::NativeFunction(nf) = ctor {
        for (n, v) in items {
            let p = match &v {
                Value::NativeFunction(_) => {
                    if let Value::NativeFunction(vnf) = &v {
                        props_on(&mut vnf.borrow_mut().props, n, arity_of(n));
                    }
                    Property::method(v.clone())
                }
                _ => Property::constant(v.clone()),
            };
            nf.borrow_mut().props.insert(Rc::from(*n), p);
        }
    }
}

/// Register a native constructor for `Error` and its subclasses.
pub(super) fn reg_error(engine: &mut Engine, name: &str, f: NativeFn, proto: Rc<RefCell<Object>>) -> Value {
    let v = native(f);
    if let Value::NativeFunction(nf) = &v {
        nf.borrow_mut().constructable = true;
        nf.borrow_mut().proto = Some(engine.function_prototype.clone());
        nf.borrow_mut().realm = Some(engine.realm.clone());
        nf.borrow_mut().intrinsic_proto = Some(RealmProto::Object);
        nf.borrow_mut()
            .props
            .insert(Rc::from("prototype"), Property::constant(Value::Object(proto.clone())));
        nf.borrow_mut()
            .props
            .insert(Rc::from("name"), Property::config(Value::String(Rc::from(name))));
        nf.borrow_mut()
            .props
            .insert(Rc::from("length"), Property::config(Value::Number(arity_of(name) as f64)));
    }
    engine.register_global(name, v.clone());
    v
}

/// Register a subclass of `Error` with its own prototype linked to `parent`.
pub(super) fn reg_error_sub(
    engine: &mut Engine,
    name: &str,
    f: NativeFn,
    parent: Rc<RefCell<Object>>,
) -> Value {
    let proto = Rc::new(RefCell::new(Object::with_proto(parent)));
    let ctor = reg_error(engine, name, f, proto.clone());
    proto_data(&proto, "constructor", ctor.clone());
    ctor
}

/// Wrap a primitive value into its corresponding wrapper object.
pub(super) fn wrap(engine: &Engine, primitive: Value) -> Value {
    let proto = match &primitive {
        Value::String(_) => engine.string_prototype.clone(),
        Value::Number(_) => engine.number_prototype.clone(),
        Value::Boolean(_) => engine.boolean_prototype.clone(),
        _ => return primitive,
    };
    let o = Rc::new(RefCell::new(Object::with_proto(proto)));
    o.borrow_mut()
        .props
        .insert(Rc::from("__value__"), Property::new(primitive));
    Value::Object(o)
}

/// Store the boxed primitive value on an existing wrapper instance.
///
/// `new Boolean(v)` / `new Number(v)` / `new String(v)` must return the *same*
/// instance that was allocated with the correct prototype (possibly from another
/// realm via `Reflect.construct`), so the primitive is written into its
/// `__value__` slot rather than allocating a fresh wrapper.
pub(super) fn set_primitive(this: &Value, primitive: Value) {
    if let Value::Object(o) = this {
        o.borrow_mut()
            .props
            .insert(Rc::from("__value__"), Property::new(primitive));
    }
}

/// Extract the `[[Prototype]]` of any value (or `None` for primitives).
pub(super) fn proto_of(v: &Value) -> Option<Rc<RefCell<Object>>> {
    match v {
        Value::Object(o) => o.borrow().proto.clone(),
        Value::Array(a) => a.borrow().proto.clone(),
        Value::Function(f) => f.borrow().proto.clone(),
        Value::NativeFunction(nf) => nf.borrow().proto.clone(),
        _ => None,
    }
}

/// Read the `.prototype` property of a constructor value.
pub(super) fn prototype_of(ctor: &Value) -> Option<Rc<RefCell<Object>>> {
    match ctor {
        Value::Function(f) => match f.borrow().props.get("prototype") {
            Some(p) => match &p.value {
                Value::Object(o) => Some(o.clone()),
                _ => None,
            },
            None => None,
        },
        Value::NativeFunction(nf) => match nf.borrow().props.get("prototype") {
            Some(p) => match &p.value {
                Value::Object(o) => Some(o.clone()),
                _ => None,
            },
            None => None,
        },
        _ => None,
    }
}

/// Set the `[[Prototype]]` of any object-like value.
pub(super) fn set_proto_of_val(v: &Value, p: Option<Rc<RefCell<Object>>>) {
    match v {
        Value::Object(o) => o.borrow_mut().proto = p,
        Value::Array(a) => a.borrow_mut().proto = p,
        Value::Function(f) => f.borrow_mut().proto = p,
        Value::NativeFunction(nf) => nf.borrow_mut().proto = p,
        _ => {}
    }
}
