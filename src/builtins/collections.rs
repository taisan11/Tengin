use alloc::rc::Rc;
use core::cell::RefCell;

use crate::error::Error;
use crate::interpreter::Engine;
use crate::value::{
    ArrayData, native, MapData, Object, Property, SetData, Value, WeakMapData, WeakSetData,
};

use super::helpers::proto_method;

/// SameValueZero: used by `Map`/`Set` key comparison (`+0`/`-0` equal, `NaN`
/// equals `NaN`).
fn same_value_zero(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            if x.is_nan() && y.is_nan() {
                true
            } else {
                x == y
            }
        }
        _ => Value::strict_eq(a, b),
    }
}

fn map_find(data: &MapData, key: &Value) -> Option<usize> {
    data.entries.iter().position(|(k, _)| same_value_zero(k, key))
}

// --- Map ---

pub(crate) fn map_ctor(
    e: &Engine,
    _this: &Value,
    a: &[Value],
    construct: bool,
) -> Result<Value, Error> {
    if !construct {
        return Err(Error::Runtime(Value::String(Rc::from(
            "TypeError: Map constructor must be called with 'new'",
        ))));
    }
    let m = Rc::new(RefCell::new(MapData { entries: Vec::new() }));
    e.register_map(&m);
    let v = Value::Map(m.clone());
    if let Some(arg) = a.first() {
        if !matches!(arg, Value::Undefined | Value::Null) {
            let items = e.iterable_values(arg)?;
            for item in items {
                if let Value::Array(arr) = &item {
                    let elems = arr.borrow();
                    let key = elems.elems.first().cloned().unwrap_or(Value::Undefined);
                    let val = elems.elems.get(1).cloned().unwrap_or(Value::Undefined);
                    let mut data = m.borrow_mut();
                    if let Some(i) = map_find(&data, &key) {
                        data.entries[i].1 = val;
                    } else {
                        data.entries.push((key, val));
                    }
                }
            }
        }
    }
    Ok(v)
}

pub(crate) fn map_set(_e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let m = match this {
        Value::Map(m) => m,
        _ => {
            return Err(Error::Runtime(Value::String(Rc::from(
                "TypeError: Map.prototype.set called on non-Map",
            ))))
        }
    };
    let key = a.first().cloned().unwrap_or(Value::Undefined);
    let val = a.get(1).cloned().unwrap_or(Value::Undefined);
    let mut data = m.borrow_mut();
    if let Some(i) = map_find(&data, &key) {
        data.entries[i].1 = val;
    } else {
        data.entries.push((key, val));
    }
    Ok(this.clone())
}

pub(crate) fn map_get(_e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let m = match this {
        Value::Map(m) => m,
        _ => {
            return Err(Error::Runtime(Value::String(Rc::from(
                "TypeError: Map.prototype.get called on non-Map",
            ))))
        }
    };
    let key = a.first().cloned().unwrap_or(Value::Undefined);
    let data = m.borrow();
    Ok(data
        .entries
        .iter()
        .find(|(k, _)| same_value_zero(k, &key))
        .map(|(_, v)| v.clone())
        .unwrap_or(Value::Undefined))
}

pub(crate) fn map_has(_e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let m = match this {
        Value::Map(m) => m,
        _ => {
            return Err(Error::Runtime(Value::String(Rc::from(
                "TypeError: Map.prototype.has called on non-Map",
            ))))
        }
    };
    let key = a.first().cloned().unwrap_or(Value::Undefined);
    let data = m.borrow();
    Ok(Value::Boolean(data.entries.iter().any(|(k, _)| same_value_zero(k, &key))))
}

pub(crate) fn map_delete(_e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let m = match this {
        Value::Map(m) => m,
        _ => {
            return Err(Error::Runtime(Value::String(Rc::from(
                "TypeError: Map.prototype.delete called on non-Map",
            ))))
        }
    };
    let key = a.first().cloned().unwrap_or(Value::Undefined);
    let mut data = m.borrow_mut();
    if let Some(i) = map_find(&data, &key) {
        data.entries.remove(i);
        Ok(Value::Boolean(true))
    } else {
        Ok(Value::Boolean(false))
    }
}

pub(crate) fn map_clear(_e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    if let Value::Map(m) = this {
        m.borrow_mut().entries.clear();
    }
    Ok(Value::Undefined)
}

pub(crate) fn map_size(_e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    if let Value::Map(m) = this {
        Ok(Value::Number(m.borrow().entries.len() as f64))
    } else {
        Ok(Value::Number(0.0))
    }
}

pub(crate) fn map_for_each(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let cb = a.first().cloned().unwrap_or(Value::Undefined);
    let this_arg = a.get(1).cloned().unwrap_or(Value::Undefined);
    if let Value::Map(m) = this {
        let data = m.borrow();
        for (k, v) in data.entries.iter() {
            e.call_value(&cb, &this_arg, &[v.clone(), k.clone(), this.clone()])?;
        }
    }
    Ok(Value::Undefined)
}

// --- Set ---

fn set_find(data: &SetData, val: &Value) -> Option<usize> {
    data.entries.iter().position(|x| same_value_zero(x, val))
}

pub(crate) fn set_ctor(
    e: &Engine,
    _this: &Value,
    a: &[Value],
    construct: bool,
) -> Result<Value, Error> {
    if !construct {
        return Err(Error::Runtime(Value::String(Rc::from(
            "TypeError: Set constructor must be called with 'new'",
        ))));
    }
    let s = Rc::new(RefCell::new(SetData { entries: Vec::new() }));
    e.register_set(&s);
    let v = Value::Set(s.clone());
    if let Some(arg) = a.first() {
        if !matches!(arg, Value::Undefined | Value::Null) {
            let items = e.iterable_values(arg)?;
            let mut data = s.borrow_mut();
            for item in items {
                if set_find(&data, &item).is_none() {
                    data.entries.push(item);
                }
            }
        }
    }
    Ok(v)
}

pub(crate) fn set_add(_e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = match this {
        Value::Set(s) => s,
        _ => {
            return Err(Error::Runtime(Value::String(Rc::from(
                "TypeError: Set.prototype.add called on non-Set",
            ))))
        }
    };
    let val = a.first().cloned().unwrap_or(Value::Undefined);
    let mut data = s.borrow_mut();
    if set_find(&data, &val).is_none() {
        data.entries.push(val);
    }
    Ok(this.clone())
}

pub(crate) fn set_has(_e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = match this {
        Value::Set(s) => s,
        _ => {
            return Err(Error::Runtime(Value::String(Rc::from(
                "TypeError: Set.prototype.has called on non-Set",
            ))))
        }
    };
    let val = a.first().cloned().unwrap_or(Value::Undefined);
    let data = s.borrow();
    Ok(Value::Boolean(data.entries.iter().any(|x| same_value_zero(x, &val))))
}

pub(crate) fn set_delete(_e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = match this {
        Value::Set(s) => s,
        _ => {
            return Err(Error::Runtime(Value::String(Rc::from(
                "TypeError: Set.prototype.delete called on non-Set",
            ))))
        }
    };
    let val = a.first().cloned().unwrap_or(Value::Undefined);
    let mut data = s.borrow_mut();
    if let Some(i) = set_find(&data, &val) {
        data.entries.remove(i);
        Ok(Value::Boolean(true))
    } else {
        Ok(Value::Boolean(false))
    }
}

pub(crate) fn set_clear(_e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    if let Value::Set(s) = this {
        s.borrow_mut().entries.clear();
    }
    Ok(Value::Undefined)
}

pub(crate) fn set_size(_e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    if let Value::Set(s) = this {
        Ok(Value::Number(s.borrow().entries.len() as f64))
    } else {
        Ok(Value::Number(0.0))
    }
}

pub(crate) fn set_for_each(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let cb = a.first().cloned().unwrap_or(Value::Undefined);
    let this_arg = a.get(1).cloned().unwrap_or(Value::Undefined);
    if let Value::Set(s) = this {
        let data = s.borrow();
        for v in data.entries.iter() {
            e.call_value(&cb, &this_arg, &[v.clone(), v.clone(), this.clone()])?;
        }
    }
    Ok(Value::Undefined)
}

// --- WeakMap ---

pub(crate) fn weakmap_ctor(
    e: &Engine,
    _this: &Value,
    a: &[Value],
    construct: bool,
) -> Result<Value, Error> {
    if !construct {
        return Err(Error::Runtime(Value::String(Rc::from(
            "TypeError: WeakMap constructor must be called with 'new'",
        ))));
    }
    let w = Rc::new(RefCell::new(WeakMapData { entries: Vec::new() }));
    e.register_weakmap(&w);
    let v = Value::WeakMap(w.clone());
    if let Some(arg) = a.first() {
        if !matches!(arg, Value::Undefined | Value::Null) {
            let items = e.iterable_values(arg)?;
            for item in items {
                if let Value::Array(arr) = &item {
                    let elems = arr.borrow();
                    let key = elems.elems.first().cloned().unwrap_or(Value::Undefined);
                    let val = elems.elems.get(1).cloned().unwrap_or(Value::Undefined);
                    w.borrow_mut().entries.push((key, val));
                }
            }
        }
    }
    Ok(v)
}

pub(crate) fn weakmap_set(_e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let w = match this {
        Value::WeakMap(w) => w,
        _ => {
            return Err(Error::Runtime(Value::String(Rc::from(
                "TypeError: WeakMap.prototype.set called on non-WeakMap",
            ))))
        }
    };
    let key = a.first().cloned().unwrap_or(Value::Undefined);
    let val = a.get(1).cloned().unwrap_or(Value::Undefined);
    let mut data = w.borrow_mut();
    if let Some(i) = data.entries.iter().position(|(k, _)| same_value_zero(k, &key)) {
        data.entries[i].1 = val;
    } else {
        data.entries.push((key, val));
    }
    Ok(this.clone())
}

pub(crate) fn weakmap_get(_e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let w = match this {
        Value::WeakMap(w) => w,
        _ => {
            return Err(Error::Runtime(Value::String(Rc::from(
                "TypeError: WeakMap.prototype.get called on non-WeakMap",
            ))))
        }
    };
    let key = a.first().cloned().unwrap_or(Value::Undefined);
    let data = w.borrow();
    Ok(data
        .entries
        .iter()
        .find(|(k, _)| same_value_zero(k, &key))
        .map(|(_, v)| v.clone())
        .unwrap_or(Value::Undefined))
}

pub(crate) fn weakmap_has(_e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let w = match this {
        Value::WeakMap(w) => w,
        _ => {
            return Err(Error::Runtime(Value::String(Rc::from(
                "TypeError: WeakMap.prototype.has called on non-WeakMap",
            ))))
        }
    };
    let key = a.first().cloned().unwrap_or(Value::Undefined);
    let data = w.borrow();
    Ok(Value::Boolean(data.entries.iter().any(|(k, _)| same_value_zero(k, &key))))
}

pub(crate) fn weakmap_delete(
    _e: &Engine,
    this: &Value,
    a: &[Value],
    _c: bool,
) -> Result<Value, Error> {
    let w = match this {
        Value::WeakMap(w) => w,
        _ => {
            return Err(Error::Runtime(Value::String(Rc::from(
                "TypeError: WeakMap.prototype.delete called on non-WeakMap",
            ))))
        }
    };
    let key = a.first().cloned().unwrap_or(Value::Undefined);
    let mut data = w.borrow_mut();
    if let Some(i) = data.entries.iter().position(|(k, _)| same_value_zero(k, &key)) {
        data.entries.remove(i);
        Ok(Value::Boolean(true))
    } else {
        Ok(Value::Boolean(false))
    }
}

// --- WeakSet ---

pub(crate) fn weakset_ctor(
    e: &Engine,
    _this: &Value,
    a: &[Value],
    construct: bool,
) -> Result<Value, Error> {
    if !construct {
        return Err(Error::Runtime(Value::String(Rc::from(
            "TypeError: WeakSet constructor must be called with 'new'",
        ))));
    }
    let w = Rc::new(RefCell::new(WeakSetData { entries: Vec::new() }));
    e.register_weakset(&w);
    let v = Value::WeakSet(w.clone());
    if let Some(arg) = a.first() {
        if !matches!(arg, Value::Undefined | Value::Null) {
            let items = e.iterable_values(arg)?;
            let mut data = w.borrow_mut();
            for item in items {
                if data.entries.iter().position(|x| same_value_zero(x, &item)).is_none() {
                    data.entries.push(item);
                }
            }
        }
    }
    Ok(v)
}

pub(crate) fn weakset_add(_e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let w = match this {
        Value::WeakSet(w) => w,
        _ => {
            return Err(Error::Runtime(Value::String(Rc::from(
                "TypeError: WeakSet.prototype.add called on non-WeakSet",
            ))))
        }
    };
    let val = a.first().cloned().unwrap_or(Value::Undefined);
    let mut data = w.borrow_mut();
    if data.entries.iter().position(|x| same_value_zero(x, &val)).is_none() {
        data.entries.push(val);
    }
    Ok(this.clone())
}

pub(crate) fn weakset_has(_e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let w = match this {
        Value::WeakSet(w) => w,
        _ => {
            return Err(Error::Runtime(Value::String(Rc::from(
                "TypeError: WeakSet.prototype.has called on non-WeakSet",
            ))))
        }
    };
    let val = a.first().cloned().unwrap_or(Value::Undefined);
    let data = w.borrow();
    Ok(Value::Boolean(data.entries.iter().any(|x| same_value_zero(x, &val))))
}

pub(crate) fn weakset_delete(
    _e: &Engine,
    this: &Value,
    a: &[Value],
    _c: bool,
) -> Result<Value, Error> {
    let w = match this {
        Value::WeakSet(w) => w,
        _ => {
            return Err(Error::Runtime(Value::String(Rc::from(
                "TypeError: WeakSet.prototype.delete called on non-WeakSet",
            ))))
        }
    };
    let val = a.first().cloned().unwrap_or(Value::Undefined);
    let mut data = w.borrow_mut();
    if let Some(i) = data.entries.iter().position(|x| same_value_zero(x, &val)) {
        data.entries.remove(i);
        Ok(Value::Boolean(true))
    } else {
        Ok(Value::Boolean(false))
    }
}

// --- Iterators ---

pub(crate) fn iterate_next(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    let kind = e.get_property(this, "__iter_kind__").to_string();
    let target = e.get_property(this, "__target__");
    let idx = e.get_property(this, "__index__").to_number() as usize;
    let (value, done) = match kind.as_ref() {
        "map" => match &target {
            Value::Map(m) => {
                let data = m.borrow();
                if idx >= data.entries.len() {
                    (Value::Undefined, true)
                } else {
                    let (k, v) = data.entries[idx].clone();
                    let pair = Value::Array(Rc::new(RefCell::new(ArrayData::new(
                        vec![k, v],
                        Some(e.array_prototype.clone()),
                    ))));
                    (pair, false)
                }
            }
            _ => (Value::Undefined, true),
        },
        "mapkeys" => match &target {
            Value::Map(m) => {
                let data = m.borrow();
                if idx >= data.entries.len() {
                    (Value::Undefined, true)
                } else {
                    (data.entries[idx].0.clone(), false)
                }
            }
            _ => (Value::Undefined, true),
        },
        "mapvalues" => match &target {
            Value::Map(m) => {
                let data = m.borrow();
                if idx >= data.entries.len() {
                    (Value::Undefined, true)
                } else {
                    (data.entries[idx].1.clone(), false)
                }
            }
            _ => (Value::Undefined, true),
        },
        "set" => match &target {
            Value::Set(s) => {
                let data = s.borrow();
                if idx >= data.entries.len() {
                    (Value::Undefined, true)
                } else {
                    (data.entries[idx].clone(), false)
                }
            }
            _ => (Value::Undefined, true),
        },
        _ => (Value::Undefined, true),
    };
    if !done {
        let next = Value::Number((idx + 1) as f64);
        if let Value::Object(o) = this {
            o.borrow_mut()
                .props
                .insert(Rc::from("__index__"), Property::new(next));
        }
    }
    let mut res = Object::with_proto(e.object_prototype.clone());
    res.props
        .insert(Rc::from("value"), Property::new(value));
    res.props
        .insert(Rc::from("done"), Property::new(Value::Boolean(done)));
    Ok(Value::Object(Rc::new(RefCell::new(res))))
}

fn make_iterator(e: &Engine, target: Value, kind: &str) -> Value {
    let mut o = Object::with_proto(e.object_prototype.clone());
    o.props
        .insert(Rc::from("__target__"), Property::new(target));
    o.props
        .insert(Rc::from("__index__"), Property::new(Value::Number(0.0)));
    o.props
        .insert(Rc::from("__iter_kind__"), Property::new(Value::String(Rc::from(kind))));
    o.props
        .insert(Rc::from("next"), Property::new(native(iterate_next)));
    Value::Object(Rc::new(RefCell::new(o)))
}

pub(crate) fn map_iter(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(make_iterator(e, this.clone(), "map"))
}
pub(crate) fn map_keys(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(make_iterator(e, this.clone(), "mapkeys"))
}
pub(crate) fn map_values(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(make_iterator(e, this.clone(), "mapvalues"))
}
pub(crate) fn set_iter(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(make_iterator(e, this.clone(), "set"))
}

/// Install `Map`, `Set`, `WeakMap`, `WeakSet` and their prototypes.
pub(crate) fn register_collections(engine: &mut Engine) {
    let map_proto = engine.map_prototype.clone();
    let set_proto = engine.set_prototype.clone();
    let wk_proto = engine.weakmap_prototype.clone();
    let ws_proto = engine.weakset_prototype.clone();

    proto_method(&map_proto, "set", map_set);
    proto_method(&map_proto, "get", map_get);
    proto_method(&map_proto, "has", map_has);
    proto_method(&map_proto, "delete", map_delete);
    proto_method(&map_proto, "clear", map_clear);
    proto_method(&map_proto, "forEach", map_for_each);
    proto_method(&map_proto, "keys", map_keys);
    proto_method(&map_proto, "values", map_values);
    proto_method(&map_proto, "entries", map_iter);
    proto_method(&map_proto, symbol_iterator_alias(), map_iter);
    map_proto
        .borrow_mut()
        .props
        .insert(Rc::from("size"), Property::accessor(Some(native(map_size)), None));

    proto_method(&set_proto, "add", set_add);
    proto_method(&set_proto, "has", set_has);
    proto_method(&set_proto, "delete", set_delete);
    proto_method(&set_proto, "clear", set_clear);
    proto_method(&set_proto, "forEach", set_for_each);
    proto_method(&set_proto, "keys", set_iter);
    proto_method(&set_proto, "values", set_iter);
    proto_method(&set_proto, "entries", set_iter);
    proto_method(&set_proto, symbol_iterator_alias(), set_iter);
    set_proto
        .borrow_mut()
        .props
        .insert(Rc::from("size"), Property::accessor(Some(native(set_size)), None));

    proto_method(&wk_proto, "set", weakmap_set);
    proto_method(&wk_proto, "get", weakmap_get);
    proto_method(&wk_proto, "has", weakmap_has);
    proto_method(&wk_proto, "delete", weakmap_delete);

    proto_method(&ws_proto, "add", weakset_add);
    proto_method(&ws_proto, "has", weakset_has);
    proto_method(&ws_proto, "delete", weakset_delete);

    let map_ctor = super::helpers::reg_ctor(engine, "Map", map_ctor, map_proto.clone());
    super::helpers::proto_data(&map_proto, "constructor", map_ctor);
    let set_ctor = super::helpers::reg_ctor(engine, "Set", set_ctor, set_proto.clone());
    super::helpers::proto_data(&set_proto, "constructor", set_ctor);
    let wk_ctor = super::helpers::reg_ctor(engine, "WeakMap", weakmap_ctor, wk_proto.clone());
    super::helpers::proto_data(&wk_proto, "constructor", wk_ctor);
    let ws_ctor = super::helpers::reg_ctor(engine, "WeakSet", weakset_ctor, ws_proto.clone());
    super::helpers::proto_data(&ws_proto, "constructor", ws_ctor);
}

/// The property-key string for `Symbol.iterator` (must match the well-known
/// symbol's canonical id used elsewhere).
fn symbol_iterator_alias() -> &'static str {
    "__wk_iterator__"
}
