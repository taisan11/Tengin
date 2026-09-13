//! The mark-and-sweep garbage collector and the engine's memory-management
//! path. Every heap cell allocated through `Engine::new_object` /
//! `Engine::new_array` / `Engine::make_function` is weakly registered here so
//! that the collector can reclaim unreachable reference cycles.

use alloc::rc::{Rc, Weak};
use alloc::vec::Vec;
use core::cell::RefCell;
use std::collections::HashSet;

use crate::environment::Env;
use crate::value::{
    ArrayData, Function, MapData, NativeFunctionData, Object, PromiseData, SetData, Value,
    WeakMapData, WeakSetData,
};

use super::Engine;

/// The per-engine garbage-collector heap.
///
/// Every cell allocated through the interpreter's `new_object` / `new_array` /
/// `make_function` paths is weakly registered here. A `Value` (or an `Env`
/// reachable from a closure) holds the only *strong* references, so any cell
/// that is not reachable from the mark roots (the globals / built-in
/// prototypes / the last evaluation result) is garbage. When one of those
/// unreachable cells is only kept alive by a reference *cycle*, the sweep
/// breaks the cycle by severing its outgoing references, which lets the
/// underlying `Rc`s count down to zero and the memory be reclaimed.
#[derive(Default)]
pub(crate) struct GcHeap {
    pub objects: Vec<Weak<RefCell<Object>>>,
    pub arrays: Vec<Weak<RefCell<ArrayData>>>,
    pub functions: Vec<Weak<RefCell<Function>>>,
    pub envs: Vec<Weak<RefCell<Env>>>,
    #[allow(dead_code)]
    pub natives: Vec<Weak<RefCell<NativeFunctionData>>>,
    #[allow(dead_code)]
    pub maps: Vec<Weak<RefCell<MapData>>>,
    #[allow(dead_code)]
    pub sets: Vec<Weak<RefCell<SetData>>>,
    #[allow(dead_code)]
    pub weakmaps: Vec<Weak<RefCell<WeakMapData>>>,
    #[allow(dead_code)]
    pub weaksets: Vec<Weak<RefCell<WeakSetData>>>,
    pub promises: Vec<Weak<RefCell<PromiseData>>>,
}

pub(crate) const GC_THRESHOLD: usize = 4096;

impl Engine {
    /// Allocate a fresh plain object linked to `Object.prototype`.
    pub(crate) fn new_object(&self) -> Rc<RefCell<Object>> {
        self.make_object(self.object_prototype.clone())
    }

    /// Allocate an object with a custom prototype, registering it for GC.
    pub(crate) fn make_object(&self, proto: Rc<RefCell<Object>>) -> Rc<RefCell<Object>> {
        let o = Rc::new(RefCell::new(Object::with_proto(proto)));
        self.gc.borrow_mut().objects.push(Rc::downgrade(&o));
        self.gc_pressure.set(self.gc_pressure.get() + 1);
        o
    }

    /// Allocate an array value registered for GC.
    pub(crate) fn new_array(&self, elems: Vec<Value>) -> Rc<RefCell<ArrayData>> {
        let a = Rc::new(RefCell::new(ArrayData::new(elems, Some(self.array_prototype.clone()))));
        self.gc.borrow_mut().arrays.push(Rc::downgrade(&a));
        self.gc_pressure.set(self.gc_pressure.get() + 1);
        a
    }

    pub(crate) fn register_function(&self, f: &Rc<RefCell<Function>>) {
        self.gc.borrow_mut().functions.push(Rc::downgrade(f));
        self.gc_pressure.set(self.gc_pressure.get() + 1);
    }

    /// Register a lexical environment for GC. Environments are kept alive
    /// strongly by closures, so they must be swept round with the value cells.
    pub(crate) fn register_env(&self, env: &Rc<RefCell<Env>>) {
        self.gc.borrow_mut().envs.push(Rc::downgrade(env));
        self.gc_pressure.set(self.gc_pressure.get() + 1);
    }

    /// Register a `Map` cell for GC.
    pub(crate) fn register_map(&self, m: &Rc<RefCell<MapData>>) {
        self.gc.borrow_mut().maps.push(Rc::downgrade(m));
        self.gc_pressure.set(self.gc_pressure.get() + 1);
    }

    /// Register a `Set` cell for GC.
    pub(crate) fn register_set(&self, s: &Rc<RefCell<SetData>>) {
        self.gc.borrow_mut().sets.push(Rc::downgrade(s));
        self.gc_pressure.set(self.gc_pressure.get() + 1);
    }

    /// Register a `WeakMap` cell for GC.
    pub(crate) fn register_weakmap(&self, w: &Rc<RefCell<WeakMapData>>) {
        self.gc.borrow_mut().weakmaps.push(Rc::downgrade(w));
        self.gc_pressure.set(self.gc_pressure.get() + 1);
    }

    /// Register a `WeakSet` cell for GC.
    pub(crate) fn register_weakset(&self, w: &Rc<RefCell<WeakSetData>>) {
        self.gc.borrow_mut().weaksets.push(Rc::downgrade(w));
        self.gc_pressure.set(self.gc_pressure.get() + 1);
    }

    /// Run the garbage collector.
    ///
    /// Surviving objects are exactly those reachable from the engine's roots
    /// (global environment, built-in prototypes, and the result of the most
    /// recent top-level evaluation). Any other cell — including complete
    /// reference cycles — is reclaimed by severing its outgoing references so
    /// the underlying `Rc`s count down to zero.
    ///
    /// Call this at a *safe point*: no code executing on the Rust stack may
    /// hold a heap value that is not also reachable from an engine root.
    pub fn collect_garbage(&self) {
        self.collect_with(None);
    }

    pub(crate) fn collect_with(&self, extra: Option<&Value>) {
        let mut marked = HashSet::new();
        let mut values: Vec<Value> = Vec::new();
        let mut envs: Vec<Rc<RefCell<Env>>> = Vec::new();

        // Roots: global object, every built-in prototype, and the current
        // result of the last top-level evaluation.
        values.push(Value::Object(self.global_object.clone()));
        values.push(Value::Object(self.function_prototype.clone()));
        values.push(Value::Object(self.object_prototype.clone()));
        values.push(Value::Object(self.array_prototype.clone()));
        values.push(Value::Object(self.string_prototype.clone()));
        values.push(Value::Object(self.number_prototype.clone()));
        values.push(Value::Object(self.boolean_prototype.clone()));
        values.push(Value::Object(self.error_prototype.clone()));
        values.push(Value::Object(self.symbol_prototype.clone()));
        values.push(Value::Object(self.map_prototype.clone()));
        values.push(Value::Object(self.set_prototype.clone()));
        values.push(Value::Object(self.weakmap_prototype.clone()));
        values.push(Value::Object(self.weakset_prototype.clone()));
        values.push(Value::Object(self.regexp_prototype.clone()));
        values.push(Value::Object(self.promise_prototype.clone()));
        values.push(Value::Object(self.async_function_prototype.clone()));
        envs.push(self.global_env.clone());
        if let Some(v) = extra {
            values.push(v.clone());
        }
        // Jobs and unhandled rejections keep their promises/values alive.
        for job in self.job_queue.borrow().iter() {
            match job {
                super::jobs::Job::Reaction { reaction, settlement } => {
                    values.push(Value::Promise(reaction.promise.clone()));
                    if let Some(f) = reaction.on_fulfilled.clone() {
                        values.push(f);
                    }
                    if let Some(f) = reaction.on_rejected.clone() {
                        values.push(f);
                    }
                    mark_settlement(settlement, &mut values);
                }
                super::jobs::Job::AsyncResume { capability, settlement, .. } => {
                    values.push(Value::Promise(capability.clone()));
                    mark_settlement(settlement, &mut values);
                }
                super::jobs::Job::Thenable { then, thenable, capability } => {
                    values.push(then.clone());
                    values.push(thenable.clone());
                    values.push(Value::Promise(capability.clone()));
                }
                super::jobs::Job::Microtask { callback } => {
                    values.push(callback.clone());
                }
            }
        }
        for v in self.unhandled.borrow().iter() {
            values.push(v.clone());
        }
        // Timers keep their callbacks/args alive.
        for t in self.timers.borrow().iter() {
            values.push(t.callback.clone());
            for a in t.args.iter() {
                values.push(a.clone());
            }
        }

        while let Some(v) = values.pop() {
            mark_value(&v, &mut marked, &mut values, &mut envs);
        }
        while let Some(e) = envs.pop() {
            mark_env(&e, &mut marked, &mut values, &mut envs);
        }

        self.sweep(&marked);
    }

    fn sweep(&self, marked: &HashSet<*const ()>) {
        self.gc_pressure.set(0);
        let mut gc = self.gc.borrow_mut();
        let mark = |o: &Rc<RefCell<Object>>| marked.contains(&(Rc::as_ptr(o) as *const ()));

        let objects = core::mem::take(&mut gc.objects);
        let mut keep = Vec::with_capacity(objects.len());
        for w in objects {
            if let Some(o) = w.upgrade() {
                if mark(&o) {
                    keep.push(Rc::downgrade(&o));
                } else {
                    let mut b = o.borrow_mut();
                    b.props.clear();
                    b.proto = None;
                    b.ctor = None;
                }
            }
        }
        gc.objects = keep;

        let arrays = core::mem::take(&mut gc.arrays);
        let mut keep = Vec::with_capacity(arrays.len());
        for w in arrays {
            if let Some(a) = w.upgrade() {
                if marked.contains(&(Rc::as_ptr(&a) as *const ())) {
                    keep.push(Rc::downgrade(&a));
                } else {
                    a.borrow_mut().elems.clear();
                    a.borrow_mut().proto = None;
                }
            }
        }
        gc.arrays = keep;

        let functions = core::mem::take(&mut gc.functions);
        let mut keep = Vec::with_capacity(functions.len());
        for w in functions {
            if let Some(f) = w.upgrade() {
                if marked.contains(&(Rc::as_ptr(&f) as *const ())) {
                    keep.push(Rc::downgrade(&f));
                } else {
                    let mut b = f.borrow_mut();
                    b.props.clear();
                    b.proto = None;
                    b.super_proto = None;
                    b.super_ctor = None;
                    b.this_capture = None;
                }
            }
        }
        gc.functions = keep;

        let maps = core::mem::take(&mut gc.maps);
        let mut keep = Vec::with_capacity(maps.len());
        for w in maps {
            if let Some(m) = w.upgrade() {
                if marked.contains(&(Rc::as_ptr(&m) as *const ())) {
                    keep.push(Rc::downgrade(&m));
                } else {
                    m.borrow_mut().entries.clear();
                }
            }
        }
        gc.maps = keep;

        let sets = core::mem::take(&mut gc.sets);
        let mut keep = Vec::with_capacity(sets.len());
        for w in sets {
            if let Some(s) = w.upgrade() {
                if marked.contains(&(Rc::as_ptr(&s) as *const ())) {
                    keep.push(Rc::downgrade(&s));
                } else {
                    s.borrow_mut().entries.clear();
                }
            }
        }
        gc.sets = keep;

        let weakmaps = core::mem::take(&mut gc.weakmaps);
        let mut keep = Vec::with_capacity(weakmaps.len());
        for w in weakmaps {
            if let Some(wm) = w.upgrade() {
                if marked.contains(&(Rc::as_ptr(&wm) as *const ())) {
                    keep.push(Rc::downgrade(&wm));
                } else {
                    wm.borrow_mut().entries.clear();
                }
            }
        }
        gc.weakmaps = keep;

        let weaksets = core::mem::take(&mut gc.weaksets);
        let mut keep = Vec::with_capacity(weaksets.len());
        for w in weaksets {
            if let Some(ws) = w.upgrade() {
                if marked.contains(&(Rc::as_ptr(&ws) as *const ())) {
                    keep.push(Rc::downgrade(&ws));
                } else {
                    ws.borrow_mut().entries.clear();
                }
            }
        }
        gc.weaksets = keep;

        let promises = core::mem::take(&mut gc.promises);
        let mut keep = Vec::with_capacity(promises.len());
        for w in promises {
            if let Some(p) = w.upgrade() {
                if marked.contains(&(Rc::as_ptr(&p) as *const ())) {
                    keep.push(Rc::downgrade(&p));
                } else {
                    let mut b = p.borrow_mut();
                    b.props.clear();
                    b.proto = None;
                    if let crate::value::PromiseState::Pending { reactions } = &mut b.state {
                        reactions.clear();
                    }
                }
            }
        }
        gc.promises = keep;

        // Environments are kept alive strongly by closures; break their variable
        // bindings when unreachable so closure/environment cycles collapse.
        let envs = core::mem::take(&mut gc.envs);
        let mut keep = Vec::with_capacity(envs.len());
        for w in envs {
            if let Some(env) = w.upgrade() {
                if marked.contains(&(Rc::as_ptr(&env) as *const ())) {
                    keep.push(Rc::downgrade(&env));
                } else {
                    let mut b = env.borrow_mut();
                    b.vars.clear();
                    b.constants.clear();
                    b.outer = None;
                }
            }
        }
        gc.envs = keep;
    }
}

/// Mark the object graph reachable from a single value. Newly discovered
/// objects are pushed to `values` (and new live environments to `envs`).
fn mark_value(v: &Value, marked: &mut HashSet<*const ()>, values: &mut Vec<Value>, envs: &mut Vec<Rc<RefCell<Env>>>) {
    match v {
        Value::Object(o) => {
            if marked.insert(Rc::as_ptr(o) as *const ()) {
                let g = o.borrow();
                for p in g.props.values() {
                    values.push(p.value.clone());
                    if let Some(get) = &p.get {
                        values.push(get.clone());
                    }
                    if let Some(set) = &p.set {
                        values.push(set.clone());
                    }
                }
                if let Some(pr) = &g.proto {
                    values.push(Value::Object(pr.clone()));
                }
            }
        }
        Value::Array(a) => {
            if marked.insert(Rc::as_ptr(a) as *const ()) {
                let g = a.borrow();
                for e in &g.elems {
                    values.push(e.clone());
                }
                if let Some(pr) = &g.proto {
                    values.push(Value::Object(pr.clone()));
                }
            }
        }
        Value::Function(f) => {
            if marked.insert(Rc::as_ptr(f) as *const ()) {
                let g = f.borrow();
                for p in g.props.values() {
                    values.push(p.value.clone());
                }
                if let Some(pr) = &g.proto {
                    values.push(Value::Object(pr.clone()));
                }
                if let Some(sp) = &g.super_proto {
                    values.push(Value::Object(sp.clone()));
                }
                if let Some(sc) = &g.super_ctor {
                    values.push(sc.clone());
                }
                if let Some(t) = &g.this_capture {
                    values.push(t.clone());
                }
                envs.push(g.closure.clone());
            }
        }
        Value::NativeFunction(nf) => {
            if marked.insert(Rc::as_ptr(nf) as *const ()) {
                let g = nf.borrow();
                for p in g.props.values() {
                    values.push(p.value.clone());
                }
                if let Some(pr) = &g.proto {
                    values.push(Value::Object(pr.clone()));
                }
            }
        }
        Value::Map(m) => {
            if marked.insert(Rc::as_ptr(m) as *const ()) {
                let g = m.borrow();
                for (k, v) in &g.entries {
                    values.push(k.clone());
                    values.push(v.clone());
                }
            }
        }
        Value::Set(s) => {
            if marked.insert(Rc::as_ptr(s) as *const ()) {
                let g = s.borrow();
                for v in &g.entries {
                    values.push(v.clone());
                }
            }
        }
        Value::WeakMap(w) => {
            if marked.insert(Rc::as_ptr(w) as *const ()) {
                // Weak keys do not keep their keys alive.
                let g = w.borrow();
                for (_, v) in &g.entries {
                    values.push(v.clone());
                }
            }
        }
        Value::WeakSet(w) => {
            if marked.insert(Rc::as_ptr(w) as *const ()) {
                // No strong references are kept.
            }
        }
        Value::Promise(p) => {
            if marked.insert(Rc::as_ptr(p) as *const ()) {
                let g = p.borrow();
                for prop in g.props.values() {
                    values.push(prop.value.clone());
                }
                if let Some(pr) = &g.proto {
                    values.push(Value::Object(pr.clone()));
                }
                // Reaction-held values (handlers, capabilities, aggregate
                // slots, finally payloads) keep their graph alive. Suspended
                // `await` continuations are opaque here, but no collection
                // runs mid-drain while they exist (see `drain_jobs`).
                match &g.state {
                    crate::value::PromiseState::Pending { reactions } => {
                        for r in reactions {
                            values.push(Value::Promise(r.promise.clone()));
                            if let Some(f) = r.on_fulfilled.clone() {
                                values.push(f);
                            }
                            if let Some(f) = r.on_rejected.clone() {
                                values.push(f);
                            }
                            match &r.kind {
                                crate::value::PromiseReactionKind::AllElement { shared, .. } => {
                                    for slot in shared.borrow().results.iter().flatten() {
                                        values.push(slot.clone());
                                    }
                                }
                                crate::value::PromiseReactionKind::AllSettled { shared, .. } => {
                                    for slot in shared.borrow().results.iter().flatten() {
                                        values.push(slot.clone());
                                    }
                                }
                                crate::value::PromiseReactionKind::FinallyCall { cb } => {
                                    values.push(cb.clone());
                                }
                                crate::value::PromiseReactionKind::Any { holder, .. } => {
                                    values.push(holder.clone());
                                }
                                crate::value::PromiseReactionKind::Custom {
                                    promise,
                                    resolve,
                                    reject,
                                } => {
                                    values.push(promise.clone());
                                    values.push(resolve.clone());
                                    values.push(reject.clone());
                                }
                                crate::value::PromiseReactionKind::FinallyPropagate { original } => {
                                    mark_settlement(original, values);
                                }
                                _ => {}
                            }
                        }
                    }
                    crate::value::PromiseState::Fulfilled(v) => values.push(v.clone()),
                    crate::value::PromiseState::Rejected(e) => values.push(e.clone()),
                }
            }
        }
        _ => {}
    }
}

/// Push a [`Settlement`](crate::error::Settlement)'s value for marking.
fn mark_settlement(
    s: &crate::error::Settlement,
    values: &mut Vec<Value>,
) {
    match s {
        crate::error::Settlement::Fulfilled(v) => values.push(v.clone()),
        crate::error::Settlement::Rejected(e) => values.push(e.clone()),
    }
}

/// Mark a live environment and its lexical scope chain.
fn mark_env(e: &Rc<RefCell<Env>>, marked: &mut HashSet<*const ()>, values: &mut Vec<Value>, envs: &mut Vec<Rc<RefCell<Env>>>) {
    if marked.insert(Rc::as_ptr(e) as *const ()) {
        let g = e.borrow();
        for v in g.vars.values() {
            values.push(v.clone());
        }
        if let Some(o) = &g.outer {
            envs.push(o.clone());
        }
    }
}
