use alloc::rc::Rc;
use core::cell::{Cell, RefCell};

use crate::ast::*;
use crate::environment::Env;
use crate::error::{Error, Result};
use crate::parser;
use crate::value::{Object, Property, RealmInfo, Value};

mod exec;
mod gc;
mod jobs;
mod object;
pub(crate) mod ops;

use self::gc::{GcHeap, GC_THRESHOLD};
pub(crate) use self::jobs::{Job, PromiseCapability, TimerEntry};

pub struct Engine {
    pub(crate) global_object: Rc<RefCell<Object>>,
    pub(crate) global_env: Rc<RefCell<Env>>,
    pub(crate) function_prototype: Rc<RefCell<Object>>,
    pub(crate) object_prototype: Rc<RefCell<Object>>,
    pub(crate) array_prototype: Rc<RefCell<Object>>,
    pub(crate) string_prototype: Rc<RefCell<Object>>,
    pub(crate) number_prototype: Rc<RefCell<Object>>,
    pub(crate) boolean_prototype: Rc<RefCell<Object>>,
    pub(crate) error_prototype: Rc<RefCell<Object>>,
    pub(crate) symbol_prototype: Rc<RefCell<Object>>,
    pub(crate) map_prototype: Rc<RefCell<Object>>,
    pub(crate) set_prototype: Rc<RefCell<Object>>,
    pub(crate) weakmap_prototype: Rc<RefCell<Object>>,
    pub(crate) weakset_prototype: Rc<RefCell<Object>>,
    pub(crate) regexp_prototype: Rc<RefCell<Object>>,
    pub(crate) bigint_prototype: Rc<RefCell<Object>>,
    /// Prototype shared by all generator objects (`next`/`return`/`throw`).
    pub(crate) generator_prototype: Rc<RefCell<Object>>,
    /// `%Promise.prototype%` shared by all promise instances.
    pub(crate) promise_prototype: Rc<RefCell<Object>>,
    /// `%AsyncFunction.prototype%` (proto: `%Function.prototype%`).
    pub(crate) async_function_prototype: Rc<RefCell<Object>>,
    /// Active `yield` sink while a generator body is being run eagerly.
    pub(crate) yield_sink: RefCell<Option<Rc<RefCell<Vec<Value>>>>>,
    /// Pending spec jobs (promise reactions + async continuations).
    pub(crate) job_queue: RefCell<alloc::collections::VecDeque<Job>>,
    /// Pending timers (macrotasks) ordered by virtual deadline.
    pub(crate) timers: RefCell<alloc::vec::Vec<TimerEntry>>,
    /// Ids cancelled while their timer was in flight (checked on reschedule).
    pub(crate) cancelled_timers: RefCell<alloc::vec::Vec<u64>>,
    /// Next timer id.
    pub(crate) next_timer_id: Cell<u64>,
    /// Virtual clock in milliseconds (advanced only by timer pumping).
    pub(crate) now_ms: Cell<u64>,
    /// Depth of nested `async` function execution (for `await` validity).
    pub(crate) async_depth: Cell<usize>,
    /// Re-entrancy depth of `drain_jobs` (jobs may enqueue more jobs).
    pub(crate) job_depth: Cell<usize>,
    /// Lexical async context: pushed on every user-function entry (`true` for
    /// `async` functions). Empty at top level. `await` consults the top.
    pub(crate) func_async_stack: RefCell<alloc::vec::Vec<bool>>,
    /// Unhandled promise rejections / microtask exceptions for the host to
    /// report (CLI exit code, test262 failure detail).
    pub(crate) unhandled: RefCell<alloc::vec::Vec<Value>>,
    pub(crate) type_error_proto: Rc<RefCell<Object>>,
    pub(crate) range_error_proto: Rc<RefCell<Object>>,
    pub(crate) reference_error_proto: Rc<RefCell<Object>>,
    pub(crate) syntax_error_proto: Rc<RefCell<Object>>,
    pub(crate) eval_error_proto: Rc<RefCell<Object>>,
    pub(crate) uri_error_proto: Rc<RefCell<Object>>,
    /// Per-realm registry for `Symbol.for(key)`.
    pub(crate) symbol_registry: RefCell<std::collections::HashMap<Rc<str>, Rc<crate::value::SymbolData>>>,
    /// The garbage-collector heap (weakly-registered live cells).
    pub(crate) gc: RefCell<GcHeap>,
    /// Nesting depth of `Engine::eval`; collection only runs at the top level.
    pub(crate) eval_depth: Cell<usize>,
    /// Whether the currently executing program is in strict mode (set by a
    /// leading `"use strict"` directive). Mutations to non-writable properties
    /// and deletes of non-configurable properties throw a `TypeError` when true.
    pub(crate) strict: Cell<bool>,
    /// This engine's realm intrinsic prototypes.
    pub(crate) realm: Rc<RealmInfo>,
    /// The realm of the native constructor currently being invoked, used by
    /// realm-aware builtins (e.g. the `Function` constructor) to create values
    /// in the correct realm.
    pub(crate) native_realm: RefCell<Option<Rc<RealmInfo>>>,
    /// Number of allocations since the last collection, used to trigger a
    /// top-level sweep once the heap has grown past [`GC_THRESHOLD`].
    pub(crate) gc_pressure: Cell<usize>,
    /// The native constructor currently executing (set around native calls) so
    /// shared native implementations (e.g. the error constructors) can identify
    /// which intrinsic they act on.
    pub(crate) active_native_ctor: RefCell<Option<Value>>,
    /// The `new.target` of the currently executing construction (set around
    /// both native and user-function calls made by `construct`). Used by
    /// `super()` calls, `promise_ctor` (subclass prototype) and species
    /// machinery.
    pub(crate) active_new_target: RefCell<Option<Value>>,
    /// In-flight `NewPromiseCapability` executor records, keyed by id.
    pub(crate) cap_records:
        RefCell<std::collections::HashMap<u64, Rc<RefCell<Option<(Value, Value)>>>>>,
    /// Next capability-record id.
    pub(crate) next_cap_id: Cell<u64>,
}


impl Engine {
    pub fn new() -> Self {
        let object_prototype = Rc::new(RefCell::new(Object::new_root()));
        let global_object = Rc::new(RefCell::new(Object::with_proto(object_prototype.clone())));
        let global_env = Rc::new(RefCell::new(Env::new_global()));
        global_env
            .borrow_mut()
            .vars
            .insert(Rc::from("__this__"), Value::Object(global_object.clone()));        let function_prototype = Rc::new(RefCell::new(Object::with_proto(object_prototype.clone())));
        let array_prototype = Rc::new(RefCell::new(Object::with_proto(object_prototype.clone())));
        let string_prototype = Rc::new(RefCell::new(Object::with_proto(object_prototype.clone())));
        let number_prototype = Rc::new(RefCell::new(Object::with_proto(object_prototype.clone())));
        let boolean_prototype = Rc::new(RefCell::new(Object::with_proto(object_prototype.clone())));
        // `String`/`Number`/`Boolean` prototypes are themselves the primitive
        // wrapper objects for `""`, `0` and `false`. Marking them with the
        // internal `__value__` slot lets `valueOf()`/`toString()`/`String(proto)`
        // resolve correctly and makes `Object.prototype.toString.call(proto)`
        // report the right `[object String]` / `[object Number]` / `[object Boolean]`.
        string_prototype.borrow_mut().props.insert(
            Rc::from("__value__"),
            Property::new(Value::String(Rc::from(""))),
        );
        number_prototype
            .borrow_mut()
            .props
            .insert(Rc::from("__value__"), Property::new(Value::Number(0.0)));
        boolean_prototype
            .borrow_mut()
            .props
            .insert(Rc::from("__value__"), Property::new(Value::Boolean(false)));
        let error_prototype = Rc::new(RefCell::new(Object::with_proto(object_prototype.clone())));
        let type_error_proto = Rc::new(RefCell::new(Object::with_proto(error_prototype.clone())));
        let range_error_proto = Rc::new(RefCell::new(Object::with_proto(error_prototype.clone())));
        let reference_error_proto = Rc::new(RefCell::new(Object::with_proto(error_prototype.clone())));
        let syntax_error_proto = Rc::new(RefCell::new(Object::with_proto(error_prototype.clone())));
        let eval_error_proto = Rc::new(RefCell::new(Object::with_proto(error_prototype.clone())));
        let uri_error_proto = Rc::new(RefCell::new(Object::with_proto(error_prototype.clone())));
        let promise_prototype =
            Rc::new(RefCell::new(Object::with_proto(object_prototype.clone())));
        let async_function_prototype =
            Rc::new(RefCell::new(Object::with_proto(function_prototype.clone())));
        let realm = Rc::new(RealmInfo {
            global_env: global_env.clone(),
            object_prototype: object_prototype.clone(),
            number_prototype: number_prototype.clone(),
            string_prototype: string_prototype.clone(),
            boolean_prototype: boolean_prototype.clone(),
            promise_prototype: promise_prototype.clone(),
            error_prototypes: alloc::vec![
                ("Error", error_prototype.clone()),
                ("TypeError", type_error_proto.clone()),
                ("RangeError", range_error_proto.clone()),
                ("ReferenceError", reference_error_proto.clone()),
                ("SyntaxError", syntax_error_proto.clone()),
                ("EvalError", eval_error_proto.clone()),
                ("URIError", uri_error_proto.clone()),
            ],
        });
        let mut engine = Engine {
            global_object,
            global_env,
            function_prototype: function_prototype.clone(),
            object_prototype: object_prototype.clone(),
            array_prototype,
            string_prototype,
            number_prototype,
            boolean_prototype,
            error_prototype: error_prototype.clone(),
            symbol_prototype: Rc::new(RefCell::new(Object::with_proto(object_prototype.clone()))),
            map_prototype: Rc::new(RefCell::new(Object::with_proto(object_prototype.clone()))),
            set_prototype: Rc::new(RefCell::new(Object::with_proto(object_prototype.clone()))),
            weakmap_prototype: Rc::new(RefCell::new(Object::with_proto(object_prototype.clone()))),
            weakset_prototype: Rc::new(RefCell::new(Object::with_proto(object_prototype.clone()))),
            regexp_prototype: Rc::new(RefCell::new(Object::with_proto(object_prototype.clone()))),
            bigint_prototype: Rc::new(RefCell::new(Object::with_proto(object_prototype.clone()))),
            generator_prototype: Rc::new(RefCell::new(Object::with_proto(object_prototype.clone()))),
            promise_prototype,
            async_function_prototype,
            yield_sink: RefCell::new(None),
            job_queue: RefCell::new(alloc::collections::VecDeque::new()),
            timers: RefCell::new(alloc::vec::Vec::new()),
            cancelled_timers: RefCell::new(alloc::vec::Vec::new()),
            next_timer_id: Cell::new(1),
            now_ms: Cell::new(0),
            async_depth: Cell::new(0),
            job_depth: Cell::new(0),
            func_async_stack: RefCell::new(alloc::vec::Vec::new()),
            unhandled: RefCell::new(alloc::vec::Vec::new()),
            type_error_proto,
            range_error_proto,
            reference_error_proto,
            syntax_error_proto,
            eval_error_proto,
            uri_error_proto,
            symbol_registry: RefCell::new(std::collections::HashMap::new()),
            gc: RefCell::new(GcHeap::default()),
            eval_depth: Cell::new(0),
            strict: Cell::new(false),
            realm,
            native_realm: RefCell::new(None),
            gc_pressure: Cell::new(0),
            active_native_ctor: RefCell::new(None),
            active_new_target: RefCell::new(None),
            cap_records: RefCell::new(std::collections::HashMap::new()),
            next_cap_id: Cell::new(1),
        };
        engine.register_env(&engine.global_env.clone());
        engine.register_builtins();
        engine
    }

    pub fn call_value(&self, func: &Value, this: &Value, args: &[Value]) -> Result<Value> {
        self.call_function(func, this, args)
    }

    /// Return the engine's global object as a `Value` (useful for host wiring
    /// such as the test262 `$262.global` binding).
    pub fn global_object_value(&self) -> Value {
        Value::Object(self.global_object.clone())
    }

    pub fn register_global(&mut self, name: &str, val: Value) {
        self.global_env
            .borrow_mut()
            .vars
            .insert(Rc::from(name), val.clone());
        let prop = match name {
            // These are non-writable, non-enumerable, non-configurable globals.
            "undefined" | "NaN" | "Infinity" => Property::constant(val),
            // Global function/object bindings are writable + configurable but
            // non-enumerable.
            _ => Property::method(val),
        };
        self.global_object
            .borrow_mut()
            .props
            .insert(Rc::from(name), prop);
    }

    pub fn parse_source(&self, src: &str) -> Result<Program> {
        parser::parse(src)
    }

    pub fn eval(&self, src: &str) -> Result<Value> {
        let prog = parser::parse(src)?;
        // Class private names are not implemented; a body that references a
        // private field is a SyntaxError (the private name is never in scope).
        if prog.has_private_member() {
            return Err(Error::Runtime(self.make_syntax_error(
                "Private field is not declared",
            )));
        }
        let depth = self.eval_depth.get();
        self.eval_depth.set(depth + 1);
        let strict = self.strict.get();
        if depth == 0 {
            self.strict.set(prog.strict);
        }
        let result = self.exec_stmts(&prog.stmts, &self.global_env);
        self.eval_depth.set(depth);
        self.strict.set(strict);
        // Control-flow signals never escape a program: a break/continue whose
        // target is inside the program completes normally with its value.
        // A stray `Suspend` (await surviving past its async boundary, which
        // cannot happen for valid code) is reported as unimplemented rather
        // than leaking an internal signal.
        let result = match result {
            Ok(v) => Ok(v.unwrap_or(Value::Undefined)),
            Err(Error::Break(_, v)) => Ok(v.unwrap_or(Value::Undefined)),
            Err(Error::Continue(_, _)) => Ok(Value::Undefined),
            Err(Error::Suspend { .. }) => Err(Error::Unimplemented(
                "top-level await is not implemented".to_string(),
            )),
            Err(e) => Err(e),
        };
        // Run pending microtasks to completion before returning to the host,
        // so `then` callbacks and async continuations are observable via
        // subsequent `eval` calls (and test262 `$DONE`). Then pump due timers
        // on the virtual clock (bounded, so `setInterval(..., 0)` terminates).
        if depth == 0 {
            self.drain_jobs();
            self.drain_timers(jobs::MAX_TIMER_TASKS);
        }
        if depth == 0 {
            let extra = match &result {
                Ok(v) => Some(v),
                Err(Error::Runtime(v)) => Some(v),
                _ => None,
            };
            if self.gc_pressure.get() > GC_THRESHOLD {
                self.collect_with(extra);
            }
        }
        result
    }

    fn register_builtins(&mut self) {
        crate::builtins::register_builtins(self);
    }
}

impl Default for Engine {
    fn default() -> Self {
        Engine::new()
    }
}

// Intrinsic constructors keep a strong reference to their realm, while the
// realm keeps the intrinsic prototypes (and those prototypes keep the
// constructors) alive.  Break that cycle when an engine is dropped so a
// long-running test262 run can reclaim each per-test realm promptly.
impl Drop for Engine {
    fn drop(&mut self) {
        self.global_env.borrow_mut().vars.clear();
        self.global_env.borrow_mut().outer = None;
        self.global_object.borrow_mut().props.clear();
        self.global_object.borrow_mut().proto = None;

        self.yield_sink.borrow_mut().take();
        self.job_queue.borrow_mut().clear();
        self.timers.borrow_mut().clear();
        self.cancelled_timers.borrow_mut().clear();
        self.func_async_stack.borrow_mut().clear();
        self.unhandled.borrow_mut().clear();
        self.native_realm.borrow_mut().take();
        self.active_native_ctor.borrow_mut().take();
        self.active_new_target.borrow_mut().take();
        self.cap_records.borrow_mut().clear();
        self.symbol_registry.borrow_mut().clear();

        let prototypes = [
            &self.function_prototype,
            &self.object_prototype,
            &self.array_prototype,
            &self.string_prototype,
            &self.number_prototype,
            &self.boolean_prototype,
            &self.error_prototype,
            &self.symbol_prototype,
            &self.map_prototype,
            &self.set_prototype,
            &self.weakmap_prototype,
            &self.weakset_prototype,
            &self.regexp_prototype,
            &self.bigint_prototype,
            &self.generator_prototype,
            &self.promise_prototype,
            &self.async_function_prototype,
            &self.type_error_proto,
            &self.range_error_proto,
            &self.reference_error_proto,
            &self.syntax_error_proto,
            &self.eval_error_proto,
            &self.uri_error_proto,
        ];
        for prototype in prototypes {
            let mut object = prototype.borrow_mut();
            object.props.clear();
            object.proto = None;
            object.ctor = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;
    use crate::value::{MapData, Property, SetData};

    #[test]
    fn gc_collects_unreachable_cycle() {
        let engine = Engine::new();
        // Build a self-referential object and drop our own handle: it is now
        // only kept alive by its own cycle, which is unreachable from roots.
        let o = engine.new_object();
        let weak = Rc::downgrade(&o);
        o.borrow_mut()
            .props
            .insert(Rc::from("self"), Property::new(Value::Object(o.clone())));
        drop(o);
        engine.collect_garbage();
        assert!(
            weak.upgrade().is_none(),
            "unreachable cycle was not collected"
        );
    }

    #[test]
    fn gc_preserves_reachable_from_global() {
        let engine = Engine::new();
        engine
            .eval("globalThis.g = {}; globalThis.g.self = globalThis.g;")
            .unwrap();
        // A cycle reachable from a global must survive collection.
        assert!(
            engine.gc.borrow().objects.iter().any(|w| w.upgrade().is_some()),
            "reachable object was wrongly collected"
        );
        // The object is still usable.
        assert_eq!(engine.eval("globalThis.g.self === globalThis.g").unwrap(), Value::Boolean(true));
    }

    #[test]
    fn gc_preserves_eval_result() {
        let engine = Engine::new();
        let v = engine.eval("({ a: 1 })").unwrap();
        assert_eq!(engine.get_property(&v, "a"), Value::Number(1.0));
    }

    #[test]
    fn block_scoping_isolates_let() {
        let e = Engine::new();
        e.eval("var g = 1; { let g = 2; }").unwrap();
        assert_eq!(e.eval("g").unwrap(), Value::Number(1.0));
        // `let` is not visible after the block.
        assert_eq!(e.eval("typeof g").unwrap(), Value::String(Rc::from("number")));
    }

    #[test]
    fn var_in_block_reaches_function_scope() {
        let e = Engine::new();
        let v = e.eval("(function(){ if (true) { var x = 5; } return x; })()").unwrap();
        assert_eq!(v, Value::Number(5.0));
    }

    #[test]
    fn function_var_does_not_leak_globally() {
        let e = Engine::new();
        e.eval("(function(){ var x = {}; })()").unwrap();
        assert_eq!(e.eval("typeof x").unwrap(), Value::String(Rc::from("undefined")));
    }

    #[test]
    fn gc_collects_unreachable_map_cycle() {
        let engine = Engine::new();
        // A Map whose only reference is its own entry is an unreachable cycle.
        let m = Rc::new(RefCell::new(MapData { entries: Vec::new() }));
        engine.register_map(&m);
        m.borrow_mut()
            .entries
            .push((Value::Map(m.clone()), Value::Map(m.clone())));
        let weak = Rc::downgrade(&m);
        drop(m);
        engine.collect_garbage();
        assert!(weak.upgrade().is_none(), "unreachable Map cycle was not collected");
    }

    #[test]
    fn gc_collects_unreachable_set() {
        let engine = Engine::new();
        let s = Rc::new(RefCell::new(SetData { entries: Vec::new() }));
        engine.register_set(&s);
        s.borrow_mut().entries.push(Value::Set(s.clone()));
        let weak = Rc::downgrade(&s);
        drop(s);
        engine.collect_garbage();
        assert!(weak.upgrade().is_none(), "unreachable Set was not collected");
    }

    #[test]
    fn for_let_head_is_loop_scoped() {
        let e = Engine::new();
        // `let i` is scoped to the loop; it must not leak afterwards.
        let v = e.eval("{ let i = 0; for (let i = 0; i < 3; i++) { } i; }").unwrap();
        assert_eq!(v, Value::Number(0.0));
    }

    #[test]
    fn const_reassignment_throws_type_error() {
        let e = Engine::new();
        assert!(e.eval("const x = 1; x = 2;").is_err());
        assert!(e.eval("const x = 1; x++;").is_err());
    }

    #[test]
    fn const_can_mutate_its_object() {
        let e = Engine::new();
        // Re-binding the variable is rejected, but mutating the object is fine.
        assert!(e.eval("const o = { a: 1 }; o.a = 2; o.a").unwrap() == Value::Number(2.0));
    }

    #[test]
    fn closure_captures_local() {
        let e = Engine::new();
        let v = e
            .eval("function make(){ let x = 5; return function(){ return x; }; } make()()")
            .unwrap();
        assert_eq!(v, Value::Number(5.0));
    }

    #[test]
    fn for_let_per_iteration_binding() {
        let e = Engine::new();
        // Each iteration must have its own `i`; closures capture per-iteration.
        let v = e
            .eval("var f = []; for (let i = 0; i < 3; i++){ f.push(function(){ return i; }); } f[0]()+f[1]()+f[2]()")
            .unwrap();
        assert_eq!(v, Value::Number(3.0)); // 0 + 1 + 2
    }

    #[test]
    fn gc_collects_unreachable_closure() {
        let engine = Engine::new();
        // Create a closure capturing a local, then discard the IIFE result.
        engine
            .eval("(function(){ var a = 1; return function(){ return a; }; })();")
            .unwrap();
        engine.collect_garbage();
        assert!(engine.gc.borrow().functions.iter().all(|w| w.upgrade().is_none()));
    }

    #[test]
    fn for_of_let_per_iteration_binding() {
        let e = Engine::new();
        let v = e
            .eval("var f = []; for (let x of [1, 2, 3]){ f.push(function(){ return x; }); } f[0]()+f[1]()+f[2]()")
            .unwrap();
        assert_eq!(v, Value::Number(6.0)); // 1 + 2 + 3
    }

    #[test]
    fn for_in_let_per_iteration_binding() {
        let e = Engine::new();
        let v = e
            .eval("var f = []; for (let k in { a: 1, b: 2 }){ f.push(function(){ return k; }); } f[0]() + f[1]()")
            .unwrap();
        // `BTreeSet` yields the keys sorted; both closures see their own key.
        assert_eq!(v, Value::String(Rc::from("ab")));
    }

    #[test]
    fn for_of_const_head_rejects_reassignment() {
        let e = Engine::new();
        assert!(e.eval("for (const x of [1, 2]){ x = 5; }").is_err());
        assert!(e.eval("for (const x of [1, 2]){ x++; }").is_err());
    }

    #[test]
    fn for_of_const_head_still_iterates() {
        let e = Engine::new();
        let v = e.eval("var s = 0; for (const x of [1, 2, 3]){ s += x; } s").unwrap();
        assert_eq!(v, Value::Number(6.0));
    }
}
