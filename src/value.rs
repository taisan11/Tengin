use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use core::cell::RefCell;

use crate::ast::Stmt;
use crate::environment::Env;

/// A JavaScript value, restricted to the subset supported by the engine so far.
#[derive(Debug, Clone)]
pub enum Value {
    Undefined,
    Null,
    Boolean(bool),
    Number(f64),
    String(Rc<str>),
    Object(Rc<RefCell<Object>>),
    Array(Rc<RefCell<ArrayData>>),
    /// A user-defined function (closure with property storage).
    Function(Rc<RefCell<Function>>),
    /// A host/native function implemented in Rust.
    NativeFunction(Rc<RefCell<NativeFunctionData>>),
}

/// Signature of native functions callable from JavaScript.
/// `this` is the receiver, `args` are the call arguments, and `construct` is
/// `true` when the function is invoked with the `new` operator.
pub type NativeFn = fn(
    &crate::interpreter::Engine,
    &Value,
    &[Value],
    bool,
) -> Result<Value, crate::error::Error>;

/// A property slot on a JavaScript object.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Property {
    pub value: Value,
    pub writable: bool,
    pub enumerable: bool,
    pub configurable: bool,
    /// Accessor getter (mutually exclusive with `value`).
    pub get: Option<Value>,
    /// Accessor setter (mutually exclusive with `value`).
    pub set: Option<Value>,
}

impl Property {
    pub fn new(value: Value) -> Self {
        Property {
            value,
            writable: true,
            enumerable: true,
            configurable: true,
            get: None,
            set: None,
        }
    }

    /// Create an accessor property with the given getter/setter.
    pub fn accessor(get: Option<Value>, set: Option<Value>) -> Self {
        Property {
            value: Value::Undefined,
            writable: false,
            enumerable: true,
            configurable: true,
            get,
            set,
        }
    }

    /// Whether this property is an accessor (getter/setter) descriptor.
    pub fn is_accessor(&self) -> bool {
        self.get.is_some() || self.set.is_some()
    }
}

/// Create an empty property map (used when constructing function values).
pub fn new_props() -> BTreeMap<Rc<str>, Property> {
    BTreeMap::new()
}

/// A JavaScript object: an ordered map of named properties plus a prototype link.
#[derive(Debug, Clone)]
pub struct Object {
    pub props: BTreeMap<Rc<str>, Property>,
    pub proto: Option<Rc<RefCell<Object>>>,
}

impl Object {
    pub fn new() -> Self {
        Object {
            props: BTreeMap::new(),
            proto: None,
        }
    }

    /// The root `Object.prototype` (its own prototype is `None`).
    pub fn new_root() -> Self {
        Object::new()
    }

    pub fn with_proto(proto: Rc<RefCell<Object>>) -> Self {
        Object {
            props: BTreeMap::new(),
            proto: Some(proto),
        }
    }
}

/// The backing data for a host/native function. Unlike `Function`, this can
/// carry its own property storage and prototype (e.g. `Object.prototype`).
#[derive(Debug, Clone)]
pub struct NativeFunctionData {
    pub func: NativeFn,
    pub props: BTreeMap<Rc<str>, Property>,
    pub proto: Option<Rc<RefCell<Object>>>,
}

impl NativeFunctionData {
    pub fn new(func: NativeFn) -> Self {
        NativeFunctionData {
            func,
            props: BTreeMap::new(),
            proto: None,
        }
    }
}

/// Construct a `Value::NativeFunction` from a native implementation.
pub fn native(func: NativeFn) -> Value {
    Value::NativeFunction(Rc::new(RefCell::new(NativeFunctionData::new(func))))
}

impl Default for Object {
    fn default() -> Self {
        Object::new()
    }
}

/// A user-defined function value.
#[derive(Debug, Clone)]
pub struct Function {
    pub name: Rc<str>,
    pub params: Vec<Rc<str>>,
    pub body: Vec<Stmt>,
    pub closure: Rc<RefCell<Env>>,
    pub props: BTreeMap<Rc<str>, Property>,
    pub proto: Option<Rc<RefCell<Object>>>,
}

/// A JavaScript array: an element vector plus a prototype link.
#[derive(Debug, Clone)]
pub struct ArrayData {
    pub elems: Vec<Value>,
    pub proto: Option<Rc<RefCell<Object>>>,
}

impl ArrayData {
    pub fn new(elems: Vec<Value>, proto: Option<Rc<RefCell<Object>>>) -> Self {
        ArrayData { elems, proto }
    }
}

impl Value {
    /// If this value is a primitive wrapper object, return the wrapped primitive;
    /// otherwise return `self`.
    pub fn primitive_value(&self) -> Option<Value> {
        if let Value::Object(o) = self {
            if let Some(pv) = o.borrow().props.get("__value__") {
                return Some(pv.value.clone());
            }
        }
        None
    }

    /// ECMAScript `ToBoolean` abstract operation (subset).
    pub fn to_boolean(&self) -> bool {
        match self {
            Value::Undefined | Value::Null => false,
            Value::Boolean(b) => *b,
            Value::Number(n) => !n.is_nan() && *n != 0.0,
            Value::String(s) => !s.is_empty(),
            _ => true,
        }
    }

    /// ECMAScript `ToNumber` abstract operation (subset).
    pub fn to_number(&self) -> f64 {
        if let Some(p) = self.primitive_value() {
            return p.to_number();
        }
        match self {
            Value::Undefined => f64::NAN,
            Value::Null => 0.0,
            Value::Boolean(b) => {
                if *b {
                    1.0
                } else {
                    0.0
                }
            }
            Value::Number(n) => *n,
            Value::String(s) => parse_f64(s),
            _ => f64::NAN,
        }
    }

    /// ECMAScript `ToString` abstract operation (subset).
    pub fn to_string(&self) -> Rc<str> {
        if let Some(p) = self.primitive_value() {
            return p.to_string();
        }
        match self {
            Value::Undefined => Rc::from("undefined"),
            Value::Null => Rc::from("null"),
            Value::Boolean(b) => Rc::from(if *b { "true" } else { "false" }),
            Value::Number(n) => Rc::from(n.to_string().as_str()),
            Value::String(s) => s.clone(),
            Value::Object(_) => Rc::from("[object Object]"),
            Value::Array(a) => {
                let parts: Vec<String> =
                    a.borrow().elems.iter().map(|v| v.to_string().to_string()).collect();
                Rc::from(parts.join(",").as_str())
            }
            Value::Function(_) | Value::NativeFunction(_) => Rc::from("function"),
        }
    }

    /// ECMAScript `typeof` operator result.
    pub fn type_of(&self) -> &'static str {
        match self {
            Value::Undefined => "undefined",
            Value::Null => "object",
            Value::Boolean(_) => "boolean",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Object(_) | Value::Array(_) => "object",
            Value::Function(_) | Value::NativeFunction(_) => "function",
        }
    }

    /// ECMAScript `SameValue` abstract operation (used by `assert.sameValue`).
    pub fn same_value(a: &Value, b: &Value) -> bool {
        match (a, b) {
            (Value::Number(x), Value::Number(y)) => {
                if x.is_nan() && y.is_nan() {
                    return true;
                }
                if *x == 0.0 && *y == 0.0 {
                    return x.is_sign_positive() == y.is_sign_positive();
                }
                x == y
            }
            (Value::Undefined, Value::Undefined) => true,
            (Value::Null, Value::Null) => true,
            (Value::Boolean(x), Value::Boolean(y)) => x == y,
            (Value::String(x), Value::String(y)) => x == y,
            (Value::Object(x), Value::Object(y)) => Rc::ptr_eq(x, y),
            (Value::Array(x), Value::Array(y)) => Rc::ptr_eq(x, y),
            (Value::Function(x), Value::Function(y)) => Rc::ptr_eq(x, y),
            (Value::NativeFunction(x), Value::NativeFunction(y)) => core::ptr::eq(x, y),
            _ => false,
        }
    }

    /// ECMAScript strict equality (`===`).
    pub fn strict_eq(a: &Value, b: &Value) -> bool {
        match (a, b) {
            (Value::Number(x), Value::Number(y)) => {
                if x.is_nan() || y.is_nan() {
                    false
                } else {
                    x == y
                }
            }
            (Value::Undefined, Value::Undefined) => true,
            (Value::Null, Value::Null) => true,
            (Value::Boolean(x), Value::Boolean(y)) => x == y,
            (Value::String(x), Value::String(y)) => x == y,
            (Value::Object(x), Value::Object(y)) => Rc::ptr_eq(x, y),
            (Value::Array(x), Value::Array(y)) => Rc::ptr_eq(x, y),
            (Value::Function(x), Value::Function(y)) => Rc::ptr_eq(x, y),
            (Value::NativeFunction(x), Value::NativeFunction(y)) => core::ptr::eq(x, y),
            _ => false,
        }
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        Value::strict_eq(self, other)
    }
}

fn parse_f64(s: &str) -> f64 {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return 0.0;
    }
    trimmed.parse::<f64>().unwrap_or(f64::NAN)
}
