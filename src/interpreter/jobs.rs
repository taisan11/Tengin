//! Single-threaded spec-Job queue backing `Promise` reactions, `await`
//! continuations, thenable assimilation and `queueMicrotask`.
//!
//! The engine stays fully synchronous from the host's point of view:
//! [`Engine::eval`] runs the program and then drains pending jobs to
//! completion before returning, so `.then` callbacks and `async` continuations
//! are observable without any OS threads or external executors.

use alloc::rc::Rc;
use alloc::vec::Vec;
use core::cell::RefCell;
use core::cmp::Ordering;

use crate::error::{Error, Result, Settlement, SuspendCont};
use crate::value::{
    AllSettledState, AllState, Object, PromiseData, PromiseReaction, PromiseReactionKind,
    PromiseState, Property, SymbolData, Value,
};

use super::Engine;

/// A pending spec job.
#[derive(Clone)]
pub(crate) enum Job {
    /// A `.then` reaction (or engine-internal reaction) on a settled promise.
    Reaction {
        reaction: PromiseReaction,
        settlement: Settlement,
    },
    /// Resume a suspended `await` with the awaited promise's settlement,
    /// settling `capability` with the continuation's outcome.
    AsyncResume {
        cont: SuspendCont,
        settlement: Settlement,
        capability: Rc<RefCell<PromiseData>>,
    },
    /// Call a thenable's `then(resolve, reject)` to assimilate it.
    Thenable {
        then: Value,
        thenable: Value,
        capability: Rc<RefCell<PromiseData>>,
    },
    /// A plain `queueMicrotask` callback.
    Microtask { callback: Value },
}

/// Bound on timer tasks per top-level `eval` pump (guards `setInterval`-style
/// infinite loops while staying generous for legitimate chains).
pub(crate) const MAX_TIMER_TASKS: usize = 1000;

/// A pending timer (macrotask) on the virtual clock.
#[derive(Clone)]
pub(crate) struct TimerEntry {
    pub id: u64,
    pub deadline: u64,
    pub callback: Value,
    pub args: Vec<Value>,
    /// Repeat interval in ms (`setInterval`; `None` for one-shot).
    pub repeat: Option<u64>,
}

// `BinaryHeap` is a max-heap; reverse the ordering so the earliest deadline
// (and then the smallest id for FIFO ties) is popped first.
impl PartialEq for TimerEntry {
    fn eq(&self, other: &Self) -> bool {
        self.deadline == other.deadline && self.id == other.id
    }
}
impl Eq for TimerEntry {}
impl PartialOrd for TimerEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for TimerEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .deadline
            .cmp(&self.deadline)
            .then_with(|| other.id.cmp(&self.id))
    }
}

/// A `NewPromiseCapability` triple: the constructed promise plus the
/// `resolve`/`reject` functions its executor received.
#[derive(Clone)]
pub(crate) struct PromiseCapability {
    pub promise: Value,
    pub resolve: Value,
    pub reject: Value,
}

/// Shared `[[AlreadyResolved]]` flag holder for a resolver pair.
fn flag_holder() -> Value {
    let o = crate::value::Object::new();
    let rc = Rc::new(RefCell::new(o));
    rc.borrow_mut()
        .props
        .insert(Rc::from("resolved"), Property::new(Value::Boolean(false)));
    Value::Object(rc)
}

fn flag_is_set(holder: &Value) -> bool {
    if let Value::Object(o) = holder {
        if let Some(p) = o.borrow().props.get("resolved") {
            return p.value.to_boolean();
        }
    }
    false
}

fn flag_set(holder: &Value) {
    if let Value::Object(o) = holder {
        o.borrow_mut().props.insert(
            Rc::from("resolved"),
            Property::new(Value::Boolean(true)),
        );
    }
}

impl Engine {
    /// Allocate a fresh pending promise, registered for GC.
    pub(crate) fn new_promise(&self) -> Rc<RefCell<PromiseData>> {
        self.new_promise_with_proto(None)
    }

    /// Allocate a fresh pending promise with an explicit `[[Prototype]]`
    /// override (`None` = `%Promise.prototype%`).
    pub(crate) fn new_promise_with_proto(
        &self,
        proto: Option<Rc<RefCell<crate::value::Object>>>,
    ) -> Rc<RefCell<PromiseData>> {
        let mut data = PromiseData::new_pending();
        data.proto = proto;
        let p = Rc::new(RefCell::new(data));
        let mut gc = self.gc.borrow_mut();
        gc.promises.push(Rc::downgrade(&p));
        gc.register(Rc::as_ptr(&p) as *const ());
        self.gc_pressure.set(self.gc_pressure.get() + 1);
        p
    }

    /// Allocate a fresh pending promise as a `Value`.
    #[allow(dead_code)]
    pub(crate) fn new_promise_value(&self) -> Value {
        Value::Promise(self.new_promise())
    }

    /// Create the `(resolve, reject)` function pair closing over `capability`.
    /// Implemented via call-site interception (see `call_function`): the
    /// functions carry `__promise_cap__` / `__promise_kind__` /
    /// `__promise_flag__` props read back when they are invoked.
    pub(crate) fn make_resolver_pair(
        &self,
        capability: &Rc<RefCell<PromiseData>>,
    ) -> (Value, Value) {
        let flag = flag_holder();
        let cap = Value::Promise(capability.clone());
        let mk = |kind: &str| {
            let mut nf = crate::value::NativeFunctionData::new(promise_resolver_fn);
            nf.props
                .insert(Rc::from("__promise_cap__"), Property::new(cap.clone()));
            nf.props.insert(
                Rc::from("__promise_kind__"),
                Property::new(Value::String(Rc::from(kind))),
            );
            nf.props.insert(
                Rc::from("__promise_flag__"),
                Property::new(flag.clone()),
            );
            nf.props.insert(
                Rc::from("name"),
                Property::config(Value::String(Rc::from(kind))),
            );
            nf.props
                .insert(Rc::from("length"), Property::config(Value::Number(1.0)));
            Value::NativeFunction(Rc::new(RefCell::new(nf)))
        };
        (mk("resolve"), mk("reject"))
    }

    /// Handle a call to a resolver function created by `make_resolver_pair`.
    pub(crate) fn call_promise_resolver(&self, func: &Value, args: &[Value]) -> Result<Value> {
        let (cap, kind, flag) = match func {
            Value::NativeFunction(nf) => {
                let b = nf.borrow();
                let cap = b.props.get("__promise_cap__").map(|p| p.value.clone());
                let kind = b.props.get("__promise_kind__").map(|p| p.value.clone());
                let flag = b.props.get("__promise_flag__").map(|p| p.value.clone());
                (cap, kind, flag)
            }
            _ => (None, None, None),
        };
        let (Some(Value::Promise(cap)), Some(Value::String(kind)), Some(flag)) = (cap, kind, flag)
        else {
            return Err(Error::Runtime(
                self.make_type_error("promise resolver invoked incorrectly"),
            ));
        };
        if flag_is_set(&flag) {
            return Ok(Value::Undefined);
        }
        flag_set(&flag);
        let arg = args.first().cloned().unwrap_or(Value::Undefined);
        if kind.as_ref() == "resolve" {
            self.resolve_value(&cap, arg);
        } else {
            self.reject_promise(&cap, arg);
        }
        Ok(Value::Undefined)
    }

    fn is_callable(v: &Value) -> bool {
        matches!(v, Value::Function(_) | Value::NativeFunction(_))
    }

    /// Read a thenable's `then` without swallowing getter throws:
    /// `Ok(None)` = no callable `then` (treat as plain value),
    /// `Ok(Some(then))` = callable, `Err(reason)` = getter threw.
    fn read_then(&self, x: &Value) -> core::result::Result<Option<Value>, Value> {
        match self.get_raw_property(x, "then") {
            None => Ok(None),
            Some(p) if p.is_accessor() => match &p.get {
                Some(g) => match self.call_function(g, x, &[]) {
                    Ok(then) => Ok(if Self::is_callable(&then) {
                        Some(then)
                    } else {
                        None
                    }),
                    Err(Error::Runtime(e)) => Err(e),
                    Err(_) => Err(self.make_type_error("then getter failed")),
                },
                None => Ok(None),
            },
            Some(p) => Ok(if Self::is_callable(&p.value) {
                Some(p.value.clone())
            } else {
                None
            }),
        }
    }

    /// Read an own-or-inherited property without invoking accessors, then
    /// invoke a getter if present. Returns `Ok(None)` when absent or
    /// getter-less; getter throws propagate.
    pub(crate) fn read_property_callable(
        &self,
        base: &Value,
        key: &str,
    ) -> Result<Option<Value>> {
        match self.get_raw_property(base, key) {
            None => Ok(None),
            Some(p) if p.is_accessor() => match &p.get {
                Some(g) => Ok(Some(self.call_function(g, base, &[])?)),
                None => Ok(None),
            },
            Some(p) => Ok(Some(p.value.clone())),
        }
    }

    /// `GetPrototypeFromConstructor`-ish for promises: `newTarget.prototype`
    /// when it is an object, else `%Promise.prototype%`. A throwing
    /// `prototype` getter propagates.
    pub(crate) fn promise_proto_from_new_target(
        &self,
        nt: &Value,
    ) -> Result<Rc<RefCell<Object>>> {
        match self.read_property_callable(nt, "prototype")? {
            Some(Value::Object(o)) => Ok(o),
            _ => Ok(self.promise_prototype.clone()),
        }
    }

    /// Prototype for instances created by a native constructor:
    /// `newTarget.prototype` when constructing (`new`, `Reflect.construct`,
    /// `super()`) with a custom new-target, else `default`. Plain calls
    /// always use `default`, so an enclosing outer construction cannot leak
    /// its new-target into them.
    pub(crate) fn instance_proto(
        &self,
        default: Rc<RefCell<Object>>,
        construct: bool,
    ) -> Result<Rc<RefCell<Object>>> {
        if !construct {
            return Ok(default);
        }
        match self.active_new_target.borrow().clone() {
            Some(nt) => match self.read_property_callable(&nt, "prototype")? {
                Some(Value::Object(o)) => Ok(o),
                _ => Ok(default),
            },
            None => Ok(default),
        }
    }

    /// `SpeciesConstructor(O, defaultConstructor)`: read `O.constructor`,
    /// then `constructor[Symbol.species`, falling back to `default` for
    /// absent/`undefined`/`null`. A non-constructor species is a `TypeError`;
    /// throwing getters propagate.
    pub(crate) fn species_constructor(
        &self,
        obj: &Value,
        default: &Value,
    ) -> Result<Value> {
        let ctor = match self.read_property_callable(obj, "constructor")? {
            None | Some(Value::Undefined) => return Ok(default.clone()),
            Some(c) => c,
        };
        let species_key = SymbolData::well_known_key("species");
        let sp = match self.read_property_callable(&ctor, species_key.as_ref())? {
            None | Some(Value::Undefined) | Some(Value::Null) => {
                return Ok(default.clone())
            }
            Some(s) => s,
        };
        if !self.is_constructor(&sp) {
            return Err(Error::Runtime(
                self.make_type_error("species is not a constructor"),
            ));
        }
        Ok(sp)
    }

    /// `NewPromiseCapability(C)`: `promise = new C(executor)`, recording the
    /// `resolve`/`reject` functions the executor receives. `C` must be a
    /// constructor; a constructor that never calls its executor (or throws)
    /// fails the capability.
    pub(crate) fn promise_capability_new(
        &self,
        ctor: &Value,
    ) -> Result<PromiseCapability> {
        if !self.is_constructor(ctor) {
            return Err(Error::Runtime(
                self.make_type_error("promise capability constructor is not a constructor"),
            ));
        }
        let id = self.next_cap_id.get();
        self.next_cap_id.set(id.wrapping_add(1));
        let record = Rc::new(RefCell::new(None));
        self.cap_records.borrow_mut().insert(id, record.clone());
        let executor = self.make_cap_executor(id);
        let result = self.construct_with_new_target(ctor, &[executor], ctor);
        // The record is single-use: drop it on every path.
        let captured = self
            .cap_records
            .borrow_mut()
            .remove(&id)
            .and_then(|r| r.borrow().clone());
        let promise = result?;
        match captured {
            Some((resolve, reject)) => Ok(PromiseCapability {
                promise,
                resolve,
                reject,
            }),
            None => Err(Error::Runtime(
                self.make_type_error("constructor did not call its executor"),
            )),
        }
    }

    /// The `executor` passed to a species-constructed `new C(executor)`:
    /// records the resolve/reject pair for `promise_capability_new`.
    fn make_cap_executor(&self, id: u64) -> Value {
        let mut nf = crate::value::NativeFunctionData::new(cap_executor_fn);
        nf.props.insert(
            Rc::from("__cap_exec__"),
            Property::new(Value::Number(id as f64)),
        );
        nf.props.insert(
            Rc::from("name"),
            Property::config(Value::String(Rc::from(""))),
        );
        nf.props
            .insert(Rc::from("length"), Property::config(Value::Number(2.0)));
        Value::NativeFunction(Rc::new(RefCell::new(nf)))
    }

    /// Interception for capability executors (see `make_cap_executor`).
    /// Returns `Some` when `func` carries the marker.
    pub(crate) fn try_call_cap_executor(
        &self,
        func: &Value,
        args: &[Value],
    ) -> Option<Result<Value>> {
        let id = match func {
            Value::NativeFunction(nf) => match nf.borrow().props.get("__cap_exec__") {
                Some(p) => match &p.value {
                    Value::Number(n) => *n as u64,
                    _ => return None,
                },
                None => return None,
            },
            _ => return None,
        };
        if let Some(rec) = self.cap_records.borrow().get(&id) {
            let resolve = args.first().cloned().unwrap_or(Value::Undefined);
            let reject = args.get(1).cloned().unwrap_or(Value::Undefined);
            *rec.borrow_mut() = Some((resolve, reject));
        }
        Some(Ok(Value::Undefined))
    }

    /// Forward a raw internal promise's settlement into a custom-constructor
    /// capability by calling its recorded `resolve`/`reject`.
    pub(crate) fn forward_raw_to_custom(
        &self,
        raw: &Rc<RefCell<PromiseData>>,
        cap: &PromiseCapability,
    ) {
        let dummy = self.new_promise();
        self.attach_reaction(
            &Value::Promise(raw.clone()),
            PromiseReaction {
                kind: PromiseReactionKind::Custom {
                    promise: cap.promise.clone(),
                    resolve: cap.resolve.clone(),
                    reject: cap.reject.clone(),
                },
                on_fulfilled: None,
                on_rejected: None,
                promise: dummy,
            },
        );
    }

    /// `FulfillPromise`: settle a pending promise as fulfilled and enqueue its
    /// reactions. No-op if already settled.
    pub(crate) fn fulfill_promise(&self, cap: &Rc<RefCell<PromiseData>>, value: Value) {
        let reactions = {
            let mut b = cap.borrow_mut();
            match &mut b.state {
                PromiseState::Pending { reactions } => {
                    let rs = core::mem::take(reactions);
                    b.state = PromiseState::Fulfilled(value.clone());
                    rs
                }
                _ => return,
            }
        };
        for r in reactions {
            self.enqueue_reaction_job(r, Settlement::Fulfilled(value.clone()));
        }
    }

    /// `RejectPromise`: settle as rejected, tracking unhandled rejections when
    /// nobody is listening yet.
    pub(crate) fn reject_promise(&self, cap: &Rc<RefCell<PromiseData>>, reason: Value) {
        let reactions = {
            let mut b = cap.borrow_mut();
            match &mut b.state {
                PromiseState::Pending { reactions } => {
                    let rs = core::mem::take(reactions);
                    b.state = PromiseState::Rejected(reason.clone());
                    rs
                }
                _ => return,
            }
        };
        if reactions.is_empty() {
            self.unhandled
                .borrow_mut()
                .push(Value::Promise(cap.clone()));
        }
        for r in reactions {
            self.enqueue_reaction_job(r, Settlement::Rejected(reason.clone()));
        }
    }

    /// `Promise Resolve Functions` ([[Resolve]](x)): self-resolution rejects,
    /// promises are adopted, thenables are assimilated via a job, plain values
    /// fulfill.
    pub(crate) fn resolve_value(&self, cap: &Rc<RefCell<PromiseData>>, x: Value) {
        if let Value::Promise(other) = &x {
            if Rc::ptr_eq(other, cap) {
                let err = self.make_type_error("Promise self-resolution");
                self.reject_promise(cap, err);
                return;
            }
        }
        match x {
            Value::Promise(other) => {
                let state = {
                    let b = other.borrow();
                    match &b.state {
                        PromiseState::Pending { .. } => None,
                        PromiseState::Fulfilled(v) => Some(Ok(v.clone())),
                        PromiseState::Rejected(e) => Some(Err(e.clone())),
                    }
                };
                match state {
                    None => {
                        self.mark_handled(&Value::Promise(other.clone()));
                        other.borrow_mut().pending_push(PromiseReaction {
                            kind: PromiseReactionKind::Forward,
                            on_fulfilled: None,
                            on_rejected: None,
                            promise: cap.clone(),
                        });
                    }
                    Some(Ok(v)) => self.fulfill_promise(cap, v),
                    Some(Err(e)) => self.reject_promise(cap, e),
                }
            }
            _ if Self::is_thenable_value(&x) => {
                match self.read_then(&x) {
                    Err(reason) => self.reject_promise(cap, reason),
                    Ok(None) => self.fulfill_promise(cap, x),
                    Ok(Some(then)) => {
                        self.job_queue.borrow_mut().push_back(Job::Thenable {
                            then,
                            thenable: x,
                            capability: cap.clone(),
                        });
                    }
                }
            }
            _ => self.fulfill_promise(cap, x),
        }
    }

    fn is_thenable_value(v: &Value) -> bool {
        matches!(
            v,
            Value::Object(_)
                | Value::Array(_)
                | Value::Function(_)
                | Value::NativeFunction(_)
                | Value::Promise(_)
                | Value::Map(_)
                | Value::Set(_)
        )
    }

    /// Remove `p` from the unhandled-rejection list (a handler was attached).
    pub(crate) fn mark_handled(&self, p: &Value) {
        if let Value::Promise(target) = p {
            let mut u = self.unhandled.borrow_mut();
            u.retain(|v| match v {
                Value::Promise(o) => !Rc::ptr_eq(o, target),
                _ => true,
            });
        }
    }

    /// `PerformPromiseThen`: attach a reaction, or enqueue it immediately when
    /// already settled. Returns the dependent promise value.
    pub(crate) fn perform_then(
        &self,
        promise: &Value,
        on_fulfilled: Option<Value>,
        on_rejected: Option<Value>,
    ) -> Value {
        let cap = self.new_promise();
        let reaction = PromiseReaction {
            kind: PromiseReactionKind::Then,
            on_fulfilled,
            on_rejected,
            promise: cap.clone(),
        };
        self.attach_reaction(promise, reaction);
        Value::Promise(cap)
    }

    /// Attach a low-level reaction to `promise` (used by `await` and
    /// `Promise.all` friends as well as ordinary `then`).
    pub(crate) fn attach_reaction(&self, promise: &Value, reaction: PromiseReaction) {
        let p: Rc<RefCell<PromiseData>> = match promise {
            Value::Promise(p) => p.clone(),
            other => {
                let cap = self.new_promise();
                self.fulfill_promise(&cap, other.clone());
                cap
            }
        };
        self.mark_handled(&Value::Promise(p.clone()));
        let state = {
            let b = p.borrow();
            match &b.state {
                PromiseState::Pending { .. } => None,
                PromiseState::Fulfilled(v) => Some(Ok(v.clone())),
                PromiseState::Rejected(e) => Some(Err(e.clone())),
            }
        };
        match state {
            None => {
                p.borrow_mut().pending_push(reaction);
            }
            Some(Ok(v)) => {
                self.enqueue_reaction_job(reaction, Settlement::Fulfilled(v));
            }
            Some(Err(e)) => {
                self.enqueue_reaction_job(reaction, Settlement::Rejected(e));
            }
        }
    }

    /// Suspend the current `async` execution on `awaited`, resuming via `cont`
    /// and settling `capability` with the outcome.
    pub(crate) fn await_suspend(
        &self,
        awaited: Value,
        cont: SuspendCont,
        capability: &Rc<RefCell<PromiseData>>,
    ) {
        // `Await(v)` is `PerformPromiseThen(PromiseResolve(%Promise%, v))`.
        let target: Rc<RefCell<PromiseData>> = match &awaited {
            Value::Promise(p) => p.clone(),
            thenable if Self::is_thenable_value(thenable) => {
                let cap = self.new_promise();
                match self.read_then(thenable) {
                    Err(reason) => self.reject_promise(&cap, reason),
                    Ok(None) => self.fulfill_promise(&cap, thenable.clone()),
                    Ok(Some(then)) => {
                        self.job_queue.borrow_mut().push_back(Job::Thenable {
                            then,
                            thenable: thenable.clone(),
                            capability: cap.clone(),
                        });
                    }
                }
                cap
            }
            other => {
                let cap = self.new_promise();
                self.fulfill_promise(&cap, other.clone());
                cap
            }
        };
        let state = target.borrow().state.clone();
        match state {
            PromiseState::Pending { .. } => {
                self.mark_handled(&Value::Promise(target.clone()));
                target.borrow_mut().pending_push(PromiseReaction {
                    kind: PromiseReactionKind::Async { cont: cont.clone() },
                    on_fulfilled: None,
                    on_rejected: None,
                    promise: capability.clone(),
                });
            }
            PromiseState::Fulfilled(v) => {
                self.job_queue.borrow_mut().push_back(Job::AsyncResume {
                    cont,
                    settlement: Settlement::Fulfilled(v),
                    capability: capability.clone(),
                });
            }
            PromiseState::Rejected(e) => {
                self.job_queue.borrow_mut().push_back(Job::AsyncResume {
                    cont,
                    settlement: Settlement::Rejected(e),
                    capability: capability.clone(),
                });
            }
        }
    }

    fn enqueue_reaction_job(&self, reaction: PromiseReaction, settlement: Settlement) {
        match &reaction.kind {
            PromiseReactionKind::Then
            | PromiseReactionKind::FinallyCall { .. }
            | PromiseReactionKind::FinallyPropagate { .. }
            | PromiseReactionKind::Custom { .. } => {
                self.job_queue.borrow_mut().push_back(Job::Reaction {
                    reaction,
                    settlement,
                });
            }
            PromiseReactionKind::Forward => {
                let target = reaction.promise.clone();
                match settlement {
                    Settlement::Fulfilled(v) => self.resolve_value(&target, v),
                    Settlement::Rejected(e) => self.reject_promise(&target, e),
                }
            }
            PromiseReactionKind::Async { cont } => {
                self.job_queue.borrow_mut().push_back(Job::AsyncResume {
                    cont: cont.clone(),
                    settlement,
                    capability: reaction.promise.clone(),
                });
            }
            PromiseReactionKind::AllElement { index, shared } => {
                let index = *index;
                let shared = shared.clone();
                let target = reaction.promise.clone();
                match settlement {
                    Settlement::Fulfilled(v) => {
                        let mut s = shared.borrow_mut();
                        if s.settled {
                            return;
                        }
                        if let Some(slot) = s.results.get_mut(index) {
                            *slot = Some(v);
                        }
                        s.remaining = s.remaining.saturating_sub(1);
                        if s.remaining == 0 {
                            s.settled = true;
                            let vals: Vec<Value> = s
                                .results
                                .iter()
                                .cloned()
                                .map(|o| o.unwrap_or(Value::Undefined))
                                .collect();
                            drop(s);
                            let arr = Value::Array(self.new_array(vals));
                            self.fulfill_promise(&target, arr);
                        }
                    }
                    Settlement::Rejected(e) => {
                        let mut s = shared.borrow_mut();
                        if s.settled {
                            return;
                        }
                        s.settled = true;
                        drop(s);
                        self.reject_promise(&target, e);
                    }
                }
            }
            PromiseReactionKind::Any { holder, index } => {
                let index = *index;
                let holder = holder.clone();
                let target = reaction.promise.clone();
                match settlement {
                    Settlement::Fulfilled(v) => {
                        // First fulfillment wins (`fulfill` is a no-op after).
                        self.fulfill_promise(&target, v);
                    }
                    Settlement::Rejected(reason) => {
                        // Ignore when the aggregate already settled.
                        if !matches!(
                            target.borrow().state,
                            PromiseState::Pending { .. }
                        ) {
                            return;
                        }
                        if let Value::Object(h) = &holder {
                            let mut hb = h.borrow_mut();
                            if let Some(Value::Array(errs)) =
                                hb.props.get("errors").map(|p| p.value.clone())
                            {
                                if let Some(slot) = errs.borrow_mut().elems.get_mut(index) {
                                    *slot = reason;
                                }
                            }
                            let left = match hb.props.get("remaining").map(|p| p.value.clone()) {
                                Some(Value::Number(n)) => {
                                    let left = (n as usize).saturating_sub(1);
                                    hb.props.insert(
                                        Rc::from("remaining"),
                                        Property::new(Value::Number(left as f64)),
                                    );
                                    left
                                }
                                _ => 1,
                            };
                            if left == 0 {
                                let errs: Vec<Value> = match hb
                                    .props
                                    .get("errors")
                                    .map(|p| p.value.clone())
                                {
                                    Some(Value::Array(a)) => a.borrow().elems.clone(),
                                    _ => Vec::new(),
                                };
                                drop(hb);
                                self.reject_promise(
                                    &target,
                                    self.make_aggregate_error(
                                        errs,
                                        "All promises were rejected",
                                    ),
                                );
                            }
                        }
                    }
                }
            }
            PromiseReactionKind::AllSettled { index, shared } => {
                let index = *index;
                let shared = shared.clone();
                let target = reaction.promise.clone();
                let outcome_obj = self.new_object();
                match settlement {
                    Settlement::Fulfilled(v) => {
                        outcome_obj.borrow_mut().props.insert(
                            Rc::from("status"),
                            Property::new(Value::String(Rc::from("fulfilled"))),
                        );
                        outcome_obj
                            .borrow_mut()
                            .props
                            .insert(Rc::from("value"), Property::new(v));
                    }
                    Settlement::Rejected(e) => {
                        outcome_obj.borrow_mut().props.insert(
                            Rc::from("status"),
                            Property::new(Value::String(Rc::from("rejected"))),
                        );
                        outcome_obj
                            .borrow_mut()
                            .props
                            .insert(Rc::from("reason"), Property::new(e));
                    }
                }
                let mut s = shared.borrow_mut();
                if let Some(slot) = s.results.get_mut(index) {
                    *slot = Some(Value::Object(outcome_obj));
                }
                s.remaining = s.remaining.saturating_sub(1);
                if s.remaining == 0 {
                    let vals: Vec<Value> = s
                        .results
                        .iter()
                        .cloned()
                        .map(|o| o.unwrap_or(Value::Undefined))
                        .collect();
                    drop(s);
                    let arr = Value::Array(self.new_array(vals));
                    self.fulfill_promise(&target, arr);
                }
            }
        }
    }

    /// Drain pending jobs. Re-entrant (jobs may enqueue more jobs). A generous
    /// iteration cap guards against infinite `then` ping-pong; pending-forever
    /// promises simply yield no further jobs and terminate.
    pub(crate) fn drain_jobs(&self) {
        let _guard = DrainGuard::enter(self);
        let mut steps = 0usize;
        loop {
            let job = self.job_queue.borrow_mut().pop_front();
            let Some(job) = job else { break };
            steps += 1;
            if steps > 10_000_000 {
                break;
            }
            self.run_job(job);
        }
    }

    fn run_job(&self, job: Job) {
        match job {
            Job::Reaction {
                reaction,
                settlement,
            } => match reaction.kind.clone() {
                PromiseReactionKind::Then => {
                    let handler = match &settlement {
                        Settlement::Fulfilled(_) => reaction.on_fulfilled.clone(),
                        Settlement::Rejected(_) => reaction.on_rejected.clone(),
                    };
                    let capability = reaction.promise.clone();
                    match handler {
                        None => match settlement {
                            Settlement::Fulfilled(v) => self.resolve_value(&capability, v),
                            Settlement::Rejected(e) => self.reject_promise(&capability, e),
                        },
                        Some(f) => {
                            let arg = match &settlement {
                                Settlement::Fulfilled(v) => v.clone(),
                                Settlement::Rejected(e) => e.clone(),
                            };
                            match self.call_function(&f, &Value::Undefined, &[arg]) {
                                Ok(r) => self.resolve_value(&capability, r),
                                Err(Error::Runtime(e)) => self.reject_promise(&capability, e),
                                Err(Error::Unimplemented(m)) => self.reject_promise(
                                    &capability,
                                    self.make_type_error(&format!(
                                        "async is not implemented: {m}"
                                    )),
                                ),
                                Err(_) => self.reject_promise(
                                    &capability,
                                    self.make_type_error("promise handler failed"),
                                ),
                            }
                        }
                    }
                }
                PromiseReactionKind::FinallyCall { cb } => {
                    let capability = reaction.promise.clone();
                    let original = settlement.clone();
                    match self.call_function(&cb, &Value::Undefined, &[]) {
                        Err(Error::Runtime(e)) => self.reject_promise(&capability, e),
                        Err(_) => self.reject_promise(
                            &capability,
                            self.make_type_error("finally callback failed"),
                        ),
                        Ok(v) => {
                            let mid = self.new_promise();
                            self.resolve_value(&mid, v);
                            let follow = PromiseReaction {
                                kind: PromiseReactionKind::FinallyPropagate { original },
                                on_fulfilled: None,
                                on_rejected: None,
                                promise: capability,
                            };
                            let state = mid.borrow().state.clone();
                            match state {
                                PromiseState::Pending { .. } => {
                                    mid.borrow_mut().pending_push(follow);
                                }
                                PromiseState::Fulfilled(x) => self.enqueue_reaction_job(
                                    follow,
                                    Settlement::Fulfilled(x),
                                ),
                                PromiseState::Rejected(e) => self.enqueue_reaction_job(
                                    follow,
                                    Settlement::Rejected(e),
                                ),
                            }
                        }
                    }
                }
                PromiseReactionKind::FinallyPropagate { original } => {
                    let capability = reaction.promise.clone();
                    match settlement {
                        Settlement::Fulfilled(_) => match original {
                            Settlement::Fulfilled(v) => self.resolve_value(&capability, v),
                            Settlement::Rejected(e) => self.reject_promise(&capability, e),
                        },
                        Settlement::Rejected(e) => self.reject_promise(&capability, e),
                    }
                }
                PromiseReactionKind::Custom {
                    resolve, reject, ..
                } => {
                    // Like `Then`, but settling a custom-constructor
                    // capability through its own functions: without a handler
                    // the settlement passes through; a handler's return value
                    // resolves, a handler throw rejects. A throw in either
                    // capability call escapes the job as unhandled.
                    let (handler, arg) = match &settlement {
                        Settlement::Fulfilled(v) => {
                            (reaction.on_fulfilled.clone(), v.clone())
                        }
                        Settlement::Rejected(e) => {
                            (reaction.on_rejected.clone(), e.clone())
                        }
                    };
                    match handler {
                        None => {
                            let (f, a) = match &settlement {
                                Settlement::Fulfilled(v) => (resolve.clone(), v.clone()),
                                Settlement::Rejected(e) => (reject.clone(), e.clone()),
                            };
                            if let Err(e) = self.call_function(&f, &Value::Undefined, &[a]) {
                                self.push_job_error(e);
                            }
                        }
                        Some(f) => {
                            match self.call_function(&f, &Value::Undefined, &[arg]) {
                                Ok(r) => {
                                    if let Err(e) = self.call_function(
                                        &resolve,
                                        &Value::Undefined,
                                        &[r],
                                    ) {
                                        self.push_job_error(e);
                                    }
                                }
                                Err(Error::Runtime(thrown)) => {
                                    if let Err(e) = self.call_function(
                                        &reject,
                                        &Value::Undefined,
                                        &[thrown],
                                    ) {
                                        self.push_job_error(e);
                                    }
                                }
                                Err(other) => {
                                    self.push_job_error(other);
                                }
                            }
                        }
                    }
                }
                _ => {}
            },
            Job::AsyncResume {
                cont,
                settlement,
                capability,
            } => {
                // Continuations resume inside `async` function bodies: restore
                // the lexical async context (`await` validity) for the resume.
                // Nested sync calls push/pop symmetrically on top of this.
                self.async_depth.set(self.async_depth.get() + 1);
                self.func_async_stack.borrow_mut().push(true);
                let outcome = cont(self, settlement);
                self.func_async_stack.borrow_mut().pop();
                self.async_depth.set(self.async_depth.get().saturating_sub(1));
                self.settle_cont_outcome(outcome, &capability);
            }
            Job::Thenable {
                then,
                thenable,
                capability,
            } => {
                let (res, rej) = self.make_resolver_pair(&capability);
                match self.call_function(&then, &thenable, &[res, rej]) {
                    Ok(_) => {}
                    Err(Error::Runtime(e)) => {
                        self.try_reject_thenable(&capability, e);
                    }
                    Err(_) => self.try_reject_thenable(
                        &capability,
                        self.make_type_error("thenable failed"),
                    ),
                }
            }
            Job::Microtask { callback } => {
                match self.call_function(&callback, &Value::Undefined, &[]) {
                    Ok(_) => {}
                    Err(Error::Runtime(e)) => {
                        self.unhandled.borrow_mut().push(e);
                    }
                    Err(Error::Unimplemented(m)) => self
                        .unhandled
                        .borrow_mut()
                        .push(Value::String(Rc::from(m.as_str()))),
                    Err(_) => {}
                }
            }
        }
    }

    /// Schedule a timer on the virtual clock. Returns the numeric timer id.
    /// `delay_ms` is clamped at zero; `repeat_ms` (`Some` for `setInterval`)
    /// reschedules after every firing.
    pub(crate) fn set_timer(
        &self,
        callback: Value,
        args: Vec<Value>,
        delay_ms: u64,
        repeat_ms: Option<u64>,
    ) -> u64 {
        let id = self.next_timer_id.get();
        self.next_timer_id.set(id.wrapping_add(1).max(1));
        let deadline = self.now_ms.get().saturating_add(delay_ms);
        self.timers.borrow_mut().push(TimerEntry {
            id,
            deadline,
            callback,
            args,
            repeat: repeat_ms,
        });
        id
    }

    /// Cancel a timer by id (shared `setTimeout`/`setInterval` namespace).
    /// Returns whether anything was removed.
    pub(crate) fn clear_timer(&self, id: u64) -> bool {
        // Entries are removed lazily from the heap. Remembering the id avoids
        // an O(n) retain and also handles cancellation from an in-flight
        // callback.
        let removed = self.timers.borrow().iter().any(|t| t.id == id);
        // Always remember the cancel: the target may be in flight right now
        // (already popped for firing), in which case `retain` finds nothing
        // but the repeat must still not reschedule. Pruned by the pump when
        // idle (never here: the queue may look empty mid-flight).
        {
            let mut list = self.cancelled_timers.borrow_mut();
            list.insert(id);
        }
        removed
    }

    /// Pump due timers on the virtual clock: advance to the next deadline,
    /// run everything due there (with a microtask checkpoint after each
    /// callback, per HTML), and repeat. Stops when no timers remain or
    /// `limit` tasks have run (guarding `setInterval(..., 0)`-style infinite
    /// loops). Returns tasks run.
    pub(crate) fn drain_timers(&self, limit: usize) -> usize {
        let _guard = DrainGuard::enter(self);
        let mut ran = 0usize;
        while ran < limit {
            // Earliest deadline (ties broken by smallest id = FIFO).
            let Some(timer) = self.timers.borrow_mut().pop() else {
                self.cancelled_timers.borrow_mut().clear();
                break;
            };
            if self.cancelled_timers.borrow_mut().remove(&timer.id) {
                continue;
            }
            if timer.deadline > self.now_ms.get() {
                self.now_ms.set(timer.deadline);
            }
            ran += 1;
            match self.call_function(&timer.callback, &Value::Undefined, &timer.args) {
                Ok(_) => {}
                Err(Error::Runtime(e)) => {
                    self.unhandled.borrow_mut().push(e);
                }
                Err(Error::Unimplemented(m)) => self
                    .unhandled
                    .borrow_mut()
                    .push(Value::String(Rc::from(m.as_str()))),
                Err(_) => {}
            }
            // Microtask checkpoint after every timer task.
            self.drain_jobs();
            if let Some(interval) = timer.repeat {
                // Skip rescheduling when cleared from inside the callback.
                let cancelled = {
                    let mut list = self.cancelled_timers.borrow_mut();
                    let hit = list.remove(&timer.id);
                    if self.timers.borrow().is_empty() {
                        list.clear();
                    }
                    hit
                };
                if !cancelled {
                    let deadline = self.now_ms.get().saturating_add(interval);
                    self.timers.borrow_mut().push(TimerEntry {
                        id: timer.id,
                        deadline,
                        callback: timer.callback,
                        args: timer.args,
                        repeat: Some(interval),
                    });
                }
            }
        }
        ran
    }

    /// Settle an async-function promise with a resumed continuation's outcome,
    /// chaining back into `await` when the continuation suspends again.
    pub(crate) fn settle_cont_outcome(
        &self,
        outcome: Result<Option<Value>>,
        capability: &Rc<RefCell<PromiseData>>,
    ) {
        match outcome {
            Ok(opt) => self.resolve_value(capability, opt.unwrap_or(Value::Undefined)),
            Err(Error::Return(v)) => self.resolve_value(capability, v),
            Err(Error::Runtime(e)) => self.reject_promise(capability, e),
            Err(Error::Suspend { awaited, cont }) => {
                self.await_suspend(awaited, cont, capability)
            }
            Err(Error::TailCall { func, this, args }) => {
                let r = self.call_function(&func, &this, &args);
                match r {
                    Ok(v) => self.resolve_value(capability, v),
                    Err(Error::Return(v)) => self.resolve_value(capability, v),
                    Err(Error::Runtime(e)) => self.reject_promise(capability, e),
                    Err(Error::Suspend { awaited, cont }) => {
                        self.await_suspend(awaited, cont, capability)
                    }
                    Err(_) => self.reject_promise(
                        capability,
                        self.make_type_error("async tail call failed"),
                    ),
                }
            }
            Err(_) => self.reject_promise(
                capability,
                self.make_type_error("async function failed"),
            ),
        }
    }

    fn try_reject_thenable(&self, cap: &Rc<RefCell<PromiseData>>, reason: Value) {
        if matches!(cap.borrow().state, PromiseState::Pending { .. }) {
            self.reject_promise(cap, reason);
        }
    }

    /// Record a job-thrown error as unhandled (host-visible failure detail).
    fn push_job_error(&self, e: Error) {
        let detail = match e {
            Error::Runtime(v) => v,
            other => Value::String(Rc::from(alloc::format!("{other}").as_str())),
        };
        self.unhandled.borrow_mut().push(detail);
    }

    /// Take (and clear) unhandled rejection/exception values for the host to
    /// report (CLI exit code, test262 failure detail).
    pub fn take_unhandled(&self) -> Vec<Value> {
        core::mem::take(&mut *self.unhandled.borrow_mut())
    }

    /// `Promise.all` shared state constructor.
    pub(crate) fn new_all_state(&self, len: usize) -> Rc<RefCell<AllState>> {
        Rc::new(RefCell::new(AllState {
            remaining: len,
            results: (0..len).map(|_| None).collect(),
            settled: false,
        }))
    }

    /// `Promise.allSettled` shared state constructor.
    pub(crate) fn new_all_settled_state(&self, len: usize) -> Rc<RefCell<AllSettledState>> {
        Rc::new(RefCell::new(AllSettledState {
            remaining: len,
            results: (0..len).map(|_| None).collect(),
        }))
    }
}

/// Push a reaction onto a promise known to be pending.
trait PendingPush {
    fn pending_push(&mut self, r: PromiseReaction);
}

impl PendingPush for PromiseData {
    fn pending_push(&mut self, r: PromiseReaction) {
        match &mut self.state {
            PromiseState::Pending { reactions } => reactions.push(r),
            _ => {}
        }
    }
}

/// Interception target for resolver functions created by
/// [`Engine::make_resolver_pair`]; real work happens in
/// `call_promise_resolver` (see `call_function`).
fn promise_resolver_fn(
    _e: &Engine,
    _this: &Value,
    _a: &[Value],
    _c: bool,
) -> Result<Value> {
    Err(Error::Runtime(Value::String(Rc::from(
        "promise resolver invoked outside call_function",
    ))))
}

/// Interception target for capability executors created by
/// [`Engine::make_cap_executor`]; real work happens in
/// `try_call_cap_executor` (see `call_function`).
fn cap_executor_fn(
    _e: &Engine,
    _this: &Value,
    _a: &[Value],
    _c: bool,
) -> Result<Value> {
    Err(Error::Runtime(Value::String(Rc::from(
        "capability executor invoked outside call_function",
    ))))
}

/// Re-entrancy depth guard for [`Engine::drain_jobs`].
struct DrainGuard<'a> {
    engine: &'a Engine,
    prev: usize,
}

impl<'a> DrainGuard<'a> {
    fn enter(engine: &'a Engine) -> Self {
        let prev = engine.job_depth.get();
        engine.job_depth.set(prev + 1);
        DrainGuard { engine, prev }
    }
}

impl Drop for DrainGuard<'_> {
    fn drop(&mut self) {
        self.engine.job_depth.set(self.prev);
    }
}
