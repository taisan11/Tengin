use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;

use libm::{self, ceil, exp, floor, fabs, log, pow, round, sin, cos, tan, sqrt, trunc};

use crate::error::Error;
use crate::interpreter::Engine;
use crate::value::{ArrayData, NativeFn, Object, Property, Value};

/// Fractional part of a float (no_std-compatible).
fn fract(x: f64) -> f64 {
    x - floor(x)
}

/// Create a `Value::NativeFunction` from a Rust implementation.
fn native(f: NativeFn) -> Value {
    crate::value::native(f)
}

/// Register a native constructor (e.g. `Object`, `Array`) together with its
/// `.prototype` and `.name` properties, and expose it as a global.
fn reg_ctor(engine: &mut Engine, name: &str, f: NativeFn, proto: Rc<RefCell<Object>>) -> Value {
    let v = native(f);
    if let Value::NativeFunction(nf) = &v {
        nf.borrow_mut().proto = Some(proto.clone());
        nf.borrow_mut()
            .props
            .insert(Rc::from("prototype"), Property::new(Value::Object(proto.clone())));
        nf.borrow_mut()
            .props
            .insert(Rc::from("name"), Property::new(Value::String(Rc::from(name))));
    }
    engine.register_global(name, v.clone());
    v
}

/// Install a method on a prototype object.
fn proto_method(proto: &Rc<RefCell<Object>>, name: &str, f: NativeFn) {
    proto
        .borrow_mut()
        .props
        .insert(Rc::from(name), Property::new(native(f)));
}

/// Install a data property on a prototype object.
fn proto_data(proto: &Rc<RefCell<Object>>, name: &str, val: Value) {
    proto
        .borrow_mut()
        .props
        .insert(Rc::from(name), Property::new(val));
}

/// Register all native globals/builtins on an engine instance.
pub fn register_builtins(engine: &mut Engine) {
    let object_proto = engine.object_prototype.clone();
    let array_proto = engine.array_prototype.clone();
    let string_proto = engine.string_prototype.clone();
    let number_proto = engine.number_prototype.clone();
    let boolean_proto = engine.boolean_prototype.clone();
    let error_proto = engine.error_prototype.clone();
    let function_proto = engine.function_prototype.clone();

    // --- Object.prototype ---
    proto_method(&object_proto, "toString", obj_to_string);
    proto_method(&object_proto, "valueOf", obj_value_of);
    proto_method(&object_proto, "hasOwnProperty", obj_has_own);
    proto_method(&object_proto, "isPrototypeOf", obj_is_proto_of);
    proto_method(&object_proto, "propertyIsEnumerable", obj_prop_enum);

    // --- Function.prototype ---
    proto_method(&function_proto, "call", call_fn);
    proto_method(&function_proto, "apply", apply_fn);
    proto_method(&function_proto, "bind", bind_fn);
    proto_method(&function_proto, "toString", func_to_string);

    // --- Array.prototype ---
    proto_method(&array_proto, "push", arr_push);
    proto_method(&array_proto, "pop", arr_pop);
    proto_method(&array_proto, "shift", arr_shift);
    proto_method(&array_proto, "unshift", arr_unshift);
    proto_method(&array_proto, "forEach", arr_for_each);
    proto_method(&array_proto, "map", arr_map);
    proto_method(&array_proto, "filter", arr_filter);
    proto_method(&array_proto, "indexOf", arr_index_of);
    proto_method(&array_proto, "lastIndexOf", arr_last_index_of);
    proto_method(&array_proto, "includes", arr_includes);
    proto_method(&array_proto, "slice", arr_slice);
    proto_method(&array_proto, "splice", arr_splice);
    proto_method(&array_proto, "concat", arr_concat);
    proto_method(&array_proto, "join", arr_join);
    proto_method(&array_proto, "toString", arr_join);
    proto_method(&array_proto, "reverse", arr_reverse);
    proto_method(&array_proto, "fill", arr_fill);
    proto_method(&array_proto, "find", arr_find);
    proto_method(&array_proto, "findIndex", arr_find_index);
    proto_method(&array_proto, "some", arr_some);
    proto_method(&array_proto, "every", arr_every);
    proto_method(&array_proto, "reduce", arr_reduce);
    proto_method(&array_proto, "at", arr_at);

    // --- String.prototype ---
    proto_method(&string_proto, "charAt", str_char_at);
    proto_method(&string_proto, "charCodeAt", str_char_code_at);
    proto_method(&string_proto, "codePointAt", str_code_point_at);
    proto_method(&string_proto, "indexOf", str_index_of);
    proto_method(&string_proto, "lastIndexOf", str_last_index_of);
    proto_method(&string_proto, "includes", str_includes);
    proto_method(&string_proto, "startsWith", str_starts_with);
    proto_method(&string_proto, "endsWith", str_ends_with);
    proto_method(&string_proto, "slice", str_slice);
    proto_method(&string_proto, "substring", str_substring);
    proto_method(&string_proto, "toUpperCase", str_upper);
    proto_method(&string_proto, "toLowerCase", str_lower);
    proto_method(&string_proto, "trim", str_trim);
    proto_method(&string_proto, "trimStart", str_trim_start);
    proto_method(&string_proto, "trimEnd", str_trim_end);
    proto_method(&string_proto, "concat", str_concat);
    proto_method(&string_proto, "split", str_split);
    proto_method(&string_proto, "repeat", str_repeat);
    proto_method(&string_proto, "replace", str_replace);
    proto_method(&string_proto, "padStart", str_pad_start);
    proto_method(&string_proto, "padEnd", str_pad_end);
    proto_method(&string_proto, "valueOf", str_value_of);
    proto_method(&string_proto, "toString", str_value_of);

    // --- Number.prototype ---
    proto_method(&number_proto, "toString", num_to_string);
    proto_method(&number_proto, "valueOf", num_value_of);
    proto_method(&number_proto, "toFixed", num_to_fixed);

    // --- Boolean.prototype ---
    proto_method(&boolean_proto, "toString", bool_to_string);
    proto_method(&boolean_proto, "valueOf", bool_value_of);

    // --- Error.prototype ---
    proto_method(&error_proto, "toString", err_to_string);
    proto_data(&error_proto, "name", Value::String(Rc::from("Error")));
    proto_data(&error_proto, "message", Value::String(Rc::from("")));

    // --- Global constructors ---
    let obj_ctor = reg_ctor(engine, "Object", object_ctor, object_proto.clone());
    proto_data(&object_proto, "constructor", obj_ctor.clone());
    let fn_ctor = reg_ctor(engine, "Function", function_ctor, function_proto.clone());
    proto_data(&function_proto, "constructor", fn_ctor.clone());
    let arr_ctor = reg_ctor(engine, "Array", array_ctor, array_proto.clone());
    proto_data(&array_proto, "constructor", arr_ctor.clone());
    let str_ctor = reg_ctor(engine, "String", string_ctor, string_proto.clone());
    proto_data(&string_proto, "constructor", str_ctor.clone());
    let num_ctor = reg_ctor(engine, "Number", number_ctor, number_proto.clone());
    proto_data(&number_proto, "constructor", num_ctor.clone());
    let bool_ctor = reg_ctor(engine, "Boolean", boolean_ctor, boolean_proto.clone());
    proto_data(&boolean_proto, "constructor", bool_ctor.clone());

    let obj_statics: &[(&str, NativeFn)] = &[
        ("keys", obj_keys),
        ("values", obj_values),
        ("entries", obj_entries),
        ("assign", obj_assign),
        ("create", obj_create),
        ("defineProperty", obj_define_property),
        ("getOwnPropertyDescriptor", obj_get_own_descriptor),
        ("getPrototypeOf", obj_get_proto_of),
        ("setPrototypeOf", obj_set_proto_of),
        ("is", obj_is),
        ("freeze", obj_identity),
        ("seal", obj_identity),
        ("preventExtensions", obj_identity),
        ("isFrozen", obj_true),
        ("isSealed", obj_true),
        ("isExtensible", obj_true),
    ];
    set_static(engine, &obj_ctor, obj_statics);

    let arr_ctor = engine_global(engine, "Array");
    set_static_value(
        &arr_ctor,
        &[
            ("isArray", native(arr_is_array)),
            ("from", native(arr_from)),
            ("of", native(arr_of)),
        ],
    );

    let num_ctor = engine_global(engine, "Number");
    set_static_value(
        &num_ctor,
        &[
            ("isNaN", native(num_is_nan)),
            ("isFinite", native(num_is_finite)),
            ("parseInt", native(int_parse)),
            ("parseFloat", native(float_parse)),
            ("MAX_VALUE", Value::Number(f64::MAX)),
            ("MIN_VALUE", Value::Number(f64::MIN_POSITIVE)),
            ("MAX_SAFE_INTEGER", Value::Number(9007199254740991.0)),
            ("MIN_SAFE_INTEGER", Value::Number(-9007199254740991.0)),
            ("POSITIVE_INFINITY", Value::Number(f64::INFINITY)),
            ("NEGATIVE_INFINITY", Value::Number(f64::NEG_INFINITY)),
            ("NaN", Value::Number(f64::NAN)),
            ("EPSILON", Value::Number(f64::EPSILON)),
        ],
    );

    let bool_ctor = engine_global(engine, "Boolean");
    set_static_value(&bool_ctor, &[("toString", native(bool_to_string))]);

    let err_ctor = reg_error(engine, "Error", error_ctor, error_proto.clone());
    proto_data(&error_proto, "constructor", err_ctor.clone());

    // Each error subclass gets its own prototype chained to `Error.prototype`,
    // each exposing a correct `constructor` property (needed by `assert.throws`).
    let _eval_err = reg_error_sub(engine, "EvalError", error_ctor, error_proto.clone());
    let _range_err = reg_error_sub(engine, "RangeError", error_ctor, error_proto.clone());
    let _ref_err = reg_error_sub(engine, "ReferenceError", error_ctor, error_proto.clone());
    let _syntax_err = reg_error_sub(engine, "SyntaxError", error_ctor, error_proto.clone());
    let _type_err = reg_error_sub(engine, "TypeError", error_ctor, error_proto.clone());
    let _uri_err = reg_error_sub(engine, "URIError", error_ctor, error_proto.clone());

    // Math.
    let mut math = Object::with_proto(object_proto.clone());
    let math_entries: [(&str, NativeFn); 16] = [
        ("abs", math_abs),
        ("floor", math_floor),
        ("ceil", math_ceil),
        ("round", math_round),
        ("trunc", math_trunc),
        ("max", math_max),
        ("min", math_min),
        ("sqrt", math_sqrt),
        ("pow", math_pow),
        ("sign", math_sign),
        ("exp", math_exp),
        ("log", math_log),
        ("sin", math_sin),
        ("cos", math_cos),
        ("tan", math_tan),
        ("random", math_random),
    ];
    for (n, f) in math_entries {
        math.props.insert(Rc::from(n), Property::new(native(f)));
    }
    for (n, v) in [
        ("E", Value::Number(core::f64::consts::E)),
        ("PI", Value::Number(core::f64::consts::PI)),
        ("LN10", Value::Number(core::f64::consts::LN_10)),
        ("LN2", Value::Number(core::f64::consts::LN_2)),
        ("LOG10E", Value::Number(core::f64::consts::LOG10_E)),
        ("LOG2E", Value::Number(core::f64::consts::LOG2_E)),
        ("SQRT2", Value::Number(core::f64::consts::SQRT_2)),
        ("SQRT1_2", Value::Number(core::f64::consts::FRAC_1_SQRT_2)),
    ] {
        math.props.insert(Rc::from(n), Property::new(v));
    }
    engine.register_global("Math", Value::Object(Rc::new(RefCell::new(math))));

    // JSON.
    let mut json = Object::with_proto(object_proto.clone());
    json.props
        .insert(Rc::from("stringify"), Property::new(native(json_stringify)));
    json.props
        .insert(Rc::from("parse"), Property::new(native(json_parse)));
    engine.register_global("JSON", Value::Object(Rc::new(RefCell::new(json))));

    engine.register_global("isNaN", native(is_nan_fn));
    engine.register_global("isFinite", native(is_finite_fn));
    engine.register_global("parseInt", native(int_parse));
    engine.register_global("parseFloat", native(float_parse));
    engine.register_global("decodeURI", native(decode_uri));
    engine.register_global("encodeURI", native(encode_uri));
    engine.register_global("Infinity", Value::Number(f64::INFINITY));
    engine.register_global("NaN", Value::Number(f64::NAN));
    engine.register_global("undefined", Value::Undefined);
    engine.register_global("globalThis", Value::Object(engine.global_object.clone()));

    engine.register_global("assert", make_assert(engine));
    engine.register_global("Test262Error", native(error_ctor));
    engine.register_global("$DONOTEVALUATE", native(donotevaluate_fn));
}

// --- helpers for static methods ---

fn engine_global(engine: &Engine, name: &str) -> Value {
    engine
        .global_env
        .borrow()
        .get(name)
        .unwrap_or(Value::Undefined)
}

fn set_static(engine: &Engine, ctor: &Value, items: &[(&str, NativeFn)]) {
    if let Value::NativeFunction(nf) = ctor {
        for (n, f) in items {
            nf.borrow_mut()
                .props
                .insert(Rc::from(*n), Property::new(native(*f)));
        }
    }
    let _ = engine;
}

fn set_static_value(ctor: &Value, items: &[(&str, Value)]) {
    if let Value::NativeFunction(nf) = ctor {
        for (n, v) in items {
            nf.borrow_mut()
                .props
                .insert(Rc::from(*n), Property::new(v.clone()));
        }
    }
}

fn reg_error(engine: &mut Engine, name: &str, f: NativeFn, proto: Rc<RefCell<Object>>) -> Value {
    let v = native(f);
    if let Value::NativeFunction(nf) = &v {
        nf.borrow_mut().proto = Some(proto.clone());
        nf.borrow_mut()
            .props
            .insert(Rc::from("prototype"), Property::new(Value::Object(proto.clone())));
        nf.borrow_mut()
            .props
            .insert(Rc::from("name"), Property::new(Value::String(Rc::from(name))));
    }
    engine.register_global(name, v.clone());
    v
}

/// Register a subclass of `Error` with its own prototype linked to `parent`.
fn reg_error_sub(
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

// --- wrapper objects ---

fn wrap(engine: &Engine, primitive: Value) -> Value {
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

// --- Object.prototype ---

fn obj_to_string(_e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
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
    } else {
        "Object"
    };
    Ok(Value::String(Rc::from(format!("[object {tag}]"))))
}

fn obj_value_of(_e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    if let Some(p) = this.primitive_value() {
        Ok(p)
    } else {
        Ok(this.clone())
    }
}

fn obj_has_own(_e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let key = a.first().cloned().unwrap_or(Value::Undefined).to_string();
    let has = match this {
        Value::Object(o) => o.borrow().props.contains_key(key.as_ref()),
        Value::Array(arr) => key.as_ref() == "length" || key.parse::<usize>().is_ok(),
        _ => false,
    };
    Ok(Value::Boolean(has))
}

fn obj_is_proto_of(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
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

fn obj_prop_enum(_e: &Engine, _this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Boolean(true))
}

// --- Function.prototype ---

fn func_to_string(_e: &Engine, _this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::String(Rc::from("function () { /* native */ }")))
}

// --- Array.prototype ---

fn as_array(this: &Value) -> Option<Rc<RefCell<ArrayData>>> {
    if let Value::Array(a) = this {
        Some(a.clone())
    } else {
        None
    }
}

fn arr_push(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let arr = as_array(this).ok_or_else(|| Error::Runtime(Value::String(Rc::from("not an array"))))?;
    for v in a {
        arr.borrow_mut().elems.push(v.clone());
    }
    Ok(Value::Number(arr.borrow().elems.len() as f64))
}

fn arr_pop(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    let arr = as_array(this).ok_or_else(|| Error::Runtime(Value::String(Rc::from("not an array"))))?;
    Ok(arr.borrow_mut().elems.pop().unwrap_or(Value::Undefined))
}

fn arr_shift(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    let arr = as_array(this).ok_or_else(|| Error::Runtime(Value::String(Rc::from("not an array"))))?;
    if arr.borrow().elems.is_empty() {
        return Ok(Value::Undefined);
    }
    Ok(arr.borrow_mut().elems.remove(0))
}

fn arr_unshift(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let arr = as_array(this).ok_or_else(|| Error::Runtime(Value::String(Rc::from("not an array"))))?;
    let mut elems = arr.borrow_mut();
    for v in a.iter().rev() {
        elems.elems.insert(0, v.clone());
    }
    Ok(Value::Number(elems.elems.len() as f64))
}

fn arr_for_each(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let arr = as_array(this).ok_or_else(|| Error::Runtime(Value::String(Rc::from("not an array"))))?;
    let cb = a.first().cloned().unwrap_or(Value::Undefined);
    let elems = arr.borrow().elems.clone();
    for (i, v) in elems.iter().enumerate() {
        e.call_value(&cb, &Value::Undefined, &[v.clone(), Value::Number(i as f64), this.clone()])?;
    }
    Ok(Value::Undefined)
}

fn arr_map(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let arr = as_array(this).ok_or_else(|| Error::Runtime(Value::String(Rc::from("not an array"))))?;
    let cb = a.first().cloned().unwrap_or(Value::Undefined);
    let elems = arr.borrow().elems.clone();
    let mut out = Vec::new();
    for (i, v) in elems.iter().enumerate() {
        let r = e.call_value(&cb, &Value::Undefined, &[v.clone(), Value::Number(i as f64), this.clone()])?;
        out.push(r);
    }
    Ok(Value::Array(Rc::new(RefCell::new(ArrayData::new(out, Some(e.array_prototype.clone()))))))
}

fn arr_filter(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let arr = as_array(this).ok_or_else(|| Error::Runtime(Value::String(Rc::from("not an array"))))?;
    let cb = a.first().cloned().unwrap_or(Value::Undefined);
    let elems = arr.borrow().elems.clone();
    let mut out = Vec::new();
    for (i, v) in elems.iter().enumerate() {
        let keep = e.call_value(&cb, &Value::Undefined, &[v.clone(), Value::Number(i as f64), this.clone()])?;
        if keep.to_boolean() {
            out.push(v.clone());
        }
    }
    Ok(Value::Array(Rc::new(RefCell::new(ArrayData::new(out, Some(e.array_prototype.clone()))))))
}

fn arr_index_of(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let arr = as_array(this).ok_or_else(|| Error::Runtime(Value::String(Rc::from("not an array"))))?;
    let target = a.first().cloned().unwrap_or(Value::Undefined);
    let from = a.get(1).and_then(|v| {
        if *v == Value::Undefined { None } else { Some(v.to_number() as usize) }
    }).unwrap_or(0);
    let elems = arr.borrow().elems.clone();
    for (i, v) in elems.iter().enumerate().skip(from) {
        if Value::strict_eq(v, &target) {
            return Ok(Value::Number(i as f64));
        }
    }
    Ok(Value::Number(-1.0))
}

fn arr_last_index_of(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let arr = as_array(this).ok_or_else(|| Error::Runtime(Value::String(Rc::from("not an array"))))?;
    let target = a.first().cloned().unwrap_or(Value::Undefined);
    let elems = arr.borrow().elems.clone();
    for i in (0..elems.len()).rev() {
        if Value::strict_eq(&elems[i], &target) {
            return Ok(Value::Number(i as f64));
        }
    }
    Ok(Value::Number(-1.0))
}

fn arr_includes(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let arr = as_array(this).ok_or_else(|| Error::Runtime(Value::String(Rc::from("not an array"))))?;
    let target = a.first().cloned().unwrap_or(Value::Undefined);
    let elems = arr.borrow().elems.clone();
    for v in &elems {
        if Value::same_value(v, &target) {
            return Ok(Value::Boolean(true));
        }
    }
    Ok(Value::Boolean(false))
}

fn arr_slice(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let arr = as_array(this).ok_or_else(|| Error::Runtime(Value::String(Rc::from("not an array"))))?;
    let elems = arr.borrow().elems.clone();
    let len = elems.len();
    let mut start = a.first().map(|v| v.to_number() as isize).unwrap_or(0);
    if start < 0 {
        start += len as isize;
    }
    start = start.clamp(0, len as isize);
    let mut end = a.get(1).map(|v| v.to_number() as isize).unwrap_or(len as isize);
    if end < 0 {
        end += len as isize;
    }
    end = end.clamp(0, len as isize);
    let out: Vec<Value> = elems[start as usize..end as usize].to_vec();
    Ok(Value::Array(Rc::new(RefCell::new(ArrayData::new(out, Some(e.array_prototype.clone()))))))
}

fn arr_splice(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let arr = as_array(this).ok_or_else(|| Error::Runtime(Value::String(Rc::from("not an array"))))?;
    if a.is_empty() {
        return Ok(Value::Array(Rc::new(RefCell::new(ArrayData::new(Vec::new(), Some(e.array_prototype.clone()))))));
    }
    let len = arr.borrow().elems.len();
    let start = a[0].to_number() as isize;
    let start = if start < 0 { (len as isize + start).clamp(0, len as isize) } else { start.clamp(0, len as isize) } as usize;
    let delete = if a.len() > 1 { a[1].to_number() as usize } else { len - start };
    let mut elems = arr.borrow_mut();
    let removed: Vec<Value> = elems.elems.drain(start..(start + delete).min(len)).collect();
    for item in a[2..].iter().rev() {
        elems.elems.insert(start, item.clone());
    }
    Ok(Value::Array(Rc::new(RefCell::new(ArrayData::new(removed, Some(e.array_prototype.clone()))))))
}

fn arr_concat(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let mut out = Vec::new();
    if let Some(arr) = as_array(this) {
        out.extend(arr.borrow().elems.clone());
    } else {
        out.push(this.clone());
    }
    for v in a {
        if let Some(arr) = as_array(v) {
            out.extend(arr.borrow().elems.clone());
        } else {
            out.push(v.clone());
        }
    }
    Ok(Value::Array(Rc::new(RefCell::new(ArrayData::new(out, Some(e.array_prototype.clone()))))))
}

fn arr_join(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let arr = as_array(this).ok_or_else(|| Error::Runtime(Value::String(Rc::from("not an array"))))?;
    let sep = match a.first() {
        Some(Value::String(s)) => s.to_string(),
        Some(Value::Undefined) | None => ",".to_string(),
        Some(v) => v.to_string().to_string(),
    };
    let elems = arr.borrow().elems.clone();
    let parts: Vec<String> = elems
        .iter()
        .map(|v| if *v == Value::Undefined || *v == Value::Null { String::new() } else { v.to_string().to_string() })
        .collect();
    Ok(Value::String(Rc::from(parts.join(&sep))))
}

fn arr_reverse(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    let arr = as_array(this).ok_or_else(|| Error::Runtime(Value::String(Rc::from("not an array"))))?;
    arr.borrow_mut().elems.reverse();
    Ok(this.clone())
}

fn arr_fill(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let arr = as_array(this).ok_or_else(|| Error::Runtime(Value::String(Rc::from("not an array"))))?;
    let val = a.first().cloned().unwrap_or(Value::Undefined);
    let len = arr.borrow().elems.len();
    let mut start = a.get(1).map(|v| v.to_number() as usize).unwrap_or(0).min(len);
    let mut end = a.get(2).map(|v| v.to_number() as usize).unwrap_or(len).min(len);
    if let Some(Value::Number(n)) = a.get(1) {
        if n.is_nan() { start = 0; }
    }
    if let Some(Value::Number(n)) = a.get(2) {
        if n.is_nan() { end = 0; }
    }
    for i in start..end {
        arr.borrow_mut().elems[i] = val.clone();
    }
    Ok(this.clone())
}

fn arr_find(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let arr = as_array(this).ok_or_else(|| Error::Runtime(Value::String(Rc::from("not an array"))))?;
    let cb = a.first().cloned().unwrap_or(Value::Undefined);
    let elems = arr.borrow().elems.clone();
    for (i, v) in elems.iter().enumerate() {
        let r = e.call_value(&cb, &Value::Undefined, &[v.clone(), Value::Number(i as f64), this.clone()])?;
        if r.to_boolean() {
            return Ok(v.clone());
        }
    }
    Ok(Value::Undefined)
}

fn arr_find_index(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let arr = as_array(this).ok_or_else(|| Error::Runtime(Value::String(Rc::from("not an array"))))?;
    let cb = a.first().cloned().unwrap_or(Value::Undefined);
    let elems = arr.borrow().elems.clone();
    for (i, v) in elems.iter().enumerate() {
        let r = e.call_value(&cb, &Value::Undefined, &[v.clone(), Value::Number(i as f64), this.clone()])?;
        if r.to_boolean() {
            return Ok(Value::Number(i as f64));
        }
    }
    Ok(Value::Number(-1.0))
}

fn arr_some(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let arr = as_array(this).ok_or_else(|| Error::Runtime(Value::String(Rc::from("not an array"))))?;
    let cb = a.first().cloned().unwrap_or(Value::Undefined);
    let elems = arr.borrow().elems.clone();
    for (i, v) in elems.iter().enumerate() {
        let r = e.call_value(&cb, &Value::Undefined, &[v.clone(), Value::Number(i as f64), this.clone()])?;
        if r.to_boolean() {
            return Ok(Value::Boolean(true));
        }
    }
    Ok(Value::Boolean(false))
}

fn arr_every(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let arr = as_array(this).ok_or_else(|| Error::Runtime(Value::String(Rc::from("not an array"))))?;
    let cb = a.first().cloned().unwrap_or(Value::Undefined);
    let elems = arr.borrow().elems.clone();
    for (i, v) in elems.iter().enumerate() {
        let r = e.call_value(&cb, &Value::Undefined, &[v.clone(), Value::Number(i as f64), this.clone()])?;
        if !r.to_boolean() {
            return Ok(Value::Boolean(false));
        }
    }
    Ok(Value::Boolean(true))
}

fn arr_reduce(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let arr = as_array(this).ok_or_else(|| Error::Runtime(Value::String(Rc::from("not an array"))))?;
    let cb = a.first().cloned().unwrap_or(Value::Undefined);
    let mut acc = a.get(1).cloned().unwrap_or(Value::Undefined);
    let elems = arr.borrow().elems.clone();
    for (i, v) in elems.iter().enumerate() {
        acc = e.call_value(&cb, &Value::Undefined, &[acc, v.clone(), Value::Number(i as f64), this.clone()])?;
    }
    Ok(acc)
}

fn arr_at(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let arr = as_array(this).ok_or_else(|| Error::Runtime(Value::String(Rc::from("not an array"))))?;
    let len = arr.borrow().elems.len();
    let mut i = a.first().map(|v| v.to_number() as isize).unwrap_or(0);
    if i < 0 {
        i += len as isize;
    }
    if i < 0 || i >= len as isize {
        return Ok(Value::Undefined);
    }
    Ok(arr.borrow().elems[i as usize].clone())
}

// --- String.prototype ---

fn as_string(this: &Value, e: &Engine) -> String {
    e.this_primitive(this).to_string().to_string()
}

fn str_char_at(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = as_string(this, e);
    let i = a.first().map(|v| v.to_number() as isize).unwrap_or(0);
    if i < 0 || i >= s.chars().count() as isize {
        return Ok(Value::String(Rc::from("")));
    }
    let ch = s.chars().nth(i as usize).unwrap();
    Ok(Value::String(Rc::from(ch.to_string())))
}

fn str_char_code_at(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = as_string(this, e);
    let i = a.first().map(|v| v.to_number() as isize).unwrap_or(0);
    if i < 0 || i >= s.chars().count() as isize {
        return Ok(Value::Number(f64::NAN));
    }
    let ch = s.chars().nth(i as usize).unwrap();
    Ok(Value::Number(ch as u32 as f64))
}

fn str_code_point_at(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    str_char_code_at(e, this, a, _c)
}

fn str_index_of(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = as_string(this, e);
    let pat = a.first().map(|v| v.to_string().to_string()).unwrap_or_default();
    let from = a.get(1).map(|v| v.to_number() as usize).unwrap_or(0);
    if from > 0 {
        let prefix: String = s.chars().take(from).collect();
        if let Some(idx) = s[prefix.len()..].find(&pat) {
            return Ok(Value::Number((prefix.chars().count() + idx) as f64));
        }
        return Ok(Value::Number(-1.0));
    }
    match s.find(&pat) {
        Some(i) => Ok(Value::Number(s[..i].chars().count() as f64)),
        None => Ok(Value::Number(-1.0)),
    }
}

fn str_last_index_of(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = as_string(this, e);
    let pat = a.first().map(|v| v.to_string().to_string()).unwrap_or_default();
    match s.rfind(&pat) {
        Some(i) => Ok(Value::Number(s[..i].chars().count() as f64)),
        None => Ok(Value::Number(-1.0)),
    }
}

fn str_includes(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = as_string(this, e);
    let pat = a.first().map(|v| v.to_string().to_string()).unwrap_or_default();
    Ok(Value::Boolean(s.contains(&pat)))
}

fn str_starts_with(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = as_string(this, e);
    let pat = a.first().map(|v| v.to_string().to_string()).unwrap_or_default();
    Ok(Value::Boolean(s.starts_with(&pat)))
}

fn str_ends_with(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = as_string(this, e);
    let pat = a.first().map(|v| v.to_string().to_string()).unwrap_or_default();
    Ok(Value::Boolean(s.ends_with(&pat)))
}

fn str_slice(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = as_string(this, e);
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    let mut start = a.first().map(|v| v.to_number() as isize).unwrap_or(0);
    if start < 0 { start += len as isize; }
    start = start.clamp(0, len as isize);
    let mut end = a.get(1).map(|v| v.to_number() as isize).unwrap_or(len as isize);
    if end < 0 { end += len as isize; }
    end = end.clamp(0, len as isize);
    let out: String = chars[start as usize..end as usize].iter().collect();
    Ok(Value::String(Rc::from(out)))
}

fn str_substring(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = as_string(this, e);
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    let mut start = a.first().map(|v| v.to_number()).unwrap_or(0.0);
    if start < 0.0 || start.is_nan() { start = 0.0; }
    let mut end = a.get(1).map(|v| v.to_number()).unwrap_or(len as f64);
    if end < 0.0 || end.is_nan() { end = 0.0; }
    let (lo, hi) = if start > end { (end as usize, start as usize) } else { (start as usize, end as usize) };
    let hi = hi.min(len);
    let out: String = chars[lo..hi].iter().collect();
    Ok(Value::String(Rc::from(out)))
}

fn str_upper(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = as_string(this, e);
    Ok(Value::String(Rc::from(s.to_uppercase())))
}

fn str_lower(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = as_string(this, e);
    Ok(Value::String(Rc::from(s.to_lowercase())))
}

fn str_trim(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = as_string(this, e);
    Ok(Value::String(Rc::from(s.trim())))
}

fn str_trim_start(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = as_string(this, e);
    Ok(Value::String(Rc::from(s.trim_start())))
}

fn str_trim_end(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = as_string(this, e);
    Ok(Value::String(Rc::from(s.trim_end())))
}

fn str_concat(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let mut s = as_string(this, e);
    for v in a {
        s.push_str(&v.to_string());
    }
    Ok(Value::String(Rc::from(s)))
}

fn str_split(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = as_string(this, e);
    let pat = a.first().map(|v| v.to_string().to_string()).unwrap_or_default();
    let parts: Vec<Value> = if pat.is_empty() {
        s.chars().map(|c| Value::String(Rc::from(c.to_string()))).collect()
    } else {
        s.split(&pat).map(|p| Value::String(Rc::from(p))).collect()
    };
    Ok(Value::Array(Rc::new(RefCell::new(ArrayData::new(parts, Some(e.array_prototype.clone()))))))
}

fn str_repeat(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = as_string(this, e);
    let n = a.first().map(|v| v.to_number()).unwrap_or(0.0);
    if n < 0.0 || n.is_nan() {
        return Err(Error::Runtime(Value::String(Rc::from("Invalid count value"))));
    }
    let n = n as usize;
    if n > 1_000_000_000 {
        return Err(Error::Runtime(Value::String(Rc::from("too many repeats"))));
    }
    Ok(Value::String(Rc::from(s.repeat(n))))
}

fn str_replace(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = as_string(this, e);
    let pat = a.first().map(|v| v.to_string().to_string()).unwrap_or_default();
    let rep = a.get(1).map(|v| v.to_string().to_string()).unwrap_or_default();
    Ok(Value::String(Rc::from(s.replacen(&pat, &rep, 1))))
}

fn str_pad_start(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    str_pad(this, e, a, true)
}

fn str_pad_end(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    str_pad(this, e, a, false)
}

fn str_pad(this: &Value, e: &Engine, a: &[Value], start: bool) -> Result<Value, Error> {
    let s = as_string(this, e);
    let target = a.first().map(|v| v.to_number()).unwrap_or(0.0) as usize;
    let pad = a.get(1).map(|v| v.to_string().to_string()).unwrap_or_else(|| " ".to_string());
    if s.chars().count() >= target || pad.is_empty() {
        return Ok(Value::String(Rc::from(s)));
    }
    let need = target - s.chars().count();
    let mut fill = String::new();
    while fill.chars().count() < need {
        fill.push_str(&pad);
    }
    let fill: String = fill.chars().take(need).collect();
    let out = if start { format!("{fill}{s}") } else { format!("{s}{fill}") };
    Ok(Value::String(Rc::from(out)))
}

fn str_value_of(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(e.this_primitive(this))
}

// --- Number.prototype ---

fn num_to_string(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let n = e.this_primitive(this).to_number();
    let radix = a.first().map(|v| v.to_number()).unwrap_or(10.0);
    let s = if fabs(radix - 10.0) < 1e-9 {
        n.to_string()
    } else if radix == 2.0 {
        if fract(n) == 0.0 && n.is_finite() { format!("{:b}", n as i64) } else { n.to_string() }
    } else if radix == 16.0 {
        if fract(n) == 0.0 && n.is_finite() { format!("{:x}", n as i64) } else { n.to_string() }
    } else if radix == 8.0 {
        if fract(n) == 0.0 && n.is_finite() { format!("{:o}", n as i64) } else { n.to_string() }
    } else {
        n.to_string()
    };
    Ok(Value::String(Rc::from(s)))
}

fn num_value_of(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(e.this_primitive(this))
}

fn num_to_fixed(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let n = e.this_primitive(this).to_number();
    let digits = a.first().map(|v| v.to_number()).unwrap_or(0.0) as usize;
    let digits = digits.min(20);
    Ok(Value::String(Rc::from(format!("{0:.1$}", n, digits))))
}

// --- Boolean.prototype ---

fn bool_to_string(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    let b = e.this_primitive(this).to_boolean();
    Ok(Value::String(Rc::from(if b { "true" } else { "false" })))
}

fn bool_value_of(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(e.this_primitive(this))
}

// --- Error.prototype ---

fn err_to_string(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    let name = e.get_property(this, "name");
    let message = e.get_property(this, "message");
    let name = if name == Value::Undefined { Value::String(Rc::from("Error")) } else { name };
    if message == Value::Undefined || message.to_string().is_empty() {
        Ok(name)
    } else {
        Ok(Value::String(Rc::from(format!("{}: {}", name.to_string(), message.to_string()))))
    }
}

// --- Constructors ---

fn object_ctor(e: &Engine, this: &Value, a: &[Value], construct: bool) -> Result<Value, Error> {
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    if construct {
        match v {
            Value::Object(_) | Value::Array(_) | Value::Function(_) | Value::NativeFunction(_) => {
                Ok(v)
            }
            Value::String(_) | Value::Number(_) | Value::Boolean(_) => Ok(wrap(e, v)),
            _ => Ok(this.clone()),
        }
    } else {
        match v {
            Value::Object(_) | Value::Array(_) | Value::Function(_) | Value::NativeFunction(_) => {
                Ok(v)
            }
            _ => Ok(Value::Object(e.new_object())),
        }
    }
}

fn function_ctor(_e: &Engine, _this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    Err(Error::Runtime(Value::String(Rc::from(
        "Function constructor is not supported",
    ))))
}

fn array_ctor(e: &Engine, _this: &Value, a: &[Value], _construct: bool) -> Result<Value, Error> {
    if a.len() == 1 && matches!(a[0], Value::Number(_)) {
        let n = a[0].to_number() as usize;
        if fract(a[0].to_number()) != 0.0 || n > 1_000_000_000 {
            return Err(Error::Runtime(Value::String(Rc::from("Invalid array length"))));
        }
        return Ok(Value::Array(Rc::new(RefCell::new(ArrayData::new(
            vec![Value::Undefined; n],
            Some(e.array_prototype.clone()),
        )))));
    }
    let elems = a.to_vec();
    Ok(Value::Array(Rc::new(RefCell::new(ArrayData::new(
        elems,
        Some(e.array_prototype.clone()),
    )))))
}

fn string_ctor(e: &Engine, _this: &Value, a: &[Value], construct: bool) -> Result<Value, Error> {
    let s = a.first().cloned().unwrap_or(Value::Undefined).to_string();
    if construct {
        Ok(wrap(e, Value::String(s)))
    } else {
        Ok(Value::String(s))
    }
}

fn number_ctor(e: &Engine, _this: &Value, a: &[Value], construct: bool) -> Result<Value, Error> {
    let n = a.first().cloned().unwrap_or(Value::Undefined).to_number();
    if construct {
        Ok(wrap(e, Value::Number(n)))
    } else {
        Ok(Value::Number(n))
    }
}

fn boolean_ctor(e: &Engine, _this: &Value, a: &[Value], construct: bool) -> Result<Value, Error> {
    let b = a.first().cloned().unwrap_or(Value::Undefined).to_boolean();
    if construct {
        Ok(wrap(e, Value::Boolean(b)))
    } else {
        Ok(Value::Boolean(b))
    }
}

fn error_ctor(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let msg = a.first().cloned().unwrap_or(Value::Undefined);
    e.set_property(this, "message", msg);
    if e.get_property(this, "name") == Value::Undefined {
        e.set_property(this, "name", Value::String(Rc::from("Error")));
    }
    Ok(this.clone())
}

// --- Object statics ---

fn obj_keys(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    let mut keys = Vec::new();
    if let Value::Object(o) = &v {
        for k in o.borrow().props.keys() {
            if k.as_ref() == "__value__" {
                continue;
            }
            keys.push(Value::String(k.clone()));
        }
    }
    Ok(Value::Array(Rc::new(RefCell::new(ArrayData::new(keys, Some(e.array_prototype.clone()))))))
}

fn obj_values(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    let mut vals = Vec::new();
    if let Value::Object(o) = &v {
        for (k, p) in o.borrow().props.iter() {
            if k.as_ref() == "__value__" {
                continue;
            }
            vals.push(p.value.clone());
        }
    }
    Ok(Value::Array(Rc::new(RefCell::new(ArrayData::new(vals, Some(e.array_prototype.clone()))))))
}

fn obj_entries(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    let mut entries = Vec::new();
    if let Value::Object(o) = &v {
        for (k, p) in o.borrow().props.iter() {
            if k.as_ref() == "__value__" {
                continue;
            }
            let pair = vec![Value::String(k.clone()), p.value.clone()];
            entries.push(Value::Array(Rc::new(RefCell::new(ArrayData::new(pair, Some(e.array_prototype.clone()))))));
        }
    }
    Ok(Value::Array(Rc::new(RefCell::new(ArrayData::new(entries, Some(e.array_prototype.clone()))))))
}

fn obj_assign(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    if a.is_empty() {
        return Ok(Value::Undefined);
    }
    let target = a[0].clone();
    for src in &a[1..] {
        if let Value::Object(o) = src {
            for (k, p) in o.borrow().props.iter() {
                if k.as_ref() == "__value__" {
                    continue;
                }
                e.set_property(&target, k.as_ref(), p.value.clone());
            }
        }
    }
    Ok(target)
}

/// Extract the `[[Prototype]]` of any value (or `None` for primitives).
fn proto_of(v: &Value) -> Option<Rc<RefCell<Object>>> {
    match v {
        Value::Object(o) => o.borrow().proto.clone(),
        Value::Array(a) => a.borrow().proto.clone(),
        Value::Function(f) => f.borrow().proto.clone(),
        Value::NativeFunction(nf) => nf.borrow().proto.clone(),
        _ => None,
    }
}

/// Set the `[[Prototype]]` of any object-like value.
fn set_proto_of_val(v: &Value, p: Option<Rc<RefCell<Object>>>) {
    match v {
        Value::Object(o) => o.borrow_mut().proto = p,
        Value::Array(a) => a.borrow_mut().proto = p,
        Value::Function(f) => f.borrow_mut().proto = p,
        Value::NativeFunction(nf) => nf.borrow_mut().proto = p,
        _ => {}
    }
}

fn obj_create(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let proto = a.first().cloned().unwrap_or(Value::Null);
    let p = match proto {
        Value::Null => None,
        _ => proto_of(&proto),
    };
    let o = Rc::new(RefCell::new(Object { props: crate::value::new_props(), proto: p }));
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

fn obj_define_property(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let obj = a.first().cloned().unwrap_or(Value::Undefined);
    let key = a.get(1).cloned().unwrap_or(Value::Undefined).to_string();
    let desc = a.get(2).cloned().unwrap_or(Value::Undefined);
    let val = if let Value::Object(_) = &desc { e.get_property(&desc, "value") } else { Value::Undefined };
    e.set_property(&obj, key.as_ref(), val);
    Ok(obj)
}

fn obj_get_own_descriptor(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let obj = a.first().cloned().unwrap_or(Value::Undefined);
    let key = a.get(1).cloned().unwrap_or(Value::Undefined).to_string();
    if let Value::Object(o) = &obj {
        if let Some(p) = o.borrow().props.get(key.as_ref()) {
            let d = e.new_object();
            e.set_property(&Value::Object(d.clone()), "value", p.value.clone());
            e.set_property(&Value::Object(d.clone()), "writable", Value::Boolean(p.writable));
            e.set_property(&Value::Object(d.clone()), "enumerable", Value::Boolean(p.enumerable));
            e.set_property(&Value::Object(d.clone()), "configurable", Value::Boolean(p.configurable));
            return Ok(Value::Object(d));
        }
    }
    Ok(Value::Undefined)
}

fn obj_get_proto_of(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let obj = a.first().cloned().unwrap_or(Value::Undefined);
    Ok(proto_of(&obj).map(Value::Object).unwrap_or(Value::Null))
}

fn obj_set_proto_of(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let obj = a.first().cloned().unwrap_or(Value::Undefined);
    let proto = match a.get(1).cloned().unwrap_or(Value::Null) {
        Value::Null => None,
        other => proto_of(&other),
    };
    set_proto_of_val(&obj, proto);
    Ok(obj)
}

fn obj_is(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let x = a.first().cloned().unwrap_or(Value::Undefined);
    let y = a.get(1).cloned().unwrap_or(Value::Undefined);
    Ok(Value::Boolean(Value::same_value(&x, &y)))
}

fn obj_identity(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(a.first().cloned().unwrap_or(Value::Undefined))
}

fn obj_true(_e: &Engine, _this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Boolean(true))
}

// --- Array statics ---

fn arr_is_array(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Boolean(matches!(a.first(), Some(Value::Array(_)))))
}

fn arr_from(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let src = a.first().cloned().unwrap_or(Value::Undefined);
    let mut out = Vec::new();
    if let Value::Array(arr) = &src {
        out.extend(arr.borrow().elems.clone());
    } else if let Value::Object(o) = &src {
        if let Some(len) = o.borrow().props.get("length") {
            let n = len.value.to_number() as usize;
            for i in 0..n {
                let key = Rc::from(i.to_string());
                if let Some(p) = o.borrow().props.get(&key) {
                    out.push(p.value.clone());
                } else {
                    out.push(Value::Undefined);
                }
            }
        }
    }
    let cb = a.get(1).cloned().unwrap_or(Value::Undefined);
    if cb != Value::Undefined {
        let mapped: Result<Vec<Value>, Error> = out
            .iter()
            .enumerate()
            .map(|(i, v)| e.call_value(&cb, &Value::Undefined, &[v.clone(), Value::Number(i as f64)]))
            .collect();
        out = mapped?;
    }
    Ok(Value::Array(Rc::new(RefCell::new(ArrayData::new(out, Some(e.array_prototype.clone()))))))
}

fn arr_of(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Array(Rc::new(RefCell::new(ArrayData::new(
        a.to_vec(),
        Some(e.array_prototype.clone()),
    )))))
}

// --- Number statics ---

fn num_is_nan(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let n = a.first().cloned().unwrap_or(Value::Undefined).to_number();
    Ok(Value::Boolean(n.is_nan()))
}

fn num_is_finite(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let n = a.first().cloned().unwrap_or(Value::Undefined).to_number();
    Ok(Value::Boolean(n.is_finite()))
}

fn int_parse(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = a.first().cloned().unwrap_or(Value::Undefined).to_string();
    let radix = a.get(1).map(|v| v.to_number()).unwrap_or(10.0) as u32;
    let radix = if radix == 0 { 10 } else { radix.clamp(2, 36) };
    let trimmed = s.trim();
    let parsed = if radix == 10 {
        trimmed.parse::<f64>().unwrap_or(f64::NAN)
    } else {
        let body = trimmed.trim_start_matches(['+', '-']);
        let sign = if trimmed.starts_with('-') { -1.0 } else { 1.0 };
        i64::from_str_radix(body, radix)
            .map(|v| sign * v as f64)
            .unwrap_or(f64::NAN)
    };
    Ok(Value::Number(parsed))
}

fn float_parse(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = a.first().cloned().unwrap_or(Value::Undefined).to_string();
    Ok(Value::Number(s.trim().parse::<f64>().unwrap_or(f64::NAN)))
}

fn is_nan_fn(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let n = a.first().cloned().unwrap_or(Value::Undefined).to_number();
    Ok(Value::Boolean(n.is_nan()))
}

fn is_finite_fn(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let n = a.first().cloned().unwrap_or(Value::Undefined).to_number();
    Ok(Value::Boolean(n.is_finite()))
}

// --- Function.prototype.call / apply / bind ---

fn call_fn(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let this_fn = this.clone();
    let this_arg = a.first().cloned().unwrap_or(Value::Undefined);
    let rest: Vec<Value> = if a.len() > 1 { a[1..].to_vec() } else { Vec::new() };
    e.call_value(&this_fn, &this_arg, &rest)
}

fn apply_fn(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let this_fn = this.clone();
    let this_arg = a.first().cloned().unwrap_or(Value::Undefined);
    let rest = match a.get(1) {
        Some(Value::Array(arr)) => arr.borrow().elems.clone(),
        _ => Vec::new(),
    };
    e.call_value(&this_fn, &this_arg, &rest)
}

fn bind_fn(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
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

fn bound_invoke(_e: &Engine, _this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    Err(Error::Runtime(Value::String(Rc::from("internal: bound function leaked"))))
}

// --- URI helpers (best-effort identity) ---

fn decode_uri(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(a.first().cloned().unwrap_or(Value::Undefined))
}

fn encode_uri(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(a.first().cloned().unwrap_or(Value::Undefined))
}

// --- Math ---

fn math_abs(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(fabs(a.first().map(|v| v.to_number()).unwrap_or(f64::NAN))))
}
fn math_floor(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(floor(a.first().map(|v| v.to_number()).unwrap_or(f64::NAN))))
}
fn math_ceil(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(ceil(a.first().map(|v| v.to_number()).unwrap_or(f64::NAN))))
}
fn math_round(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(round(a.first().map(|v| v.to_number()).unwrap_or(f64::NAN))))
}
fn math_trunc(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(trunc(a.first().map(|v| v.to_number()).unwrap_or(f64::NAN))))
}
fn math_max(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let mut m = f64::NEG_INFINITY;
    for v in a {
        m = m.max(v.to_number());
    }
    Ok(Value::Number(m))
}
fn math_min(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    if a.is_empty() {
        return Ok(Value::Number(f64::INFINITY));
    }
    let mut m = f64::INFINITY;
    for v in a {
        m = m.min(v.to_number());
    }
    Ok(Value::Number(m))
}
fn math_sqrt(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(sqrt(a.first().map(|v| v.to_number()).unwrap_or(f64::NAN))))
}
fn math_pow(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let x = a.first().map(|v| v.to_number()).unwrap_or(f64::NAN);
    let y = a.get(1).map(|v| v.to_number()).unwrap_or(f64::NAN);
    Ok(Value::Number(pow(x, y)))
}
fn math_sign(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let n = a.first().map(|v| v.to_number()).unwrap_or(f64::NAN);
    Ok(Value::Number(if n > 0.0 { 1.0 } else if n < 0.0 { -1.0 } else { n }))
}
fn math_exp(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(exp(a.first().map(|v| v.to_number()).unwrap_or(f64::NAN))))
}
fn math_log(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(log(a.first().map(|v| v.to_number()).unwrap_or(f64::NAN))))
}
fn math_sin(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(sin(a.first().map(|v| v.to_number()).unwrap_or(f64::NAN))))
}
fn math_cos(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(cos(a.first().map(|v| v.to_number()).unwrap_or(f64::NAN))))
}
fn math_tan(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(tan(a.first().map(|v| v.to_number()).unwrap_or(f64::NAN))))
}
fn math_random(_e: &Engine, _this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Number(0.0))
}

// --- JSON ---

fn json_stringify(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    Ok(Value::String(Rc::from(json_to_string(e, &v))))
}

fn json_to_string(e: &Engine, v: &Value) -> String {
    match v {
        Value::Undefined | Value::Function(_) | Value::NativeFunction(_) => "null".to_string(),
        Value::Null => "null".to_string(),
        Value::Boolean(b) => if *b { "true".to_string() } else { "false".to_string() },
        Value::Number(n) => {
            if n.is_nan() || n.is_infinite() {
                "null".to_string()
            } else {
                n.to_string()
            }
        }
        Value::String(s) => format!("\"{}\"", s.as_ref()),
        Value::Array(arr) => {
            let parts: Vec<String> = arr
                .borrow()
                .elems
                .iter()
                .map(|x| json_to_string(e, x))
                .collect();
            format!("[{}]", parts.join(","))
        }
        Value::Object(o) => {
            let mut parts = Vec::new();
            for (k, p) in o.borrow().props.iter() {
                if k.as_ref() == "__value__" {
                    continue;
                }
                parts.push(format!("\"{}\":{}", k.as_ref(), json_to_string(e, &p.value)));
            }
            format!("{{{}}}", parts.join(","))
        }
    }
}

fn json_parse(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = a.first().cloned().unwrap_or(Value::Undefined).to_string();
    parse_json(&s).ok_or_else(|| Error::Runtime(Value::String(Rc::from("JSON.parse failed"))))
}

/// Minimal JSON parser supporting objects, arrays, strings, numbers, and literals.
fn parse_json(s: &str) -> Option<Value> {
    let mut p = JsonParser { chars: s.trim().chars().collect(), pos: 0 };
    p.parse_value()
}

struct JsonParser {
    chars: Vec<char>,
    pos: usize,
}

impl JsonParser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }
    fn next(&mut self) -> Option<char> {
        let c = self.chars.get(self.pos).copied();
        self.pos += 1;
        c
    }
    fn skip_ws(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_whitespace() {
                self.pos += 1;
            } else {
                break;
            }
        }
    }
    fn parse_value(&mut self) -> Option<Value> {
        self.skip_ws();
        match self.peek()? {
            '{' => self.parse_object(),
            '[' => self.parse_array(),
            '"' => Some(Value::String(self.parse_string())),
            't' | 'f' => self.parse_bool(),
            'n' => self.parse_null(),
            _ => self.parse_number(),
        }
    }
    fn parse_object(&mut self) -> Option<Value> {
        self.next()?;
        let o = crate::value::new_props();
        let mut obj = Value::Object(Rc::new(RefCell::new(Object { props: o, proto: None })));
        self.skip_ws();
        if self.peek()? == '}' {
            self.next();
            return Some(obj);
        }
        loop {
            self.skip_ws();
            let key = self.parse_string();
            self.skip_ws();
            if self.next()? != ':' {
                return None;
            }
            let val = self.parse_value()?;
            if let Value::Object(o) = &obj {
                o.borrow_mut()
                    .props
                    .insert(key.clone(), Property::new(val));
            }
            self.skip_ws();
            match self.next()? {
                ',' => continue,
                '}' => break,
                _ => return None,
            }
        }
        Some(obj)
    }
    fn parse_array(&mut self) -> Option<Value> {
        self.next()?;
        let mut elems = Vec::new();
        self.skip_ws();
        if self.peek()? == ']' {
            self.next();
            return Some(Value::Array(Rc::new(RefCell::new(ArrayData::new(elems, None)))));
        }
        loop {
            let val = self.parse_value()?;
            elems.push(val);
            self.skip_ws();
            match self.next()? {
                ',' => continue,
                ']' => break,
                _ => return None,
            }
        }
        Some(Value::Array(Rc::new(RefCell::new(ArrayData::new(elems, None)))))
    }
    fn parse_string(&mut self) -> Rc<str> {
        self.next();
        let mut s = String::new();
        while let Some(c) = self.next() {
            if c == '"' {
                break;
            }
            if c == '\\' {
                if let Some(e) = self.next() {
                    match e {
                        '"' => s.push('"'),
                        '\\' => s.push('\\'),
                        '/' => s.push('/'),
                        'n' => s.push('\n'),
                        't' => s.push('\t'),
                        'r' => s.push('\r'),
                        _ => s.push(e),
                    }
                }
            } else {
                s.push(c);
            }
        }
        Rc::from(s)
    }
    fn parse_bool(&mut self) -> Option<Value> {
        let start = self.pos;
        let word: String = (0..5).filter_map(|_| self.next()).collect();
        if word.starts_with("true") {
            self.pos = start + 4;
            Some(Value::Boolean(true))
        } else if word.starts_with("false") {
            self.pos = start + 5;
            Some(Value::Boolean(false))
        } else {
            None
        }
    }
    fn parse_null(&mut self) -> Option<Value> {
        let start = self.pos;
        let word: String = (0..4).filter_map(|_| self.next()).collect();
        if word.starts_with("null") {
            self.pos = start + 4;
            Some(Value::Null)
        } else {
            None
        }
    }
    fn parse_number(&mut self) -> Option<Value> {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() || c == '-' || c == '+' || c == '.' || c == 'e' || c == 'E' {
                self.pos += 1;
            } else {
                break;
            }
        }
        let s: String = self.chars[start..self.pos].iter().collect();
        s.parse::<f64>().ok().map(Value::Number)
    }
}

// --- harness fallback ---

fn make_assert(e: &Engine) -> Value {
    let mut o = Object::with_proto(e.object_prototype.clone());
    o.props.insert(Rc::from("sameValue"), Property::new(native(same_value_fn)));
    o.props.insert(Rc::from("notSameValue"), Property::new(native(not_same_value_fn)));
    o.props.insert(Rc::from("throws"), Property::new(native(throws_fn)));
    o.props.insert(Rc::from("true"), Property::new(native(assert_true_fn)));
    o.props.insert(Rc::from("false"), Property::new(native(assert_false_fn)));
    Value::Object(Rc::new(RefCell::new(o)))
}

fn same_value_fn(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let x = a.first().cloned().unwrap_or(Value::Undefined);
    let y = a.get(1).cloned().unwrap_or(Value::Undefined);
    if Value::same_value(&x, &y) {
        Ok(Value::Undefined)
    } else {
        let extra = if a.len() > 2 {
            format!(" {}", a[2].to_string())
        } else {
            String::new()
        };
        Err(Error::Runtime(Value::String(Rc::from(format!(
            "Expected SameValue(«{}», «{}») to be true{}",
            x.to_string(),
            y.to_string(),
            extra
        )))))
    }
}

fn not_same_value_fn(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let x = a.first().cloned().unwrap_or(Value::Undefined);
    let y = a.get(1).cloned().unwrap_or(Value::Undefined);
    if !Value::same_value(&x, &y) {
        Ok(Value::Undefined)
    } else {
        Err(Error::Runtime(Value::String(Rc::from(format!(
            "Expected not SameValue(«{}», «{}»)",
            x.to_string(),
            y.to_string()
        )))))
    }
}

fn assert_true_fn(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    if v.to_boolean() {
        Ok(Value::Undefined)
    } else {
        Err(Error::Runtime(Value::String(Rc::from("Expected true"))))
    }
}

fn assert_false_fn(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    if !v.to_boolean() {
        Ok(Value::Undefined)
    } else {
        Err(Error::Runtime(Value::String(Rc::from("Expected false"))))
    }
}

fn throws_fn(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let f = a.first().cloned().unwrap_or(Value::Undefined);
    match e.call_value(&f, &Value::Undefined, &[]) {
        Ok(_) => Err(Error::Runtime(Value::String(Rc::from(
            "Expected an exception to be thrown",
        )))),
        Err(_) => Ok(Value::Undefined),
    }
}

fn donotevaluate_fn(_e: &Engine, _this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    Err(Error::Runtime(Value::String(Rc::from(
        "Test262: This statement should not be evaluated.",
    ))))
}
