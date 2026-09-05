use alloc::rc::{Rc, Weak};
use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use core::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::ast::Stmt;
use crate::environment::Env;

/// Which intrinsic default prototype a constructor produces, used by
/// `GetPrototypeFromConstructor` when the new target's `prototype` is null.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealmProto {
    Object,
    Number,
    String,
    Boolean,
}

/// Per-realm intrinsic objects. Each `Engine` is its own realm; a real cross-
/// realm escape (via `$262.createRealm`) builds a distinct `RealmInfo`.
#[derive(Debug)]
pub struct RealmInfo {
    pub global_env: Rc<RefCell<Env>>,
    pub object_prototype: Rc<RefCell<Object>>,
    pub number_prototype: Rc<RefCell<Object>>,
    pub string_prototype: Rc<RefCell<Object>>,
    pub boolean_prototype: Rc<RefCell<Object>>,
}
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
    /// A BigInt literal value, stored as its decimal text representation.
    BigInt(Rc<str>),
    /// A regular expression literal value.
    Regex(Rc<RegexData>),
    /// A `Symbol` primitive value.
    Symbol(Rc<SymbolData>),
    /// A `Map` instance.
    Map(Rc<RefCell<MapData>>),
    /// A `Set` instance.
    Set(Rc<RefCell<SetData>>),
    /// A `WeakMap` instance.
    WeakMap(Rc<RefCell<WeakMapData>>),
    /// A `WeakSet` instance.
    WeakSet(Rc<RefCell<WeakSetData>>),
}

/// Backing storage for a `Map`.
#[derive(Debug, Clone)]
pub struct MapData {
    pub entries: Vec<(Value, Value)>,
}

/// Backing storage for a `Set`.
#[derive(Debug, Clone)]
pub struct SetData {
    pub entries: Vec<Value>,
}

/// Backing storage for a `WeakMap`.
#[derive(Debug, Clone)]
pub struct WeakMapData {
    pub entries: Vec<(Value, Value)>,
}

/// Backing storage for a `WeakSet`.
#[derive(Debug, Clone)]
pub struct WeakSetData {
    pub entries: Vec<Value>,
}

/// Backing data for a regular expression literal.
#[derive(Debug, Clone)]
pub struct RegexData {
    pub pattern: Rc<str>,
    pub flags: Rc<str>,
    /// The compiled pattern (built from `pattern` + `flags`). Stored lazily as
    /// `Option` so that patterns valid in ECMAScript but rejected by the `regex`
    /// crate (e.g. look-arounds / backreferences) still produce a usable value
    /// that simply never matches.
    pub re: RefCell<Option<regex::Regex>>,
    /// The `lastIndex` state, in character units (matches how the rest of the
    /// engine indexes strings). Only meaningful for the `g`/`y` flags.
    pub last_index: RefCell<usize>,
}

impl RegexData {
    /// Build a new regular expression. Compilation happens eagerly; a pattern
    /// the `regex` crate cannot compile is stored as `None` (never matches)
    /// rather than failing, so literal `/.../` values always resolve.
    pub fn new(pattern: Rc<str>, flags: Rc<str>) -> Self {
        let re = compile_regex(&pattern, &flags).ok();
        RegexData {
            pattern,
            flags,
            re: RefCell::new(re),
            last_index: RefCell::new(0),
        }
    }

    /// The `RegExp.prototype.source` value: the canonical pattern, with an empty
    /// pattern rendered as `(?:)`.
    pub fn source(&self) -> Rc<str> {
        if self.pattern.is_empty() {
            Rc::from("(?:)")
        } else {
            self.pattern.clone()
        }
    }

    /// The `RegExp.prototype.flags` value: the canonical flag string in
    /// ECMAScript order (`dgimsuvy`), with only the set flags included.
    pub fn flag_string(&self) -> Rc<str> {
        let mut s = alloc::string::String::new();
        for c in ['d', 'g', 'i', 'm', 's', 'u', 'v', 'y'] {
            if self.flags.contains(c) {
                s.push(c);
            }
        }
        Rc::from(s.as_str())
    }

    pub fn has_flag(&self, c: char) -> bool {
        self.flags.contains(c)
    }
}

/// Compile a pattern according to the ECMAScript flags that map onto the
/// `regex` crate. The `g`, `y`, `d`, `u` and `v` flags do not affect
/// compilation; `i`, `m` and `s` map to the corresponding builder options.
/// (Unicode mode is always enabled in the `regex` crate.)
pub fn compile_regex(
    pattern: &str,
    flags: &str,
) -> core::result::Result<regex::Regex, alloc::string::String> {
    let mut b = regex::RegexBuilder::new(pattern);
    b.case_insensitive(flags.contains('i'));
    b.multi_line(flags.contains('m'));
    b.dot_matches_new_line(flags.contains('s'));
    b.build().map_err(|e| e.to_string())
}

/// Backing data for a `Symbol` primitive.
///
/// `id` is a process-unique string used for identity comparisons and as the
/// canonical property-key string (so `obj[someSymbol]` resolves through the
/// engine's existing string-keyed property storage). `description` is the
/// optional `Symbol(description)` text.
#[derive(Debug, Clone)]
pub struct SymbolData {
    pub id: Rc<str>,
    pub description: Option<Rc<str>>,
    pub well_known: bool,
}

impl SymbolData {
    /// Create a fresh, uniquely-identifiable symbol (not well-known).
    pub fn new(description: Option<Rc<str>>) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let id = Rc::from(alloc::format!("__symbol_{}__", n).as_str());
        SymbolData {
            id,
            description,
            well_known: false,
        }
    }

    /// Create a well-known symbol with a fixed identity string.
    pub fn well_known(name: &str) -> Self {
        SymbolData {
            id: Rc::from(alloc::format!("__wk_{}__", name).as_str()),
            description: Some(Rc::from(name)),
            well_known: true,
        }
    }
}

impl PartialEq for SymbolData {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for SymbolData {}

impl core::hash::Hash for SymbolData {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
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

    /// A built-in method value property: writable + configurable, non-enumerable.
    pub fn method(value: Value) -> Self {
        Property {
            value,
            writable: true,
            enumerable: false,
            configurable: true,
            get: None,
            set: None,
        }
    }

    /// A built-in config value property: non-writable, non-enumerable, configurable.
    pub fn config(value: Value) -> Self {
        Property {
            value,
            writable: false,
            enumerable: false,
            configurable: true,
            get: None,
            set: None,
        }
    }

    /// A data property with explicit attributes (for `Object.defineProperty`).
    pub fn data(value: Value, writable: bool, enumerable: bool, configurable: bool) -> Self {
        Property {
            value,
            writable,
            enumerable,
            configurable,
            get: None,
            set: None,
        }
    }

    /// A non-writable, non-enumerable, non-configurable constant.
    pub fn constant(value: Value) -> Self {
        Property {
            value,
            writable: false,
            enumerable: false,
            configurable: false,
            get: None,
            set: None,
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

/// A weak back-reference to a constructor function, stored on a prototype object
/// so that `prototype.constructor` can be resolved without creating a strong
/// reference-count cycle (`Function` ↔ `Function.prototype.constructor`) that
/// `Rc` would otherwise leak.
#[derive(Debug, Clone)]
pub enum CtorRef {
    Func(Weak<RefCell<Function>>),
    Native(Weak<RefCell<NativeFunctionData>>),
}

/// A JavaScript object: an ordered map of named properties plus a prototype link.
#[derive(Debug, Clone)]
pub struct Object {
    pub props: BTreeMap<Rc<str>, Property>,
    pub proto: Option<Rc<RefCell<Object>>>,
    /// Weak link back to the constructor whose `prototype` is this object. Only
    /// consulted when resolving the `constructor` property, never enumerated.
    pub ctor: Option<CtorRef>,
}

impl Object {
    pub fn new() -> Self {
        Object {
            props: BTreeMap::new(),
            proto: None,
            ctor: None,
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
            ctor: None,
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
    /// Whether `new nativeFn(...)` is allowed (true for constructors).
    pub constructable: bool,
    /// The realm this intrinsic belongs to (`None` for realm-independent natives).
    pub realm: Option<Rc<RealmInfo>>,
    /// The intrinsic default prototype this constructor produces.
    pub intrinsic_proto: Option<RealmProto>,
}

impl NativeFunctionData {
    pub fn new(func: NativeFn) -> Self {
        NativeFunctionData {
            func,
            props: BTreeMap::new(),
            proto: None,
            constructable: false,
            realm: None,
            intrinsic_proto: None,
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
    pub params: Vec<crate::ast::Pattern>,
    pub body: Vec<Stmt>,
    /// The lexical environment captured by this function. Held *strongly* so that
    /// closures can capture locals; the engine's mark-and-sweep GC reclaims any
    /// cycle that this creates when the function becomes unreachable.
    pub closure: Rc<RefCell<Env>>,
    pub props: BTreeMap<Rc<str>, Property>,
    pub proto: Option<Rc<RefCell<Object>>>,
    /// Prototype object used to resolve `super` member access inside this
    /// function (generally the class prototype that owns the method).
    pub super_proto: Option<Rc<RefCell<Object>>>,
    /// Parent constructor used to resolve `super(...)` inside this constructor.
    pub super_ctor: Option<Value>,
    /// For arrow functions: the lexically-captured `this` binding.
    pub this_capture: Option<Value>,
    /// The realm this function was created in (`None` = the engine's home realm).
    pub realm: Option<Rc<RealmInfo>>,
}

/// A JavaScript array: an element vector plus a prototype link and (for arrays
/// that carry extra named properties such as a `RegExp` `exec` result) an
/// optional property map for non-index keys.
#[derive(Debug, Clone)]
pub struct ArrayData {
    pub elems: Vec<Value>,
    pub proto: Option<Rc<RefCell<Object>>>,
    /// Named (non-index) own properties, e.g. `index`/`input`/`groups` on a
    /// `RegExp` `exec` result.
    pub props: BTreeMap<Rc<str>, Property>,
}

impl ArrayData {
    pub fn new(elems: Vec<Value>, proto: Option<Rc<RefCell<Object>>>) -> Self {
        ArrayData {
            elems,
            proto,
            props: BTreeMap::new(),
        }
    }
}


pub(crate) mod convert;
