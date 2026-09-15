//! The JavaScript object model: property lookup, mutation, descriptors and
//! the associated coercion helpers that operate on the interpreter's heap
//! values. All functions are [`crate::interpreter::Engine`] methods so they
//! can share the engine's prototype slots and allocator.

use alloc::collections::BTreeSet;
use alloc::rc::Rc;
use alloc::string::ToString;
use alloc::vec::Vec;
use core::cell::RefCell;

use crate::error::{Error, Result};
use crate::value::{ArrayValueIter, CtorRef, Object, Property, SymbolData, Value};

use super::ops::symbol_iterator_id;
use super::Engine;

/// The `ToPrimitive` preferred-type hint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrimitiveHint {
    /// No preference (binary `+`, template literals, `==`).
    Default,
    /// Numeric contexts (`ToNumber` and friends).
    Number,
    /// String contexts (`ToString`, computed property keys).
    String,
}

impl Engine {
    /// Return a lazy iterator for an array's indexed values. This is used by
    /// streaming consumers so a sparse array with a huge logical length does
    /// not first materialize every hole in memory.
    pub(crate) fn array_values_iter(&self, array: Rc<RefCell<crate::value::ArrayData>>) -> ArrayValueIter {
        ArrayValueIter::new(array)
    }

    /// Upgrade a weak constructor back-reference into a usable value.
    fn resolve_ctor(ctor: &Option<CtorRef>) -> Option<Value> {
        match ctor {
            Some(CtorRef::Func(w)) => w.upgrade().map(Value::Function),
            Some(CtorRef::Native(w)) => w.upgrade().map(Value::NativeFunction),
            None => None,
        }
    }

    /// Resolve the prototype object of a constructor value (used by `instanceof`
    /// and class heritage).
    pub(crate) fn proto_of(&self, ctor: &Value) -> Option<Rc<RefCell<Object>>> {
        match ctor {
            Value::Function(f) => f
                .borrow()
                .props
                .get("prototype")
                .and_then(|p| {
                    if let Value::Object(o) = &p.value {
                        Some(o.clone())
                    } else {
                        None
                    }
                }),
            Value::NativeFunction(nf) => nf
                .borrow()
                .props
                .get("prototype")
                .and_then(|p| {
                    if let Value::Object(o) = &p.value {
                        Some(o.clone())
                    } else {
                        None
                    }
                })
                .or_else(|| nf.borrow().proto.clone()),
            _ => None,
        }
    }

    /// Resolve a property on `base`, following the prototype chain. Primitives are
    /// auto-boxed to their corresponding prototype objects for the lookup.
    pub(crate) fn get_property(&self, base: &Value, key: &str) -> Value {
        match base {
            Value::Object(o) => {
                if let Some(pv) = o.borrow().props.get("__value__") {
                    if key == "length" {
                        return Value::Number(pv.value.to_string().len() as f64);
                    }
                }
                let mut cur = Some(o.clone());
                while let Some(c) = cur {
                    if let Some(p) = c.borrow().props.get(key) {
                        if p.is_accessor() {
                            if let Some(g) = &p.get {
                                return self.call_function(g, base, &[]).unwrap_or(Value::Undefined);
                            }
                            return Value::Undefined;
                        }
                        return p.value.clone();
                    }
                    if key == "constructor" {
                        if let Some(v) = Self::resolve_ctor(&c.borrow().ctor) {
                            return v;
                        }
                    }
                    cur = c.borrow().proto.clone();
                }
                Value::Undefined
            }
            Value::Array(a) => {
                if key == "length" {
                    return Value::Number(a.borrow().logical_len() as f64);
                }
                if let Ok(i) = key.parse::<usize>() {
                    return a.borrow().get_index(i).unwrap_or(Value::Undefined);
                }
                if let Some(p) = a.borrow().props.get(key) {
                    if p.is_accessor() {
                        if let Some(g) = &p.get {
                            return self.call_function(g, base, &[]).unwrap_or(Value::Undefined);
                        }
                        return Value::Undefined;
                    }
                    return p.value.clone();
                }
                return self.lookup_proto(&self.array_prototype, key);
            }
            Value::Function(f) => {
                if let Some(p) = f.borrow().props.get(key).cloned() {
                    if p.is_accessor() {
                        if let Some(g) = &p.get {
                            return self
                                .call_function(&g, base, &[])
                                .unwrap_or(Value::Undefined);
                        }
                        return Value::Undefined;
                    }
                    return p.value.clone();
                }
                let mut cur = f.borrow().proto.clone();
                while let Some(c) = cur {
                    if let Some(p) = c.borrow().props.get(key) {
                        if p.is_accessor() {
                            if let Some(g) = &p.get {
                                return self
                                    .call_function(g, base, &[])
                                    .unwrap_or(Value::Undefined);
                            }
                            return Value::Undefined;
                        }
                        return p.value.clone();
                    }
                    if key == "constructor" {
                        if let Some(v) = Self::resolve_ctor(&c.borrow().ctor) {
                            return v;
                        }
                    }
                    cur = c.borrow().proto.clone();
                }
                // Static inheritance: a derived class constructor falls back
                // to its parent constructor (`super_ctor` chain). Accessors
                // run with the original receiver (`base`).
                if let Some(sup) = f.borrow().super_ctor.clone() {
                    match self.get_raw_property(&sup, key) {
                        Some(p) if p.is_accessor() => {
                            if let Some(g) = &p.get {
                                return self
                                    .call_function(&g, base, &[])
                                    .unwrap_or(Value::Undefined);
                            }
                            return Value::Undefined;
                        }
                        Some(p) => return p.value.clone(),
                        None => {}
                    }
                }
                Value::Undefined
            }
            Value::String(s) => {
                if key == "length" {
                    return Value::Number(s.len() as f64);
                }
                // String exotic indexing: `str[i]` yields the substring at index
                // `i` (this engine indexes strings by Unicode code point, the
                // same convention used by `charAt`). Out-of-range indices return
                // `undefined` exactly as the spec requires.
                if let Ok(i) = key.parse::<usize>() {
                    return s
                        .chars()
                        .nth(i)
                        .map(|c| Value::String(Rc::from(c.to_string().as_str())))
                        .unwrap_or(Value::Undefined);
                }
                self.lookup_proto(&self.string_prototype, key)
            }
            Value::Number(_) => self.lookup_proto(&self.number_prototype, key),
            Value::Boolean(_) => self.lookup_proto(&self.boolean_prototype, key),
            Value::Symbol(s) => {
                let mut cur = Some(self.symbol_prototype.clone());
                while let Some(c) = cur {
                    if let Some(p) = c.borrow().props.get(key) {
                        if p.is_accessor() {
                            if let Some(g) = &p.get {
                                return self
                                    .call_function(g, &Value::Symbol(s.clone()), &[])
                                    .unwrap_or(Value::Undefined);
                            }
                            return Value::Undefined;
                        }
                        return p.value.clone();
                    }
                    cur = c.borrow().proto.clone();
                }
                Value::Undefined
            }
            Value::BigInt(s) => {
                let mut cur = Some(self.bigint_prototype.clone());
                while let Some(c) = cur {
                    if let Some(p) = c.borrow().props.get(key) {
                        if p.is_accessor() {
                            if let Some(g) = &p.get {
                                return self
                                    .call_function(g, &Value::BigInt(s.clone()), &[])
                                    .unwrap_or(Value::Undefined);
                            }
                            return Value::Undefined;
                        }
                        return p.value.clone();
                    }
                    cur = c.borrow().proto.clone();
                }
                Value::Undefined
            }
            Value::Regex(rx) => {
                if key == "lastIndex" {
                    return Value::Number(*rx.last_index.borrow() as f64);
                }
                self.get_via_proto(Some(self.regexp_prototype.clone()), key, &Value::Regex(rx.clone()))
            }
            Value::Map(m) => self.get_via_proto(Some(self.map_prototype.clone()), key, &Value::Map(m.clone())),
            Value::Set(s) => self.get_via_proto(Some(self.set_prototype.clone()), key, &Value::Set(s.clone())),
            Value::Promise(p) => {
                if let Some(prop) = p.borrow().props.get(key) {
                    return prop.value.clone();
                }
                let start = p.borrow().proto.clone().or_else(|| Some(self.promise_prototype.clone()));
                self.get_via_proto(start, key, &Value::Promise(p.clone()))
            }
            Value::WeakMap(w) => {
                self.get_via_proto(Some(self.weakmap_prototype.clone()), key, &Value::WeakMap(w.clone()))
            }
            Value::WeakSet(w) => {
                self.get_via_proto(Some(self.weakset_prototype.clone()), key, &Value::WeakSet(w.clone()))
            }
            Value::NativeFunction(nf) => {
                if let Some(p) = nf.borrow().props.get(key).cloned() {
                    if p.is_accessor() {
                        if let Some(g) = &p.get {
                            return self
                                .call_function(&g, base, &[])
                                .unwrap_or(Value::Undefined);
                        }
                        return Value::Undefined;
                    }
                    return p.value.clone();
                }
                // A native function (e.g. a builtin prototype method) may have no
                // explicit prototype; fall back to `Function.prototype` so that
                // `.call` / `.apply` / `.bind` resolve correctly.
                let mut cur = nf
                    .borrow()
                    .proto
                    .clone()
                    .or_else(|| Some(self.function_prototype.clone()));
                while let Some(c) = cur {
                    if let Some(p) = c.borrow().props.get(key) {
                        return p.value.clone();
                    }
                    if key == "constructor" {
                        if let Some(v) = Self::resolve_ctor(&c.borrow().ctor) {
                            return v;
                        }
                    }
                    cur = c.borrow().proto.clone();
                }
                Value::Undefined
            }
            _ => Value::Undefined,
        }
    }

    /// Look up `key` on a prototype chain rooted at `start`.
    fn lookup_proto(&self, start: &Rc<RefCell<Object>>, key: &str) -> Value {
        let mut cur = Some(start.clone());
        while let Some(c) = cur {
            if let Some(p) = c.borrow().props.get(key) {
                return p.value.clone();
            }
            if key == "constructor" {
                if let Some(v) = Self::resolve_ctor(&c.borrow().ctor) {
                    return v;
                }
            }
            cur = c.borrow().proto.clone();
        }
        Value::Undefined
    }

    /// Walk a prototype chain invoking accessor getters, used by the
    /// collection value types (and mirrors the `Object` lookup).
    fn get_via_proto(
        &self,
        start: Option<Rc<RefCell<Object>>>,
        key: &str,
        base: &Value,
    ) -> Value {
        let mut cur = start;
        while let Some(c) = cur {
            if let Some(p) = c.borrow().props.get(key) {
                if p.is_accessor() {
                    if let Some(g) = &p.get {
                        return self.call_function(g, base, &[]).unwrap_or(Value::Undefined);
                    }
                    return Value::Undefined;
                }
                return p.value.clone();
            }
            if key == "constructor" {
                if let Some(v) = Self::resolve_ctor(&c.borrow().ctor) {
                    return v;
                }
            }
            cur = c.borrow().proto.clone();
        }
        Value::Undefined
    }

    /// Raw (getter-preserving) property lookup: walk own + prototype chain
    /// returning the stored [`Property`] without invoking accessors. Used by
    /// thenable assimilation, where a throwing `then` getter must reject
    /// rather than be swallowed.
    pub(crate) fn get_raw_property(&self, base: &Value, key: &str) -> Option<Property> {
        fn walk(start: Option<Rc<RefCell<Object>>>, key: &str) -> Option<Property> {
            let mut cur = start;
            while let Some(c) = cur {
                if let Some(p) = c.borrow().props.get(key).cloned() {
                    return Some(p);
                }
                cur = c.borrow().proto.clone();
            }
            None
        }
        match base {
            Value::Object(o) => walk(Some(o.clone()), key),
            Value::Array(a) => a
                .borrow()
                .props
                .get(key)
                .cloned()
                .or_else(|| walk(Some(self.array_prototype.clone()), key)),
            Value::Function(f) => f
                .borrow()
                .props
                .get(key)
                .cloned()
                .or_else(|| {
                    walk(
                        f.borrow()
                            .proto
                            .clone()
                            .or_else(|| Some(self.function_prototype.clone())),
                        key,
                    )
                })
                .or_else(|| {
                    // Static inheritance via the parent constructor.
                    f.borrow()
                        .super_ctor
                        .clone()
                        .and_then(|sup| self.get_raw_property(&sup, key))
                }),
            Value::NativeFunction(nf) => nf
                .borrow()
                .props
                .get(key)
                .cloned()
                .or_else(|| {
                    walk(
                        nf.borrow()
                            .proto
                            .clone()
                            .or_else(|| Some(self.function_prototype.clone())),
                        key,
                    )
                }),
            Value::Promise(p) => p
                .borrow()
                .props
                .get(key)
                .cloned()
                .or_else(|| {
                    walk(
                        p.borrow()
                            .proto
                            .clone()
                            .or_else(|| Some(self.promise_prototype.clone())),
                        key,
                    )
                }),
            Value::String(_) => walk(Some(self.string_prototype.clone()), key),
            Value::Number(_) => walk(Some(self.number_prototype.clone()), key),
            Value::Boolean(_) => walk(Some(self.boolean_prototype.clone()), key),
            Value::Symbol(_) => walk(Some(self.symbol_prototype.clone()), key),
            Value::BigInt(_) => walk(Some(self.bigint_prototype.clone()), key),
            Value::Regex(_) => walk(Some(self.regexp_prototype.clone()), key),
            Value::Map(_) => walk(Some(self.map_prototype.clone()), key),
            Value::Set(_) => walk(Some(self.set_prototype.clone()), key),
            Value::WeakMap(_) => walk(Some(self.weakmap_prototype.clone()), key),
            Value::WeakSet(_) => walk(Some(self.weakset_prototype.clone()), key),
            Value::Undefined | Value::Null => None,
        }
    }

    /// If `this` is a primitive wrapper object, return the wrapped primitive;
    /// otherwise return `this` unchanged.
    pub(crate) fn this_primitive(&self, this: &Value) -> Value {
        if let Value::Object(o) = this {
            if let Some(pv) = o.borrow().props.get("__value__") {
                return pv.value.clone();
            }
        }
        this.clone()
    }

    /// Collect the enumerable own + inherited string-keyed properties of `base`,
    /// in prototype-chain order (closest first). Used by `for…in`.
    pub(crate) fn enumerable_keys(&self, base: &Value) -> Vec<Rc<str>> {
        let mut seen: BTreeSet<Rc<str>> = BTreeSet::new();
        match base {
            Value::Array(a) => {
                let len = a.borrow().logical_len();
                for i in 0..len {
                    if a.borrow().get_index(i).is_some() {
                        seen.insert(Rc::from(i.to_string()));
                    }
                }
                for i in a.borrow().sparse.keys() { seen.insert(Rc::from(i.to_string())); }
                let b = a.borrow();
                for (k, p) in b.props.iter() {
                    if p.enumerable {
                        seen.insert(k.clone());
                    }
                }
            }
            Value::String(s) => {
                for (i, _) in s.chars().enumerate() {
                    seen.insert(Rc::from(i.to_string()));
                }
            }
            Value::Object(o) => {
                let mut cur = Some(o.clone());
                while let Some(c) = cur {
                    for (k, p) in c.borrow().props.iter() {
                        if k.as_ref() == "__value__" {
                            continue;
                        }
                        if p.enumerable {
                            seen.insert(k.clone());
                        }
                    }
                    cur = c.borrow().proto.clone();
                }
            }
            _ => {}
        }
        seen.into_iter().collect()
    }

    /// Produce an indexed (array-like) sequence of values for `for…of` and
    /// array destructuring.
    pub(crate) fn iterable_values(&self, base: &Value) -> Result<Vec<Value>> {
        // An explicit (or inherited) `Symbol.iterator` takes precedence over
        // the built-in fast paths, per GetIterator.
        if Self::is_object_value(base) {
            let iter_key = symbol_iterator_id();
            let iter_fn = self.get_property(base, &iter_key);
            if Self::is_callable_value(&iter_fn) {
                return self.iterate_protocol(base);
            }
        }
        match base {
            Value::Array(a) => {
                let b = a.borrow();
                if b.sparse.is_empty() { Ok(b.elems.clone()) }
                else { Ok((0..b.logical_len()).map(|i| b.get_index(i).unwrap_or(Value::Undefined)).collect()) }
            }
            Value::String(s) => {
                let mut v = Vec::new();
                for ch in s.chars() {
                    v.push(Value::String(Rc::from(ch.to_string().as_str())));
                }
                Ok(v)
            }
            Value::Map(m) => {
                let data = m.borrow();
                Ok(data
                    .entries
                    .iter()
                    .map(|(k, v)| {
                        Value::Array(self.new_array(vec![k.clone(), v.clone()]))
                    })
                    .collect())
            }
            Value::Set(s) => Ok(s.borrow().entries.clone()),
            _ => {
                // Try the iterator protocol (Symbol.iterator).
                let iter_key = symbol_iterator_id();
                let iter_fn = self.get_property(base, &iter_key);
                if matches!(iter_fn, Value::Function(_) | Value::NativeFunction(_)) {
                    return self.iterate_protocol(base);
                }
                // Fall back to array-like indexing when a `length` exists.
                if let Some(len_v) = match base {
                    Value::Object(o) => o.borrow().props.get("length").map(|p| p.value.clone()),
                    _ => None,
                } {
                    let len = len_v.to_number() as usize;
                    let mut v = Vec::new();
                    for i in 0..len {
                        v.push(self.get_property(base, &i.to_string()));
                    }
                    Ok(v)
                } else {
                    Err(Error::Runtime(self.make_type_error("value is not iterable")))
                }
            }
        }
    }

    /// Consume an object's `Symbol.iterator` iterator, returning the sequence of
    /// `value` results until `done` is true.
    fn iterate_protocol(&self, base: &Value) -> Result<Vec<Value>> {
        let iter_key = symbol_iterator_id();
        let iter_fn = self.get_property(base, &iter_key);
        let iter_obj = self.call_value(&iter_fn, base, &[])?;
        let mut out = Vec::new();
        loop {
            let next_fn = self.get_property(&iter_obj, "next");
            let res = self.call_value(&next_fn, &iter_obj, &[])?;
            let done = self.get_property(&res, "done").to_boolean();
            if done {
                break;
            }
            out.push(self.get_property(&res, "value"));
        }
        Ok(out)
    }

    /// Produce an indexed vector of values from an array-like value.
    pub(crate) fn to_indexed(&self, base: &Value) -> Result<Vec<Value>> {
        self.iterable_values(base)
    }

    /// Coerce `base` to an object, boxing array indices as needed.
    pub(crate) fn to_object(&self, base: &Value) -> Result<Rc<RefCell<Object>>> {
        match base {
            Value::Object(o) => Ok(o.clone()),
            Value::Array(a) => {
                let mut obj = Object::with_proto(self.object_prototype.clone());
                let elems = a.borrow();
                for (i, v) in elems.elems.iter().enumerate() {
                    obj.props.insert(
                        Rc::from(i.to_string()),
                        Property::new(v.clone()),
                    );
                }
                for (i, v) in elems.sparse.iter() {
                    obj.props.insert(Rc::from(i.to_string().as_str()), Property::new(v.clone()));
                }
                obj.props
                    .insert(Rc::from("length"), Property::new(Value::Number(elems.logical_len() as f64)));
                Ok(Rc::new(RefCell::new(obj)))
            }
            _ => Err(Error::Runtime(Value::String(Rc::from(
                "TypeError: cannot destructure non-object",
            )))),
        }
    }

    pub(crate) fn set_property(&self, base: &Value, key: &str, val: Value) -> Result<()> {
        // In strict mode, assignment to a non-writable data property (or an
        // accessor lacking a setter) throws a TypeError.
        match base {
            Value::Object(o) => {
                let existing = o.borrow().props.get(key).cloned();
                match existing {
                    // Accessor with a setter: invoke it. Without a setter the
                    // assignment is a no-op in sloppy mode and a TypeError in
                    // strict mode.
                    Some(prop) if prop.is_accessor() => {
                        if let Some(s) = &prop.set {
                            self.call_function(&s, base, &[val])?;
                        } else if self.strict.get() {
                            return Err(Error::Runtime(self.make_type_error(
                                "Assignment to a property with no setter",
                            )));
                        }
                    }
                    // Assignment to a non-writable data property silently fails
                    // in sloppy mode; in strict mode it throws a TypeError.
                    Some(prop) if !prop.writable => {
                        if self.strict.get() {
                            return Err(Error::Runtime(self.make_type_error(
                                "Assignment to a read-only property",
                            )));
                        }
                    }
                    // Writable data property: update the value, keep the parent's
                    // attribute flags (so a write does not turn it into a fresh
                    // writable/enumerable property).
                    Some(prop) => {
                        o.borrow_mut().props.insert(
                            self.intern_key(key),
                            prop.with_value(val),
                        );
                    }
                    None => {
                        o.borrow_mut().props.insert(self.intern_key(key), Property::new(val));
                    }
                }
            }
            Value::Array(a) => {
                if key == "length" {
                    let n = val.to_number() as usize;
                    a.borrow_mut().set_length(n);
                } else if let Ok(i) = key.parse::<usize>() {
                    a.borrow_mut().set_index(i, val);
                } else {
                    let existing = a.borrow().props.get(key).cloned();
                    match existing {
                        Some(prop) if prop.is_accessor() => {
                            if let Some(s) = &prop.set {
                                self.call_function(&s, base, &[val])?;
                            } else if self.strict.get() {
                                return Err(Error::Runtime(self.make_type_error(
                                    "Assignment to a property with no setter",
                                )));
                            }
                        }
                        Some(prop) if !prop.writable => {
                            if self.strict.get() {
                                return Err(Error::Runtime(self.make_type_error(
                                    "Assignment to a read-only property",
                                )));
                            }
                        }
                        Some(prop) => {
                            a.borrow_mut()
                                .props
                                .insert(self.intern_key(key), prop.with_value(val));
                        }
                        None => {
                            a.borrow_mut().props.insert(self.intern_key(key), Property::new(val));
                        }
                    }
                }
            }
            Value::Function(f) => {
                let existing = f.borrow().props.get(key).cloned();
                match existing {
                    Some(prop) if prop.is_accessor() => {
                        if let Some(s) = &prop.set {
                            self.call_function(&s, base, &[val])?;
                        } else if self.strict.get() {
                            return Err(Error::Runtime(self.make_type_error(
                                "Assignment to a property with no setter",
                            )));
                        }
                    }
                    Some(prop) if !prop.writable => {
                        if self.strict.get() {
                            return Err(Error::Runtime(self.make_type_error(
                                "Assignment to a read-only property",
                            )));
                        }
                    }
                    Some(prop) => {
                        f.borrow_mut()
                            .props
                            .insert(self.intern_key(key), prop.with_value(val));
                    }
                    None => {
                        f.borrow_mut().props.insert(self.intern_key(key), Property::new(val));
                    }
                }
            }
            Value::NativeFunction(nf) => {
                let existing = nf.borrow().props.get(key).cloned();
                match existing {
                    Some(prop) if prop.is_accessor() => {
                        if let Some(s) = &prop.set {
                            self.call_function(&s, base, &[val])?;
                        } else if self.strict.get() {
                            return Err(Error::Runtime(self.make_type_error(
                                "Assignment to a property with no setter",
                            )));
                        }
                    }
                    Some(prop) if !prop.writable => {
                        if self.strict.get() {
                            return Err(Error::Runtime(self.make_type_error(
                                "Assignment to a read-only property",
                            )));
                        }
                    }
                    Some(prop) => {
                        nf.borrow_mut()
                            .props
                            .insert(self.intern_key(key), prop.with_value(val));
                    }
                    None => {
                        nf.borrow_mut().props.insert(self.intern_key(key), Property::new(val));
                    }
                }
            }
            Value::Regex(rx) => {
                if key == "lastIndex" {
                    let n = val.to_number();
                    let c = if n.is_nan() || n < 0.0 { 0 } else { n as usize };
                    *rx.last_index.borrow_mut() = c;
                }
            }
            Value::Promise(p) => {
                p.borrow_mut().props.insert(self.intern_key(key), Property::new(val));
            }
            _ => {}
        }
        Ok(())
    }

    /// True if `base` has an own property `key`.
    pub(crate) fn has_own(&self, base: &Value, key: &str) -> bool {
        match base {
            Value::Object(o) => o.borrow().props.contains_key(key),
            Value::Array(a) => {
                key == "length"
                    || key.parse::<usize>().map(|i| i < a.borrow().logical_len() && a.borrow().get_index(i).is_some()).unwrap_or(false)
                    || a.borrow().props.contains_key(key)
            }
            Value::Function(f) => f.borrow().props.contains_key(key),
            Value::NativeFunction(nf) => nf.borrow().props.contains_key(key),
            Value::Promise(p) => p.borrow().props.contains_key(key),
            Value::String(s) => key == "length" || key.parse::<usize>().map(|i| i < s.chars().count()).unwrap_or(false),
            _ => false,
        }
    }

    /// The `[[Enumerable]]` flag of an own property, if it exists.
    pub(crate) fn prop_enumerable(&self, base: &Value, key: &str) -> Option<bool> {
        match base {
            Value::Object(o) => o.borrow().props.get(key).map(|p| p.enumerable),
            Value::Array(a) => {
                if let Ok(i) = key.parse::<usize>() {
                    Some(i < a.borrow().logical_len() && a.borrow().get_index(i).is_some())
                } else {
                    a.borrow().props.get(key).map(|p| p.enumerable)
                }
            }
            Value::String(_) => Some(false),
            Value::Function(f) => f.borrow().props.get(key).map(|p| p.enumerable),
            Value::NativeFunction(nf) => nf.borrow().props.get(key).map(|p| p.enumerable),
            Value::Promise(p) => p.borrow().props.get(key).map(|p| p.enumerable),
            _ => None,
        }
    }

    /// An own property `key` (cloned), if present.
    fn own_property(&self, base: &Value, key: &str) -> Option<Property> {
        match base {
            Value::Object(o) => o.borrow().props.get(key).cloned(),
            Value::Array(a) => a.borrow().props.get(key).cloned(),
            Value::Function(f) => f.borrow().props.get(key).cloned(),
            Value::NativeFunction(nf) => nf.borrow().props.get(key).cloned(),
            Value::Promise(p) => p.borrow().props.get(key).cloned(),
            _ => None,
        }
    }

    /// Define or modify an own property following `Object.defineProperty`.
    pub(crate) fn define_property(&self, base: &Value, key: &str, desc: &Value) -> Result<()> {
        let mut value = Value::Undefined;
        let mut writable = false;
        let mut enumerable = false;
        let mut configurable = false;
        let mut get: Option<Value> = None;
        let mut set: Option<Value> = None;
        let mut has_value = false;
        if let Value::Object(d) = desc {
            let dp = d.borrow();
            if let Some(p) = dp.props.get("value") {
                value = p.value.clone();
                has_value = true;
            }
            if let Some(p) = dp.props.get("writable") {
                writable = p.value.to_boolean();
            }
            if let Some(p) = dp.props.get("enumerable") {
                enumerable = p.value.to_boolean();
            }
            if let Some(p) = dp.props.get("configurable") {
                configurable = p.value.to_boolean();
            }
            if let Some(p) = dp.props.get("get") {
                get = Some(p.value.clone());
            }
            if let Some(p) = dp.props.get("set") {
                set = Some(p.value.clone());
            }
        }
        let accessor = get.is_some() || set.is_some();

        // Enforce redefinition restrictions against a non-configurable existing
        // property.
        if let Some(prop) = self.own_property(base, key) {
            if !prop.configurable {
                if configurable || prop.is_accessor() != accessor {
                    return Err(self.type_error("Cannot redefine property"));
                }
                if prop.is_accessor() {
                    if get.is_some() && !Value::same_value(prop.get.as_ref().unwrap_or(&Value::Undefined), get.as_ref().unwrap_or(&Value::Undefined)) {
                        return Err(self.type_error("Cannot redefine property"));
                    }
                    if set.is_some() && !Value::same_value(prop.set.as_ref().unwrap_or(&Value::Undefined), set.as_ref().unwrap_or(&Value::Undefined)) {
                        return Err(self.type_error("Cannot redefine property"));
                    }
                } else if !prop.writable && has_value && !Value::same_value(&prop.value, &value) {
                    return Err(self.type_error("Cannot redefine property"));
                }
            }
        }

        let newprop = if accessor {
            let mut p = Property::accessor(get, set);
            p.enumerable = enumerable;
            p.configurable = configurable;
            p
        } else {
            Property::data(value, writable, enumerable, configurable)
        };
        self.define_raw(base, key, newprop);
        Ok(())
    }

    fn define_raw(&self, base: &Value, key: &str, prop: Property) {
        match base {
            Value::Object(o) => {
                o.borrow_mut().props.insert(self.intern_key(key), prop);
            }
            Value::Function(f) => {
                f.borrow_mut().props.insert(self.intern_key(key), prop);
            }
            Value::NativeFunction(nf) => {
                nf.borrow_mut().props.insert(self.intern_key(key), prop);
            }
            Value::Array(a) => {
                if key == "length" {
                    let n = prop.value.to_number() as usize;
                    a.borrow_mut().set_length(n);
                } else if let Ok(i) = key.parse::<usize>() {
                    a.borrow_mut().set_index(i, prop.value.clone());
                } else {
                    a.borrow_mut().props.insert(self.intern_key(key), prop);
                }
            }
            Value::Promise(p) => {
                p.borrow_mut().props.insert(self.intern_key(key), prop);
            }
            _ => {}
        }
    }

    /// Build a property descriptor object for an own [`Property`].
    fn descriptor_object(&self, p: &Property) -> Value {
        let d = self.new_object();
        {
            let mut b = d.borrow_mut();
            if p.is_accessor() {
                b.props.insert(Rc::from("get"), Property::new(p.get.clone().unwrap_or(Value::Undefined)));
                b.props.insert(Rc::from("set"), Property::new(p.set.clone().unwrap_or(Value::Undefined)));
            } else {
                b.props.insert(Rc::from("value"), Property::new(p.value.clone()));
                b.props.insert(Rc::from("writable"), Property::new(Value::Boolean(p.writable)));
            }
            b.props.insert(Rc::from("enumerable"), Property::new(Value::Boolean(p.enumerable)));
            b.props.insert(Rc::from("configurable"), Property::new(Value::Boolean(p.configurable)));
        }
        Value::Object(d)
    }

    /// Return a property descriptor object for an own property, or `undefined`.
    pub(crate) fn get_own_descriptor(&self, base: &Value, key: &str) -> Value {
        if let Some(p) = self.own_property(base, key) {
            return self.descriptor_object(&p);
        }
        match base {
            Value::Array(a) => {
                let elems = a.borrow();
                let p = if key == "length" {
                    Property::data(Value::Number(elems.logical_len() as f64), true, false, false)
                } else if let Ok(i) = key.parse::<usize>() {
                    if i < elems.logical_len() {
                        if let Some(v) = elems.get_index(i) { Property::new(v) }
                        else { return Value::Undefined; }
                    } else {
                        return Value::Undefined;
                    }
                } else {
                    return Value::Undefined;
                };
                self.descriptor_object(&p)
            }
            Value::String(s) => {
                if key == "length" {
                    self.descriptor_object(&Property::data(Value::Number(s.chars().count() as f64), false, false, false))
                } else {
                    Value::Undefined
                }
            }
            _ => Value::Undefined,
        }
    }

    /// The string own-property keys of `base` (excluding internal `__value__`).
    pub(crate) fn own_property_names(&self, base: &Value) -> Vec<Rc<str>> {
        let mut names: Vec<Rc<str>> = Vec::new();
        match base {
            Value::Object(o) => {
                for k in o.borrow().props.keys() {
                    if k.as_ref() != "__value__" && k.as_ref() != "__error_data__" {
                        names.push(k.clone());
                    }
                }
            }
            Value::Array(a) => {
                names.push(Rc::from("length"));
                for i in 0..a.borrow().elems.len() {
                    names.push(Rc::from(i.to_string().as_str()));
                }
                let b = a.borrow();
                for i in b.sparse.keys() {
                    names.push(Rc::from(i.to_string().as_str()));
                }
                for k in b.props.keys() {
                    if k.as_ref() != "__value__" && k.as_ref() != "__error_data__" {
                        names.push(k.clone());
                    }
                }
            }
            Value::String(s) => {
                names.push(Rc::from("length"));
                for i in 0..s.chars().count() {
                    names.push(Rc::from(i.to_string().as_str()));
                }
            }
            Value::Function(f) => {
                for k in f.borrow().props.keys() {
                    if k.as_ref() != "__value__" && k.as_ref() != "__error_data__" {
                        names.push(k.clone());
                    }
                }
            }
            Value::NativeFunction(nf) => {
                for k in nf.borrow().props.keys() {
                    if k.as_ref() != "__value__" && k.as_ref() != "__error_data__" {
                        names.push(k.clone());
                    }
                }
            }
            Value::Promise(p) => {
                for k in p.borrow().props.keys() {
                    names.push(k.clone());
                }
            }
            _ => {}
        }
        names
    }

    /// The own symbol-keyed properties of `base`.
    pub(crate) fn own_symbol_props(&self, base: &Value) -> Vec<Rc<SymbolData>> {
        // Symbol keys are stored under synthetic id strings; report none for now.
        let _ = (base, self);
        Vec::new()
    }

    /// A runtime `TypeError` value used as a thrown exception.
    fn type_error(&self, msg: &str) -> Error {
        Error::Runtime(self.make_type_error(msg))
    }

    /// Create a brand-new `TypeError` object instance.
    pub(crate) fn make_type_error(&self, msg: &str) -> Value {
        self.make_error(&self.type_error_proto, "TypeError", msg)
    }

    /// Create a brand-new `RangeError` object instance.
    pub(crate) fn make_range_error(&self, msg: &str) -> Value {
        self.make_error(&self.range_error_proto, "RangeError", msg)
    }

    /// Create a brand-new `URIError` object instance.
    pub(crate) fn make_uri_error(&self, msg: &str) -> Value {
        self.make_error(&self.uri_error_proto, "URIError", msg)
    }

    /// Create a brand-new `SyntaxError` object instance.
    pub(crate) fn make_syntax_error(&self, msg: &str) -> Value {
        self.make_error(&self.syntax_error_proto, "SyntaxError", msg)
    }

    /// Whether a value is an object (as opposed to a primitive).
    pub(crate) fn is_object_value(v: &Value) -> bool {
        matches!(
            v,
            Value::Object(_)
                | Value::Array(_)
                | Value::Function(_)
                | Value::NativeFunction(_)
                | Value::Regex(_)
                | Value::Map(_)
                | Value::Set(_)
                | Value::WeakMap(_)
                | Value::WeakSet(_)
                | Value::Promise(_)
        )
    }

    /// Whether a value is callable (a user function or a native function).
    pub(crate) fn is_callable_value(v: &Value) -> bool {
        matches!(v, Value::Function(_) | Value::NativeFunction(_))
    }

    /// ECMAScript `ToPrimitive` with an explicit hint.
    ///
    /// With a *number* hint (`hint_string == false`) `valueOf` is tried before
    /// `toString`; with a *string* hint the order is reversed. Object-valued
    /// results from each method cause the other to be tried; if both return
    /// objects a `TypeError` is thrown. Errors thrown by the user methods are
    /// propagated.
    pub(crate) fn to_primitive(&self, v: &Value, hint_string: bool) -> Result<Value> {
        let hint = if hint_string {
            PrimitiveHint::String
        } else {
            PrimitiveHint::Number
        };
        self.to_primitive_hint(v, hint)
    }

    /// ECMAScript `ToPrimitive` with a full three-way hint (including the
    /// no-preference `Default` hint used by `+`). A `Symbol.toPrimitive`
    /// method, when present and callable, takes precedence over the ordinary
    /// `valueOf`/`toString` algorithm.
    pub(crate) fn to_primitive_hint(&self, v: &Value, hint: PrimitiveHint) -> Result<Value> {
        if !Self::is_object_value(v) {
            return Ok(v.clone());
        }
        let hint_str = match hint {
            PrimitiveHint::Default => "default",
            PrimitiveHint::Number => "number",
            PrimitiveHint::String => "string",
        };
        let exotic_key = crate::value::SymbolData::well_known_key("toPrimitive");
        let exotic = self.get_property(v, exotic_key.as_ref());
        if matches!(exotic, Value::Undefined | Value::Null) {
            // No @@toPrimitive method: fall through to the ordinary algorithm.
        } else if Self::is_callable_value(&exotic) {
            let r = self.call_value(&exotic, v, &[Value::String(Rc::from(hint_str))])?;
            if !Self::is_object_value(&r) {
                return Ok(r);
            }
            return Err(self.type_error("Cannot convert object to primitive value"));
        } else {
            return Err(self.type_error("Symbol.toPrimitive is not callable"));
        }
        self.ordinary_to_primitive(v, hint != PrimitiveHint::String)
    }

    /// ECMAScript `OrdinaryToPrimitive`: try `valueOf` then `toString`
    /// (number-ish hint) or the reverse (string hint).
    fn ordinary_to_primitive(&self, v: &Value, hint_number: bool) -> Result<Value> {
        let (first, second) = if hint_number {
            ("valueOf", "toString")
        } else {
            ("toString", "valueOf")
        };
        let first_fn = self.get_property(v, first);
        if Self::is_callable_value(&first_fn) {
            let r = self.call_value(&first_fn, v, &[])?;
            if !Self::is_object_value(&r) {
                return Ok(r);
            }
        }
        let second_fn = self.get_property(v, second);
        if Self::is_callable_value(&second_fn) {
            let r = self.call_value(&second_fn, v, &[])?;
            if !Self::is_object_value(&r) {
                return Ok(r);
            }
        }
        Err(self.type_error("Cannot convert object to primitive value"))
    }

    /// ECMAScript `ToNumber` that can fail: `Symbol`/`BigInt` primitives and
    /// objects that cannot be converted to a primitive throw a `TypeError`, and
    /// errors raised by user-supplied `valueOf`/`toString` propagate.
    pub(crate) fn to_number_fallible(&self, v: &Value) -> Result<f64> {
        if matches!(v, Value::Symbol(_) | Value::BigInt(_)) {
            return Err(self.type_error("Cannot convert a Symbol or BigInt to a number"));
        }
        let prim = self.to_primitive(v, false)?;
        match prim {
            Value::Symbol(_) | Value::BigInt(_) => {
                Err(self.type_error("Cannot convert a Symbol or BigInt to a number"))
            }
            _ if Self::is_object_value(&prim) => {
                Err(self.type_error("Cannot convert object to primitive value"))
            }
            _ => Ok(prim.to_number()),
        }
    }

    /// ECMAScript `ToIntegerOrInfinity` (with `NaN` mapped to `+0`), which fails
    /// exactly as [`Self::to_number_fallible`] does.
    pub(crate) fn to_integer_fallible(&self, v: &Value) -> Result<f64> {
        let n = self.to_number_fallible(v)?;
        if n.is_nan() {
            Ok(0.0)
        } else {
            Ok(n.trunc())
        }
    }

    /// ECMAScript `ToString` that can fail: runs `ToPrimitive` (string hint) —
    /// user `toString`/`valueOf` may throw — and rejects `Symbol`s.
    pub(crate) fn to_string_fallible(&self, v: &Value) -> Result<Rc<str>> {
        let prim = self.to_primitive(v, true)?;
        if matches!(prim, Value::Symbol(_)) {
            return Err(self.type_error("Cannot convert a Symbol to a string"));
        }
        Ok(prim.to_string())
    }

    /// Create a brand-new error object whose prototype is `proto`.
    fn make_error(&self, proto: &Rc<RefCell<Object>>, name: &str, msg: &str) -> Value {
        let obj = self.make_object(proto.clone());
        {
            let mut b = obj.borrow_mut();
            b.props.insert(
                Rc::from("__error_data__"),
                Property::new(Value::Boolean(true)),
            );
            b.props.insert(
                Rc::from("name"),
                Property::data(Value::String(Rc::from(name)), true, false, true),
            );
            b.props.insert(
                Rc::from("message"),
                Property::data(Value::String(Rc::from(msg)), true, false, true),
            );
        }
        Value::Object(obj)
    }
}
