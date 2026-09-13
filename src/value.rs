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
    /// `%Promise.prototype%`.
    Promise,
    /// `%Error.prototype%`.
    Error,
    /// A `NativeError.prototype%` (e.g. `"EvalError"`).
    NativeError(&'static str),
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
    pub promise_prototype: Rc<RefCell<Object>>,
    /// The realm's `Error`/`NativeError` prototypes by constructor name.
    pub error_prototypes: Vec<(&'static str, Rc<RefCell<Object>>)>,
}

impl RealmInfo {
    /// The realm's intrinsic prototype for an error constructor name.
    pub fn error_prototype_for(&self, name: &str) -> Option<Rc<RefCell<Object>>> {
        self.error_prototypes
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, p)| p.clone())
    }
}
/// A JavaScript value, restricted to the subset supported by the engine so far.
///
/// `Debug` is cycle-safe: heap graphs routinely contain reference cycles
/// (`ctor.prototype.constructor`), so nested values/objects past a small
/// depth render as `…` instead of recursing forever.
#[derive(Clone)]
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
    /// A `Promise` instance.
    Promise(Rc<RefCell<PromiseData>>),
}

/// State of a `Promise` instance.
#[derive(Debug, Clone)]
pub enum PromiseState {
    /// Still pending; `reactions` are settled (in registration order) when the
    /// promise settles.
    Pending { reactions: Vec<PromiseReaction> },
    Fulfilled(Value),
    Rejected(Value),
}

/// Internal flavour of a promise reaction. `Then` is the ordinary `.then`
/// case; the rest are engine-internal (`await`, `Promise.all`, `finally`,
/// promise adoption) and carry their state directly so no JS-visible closure
/// values are needed.
pub enum PromiseReactionKind {
    /// Ordinary `.then(onF, onR)`; handlers live on the reaction itself.
    Then,
    /// Adopt another promise's settlement into the dependent promise.
    Forward,
    /// Suspended `await` continuation.
    Async { cont: crate::error::SuspendCont },
    /// One element of `Promise.all`: writes into shared slot `index`.
    AllElement {
        index: usize,
        shared: Rc<RefCell<AllState>>,
    },
    /// One element of `Promise.any`: fulfillment settles the aggregate
    /// immediately; rejections accumulate into the shared holder object
    /// (`{ remaining, errors }`) until the last one rejects the aggregate.
    Any { holder: Value, index: usize },
    /// One element of `Promise.allSettled`: records its outcome object.
    AllSettled {
        index: usize,
        shared: Rc<RefCell<AllSettledState>>,
    },
    /// Settle a custom-constructor capability by calling its recorded
    /// `resolve`/`reject` functions (used for species-derived promises).
    Custom {
        promise: Value,
        resolve: Value,
        reject: Value,
    },
    /// First stage of `finally(cb)`: call `cb`, then propagate `original`.
    FinallyCall { cb: Value },
    /// Second stage of `finally`: `cb` fulfilled, settle with `original`.
    FinallyPropagate { original: crate::error::Settlement },
}

impl Clone for PromiseReactionKind {
    fn clone(&self) -> Self {
        match self {
            PromiseReactionKind::Then => PromiseReactionKind::Then,
            PromiseReactionKind::Forward => PromiseReactionKind::Forward,
            PromiseReactionKind::Async { cont } => {
                PromiseReactionKind::Async { cont: cont.clone() }
            }
            PromiseReactionKind::AllElement { index, shared } => {
                PromiseReactionKind::AllElement {
                    index: *index,
                    shared: shared.clone(),
                }
            }
            PromiseReactionKind::Any { holder, index } => PromiseReactionKind::Any {
                holder: holder.clone(),
                index: *index,
            },
            PromiseReactionKind::AllSettled { index, shared } => {
                PromiseReactionKind::AllSettled {
                    index: *index,
                    shared: shared.clone(),
                }
            }
            PromiseReactionKind::Custom {
                promise,
                resolve,
                reject,
            } => PromiseReactionKind::Custom {
                promise: promise.clone(),
                resolve: resolve.clone(),
                reject: reject.clone(),
            },
            PromiseReactionKind::FinallyCall { cb } => {
                PromiseReactionKind::FinallyCall { cb: cb.clone() }
            }
            PromiseReactionKind::FinallyPropagate { original } => {
                PromiseReactionKind::FinallyPropagate {
                    original: original.clone(),
                }
            }
        }
    }
}

impl core::fmt::Debug for PromiseReactionKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PromiseReactionKind::Then => write!(f, "Then"),
            PromiseReactionKind::Forward => write!(f, "Forward"),
            PromiseReactionKind::Async { .. } => write!(f, "Async(..)"),
            PromiseReactionKind::AllElement { index, .. } => {
                write!(f, "AllElement({index})")
            }
            PromiseReactionKind::Any { index, .. } => {
                write!(f, "Any({index})")
            }
            PromiseReactionKind::AllSettled { index, .. } => {
                write!(f, "AllSettled({index})")
            }
            PromiseReactionKind::Custom { .. } => {
                write!(f, "Custom(..)")
            }
            PromiseReactionKind::FinallyCall { .. } => write!(f, "FinallyCall(..)"),
            PromiseReactionKind::FinallyPropagate { .. } => {
                write!(f, "FinallyPropagate(..)")
            }
        }
    }
}

/// Shared countdown state for `Promise.all`.
#[derive(Debug, Clone)]
pub struct AllState {
    pub remaining: usize,
    pub results: Vec<Option<Value>>,
    /// Set once the aggregate settles (first rejection wins; fulfillment when
    /// `remaining` hits zero) so late settlements are ignored.
    pub settled: bool,
}

/// Shared countdown state for `Promise.allSettled`.
#[derive(Debug, Clone)]
pub struct AllSettledState {
    pub remaining: usize,
    pub results: Vec<Option<Value>>,
}

/// A reaction registered on a pending promise: the dependent ("capability")
/// promise to settle with the handler's outcome.
#[derive(Clone)]
pub struct PromiseReaction {
    pub kind: PromiseReactionKind,
    pub on_fulfilled: Option<Value>,
    pub on_rejected: Option<Value>,
    pub promise: Rc<RefCell<PromiseData>>,
}

impl core::fmt::Debug for PromiseReaction {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PromiseReaction")
            .field("kind", &self.kind)
            .field("on_fulfilled", &self.on_fulfilled)
            .field("on_rejected", &self.on_rejected)
            .finish()
    }
}

/// Backing storage for a `Promise`.
#[derive(Debug, Clone)]
pub struct PromiseData {
    pub state: PromiseState,
    /// Own (non-index) properties, e.g. user-added `p.foo = 1`.
    pub props: BTreeMap<Rc<str>, Property>,
    /// `[[Prototype]]` override set via `Object.setPrototypeOf` (`None` means
    /// the engine's `%Promise.prototype%`).
    pub proto: Option<Rc<RefCell<Object>>>,
}

impl PromiseData {
    pub fn new_pending() -> Self {
        PromiseData {
            state: PromiseState::Pending { reactions: Vec::new() },
            props: BTreeMap::new(),
            proto: None,
        }
    }

    pub fn is_pending(&self) -> bool {
        matches!(self.state, PromiseState::Pending { .. })
    }
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
    /// The synthetic property-key string for a well-known symbol (mirrors
    /// [`SymbolData::well_known`]'s id format).
    pub fn well_known_key(name: &str) -> Rc<str> {
        Rc::from(alloc::format!("__wk_{}__", name).as_str())
    }

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
            id: SymbolData::well_known_key(name),
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
///
/// `Debug` shares `Value`'s cycle guard (see above).
#[derive(Clone)]
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

// Cycle-safe `Debug` support: heap graphs contain reference cycles, so
// formatting bails out with `…` past a small nesting depth.
thread_local! {
    static DEBUG_DEPTH: core::cell::Cell<usize> = const { core::cell::Cell::new(0) };
}
const DEBUG_MAX_DEPTH: usize = 8;

fn debug_enter() -> bool {
    let d = DEBUG_DEPTH.get();
    if d >= DEBUG_MAX_DEPTH {
        return false;
    }
    DEBUG_DEPTH.set(d + 1);
    true
}

fn debug_exit() {
    DEBUG_DEPTH.set(DEBUG_DEPTH.get().saturating_sub(1));
}

impl core::fmt::Debug for Value {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if !debug_enter() {
            return write!(f, "…");
        }
        let r = match self {
            Value::Undefined => write!(f, "Undefined"),
            Value::Null => write!(f, "Null"),
            Value::Boolean(b) => f.debug_tuple("Boolean").field(b).finish(),
            Value::Number(n) => f.debug_tuple("Number").field(n).finish(),
            Value::String(s) => f.debug_tuple("String").field(s).finish(),
            Value::Object(o) => f.debug_tuple("Object").field(o).finish(),
            Value::Array(a) => f.debug_tuple("Array").field(a).finish(),
            Value::Function(func) => f.debug_tuple("Function").field(func).finish(),
            Value::NativeFunction(nf) => f.debug_tuple("NativeFunction").field(nf).finish(),
            Value::BigInt(s) => f.debug_tuple("BigInt").field(s).finish(),
            Value::Regex(r) => f.debug_tuple("Regex").field(r).finish(),
            Value::Symbol(s) => f.debug_tuple("Symbol").field(s).finish(),
            Value::Map(m) => f.debug_tuple("Map").field(m).finish(),
            Value::Set(s) => f.debug_tuple("Set").field(s).finish(),
            Value::WeakMap(w) => f.debug_tuple("WeakMap").field(w).finish(),
            Value::WeakSet(w) => f.debug_tuple("WeakSet").field(w).finish(),
            Value::Promise(p) => f.debug_tuple("Promise").field(p).finish(),
        };
        debug_exit();
        r
    }
}

impl core::fmt::Debug for Object {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if !debug_enter() {
            return write!(f, "…");
        }
        let r = f
            .debug_struct("Object")
            .field("props", &self.props)
            .field("proto", &self.proto)
            .field("ctor", &self.ctor)
            .finish();
        debug_exit();
        r
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
    /// The `[[Prototype]]` when it is a *function value* (e.g. a NativeError
    /// constructor's prototype is the `Error` constructor itself). Consulted
    /// by `Object.getPrototypeOf`; `proto` handles Object-valued chains.
    pub fn_value_proto: Option<Value>,
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
            fn_value_proto: None,
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
    /// Whether this is a generator function (`function*`): calls return a
    /// generator object instead of executing the body.
    pub generator: bool,
    /// Whether this is an `async` function: calls return a `Promise` and the
    /// body runs with `await` suspension enabled.
    pub is_async: bool,
    /// Whether this is a derived class constructor (`class C extends B`):
    /// construction runs `super()` (binding `this` from it) and the
    /// constructed `this` value—not the placeholder instance—is returned.
    pub is_derived: bool,
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
