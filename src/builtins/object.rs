use alloc::format;
use alloc::rc::Rc;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;

use crate::error::Error;
use crate::interpreter::Engine;
use crate::ast::Pattern;
use crate::value::{ArrayData, Object, Property, Value};

use super::helpers::{key_arg, native, proto_of, set_proto_of_val, wrap};
// --- Object.prototype ---

pub(crate) fn obj_to_string(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    // A well-known `Symbol.toStringTag` on the object (or its chain) takes
    // precedence, per `Object.prototype.toString`.
    let tag_key = crate::value::SymbolData::well_known_key("toStringTag");
    let custom = e.get_property(this, tag_key.as_ref());
    if let Value::String(s) = custom {
        return Ok(Value::String(Rc::from(format!("[object {}]", s))));
    }
    let tag = if let Some(p) = this.primitive_value() {
        match p {
            Value::String(_) => "String",
            Value::Number(_) => "Number",
            Value::Boolean(_) => "Boolean",
            _ => "Object",
        }
    } else if let Value::Array(_) = this {
        "Array"
    } else if let Value::Function(_) | Value::NativeFunction(_) = this {
        "Function"
    } else if has_error_data_slot(e, this) {
        "Error"
    } else {
        "Object"
    };
    Ok(Value::String(Rc::from(format!("[object {tag}]"))))
}

/// Whether the object carries the `[[ErrorData]]` internal slot (stored as a
/// non-enumerable internal property).
fn has_error_data_slot(_e: &Engine, this: &Value) -> bool {
    match this {
        Value::Object(o) => o.borrow().props.contains_key("__error_data__"),
        _ => false,
    }
}

pub(crate) fn obj_value_of(_e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    if let Some(p) = this.primitive_value() {
        Ok(p)
    } else {
        Ok(this.clone())
    }
}

pub(crate) fn obj_has_own(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let key = key_arg(a.first().unwrap_or(&Value::Undefined));
    Ok(Value::Boolean(e.has_own(this, key.as_ref())))
}

pub(crate) fn obj_is_proto_of(_e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    let mut cur = match &v {
        Value::Object(o) => o.borrow().proto.clone(),
        Value::Array(arr) => arr.borrow().proto.clone(),
        Value::Function(f) => f.borrow().proto.clone(),
        Value::NativeFunction(nf) => nf.borrow().proto.clone(),
        _ => None,
    };
    while let Some(c) = cur {
        if let Value::Object(target) = this {
            if Rc::ptr_eq(&c, target) {
                return Ok(Value::Boolean(true));
            }
        }
        cur = c.borrow().proto.clone();
    }
    Ok(Value::Boolean(false))
}

pub(crate) fn obj_prop_enum(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let key = key_arg(a.first().unwrap_or(&Value::Undefined));
    Ok(Value::Boolean(e.prop_enumerable(this, key.as_ref()).unwrap_or(false)))
}

// --- Function.prototype ---

pub(crate) fn func_to_string(_e: &Engine, _this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::String(Rc::from("function () { /* native */ }")))
}

// --- Constructors ---

pub(crate) fn object_ctor(e: &Engine, this: &Value, a: &[Value], construct: bool) -> Result<Value, Error> {
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    if construct {
        match v {
            Value::Object(_) | Value::Array(_) | Value::Function(_) | Value::NativeFunction(_) => {
                Ok(v)
            }
            Value::String(_)
            | Value::Number(_)
            | Value::Boolean(_)
            | Value::BigInt(_)
            | Value::Symbol(_) => Ok(wrap(e, v)),
            _ => Ok(this.clone()),
        }
    } else {
        match v {
            Value::Object(_) | Value::Array(_) | Value::Function(_) | Value::NativeFunction(_) => {
                Ok(v)
            }
            // `Object(value)` == ToObject(value): primitives are wrapped.
            Value::String(_)
            | Value::Number(_)
            | Value::Boolean(_)
            | Value::BigInt(_)
            | Value::Symbol(_) => Ok(wrap(e, v)),
            _ => Ok(Value::Object(e.new_object())),
        }
    }
}

pub(crate) fn function_ctor(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    // `Function(p1, p2, ..., body)`: every argument but the last is a parameter
    // name, the last is the function body source.
    let (params_str, body_str) = if a.is_empty() {
        (Vec::new(), alloc::string::String::new())
    } else {
        let body = a.last().unwrap().to_string().to_string();
        let params: Vec<alloc::string::String> = a[..a.len() - 1]
            .iter()
            .map(|v| v.to_string().to_string())
            .collect();
        (params, body)
    };
    let params: Vec<Pattern> = params_str
        .iter()
        .flat_map(|s| s.split(','))
        .map(|p| Pattern::Ident(Rc::from(p.trim())))
        .collect();
    let prog = crate::parser::parse(&body_str)
        .map_err(|e| Error::Runtime(Value::String(Rc::from(e.to_string()))))?;
    // Create the function in the realm of the `Function` intrinsic being invoked
    // (so `$262.createRealm().global.Function` produces realm-`Function` values).
    let realm = e
        .native_realm
        .borrow()
        .clone()
        .unwrap_or_else(|| e.realm.clone());
    let closure = realm.global_env.clone();
    let fval = e.make_function_value(Rc::from(""), params, prog.stmts, closure);
    if let Value::Function(f) = &fval {
        f.borrow_mut().realm = Some(realm);
    }
    Ok(fval)
}

/// `AsyncFunction(p1, p2, ..., body)`: like `Function`, but the body runs as
/// an async function (created functions return promises).
pub(crate) fn async_function_ctor(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let (params_str, body_str) = if a.is_empty() {
        (Vec::new(), alloc::string::String::new())
    } else {
        let body = a.last().unwrap().to_string().to_string();
        let params: Vec<alloc::string::String> = a[..a.len() - 1]
            .iter()
            .map(|v| v.to_string().to_string())
            .collect();
        (params, body)
    };
    let params: Vec<Pattern> = params_str
        .iter()
        .flat_map(|s| s.split(','))
        .map(|p| Pattern::Ident(Rc::from(p.trim())))
        .collect();
    // Parse the body as an async function body (so `await`/`return` are
    // valid) by wrapping it in a synthetic declaration and unwrapping.
    let wrapped = alloc::format!("async function __async_fn_tmp(){{{body_str}}}");
    let prog = crate::parser::parse(&wrapped)
        .map_err(|e| Error::Runtime(Value::String(Rc::from(e.to_string()))))?;
    let body_stmts = prog
        .stmts
        .iter()
        .find_map(|s| match s {
            crate::ast::Stmt::FunctionDecl { name, body, .. }
                if name.as_ref() == "__async_fn_tmp" =>
            {
                Some(body.clone())
            }
            _ => None,
        })
        .ok_or_else(|| {
            Error::Runtime(Value::String(Rc::from(
                "AsyncFunction body parse failed",
            )))
        })?;
    let realm = e
        .native_realm
        .borrow()
        .clone()
        .unwrap_or_else(|| e.realm.clone());
    let closure = realm.global_env.clone();
    // The unwrapped declaration body is already `[AsyncBody(..)]` (the
    // wrapped source is async); pass it through so `make_function` flags it.
    let fval = e.make_function_value(Rc::from("anonymous"), params, body_stmts, closure);
    if let Value::Function(f) = &fval {
        f.borrow_mut().realm = Some(realm);
    }
    Ok(fval)
}

// --- Object statics ---

pub(crate) fn obj_keys(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    let mut keys = Vec::new();
    if let Value::Object(o) = &v {
        for (k, p) in o.borrow().props.iter() {
            if k.as_ref() == "__value__" || !p.enumerable {
                continue;
            }
            keys.push(Value::String(k.clone()));
        }
    }
    Ok(Value::Array(Rc::new(RefCell::new(ArrayData::new(keys, Some(e.array_prototype.clone()))))))
}

pub(crate) fn obj_values(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    let mut vals = Vec::new();
    if let Value::Object(o) = &v {
        for (k, p) in o.borrow().props.iter() {
            if k.as_ref() == "__value__" || !p.enumerable {
                continue;
            }
            vals.push(p.value.clone());
        }
    }
    Ok(Value::Array(Rc::new(RefCell::new(ArrayData::new(vals, Some(e.array_prototype.clone()))))))
}

pub(crate) fn obj_entries(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    let mut entries = Vec::new();
    if let Value::Object(o) = &v {
        for (k, p) in o.borrow().props.iter() {
            if k.as_ref() == "__value__" || !p.enumerable {
                continue;
            }
            let pair = vec![Value::String(k.clone()), p.value.clone()];
            entries.push(Value::Array(Rc::new(RefCell::new(ArrayData::new(pair, Some(e.array_prototype.clone()))))));
        }
    }
    Ok(Value::Array(Rc::new(RefCell::new(ArrayData::new(entries, Some(e.array_prototype.clone()))))))
}

pub(crate) fn obj_assign(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    if a.is_empty() {
        return Ok(Value::Undefined);
    }
    let target = a[0].clone();
    for src in &a[1..] {
        if let Value::Object(o) = src {
            for (k, p) in o.borrow().props.iter() {
                if k.as_ref() == "__value__" || !p.enumerable {
                    continue;
                }
                e.set_property(&target, k.as_ref(), p.value.clone())?;
            }
        }
    }
    Ok(target)
}

pub(crate) fn obj_create(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let proto = a.first().cloned().unwrap_or(Value::Null);
    let p = match proto {
        Value::Null => None,
        _ => proto_of(&proto),
    };
    let o = Rc::new(RefCell::new(Object {
        props: crate::value::new_props(),
        proto: p,
        ctor: None,
    }));
    if let Some(props) = a.get(1) {
        if let Value::Object(desc) = props {
            for (k, p) in desc.borrow().props.iter() {
                if let Value::Object(d) = &p.value {
                    let val = e.get_property(&Value::Object(d.clone()), "value");
                    o.borrow_mut().props.insert(k.clone(), Property::new(val));
                }
            }
        }
    }
    Ok(Value::Object(o))
}

pub(crate) fn obj_define_property(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let obj = a.first().cloned().unwrap_or(Value::Undefined);
    let key = key_arg(a.get(1).unwrap_or(&Value::Undefined));
    let desc = a.get(2).cloned().unwrap_or(Value::Undefined);
    e.define_property(&obj, key.as_ref(), &desc)?;
    Ok(obj)
}

pub(crate) fn obj_define_properties(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let obj = a.first().cloned().unwrap_or(Value::Undefined);
    let props = a.get(1).cloned().unwrap_or(Value::Undefined);
    if let Value::Object(o) = &props {
        for (k, p) in o.borrow().props.iter() {
            e.define_property(&obj, k.as_ref(), &p.value)?;
        }
    }
    Ok(obj)
}

pub(crate) fn obj_get_own_descriptor(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let obj = a.first().cloned().unwrap_or(Value::Undefined);
    let key = key_arg(a.get(1).unwrap_or(&Value::Undefined));
    Ok(e.get_own_descriptor(&obj, key.as_ref()))
}

pub(crate) fn obj_get_own_descriptors(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let obj = a.first().cloned().unwrap_or(Value::Undefined);
    let names = e.own_property_names(&obj);
    let d = e.new_object();
    for k in names {
        let desc = e.get_own_descriptor(&obj, k.as_ref());
        d.borrow_mut().props.insert(k, Property::new(desc));
    }
    Ok(Value::Object(d))
}

pub(crate) fn obj_get_own_names(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let obj = a.first().cloned().unwrap_or(Value::Undefined);
    let names = e.own_property_names(&obj)
        .into_iter()
        .map(Value::String)
        .collect();
    Ok(Value::Array(Rc::new(RefCell::new(ArrayData::new(names, Some(e.array_prototype.clone()))))))
}

pub(crate) fn obj_get_own_symbols(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let obj = a.first().cloned().unwrap_or(Value::Undefined);
    let mut syms = Vec::new();
    if e.own_symbol_props(&obj).len() > 0 {
        for s in e.own_symbol_props(&obj) {
            syms.push(Value::Symbol(s));
        }
    }
    Ok(Value::Array(Rc::new(RefCell::new(ArrayData::new(syms, Some(e.array_prototype.clone()))))))
}

pub(crate) fn obj_get_proto_of(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let obj = a.first().cloned().unwrap_or(Value::Undefined);
    // Native functions may have a function-value `[[Prototype]]` (NativeError
    // constructors chain to the `Error` constructor itself).
    if let Value::NativeFunction(nf) = &obj {
        if let Some(pv) = nf.borrow().fn_value_proto.clone() {
            return Ok(pv);
        }
    }
    if let Some(p) = proto_of(&obj) {
        return Ok(Value::Object(p));
    }
    // Collection/promise values carry no explicit `[[Prototype]]` until
    // `setPrototypeOf` overrides it; report the engine default.
    let fallback = match &obj {
        Value::Map(_) => Some(e.map_prototype.clone()),
        Value::Set(_) => Some(e.set_prototype.clone()),
        Value::WeakMap(_) => Some(e.weakmap_prototype.clone()),
        Value::WeakSet(_) => Some(e.weakset_prototype.clone()),
        Value::Promise(_) => Some(e.promise_prototype.clone()),
        _ => None,
    };
    Ok(fallback.map(Value::Object).unwrap_or(Value::Null))
}

pub(crate) fn obj_set_proto_of(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let obj = a.first().cloned().unwrap_or(Value::Undefined);
    let proto = match a.get(1).cloned().unwrap_or(Value::Null) {
        Value::Null => None,
        other => proto_of(&other),
    };
    set_proto_of_val(&obj, proto);
    Ok(obj)
}

pub(crate) fn obj_is(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let x = a.first().cloned().unwrap_or(Value::Undefined);
    let y = a.get(1).cloned().unwrap_or(Value::Undefined);
    Ok(Value::Boolean(Value::same_value(&x, &y)))
}

pub(crate) fn obj_identity(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(a.first().cloned().unwrap_or(Value::Undefined))
}

pub(crate) fn obj_true(_e: &Engine, _this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Boolean(true))
}

// --- Function.prototype.call / apply / bind ---

pub(crate) fn call_fn(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let this_fn = this.clone();
    let this_arg = a.first().cloned().unwrap_or(Value::Undefined);
    let rest: Vec<Value> = if a.len() > 1 { a[1..].to_vec() } else { Vec::new() };
    e.call_value(&this_fn, &this_arg, &rest)
}

pub(crate) fn apply_fn(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let this_fn = this.clone();
    let this_arg = a.first().cloned().unwrap_or(Value::Undefined);
    let rest = match a.get(1) {
        Some(Value::Array(arr)) => arr.borrow().elems.clone(),
        _ => Vec::new(),
    };
    e.call_value(&this_fn, &this_arg, &rest)
}

pub(crate) fn bind_fn(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let this_arg = a.first().cloned().unwrap_or(Value::Undefined);
    let bound: Vec<Value> = if a.len() > 1 { a[1..].to_vec() } else { Vec::new() };
    let bf = native(bound_invoke);
    if let Value::NativeFunction(nf) = &bf {
        nf.borrow_mut()
            .props
            .insert(Rc::from("__target__"), Property::new(this.clone()));
        nf.borrow_mut()
            .props
            .insert(Rc::from("__this__"), Property::new(this_arg));
        nf.borrow_mut()
            .props
            .insert(Rc::from("__args__"), Property::new(Value::Array(Rc::new(RefCell::new(ArrayData::new(bound, Some(e.array_prototype.clone())))))));
    }
    Ok(bf)
}

pub(crate) fn bound_invoke(_e: &Engine, _this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    Err(Error::Runtime(Value::String(Rc::from("internal: bound function leaked"))))
}
