use alloc::rc::Rc;
use core::cell::RefCell;

use crate::interpreter::Engine;
use crate::value::{NativeFn, Object, Property, Value};

mod array;
mod bigint;
pub(crate) mod bigint_num;
mod boolean;
mod collections;
mod error;
mod generator;
mod global;
mod helpers;
mod json;
mod math;
mod misc;
mod number;
mod object;
mod promise;
mod regexp;
mod string;
mod symbol;
mod timers;

use array::{
    arr_at, arr_concat, arr_every, arr_fill, arr_filter, arr_find, arr_find_index, arr_for_each,
    arr_from, arr_includes, arr_index_of, arr_is_array, arr_join, arr_last_index_of, arr_map,
    arr_of, arr_pop, arr_push, arr_reduce, arr_reverse, arr_shift, arr_slice, arr_some, arr_sort,
    arr_splice,
    arr_unshift, array_ctor,
};
use boolean::{bool_to_string, bool_value_of, boolean_ctor};
use collections::register_collections;
use error::{err_to_string, error_ctor};
use global::{
    decode_uri, decode_uri_component, donotevaluate_fn, encode_uri, encode_uri_component, eval_fn,
    is_finite_fn, is_nan_fn, make_assert,
};
use helpers::{
    engine_global, named_native, named_native_len, native, proto_data, proto_method,
    proto_method_len, prototype_of, reg_ctor, reg_error, reg_error_sub, set_static,
    set_static_value,
};use json::{json_parse, json_stringify};
use math::{
    math_abs, math_acos, math_acosh, math_asin, math_asinh, math_atan, math_atan2, math_atanh,
    math_cbrt, math_ceil, math_clz32, math_cos, math_cosh, math_exp, math_expm1, math_f16round,
    math_floor, math_fround, math_hypot, math_imul, math_log, math_log10, math_log1p, math_log2,
    math_max, math_min, math_pow, math_random, math_round, math_sign, math_sin, math_sinh,
    math_sqrt, math_sumprecise, math_tan, math_tanh, math_trunc,
};
use misc::{register_misc, register_reflect};
use number::{
    float_parse, int_parse, number_ctor, num_is_finite, num_is_integer, num_is_nan,
    num_is_safe_integer, num_to_exponential, num_to_fixed, num_to_locale_string, num_to_precision,
    num_to_string, num_value_of,
};
use object::{
    apply_fn, bind_fn, call_fn, func_to_string, function_ctor, async_function_ctor, obj_assign, obj_create,
    obj_define_properties, obj_define_property, obj_entries, obj_get_own_descriptor,
    obj_get_own_descriptors, obj_get_own_names, obj_get_own_symbols, obj_get_proto_of,
    obj_has_own, obj_identity, obj_is, obj_is_proto_of, obj_keys, obj_prop_enum, obj_set_proto_of,
    obj_to_string, obj_true, obj_value_of, object_ctor, obj_values,
};
use string::{
    str_anchor, str_at, str_big, str_blink, str_bold, str_char_at, str_char_code_at,
    str_code_point_at, str_concat, str_ends_with, str_fixed, str_fontcolor, str_fontsize,
    str_from_char_code, str_from_code_point, str_includes, str_index_of, str_is_well_formed,
    str_italics, str_last_index_of, str_link, str_locale_compare, str_lower, str_match,
    str_pad_end, str_pad_start, str_raw, str_repeat, str_replace, str_replace_all, str_search,
    str_slice, str_small, str_split, str_starts_with, str_strike, str_sub, str_substr,
    str_substring, str_sup, str_to_well_formed, str_trim, str_trim_end, str_trim_start,
    str_upper, str_value_of, string_ctor,
};
use symbol::register_symbol;


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
    // `Function.prototype` is itself a built-in function object with `name`
    // "" and `length` 0; this also lets a deleted bound of a function resolve
    // to the inherited `length` (e.g. `delete eval.length` → `eval.length`).
    function_proto
        .borrow_mut()
        .props
        .insert(Rc::from("name"), Property::config(Value::String(Rc::from(""))));
    function_proto
        .borrow_mut()
        .props
        .insert(Rc::from("length"), Property::config(Value::Number(0.0)));

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
    proto_method(&array_proto, "sort", arr_sort);
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
    proto_method(&string_proto, "match", str_match);
    proto_method(&string_proto, "search", str_search);
    proto_method(&string_proto, "replace", str_replace);
    proto_method(&string_proto, "padStart", str_pad_start);
    proto_method(&string_proto, "padEnd", str_pad_end);
    proto_method(&string_proto, "valueOf", str_value_of);
    proto_method(&string_proto, "toString", str_value_of);
    proto_method(&string_proto, "at", str_at);
    proto_method(&string_proto, "localeCompare", str_locale_compare);
    proto_method(&string_proto, "toLocaleLowerCase", str_lower);
    proto_method(&string_proto, "toLocaleUpperCase", str_upper);
    proto_method(&string_proto, "isWellFormed", str_is_well_formed);
    proto_method(&string_proto, "toWellFormed", str_to_well_formed);

    // --- String.prototype (Annex B legacy HTML wrappers) ---
    proto_method(&string_proto, "anchor", str_anchor);
    proto_method(&string_proto, "big", str_big);
    proto_method(&string_proto, "blink", str_blink);
    proto_method(&string_proto, "bold", str_bold);
    proto_method(&string_proto, "fixed", str_fixed);
    proto_method(&string_proto, "fontcolor", str_fontcolor);
    proto_method(&string_proto, "fontsize", str_fontsize);
    proto_method(&string_proto, "italics", str_italics);
    proto_method(&string_proto, "link", str_link);
    proto_method(&string_proto, "small", str_small);
    proto_method(&string_proto, "strike", str_strike);
    proto_method(&string_proto, "sub", str_sub);
    proto_method(&string_proto, "sup", str_sup);
    proto_method(&string_proto, "substr", str_substr);
    proto_method(&string_proto, "replaceAll", str_replace_all);

    // --- Number.prototype ---
    // `Number.prototype.toString` has length 1 and `toLocaleString` length 0,
    // both of which differ from the generic arity table.
    proto_method_len(&number_proto, "toString", num_to_string, 1);
    proto_method(&number_proto, "valueOf", num_value_of);
    proto_method(&number_proto, "toFixed", num_to_fixed);
    proto_method(&number_proto, "toExponential", num_to_exponential);
    proto_method(&number_proto, "toPrecision", num_to_precision);
    proto_method_len(&number_proto, "toLocaleString", num_to_locale_string, 0);

    // --- Boolean.prototype ---
    proto_method(&boolean_proto, "toString", bool_to_string);
    proto_method(&boolean_proto, "valueOf", bool_value_of);

    // --- Error.prototype ---
    proto_method(&error_proto, "toString", err_to_string);
    error_proto.borrow_mut().props.insert(
        Rc::from("name"),
        Property::data(Value::String(Rc::from("Error")), true, false, true),
    );
    error_proto.borrow_mut().props.insert(
        Rc::from("message"),
        Property::data(Value::String(Rc::from("")), true, false, true),
    );

    // --- Global constructors ---
    let obj_ctor = reg_ctor(engine, "Object", object_ctor, object_proto.clone());
    proto_data(&object_proto, "constructor", obj_ctor.clone());
    let fn_ctor = reg_ctor(engine, "Function", function_ctor, function_proto.clone());
    proto_data(&function_proto, "constructor", fn_ctor.clone());
    // `AsyncFunction` with `%AsyncFunction.prototype%` (proto:
    // `%Function.prototype%`, tagged for `Object.prototype.toString`).
    let async_proto = engine.async_function_prototype.clone();
    let async_ctor = reg_ctor(engine, "AsyncFunction", async_function_ctor, async_proto.clone());
    proto_data(&async_proto, "constructor", async_ctor.clone());
    async_proto.borrow_mut().props.insert(
        Rc::from(crate::value::SymbolData::well_known_key("toStringTag").as_ref()),
        Property::config(Value::String(Rc::from("AsyncFunction"))),
    );
    let arr_ctor = reg_ctor(engine, "Array", array_ctor, array_proto.clone());
    proto_data(&array_proto, "constructor", arr_ctor.clone());
    let str_ctor = reg_ctor(engine, "String", string_ctor, string_proto.clone());
    proto_data(&string_proto, "constructor", str_ctor.clone());
    set_static_value(
        &str_ctor,
        &[
            ("fromCharCode", named_native("fromCharCode", str_from_char_code)),
            ("fromCodePoint", named_native("fromCodePoint", str_from_code_point)),
            ("raw", named_native("raw", str_raw)),
        ],
    );
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
        ("defineProperties", obj_define_properties),
        ("getOwnPropertyDescriptor", obj_get_own_descriptor),
        ("getOwnPropertyDescriptors", obj_get_own_descriptors),
        ("getOwnPropertyNames", obj_get_own_names),
        ("getOwnPropertySymbols", obj_get_own_symbols),
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
            ("isArray", named_native("isArray", arr_is_array)),
            ("from", named_native("from", arr_from)),
            ("of", named_native("of", arr_of)),
        ],
    );

    let num_ctor = engine_global(engine, "Number");
    // `Number.parseInt`/`Number.parseFloat` must be the *same* built-in function
    // object as the global `parseInt`/`parseFloat` (test262 checks identity).
    let num_parse_int = named_native("parseInt", int_parse);
    let num_parse_float = named_native("parseFloat", float_parse);
    set_static_value(
        &num_ctor,
        &[
            ("isNaN", named_native("isNaN", num_is_nan)),
            ("isFinite", named_native("isFinite", num_is_finite)),
            ("isInteger", named_native("isInteger", num_is_integer)),
            ("isSafeInteger", named_native("isSafeInteger", num_is_safe_integer)),
            ("parseInt", num_parse_int.clone()),
            ("parseFloat", num_parse_float.clone()),
            ("MAX_VALUE", Value::Number(f64::MAX)),
            ("MIN_VALUE", Value::Number(f64::from_bits(1))),
            ("MAX_SAFE_INTEGER", Value::Number(9007199254740991.0)),
            ("MIN_SAFE_INTEGER", Value::Number(-9007199254740991.0)),
            ("POSITIVE_INFINITY", Value::Number(f64::INFINITY)),
            ("NEGATIVE_INFINITY", Value::Number(f64::NEG_INFINITY)),
            ("NaN", Value::Number(f64::NAN)),
            ("EPSILON", Value::Number(f64::EPSILON)),
        ],
    );

    engine.register_global("parseInt", num_parse_int);
    engine.register_global("parseFloat", num_parse_float);

    let bool_ctor = engine_global(engine, "Boolean");
    set_static_value(&bool_ctor, &[("toString", named_native("toString", bool_to_string))]);

    let err_ctor = reg_error(engine, "Error", error_ctor, error_proto.clone());
    proto_data(&error_proto, "constructor", err_ctor.clone());

    // Each error subclass gets its own prototype chained to `Error.prototype`,
    // each exposing a correct `constructor` property (needed by `assert.throws`).
    let eval_err = reg_error_sub(engine, "EvalError", error_ctor, error_proto.clone());
    let range_err = reg_error_sub(engine, "RangeError", error_ctor, error_proto.clone());
    let ref_err = reg_error_sub(engine, "ReferenceError", error_ctor, error_proto.clone());
    let syntax_err = reg_error_sub(engine, "SyntaxError", error_ctor, error_proto.clone());
    let type_err = reg_error_sub(engine, "TypeError", error_ctor, error_proto.clone());
    let uri_err = reg_error_sub(engine, "URIError", error_ctor, error_proto.clone());
    // NativeError constructors have the `Error` constructor as their
    // `[[Prototype]]` (`Object.getPrototypeOf(EvalError) === Error`).
    for sub in [&eval_err, &range_err, &ref_err, &syntax_err, &type_err, &uri_err] {
        if let Value::NativeFunction(nf) = sub {
            nf.borrow_mut().fn_value_proto = Some(err_ctor.clone());
        }
    }
    engine.type_error_proto = prototype_of(&type_err).unwrap_or(error_proto.clone());
    engine.range_error_proto = prototype_of(&range_err).unwrap_or(error_proto.clone());
    engine.reference_error_proto = prototype_of(&ref_err).unwrap_or(error_proto.clone());
    engine.syntax_error_proto = prototype_of(&syntax_err).unwrap_or(error_proto.clone());
    engine.eval_error_proto = prototype_of(&eval_err).unwrap_or(error_proto.clone());
    engine.uri_error_proto = prototype_of(&uri_err).unwrap_or(error_proto.clone());

    // Math.
    let mut math = Object::with_proto(object_proto.clone());
    // (name, fn, length): the `length` values are specified per method.
    let math_entries: [(&str, NativeFn, usize); 37] = [
        ("abs", math_abs, 1),
        ("acos", math_acos, 1),
        ("acosh", math_acosh, 1),
        ("asin", math_asin, 1),
        ("asinh", math_asinh, 1),
        ("atan", math_atan, 1),
        ("atan2", math_atan2, 2),
        ("atanh", math_atanh, 1),
        ("cbrt", math_cbrt, 1),
        ("ceil", math_ceil, 1),
        ("clz32", math_clz32, 1),
        ("cos", math_cos, 1),
        ("cosh", math_cosh, 1),
        ("exp", math_exp, 1),
        ("expm1", math_expm1, 1),
        ("floor", math_floor, 1),
        ("fround", math_fround, 1),
        ("f16round", math_f16round, 1),
        ("hypot", math_hypot, 2),
        ("imul", math_imul, 2),
        ("log", math_log, 1),
        ("log10", math_log10, 1),
        ("log1p", math_log1p, 1),
        ("log2", math_log2, 1),
        ("max", math_max, 2),
        ("min", math_min, 2),
        ("pow", math_pow, 2),
        ("random", math_random, 0),
        ("round", math_round, 1),
        ("sign", math_sign, 1),
        ("sin", math_sin, 1),
        ("sinh", math_sinh, 1),
        ("sqrt", math_sqrt, 1),
        ("sumPrecise", math_sumprecise, 1),
        ("tan", math_tan, 1),
        ("tanh", math_tanh, 1),
        ("trunc", math_trunc, 1),
    ];
    for (n, f, len) in math_entries {
        math.props.insert(Rc::from(n), Property::method(named_native_len(n, f, len)));
    }
    // `Math[Symbol.toStringTag]` = "Math" ({ Writable: false, Enumerable:
    // false, Configurable: true }).
    math.props.insert(
        Rc::from(crate::value::SymbolData::well_known_key("toStringTag").as_ref()),
        Property::config(Value::String(Rc::from("Math"))),
    );
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
        math.props.insert(Rc::from(n), Property::constant(v));
    }
    engine.register_global("Math", Value::Object(Rc::new(RefCell::new(math))));

    // JSON.
    let mut json = Object::with_proto(object_proto.clone());
    json.props.insert(Rc::from("stringify"), Property::method(named_native("stringify", json_stringify)));
    json.props.insert(Rc::from("parse"), Property::method(named_native("parse", json_parse)));
    engine.register_global("JSON", Value::Object(Rc::new(RefCell::new(json))));

    engine.register_global("isNaN", named_native("isNaN", is_nan_fn));
    engine.register_global("isFinite", named_native("isFinite", is_finite_fn));
    engine.register_global("decodeURI", named_native("decodeURI", decode_uri));
    engine.register_global("encodeURI", named_native("encodeURI", encode_uri));
    engine.register_global("decodeURIComponent", named_native("decodeURIComponent", decode_uri_component));
    engine.register_global("encodeURIComponent", named_native("encodeURIComponent", encode_uri_component));
    engine.register_global("Infinity", Value::Number(f64::INFINITY));
    engine.register_global("NaN", Value::Number(f64::NAN));
    engine.register_global("undefined", Value::Undefined);
    engine.register_global("globalThis", Value::Object(engine.global_object.clone()));

    engine.register_global("assert", make_assert(engine));
    engine.register_global("Test262Error", native(error_ctor));
    engine.register_global("$DONOTEVALUATE", native(donotevaluate_fn));
    engine.register_global("eval", named_native("eval", eval_fn));

    register_reflect(engine);
    register_symbol(engine);
    register_collections(engine);
    register_misc(engine);
    generator::register_generator(engine);
    promise::register_promise(engine);
    timers::register_timers(engine);
}
