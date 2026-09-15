//! `Promise` constructor, prototype methods and statics, plus
//! `queueMicrotask` and a minimal `AggregateError` (for `Promise.any`).

use alloc::rc::Rc;
use alloc::vec::Vec;
use core::cell::RefCell;

use crate::error::Error;
use crate::interpreter::Engine;
use crate::value::{
    Object, PromiseData, PromiseReaction, PromiseReactionKind, Property, Value,
};

use super::helpers::{
    engine_global, named_native, named_native_len, proto_data, proto_method, proto_method_len,
    reg_ctor,
};
use crate::interpreter::PromiseCapability;

/// The default constructor for species/static capability creation.
fn default_ctor(e: &Engine) -> Value {
    engine_global(e, "Promise")
}

/// Whether `v` is the engine's built-in `%Promise%` constructor.
fn is_default_ctor(v: &Value, def: &Value) -> bool {
    match (v, def) {
        (Value::NativeFunction(a), Value::NativeFunction(b)) => Rc::ptr_eq(a, b),
        (Value::Function(a), Value::Function(b)) => Rc::ptr_eq(a, b),
        _ => false,
    }
}

/// `new Promise(executor)`, honoring `newTarget.prototype` for subclasses.
fn promise_ctor(e: &Engine, _this: &Value, a: &[Value], construct: bool) -> Result<Value, Error> {
    if !construct {
        return Err(Error::Runtime(
            e.make_type_error("Promise constructor must be called with new"),
        ));
    }
    let executor = a.first().cloned().unwrap_or(Value::Undefined);
    if !matches!(executor, Value::Function(_) | Value::NativeFunction(_)) {
        return Err(Error::Runtime(
            e.make_type_error("Promise executor is not a function"),
        ));
    }
    let proto = match e.active_new_target.borrow().clone() {
        Some(nt) => e.promise_proto_from_new_target(&nt)?,
        None => e.promise_prototype.clone(),
    };
    let cap = e.new_promise_with_proto(Some(proto));
    let out = Value::Promise(cap.clone());
    let (resolve, reject) = e.make_resolver_pair(&cap);
    match e.call_function(&executor, &Value::Undefined, &[resolve, reject]) {
        Ok(_) => {}
        Err(Error::Runtime(v)) => e.reject_promise(&cap, v),
        Err(_) => e.reject_promise(&cap, e.make_type_error("Promise executor failed")),
    }
    Ok(out)
}

fn callable_or_none(v: &Value) -> Option<Value> {
    match v {
        Value::Function(_) | Value::NativeFunction(_) => Some(v.clone()),
        _ => None,
    }
}

/// `Promise.prototype.then(onFulfilled, onRejected)`, honoring
/// `Symbol.species` for the derived promise.
fn promise_then(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let on_f = a.first().map(callable_or_none).unwrap_or(None);
    let on_r = a.get(1).map(callable_or_none).unwrap_or(None);
    let def = default_ctor(e);
    let ctor = e.species_constructor(this, &def)?;
    if is_default_ctor(&ctor, &def) {
        return Ok(e.perform_then(this, on_f, on_r));
    }
    let cap = e.promise_capability_new(&ctor)?;
    let dummy = e.new_promise();
    e.attach_reaction(
        this,
        PromiseReaction {
            kind: PromiseReactionKind::Custom {
                promise: cap.promise.clone(),
                resolve: cap.resolve.clone(),
                reject: cap.reject.clone(),
            },
            on_fulfilled: on_f,
            on_rejected: on_r,
            promise: dummy,
        },
    );
    Ok(cap.promise)
}

/// `Promise.prototype.catch(onRejected)`.
fn promise_catch(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let on_r = a.first().map(callable_or_none).unwrap_or(None);
    let def = default_ctor(e);
    let ctor = e.species_constructor(this, &def)?;
    if is_default_ctor(&ctor, &def) {
        return Ok(e.perform_then(this, None, on_r));
    }
    let cap = e.promise_capability_new(&ctor)?;
    let dummy = e.new_promise();
    e.attach_reaction(
        this,
        PromiseReaction {
            kind: PromiseReactionKind::Custom {
                promise: cap.promise.clone(),
                resolve: cap.resolve.clone(),
                reject: cap.reject.clone(),
            },
            on_fulfilled: None,
            on_rejected: on_r,
            promise: dummy,
        },
    );
    Ok(cap.promise)
}

/// `Promise.prototype.finally(onFinally)`.
fn promise_finally(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let cb = a.first().cloned().unwrap_or(Value::Undefined);
    let def = default_ctor(e);
    let ctor = e.species_constructor(this, &def)?;
    if is_default_ctor(&ctor, &def) {
        if !matches!(cb, Value::Function(_) | Value::NativeFunction(_)) {
            // Non-callable `finally` behaves like `then` (pass-through).
            return Ok(e.perform_then(this, None, None));
        }
        let cap = e.new_promise();
        let out = Value::Promise(cap.clone());
        e.attach_reaction(
            this,
            PromiseReaction {
                kind: PromiseReactionKind::FinallyCall { cb },
                on_fulfilled: None,
                on_rejected: None,
                promise: cap,
            },
        );
        return Ok(out);
    }
    // Custom constructor: run the internal `finally` machinery on a raw
    // promise, then forward into the custom capability.
    let cap = e.promise_capability_new(&ctor)?;
    if !matches!(cb, Value::Function(_) | Value::NativeFunction(_)) {
        let dummy = e.new_promise();
        e.attach_reaction(
            this,
            PromiseReaction {
                kind: PromiseReactionKind::Custom {
                    promise: cap.promise.clone(),
                    resolve: cap.resolve.clone(),
                    reject: cap.reject.clone(),
                },
                on_fulfilled: None,
                on_rejected: None,
                promise: dummy,
            },
        );
        return Ok(cap.promise);
    }
    let raw = e.new_promise();
    e.attach_reaction(
        this,
        PromiseReaction {
            kind: PromiseReactionKind::FinallyCall { cb },
            on_fulfilled: None,
            on_rejected: None,
            promise: raw.clone(),
        },
    );
    e.forward_raw_to_custom(&raw, &cap);
    Ok(cap.promise)
}

/// `Promise.resolve(x)`: same-constructor promises pass through, otherwise a
/// capability of `this` assimilates `x`.
fn promise_resolve(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let x = a.first().cloned().unwrap_or(Value::Undefined);
    if matches!(x, Value::Promise(_)) {
        // `Get(promise, "constructor")` then SameValue with `this`.
        let ctor_x = match e.read_property_callable(&x, "constructor")? {
            None | Some(Value::Undefined) => default_ctor(e),
            Some(c) => c,
        };
        if Value::strict_eq(&ctor_x, this) {
            return Ok(x);
        }
    }
    let def = default_ctor(e);
    if is_default_ctor(this, &def) {
        let cap = e.new_promise();
        let out = Value::Promise(cap.clone());
        e.resolve_value(&cap, x);
        return Ok(out);
    }
    let cap = e.promise_capability_new(this)?;
    let raw = e.new_promise();
    e.resolve_value(&raw, x);
    e.forward_raw_to_custom(&raw, &cap);
    Ok(cap.promise)
}

/// `Promise.reject(reason)` on a capability of `this`.
fn promise_reject(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let reason = a.first().cloned().unwrap_or(Value::Undefined);
    let def = default_ctor(e);
    if is_default_ctor(this, &def) {
        let cap = e.new_promise();
        let out = Value::Promise(cap.clone());
        e.reject_promise(&cap, reason);
        return Ok(out);
    }
    let cap = e.promise_capability_new(this)?;
    e.call_function(&cap.reject, &Value::Undefined, &[reason])?;
    Ok(cap.promise)
}

/// Resolve `this` as a static-method capability constructor: the built-in
/// `%Promise%` takes the raw fast path, anything else goes through
/// `NewPromiseCapability`. Returns `(raw, custom, return_promise)`.
fn static_capability(
    e: &Engine,
    this: &Value,
) -> Result<(Rc<RefCell<PromiseData>>, Option<PromiseCapability>, Value), Error> {
    let def = default_ctor(e);
    if is_default_ctor(this, &def) {
        let raw = e.new_promise();
        let out = Value::Promise(raw.clone());
        return Ok((raw, None, out));
    }
    if !Engine::is_object_value(this) {
        return Err(Error::Runtime(
            e.make_type_error("promise combinator called on non-object"),
        ));
    }
    let cap = e.promise_capability_new(this)?;
    let raw = e.new_promise();
    let out = cap.promise.clone();
    Ok((raw, Some(cap), out))
}

/// Settle a static combinator's failure path.
fn static_rejected(
    e: &Engine,
    raw: &Rc<RefCell<PromiseData>>,
    custom: &Option<PromiseCapability>,
    ret: Value,
    reason: Value,
) -> Value {
    match custom {
        Some(cap) => {
            let _ = e.call_function(&cap.reject, &Value::Undefined, &[reason]);
            ret
        }
        None => {
            e.reject_promise(raw, reason);
            ret
        }
    }
}

/// Finish a static combinator: forward a custom capability, return `ret`.
fn static_finish(
    e: &Engine,
    raw: &Rc<RefCell<PromiseData>>,
    custom: &Option<PromiseCapability>,
    ret: Value,
) -> Value {
    if let Some(cap) = custom {
        e.forward_raw_to_custom(raw, cap);
    }
    ret
}

/// Collect an iterable argument into values, or `None` when not iterable.
fn iterable_arg(e: &Engine, a: &[Value]) -> Result<Vec<Value>, Value> {
    let items = a.first().cloned().unwrap_or(Value::Undefined);
    match e.iterable_values(&items) {
        Ok(v) => Ok(v),
        Err(Error::Runtime(v)) => Err(v),
        Err(_) => Err(e.make_type_error("promise combinator failed")),
    }
}

/// `Promise.all(iterable)`.
fn promise_all(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let (raw, custom, ret) = static_capability(e, this)?;
    let items = match iterable_arg(e, a) {
        Ok(v) => v,
        Err(reason) => return Ok(static_rejected(e, &raw, &custom, ret, reason)),
    };
    if items.is_empty() {
        e.fulfill_promise(&raw, Value::Array(e.new_array(Vec::new())));
        return Ok(static_finish(e, &raw, &custom, ret));
    }
    let shared = e.new_all_state(items.len());
    for (i, item) in items.into_iter().enumerate() {
        e.attach_reaction(
            &item,
            PromiseReaction {
                kind: PromiseReactionKind::AllElement {
                    index: i,
                    shared: shared.clone(),
                },
                on_fulfilled: None,
                on_rejected: None,
                promise: raw.clone(),
            },
        );
    }
    Ok(static_finish(e, &raw, &custom, ret))
}

/// `Promise.race(iterable)`.
fn promise_race(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let (raw, custom, ret) = static_capability(e, this)?;
    let items = match iterable_arg(e, a) {
        Ok(v) => v,
        Err(reason) => return Ok(static_rejected(e, &raw, &custom, ret, reason)),
    };
    for item in items {
        e.attach_reaction(
            &item,
            PromiseReaction {
                kind: PromiseReactionKind::Then,
                on_fulfilled: None,
                on_rejected: None,
                promise: raw.clone(),
            },
        );
    }
    Ok(static_finish(e, &raw, &custom, ret))
}

/// `Promise.allSettled(iterable)`.
fn promise_all_settled(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let (raw, custom, ret) = static_capability(e, this)?;
    let items = match iterable_arg(e, a) {
        Ok(v) => v,
        Err(reason) => return Ok(static_rejected(e, &raw, &custom, ret, reason)),
    };
    if items.is_empty() {
        e.fulfill_promise(&raw, Value::Array(e.new_array(Vec::new())));
        return Ok(static_finish(e, &raw, &custom, ret));
    }
    let shared = e.new_all_settled_state(items.len());
    for (i, item) in items.into_iter().enumerate() {
        e.attach_reaction(
            &item,
            PromiseReaction {
                kind: PromiseReactionKind::AllSettled {
                    index: i,
                    shared: shared.clone(),
                },
                on_fulfilled: None,
                on_rejected: None,
                promise: raw.clone(),
            },
        );
    }
    Ok(static_finish(e, &raw, &custom, ret))
}

/// `Promise.any(iterable)`: first fulfillment wins; rejection collects every
/// reason and rejects with an `AggregateError` once all settle.
fn promise_any(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let (raw, custom, ret) = static_capability(e, this)?;
    let items = match iterable_arg(e, a) {
        Ok(v) => v,
        Err(reason) => return Ok(static_rejected(e, &raw, &custom, ret, reason)),
    };
    if items.is_empty() {
        let agg = e.make_aggregate_error(Vec::new(), "All promises were rejected");
        e.reject_promise(&raw, agg);
        return Ok(static_finish(e, &raw, &custom, ret));
    }
    // Shared countdown holder (a plain object so every per-element handler
    // observes the same state through its marker props).
    let holder = e.new_object();
    holder.borrow_mut().props.insert(
        Rc::from("remaining"),
        Property::new(Value::Number(items.len() as f64)),
    );
    holder.borrow_mut().props.insert(
        Rc::from("errors"),
        Property::new(Value::Array(e.new_array(
            (0..items.len()).map(|_| Value::Undefined).collect(),
        ))),
    );
    for (i, item) in items.into_iter().enumerate() {
        e.attach_reaction(
            &item,
            PromiseReaction {
                kind: PromiseReactionKind::Any {
                    holder: Value::Object(holder.clone()),
                    index: i,
                },
                on_fulfilled: None,
                on_rejected: None,
                promise: raw.clone(),
            },
        );
    }
    Ok(static_finish(e, &raw, &custom, ret))
}

/// `Promise.withResolvers()` on a capability of `this`.
fn promise_with_resolvers(
    e: &Engine,
    this: &Value,
    _a: &[Value],
    _c: bool,
) -> Result<Value, Error> {
    let def = default_ctor(e);
    let (promise, resolve, reject) = if is_default_ctor(this, &def) {
        let cap = e.new_promise();
        let (resolve, reject) = e.make_resolver_pair(&cap);
        (Value::Promise(cap), resolve, reject)
    } else {
        let cap = e.promise_capability_new(this)?;
        (cap.promise, cap.resolve, cap.reject)
    };
    let o = e.new_object();
    {
        let mut b = o.borrow_mut();
        b.props.insert(Rc::from("promise"), Property::new(promise));
        b.props.insert(Rc::from("resolve"), Property::new(resolve));
        b.props.insert(Rc::from("reject"), Property::new(reject));
    }
    Ok(Value::Object(o))
}

/// `queueMicrotask(fn)`.
fn queue_microtask(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let cb = a.first().cloned().unwrap_or(Value::Undefined);
    if !matches!(cb, Value::Function(_) | Value::NativeFunction(_)) {
        return Err(Error::Runtime(
            e.make_type_error("queueMicrotask argument is not a function"),
        ));
    }
    e.enqueue_microtask(cb);
    Ok(Value::Undefined)
}

/// `get Promise[Symbol.species]` — returns the constructor itself.
fn promise_species_get(
    _e: &Engine,
    this: &Value,
    _a: &[Value],
    _c: bool,
) -> Result<Value, Error> {
    Ok(this.clone())
}

impl Engine {
    /// Build an `AggregateError` instance holding `errors` with `message`.
    pub(crate) fn make_aggregate_error(&self, errors: Vec<Value>, message: &str) -> Value {
        let ctor = engine_global(self, "AggregateError");
        let arr = Value::Array(self.new_array(errors));
        let msg = Value::String(Rc::from(message));
        match self.construct_with_new_target(&ctor, &[arr, msg], &ctor) {
            Ok(v) => v,
            Err(_) => self.make_type_error("AggregateError construction failed"),
        }
    }

    /// Enqueue a `queueMicrotask` callback.
    pub(crate) fn enqueue_microtask(&self, cb: Value) {
        self.job_queue
            .borrow_mut()
            .push_back(crate::interpreter::Job::Microtask { callback: cb });
    }
}

/// Minimal `AggregateError(errors, message)` constructor.
fn aggregate_error_ctor(
    e: &Engine,
    this: &Value,
    a: &[Value],
    construct: bool,
) -> Result<Value, Error> {
    let target = if construct && Engine::is_object_value(this) {
        this.clone()
    } else {
        let ctor = e.active_native_ctor.borrow().clone();
        let proto = ctor
            .as_ref()
            .and_then(|c| e.proto_of(c))
            .unwrap_or_else(|| e.error_prototype.clone());
        Value::Object(e.make_object(proto))
    };
    if let Value::Object(o) = &target {
        o.borrow_mut().props.insert(
            Rc::from("__error_data__"),
            Property::new(Value::Boolean(true)),
        );
    }
    let errors = a.first().cloned().unwrap_or(Value::Undefined);
    let errs: Vec<Value> = match e.iterable_values(&errors) {
        Ok(v) => v,
        Err(_) => Vec::new(),
    };
    if let Value::Object(o) = &target {
        let mut b = o.borrow_mut();
        b.props.insert(
            Rc::from("name"),
            Property::data(Value::String(Rc::from("AggregateError")), true, false, true),
        );
        let msg = a.get(1).cloned().unwrap_or(Value::Undefined);
        if !matches!(msg, Value::Undefined) {
            if let Ok(s) = e.to_string_fallible(&msg) {
                b.props.insert(
                    Rc::from("message"),
                    Property::data(Value::String(s), true, false, true),
                );
            }
        } else {
            b.props.insert(
                Rc::from("message"),
                Property::data(Value::String(Rc::from("")), true, false, true),
            );
        }
        b.props.insert(
            Rc::from("errors"),
            Property::data(Value::Array(e.new_array(errs)), true, false, true),
        );
    }
    Ok(target)
}

/// Install `Promise`, `queueMicrotask` and `AggregateError` on the engine.
pub(crate) fn register_promise(engine: &mut Engine) {
    let proto = engine.promise_prototype.clone();
    proto_method_len(&proto, "then", promise_then, 2);
    proto_method(&proto, "catch", promise_catch);
    proto_method(&proto, "finally", promise_finally);
    // `Promise.prototype[Symbol.toStringTag]` = "Promise".
    proto.borrow_mut().props.insert(
        Rc::from(crate::value::SymbolData::well_known_key("toStringTag").as_ref()),
        Property::config(Value::String(Rc::from("Promise"))),
    );

    let ctor = reg_ctor(engine, "Promise", promise_ctor, proto.clone());
    proto_data(&proto, "constructor", ctor.clone());
    super::helpers::set_static(
        engine,
        &ctor,
        &[
            ("resolve", promise_resolve),
            ("reject", promise_reject),
            ("all", promise_all),
            ("race", promise_race),
            ("any", promise_any),
        ],
    );
    super::helpers::set_static_value(
        &ctor,
        &[
            ("allSettled", named_native("allSettled", promise_all_settled)),
            (
                "withResolvers",
                named_native_len("withResolvers", promise_with_resolvers, 0),
            ),
        ],
    );
    // `get Promise[Symbol.species]`.
    if let Value::NativeFunction(nf) = &ctor {
        let mut getter =
            crate::value::NativeFunctionData::new(promise_species_get);
        getter.props.insert(
            Rc::from("name"),
            Property::config(Value::String(Rc::from("get [Symbol.species]"))),
        );
        getter
            .props
            .insert(Rc::from("length"), Property::config(Value::Number(0.0)));
        let getter = Value::NativeFunction(Rc::new(RefCell::new(getter)));
        nf.borrow_mut().props.insert(
            Rc::from(crate::value::SymbolData::well_known_key("species").as_ref()),
            Property::accessor(Some(getter), None),
        );
    }

    engine.register_global("queueMicrotask", named_native("queueMicrotask", queue_microtask));

    // Minimal `AggregateError` (prototype chained to `Error.prototype`).
    let agg_proto =
        Rc::new(RefCell::new(Object::with_proto(engine.error_prototype.clone())));
    agg_proto.borrow_mut().props.insert(
        Rc::from("name"),
        Property::data(Value::String(Rc::from("AggregateError")), true, false, true),
    );
    agg_proto.borrow_mut().props.insert(
        Rc::from("message"),
        Property::data(Value::String(Rc::from("")), true, false, true),
    );
    let agg_ctor = reg_ctor(engine, "AggregateError", aggregate_error_ctor, agg_proto.clone());
    proto_data(&agg_proto, "constructor", agg_ctor.clone());
    if let Value::NativeFunction(nf) = &agg_ctor {
        nf.borrow_mut().fn_value_proto = Some(engine_global(engine, "Error"));
    }
}
