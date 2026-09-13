use alloc::rc::Rc;

use crate::error::Error;
use crate::interpreter::Engine;
use crate::value::{native, Property, SymbolData, Value};

use super::helpers::proto_method;

pub(crate) fn symbol_ctor(
    _e: &Engine,
    _this: &Value,
    a: &[Value],
    construct: bool,
) -> Result<Value, Error> {
    if construct {
        return Err(Error::Runtime(Value::String(Rc::from(
            "TypeError: Symbol is not a constructor",
        ))));
    }
    let desc = match a.first() {
        Some(Value::String(s)) => Some(s.clone()),
        Some(v) if !matches!(v, Value::Undefined | Value::Null) => Some(v.to_string()),
        _ => None,
    };
    Ok(Value::Symbol(Rc::new(SymbolData::new(desc))))
}

pub(crate) fn symbol_for(
    e: &Engine,
    _this: &Value,
    a: &[Value],
    _c: bool,
) -> Result<Value, Error> {
    let key = a.first().cloned().unwrap_or(Value::Undefined).to_string();
    let mut reg = e.symbol_registry.borrow_mut();
    if let Some(existing) = reg.get(&key) {
        return Ok(Value::Symbol(existing.clone()));
    }
    let sym = Rc::new(SymbolData::new(Some(key.clone())));
    reg.insert(key, sym.clone());
    Ok(Value::Symbol(sym))
}

pub(crate) fn symbol_key_for(
    _e: &Engine,
    _this: &Value,
    a: &[Value],
    _c: bool,
) -> Result<Value, Error> {
    match a.first() {
        Some(Value::Symbol(s)) => Ok(s
            .description
            .clone()
            .map(Value::String)
            .unwrap_or(Value::Undefined)),
        _ => Err(Error::Runtime(Value::String(Rc::from(
            "TypeError: Symbol.keyFor requires a symbol",
        )))),
    }
}

/// `thisSymbolValue`: a Symbol primitive or a Symbol wrapper's `__value__`.
fn this_symbol_value(this: &Value) -> Option<Value> {
    match this {
        Value::Symbol(_) => Some(this.clone()),
        Value::Object(o) => match o.borrow().props.get("__value__").map(|p| p.value.clone()) {
            Some(s @ Value::Symbol(_)) => Some(s),
            _ => None,
        },
        _ => None,
    }
}

pub(crate) fn symbol_to_string(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    match this_symbol_value(this) {
        Some(Value::Symbol(s)) => Ok(Value::String(Rc::from(
            alloc::format!("Symbol({})", s.description.as_deref().unwrap_or("")).as_str(),
        ))),
        _ => Err(Error::Runtime(e.make_type_error(
            "Symbol.prototype.toString called on non-symbol",
        ))),
    }
}

pub(crate) fn symbol_value_of(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    match this_symbol_value(this) {
        Some(s @ Value::Symbol(_)) => Ok(s),
        _ => Err(Error::Runtime(e.make_type_error(
            "Symbol.prototype.valueOf called on non-symbol",
        ))),
    }
}

/// Getter for `Symbol.prototype.description`.
pub(crate) fn symbol_description_get(
    _e: &Engine,
    this: &Value,
    _a: &[Value],
    _c: bool,
) -> Result<Value, Error> {
    match this {
        Value::Symbol(s) => Ok(s
            .description
            .clone()
            .map(Value::String)
            .unwrap_or(Value::Undefined)),
        _ => Ok(Value::Undefined),
    }
}

/// Install `Symbol`, its prototype, well-known symbols, and statics.
pub(crate) fn register_symbol(engine: &mut Engine) {
    let sym_proto = engine.symbol_prototype.clone();

    proto_method(&sym_proto, "toString", symbol_to_string);
    proto_method(&sym_proto, "valueOf", symbol_value_of);
    // `description` accessor.
    sym_proto
        .borrow_mut()
        .props
        .insert(Rc::from("description"), Property::accessor(Some(native(symbol_description_get)), None));

    let sym_ctor = native(symbol_ctor);
    if let Value::NativeFunction(nf) = &sym_ctor {
        nf.borrow_mut().proto = Some(engine.function_prototype.clone());
        nf.borrow_mut()
            .props
            .insert(Rc::from("prototype"), Property::new(Value::Object(sym_proto.clone())));
        nf.borrow_mut()
            .props
            .insert(Rc::from("name"), Property::new(Value::String(Rc::from("Symbol"))));
        nf.borrow_mut().props.insert(
            Rc::from("for"),
            Property::new(native(symbol_for)),
        );
        nf.borrow_mut().props.insert(
            Rc::from("keyFor"),
            Property::new(native(symbol_key_for)),
        );
    }
    sym_proto
        .borrow_mut()
        .props
        .insert(Rc::from("constructor"), Property::new(sym_ctor.clone()));
    super::helpers::proto_data(&sym_proto, "constructor", sym_ctor.clone());

    // Well-known symbols.
    let well_known = [
        "iterator",
        "asyncIterator",
        "hasInstance",
        "isConcatSpreadable",
        "unscopables",
        "match",
        "replace",
        "search",
        "split",
        "toPrimitive",
        "toStringTag",
        "species",
    ];
    if let Value::NativeFunction(nf) = &sym_ctor {
        let mut guard = nf.borrow_mut();
        for wk in well_known {
            let sym = Value::Symbol(Rc::new(SymbolData::well_known(wk)));
            guard.props.insert(Rc::from(wk), Property::new(sym));
        }
    }

    engine.register_global("Symbol", sym_ctor);
}
