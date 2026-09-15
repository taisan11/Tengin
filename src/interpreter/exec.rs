//! The tree-walking evaluator: statement execution and expression evaluation.
//! All functions are [`crate::interpreter::Engine`] methods so they can share
//! the engine's allocator and lexical environments.

use alloc::rc::Rc;
use alloc::vec::Vec;
use core::cell::RefCell;

use crate::ast::*;
use crate::environment::Env;
use crate::error::{Error, Result, Settlement, SuspendCont};
use crate::value::{CtorRef, Function, Object, Property, Value};

use super::ops::{eval_binary, is_nullish, key_matches, lit_to_value, to_int32};
use crate::builtins::bigint_num::Big;
use super::Engine;
use super::object::PrimitiveHint;

/// Remainder of a suspended expression fragment: given the fragment's value,
/// compute the enclosing expression's value (may itself suspend).
type ValueNext = Rc<dyn Fn(&Engine, Value) -> Result<Value>>;
/// Remainder of suspended statement-list execution: given the suspended
/// statement's completion value, run the rest of the list.
type StmtsNext = Rc<dyn Fn(&Engine, Option<Value>) -> Result<Option<Value>>>;
/// Post-processing remainder: given the suspended fragment's *full* outcome
/// (normal or abrupt, e.g. `return`/`throw` inside `try`), compute the
/// enclosing statement's outcome. Used where abrupt completions must still be
/// observed (`try`/`catch`/`finally`, loops, labels).
type OutcomeNext = Rc<dyn Fn(&Engine, Result<Option<Value>>) -> Result<Option<Value>>>;
/// Remainder receiving evaluated instance-field keys for class construction.
type ClassKeysNext = Rc<dyn Fn(&Engine, Vec<(Rc<str>, Option<Expr>)>) -> Result<Value>>;

/// Compose a suspended fragment with its remainder: if resuming the fragment
/// suspends again, the remainder is re-attached to the new suspension.
fn chain_value(first: SuspendCont, next: ValueNext) -> SuspendCont {
    Rc::new(move |engine: &Engine, s: Settlement| {
        let opt = match first(engine, s) {
            Ok(o) => o,
            Err(Error::Suspend { awaited, cont }) => {
                return Err(Error::Suspend {
                    awaited,
                    cont: chain_value(cont, next.clone()),
                });
            }
            Err(e) => return Err(e),
        };
        next(engine, opt.unwrap_or(Value::Undefined)).map(Some)
    })
}

/// Statement-list version of [`chain_value`]: empty completions flow through.
fn chain_stmts(first: SuspendCont, next: StmtsNext) -> SuspendCont {
    Rc::new(move |engine: &Engine, s: Settlement| {
        let opt = match first(engine, s) {
            Ok(o) => o,
            Err(Error::Suspend { awaited, cont }) => {
                return Err(Error::Suspend {
                    awaited,
                    cont: chain_stmts(cont, next.clone()),
                });
            }
            Err(e) => return Err(e),
        };
        next(engine, opt)
    })
}

/// Loop iteration decision shared by all loop runners.
enum LoopStep {
    Next(Option<Value>),
    Done(Result<Option<Value>>),
}

/// One body+update step outcome: keep iterating or finish.
enum IterOut {
    Next(Option<Value>),
    Done(Result<Option<Value>>),
}

/// Post-processing version of [`chain_value`]: every resume outcome — normal
/// *and* abrupt — flows into `next` (which e.g. runs `catch`/`finally` or loop
/// bookkeeping). A re-suspension re-attaches `next`.
fn chain_outcome(first: SuspendCont, next: OutcomeNext) -> SuspendCont {
    Rc::new(move |engine: &Engine, s: Settlement| {
        match first(engine, s) {
            Err(Error::Suspend { awaited, cont }) => Err(Error::Suspend {
                awaited,
                cont: chain_outcome(cont, next.clone()),
            }),
            other => next(engine, other),
        }
    })
}

/// Remainder receiving an evaluated argument vector.
type ExprVecNext = Rc<dyn Fn(&Engine, Vec<Value>) -> Result<Value>>;

impl Engine {
    /// Read a member like `obj[key]`, throwing a `TypeError` when the base is
    /// `null`/`undefined` (the optional-chaining call sites check nullishness
    /// before reaching here).
    fn get_member(&self, base: &Value, key: &str) -> Result<Value> {
        if is_nullish(base) {
            return Err(Error::Runtime(self.make_type_error(&format!(
                "Cannot read properties of {} (reading '{}')",
                if matches!(base, Value::Null) { "null" } else { "undefined" },
                key,
            ))));
        }
        Ok(self.get_property(base, key))
    }

    // --- function values ---

    /// Build a user function value from parts (used by the `Function` constructor).
    pub(crate) fn make_function_value(
        &self,
        name: Rc<str>,
        params: Vec<Pattern>,
        body: Vec<Stmt>,
        closure: Rc<RefCell<Env>>,
    ) -> Value {
        self.make_function(name, params, body, closure, None)
    }

    fn make_function(
        &self,
        name: Rc<str>,
        params: Vec<Pattern>,
        body: Vec<Stmt>,
        closure: Rc<RefCell<Env>>,
        this_capture: Option<Value>,
    ) -> Value {
        let params_count = params.len();
        // Generator/async bodies arrive wrapped in marker statements; unwrap
        // them and flag the function (calls then return a generator object or
        // a promise respectively).
        let (generator, is_async, body) = match body.first() {
            Some(Stmt::GeneratorBody(inner)) => (true, false, inner.clone()),
            Some(Stmt::AsyncBody(inner)) => (false, true, inner.clone()),
            _ => (false, false, body),
        };
        let f = Rc::new(RefCell::new(Function {
            name: name.clone(),
            params,
            body: Rc::new(body),
            closure,
            props: crate::value::new_props(),
            proto: None,
            super_proto: None,
            super_ctor: None,
            this_capture,
            generator,
            is_async,
            // Plain functions/methods are never derived constructors; class
            // constructors set the flag directly.
            is_derived: false,
            realm: Some(self.realm.clone()),
        }));
        let proto = self.new_object();        proto
            .borrow_mut()
            .ctor = Some(CtorRef::Func(Rc::downgrade(&f)));
        // Async functions have no `prototype` property and are not constructable.
        if !is_async {
            f.borrow_mut()
                .props
                .insert(Rc::from("prototype"), Property::new(Value::Object(proto)));
        }
        f.borrow_mut()
            .props
            .insert(Rc::from("name"), Property::config(Value::String(name)));
        f.borrow_mut()
            .props
            .insert(Rc::from("length"), Property::config(Value::Number(params_count as f64)));
        // Async functions inherit from `%AsyncFunction.prototype%` (whose own
        // proto is `%Function.prototype%`); everything else uses the latter.
        let fn_proto = if is_async {
            self.async_function_prototype.clone()
        } else {
            self.function_prototype.clone()
        };
        f.borrow_mut().proto = Some(fn_proto);
        self.register_function(&f);
        Value::Function(f)
    }
    // --- statement execution ---

    fn hoist(&self, stmts: &[Stmt], env: &Rc<RefCell<Env>>) {
        for s in stmts {
            if let Stmt::FunctionDecl { name, params, body } = s {
                let f = self.make_function(
                    name.clone(),
                    params.clone(),
                    body.clone(),
                    env.clone(),
                    None,
                );
                env.borrow_mut().vars.insert(name.clone(), f);
            }
        }
        // `var` declarations are hoisted (as `undefined`) to the nearest
        // function/global scope, even when nested inside blocks.
        let mut names = Vec::new();
        collect_var_names(stmts, &mut names);
        let scope = Env::function_scope(env);
        let mut b = scope.borrow_mut();
        for name in names {
            if !b.vars.contains_key(name.as_ref()) {
                b.vars.insert(name, Value::Undefined);
            }
        }
    }
    pub(crate) fn exec_stmts(&self, stmts: &[Stmt], env: &Rc<RefCell<Env>>) -> Result<Option<Value>> {
        self.hoist(stmts, env);
        self.exec_stmts_from(stmts, 0, None, env)
    }

    /// Execute `stmts[start..]` threading the accumulated completion `last`.
    /// Suspension-aware: an `await` inside statement `i` suspends with a
    /// continuation that resumes statement `i`'s fragment and then runs the
    /// rest of the list (so `UpdateEmpty` and completion merging still apply
    /// on resume).
    fn exec_stmts_from(
        &self,
        stmts: &[Stmt],
        start: usize,
        last: Option<Value>,
        env: &Rc<RefCell<Env>>,
    ) -> Result<Option<Value>> {
        // A statement list's completion value is the value of the last
        // value-producing statement; empty completions (`var`, empty blocks,
        // function declarations, …) never replace a previous value.
        let mut last = last;
        for (i, s) in stmts.iter().enumerate().skip(start) {
            match self.eval_stmt(s, env) {
                Ok(v) => {
                    if v.is_some() {
                        last = v;
                    }
                }
                // An abrupt completion with an empty value is filled in from
                // the statement list's accumulated value (spec `UpdateEmpty`).
                Err(Error::Break(label, None)) => {
                    return Err(Error::Break(label, last))
                }
                Err(Error::Continue(label, None)) => {
                    return Err(Error::Continue(label, last))
                }
                Err(Error::Suspend { awaited, cont }) => {
                    let rest: Vec<Stmt> = stmts[i + 1..].to_vec();
                    let env2 = env.clone();
                    let last2 = last.clone();
                    let next: StmtsNext = Rc::new(move |engine, frag| {
                        let mut last = last2.clone();
                        if frag.is_some() {
                            last = frag;
                        }
                        engine.exec_stmts_from(&rest, 0, last, &env2)
                    });
                    return Err(Error::Suspend {
                        awaited,
                        cont: chain_stmts(cont, next),
                    });
                }
                Err(e) => return Err(e),
            }
        }
        Ok(last)
    }

    fn eval_stmt(&self, stmt: &Stmt, env: &Rc<RefCell<Env>>) -> Result<Option<Value>> {
        match stmt {
            // Unwrapped by the function factory; execute inline if it ever
            // reaches the evaluator directly.
            Stmt::GeneratorBody(body) => self.exec_stmts(body, env),
            Stmt::AsyncBody(body) => self.exec_stmts(body, env),
            Stmt::Var(list, kind) => {
                let is_const = matches!(kind, VarKind::Const);
                // `var` binds in the nearest function/global scope; `let`/`const`
                // bind in the current (block) scope.
                let scope = match kind {
                    VarKind::Var => Env::function_scope(env),
                    _ => env.clone(),
                };
                self.eval_var_list(list, 0, is_const, &scope, env)
            }
            Stmt::Expr(e) => match self.eval_expr(e, env) {
                Ok(v) => Ok(Some(v)),
                Err(Error::Suspend { awaited, cont }) => {
                    let next: StmtsNext = Rc::new(move |_, frag| Ok(frag));
                    Err(Error::Suspend {
                        awaited,
                        cont: chain_stmts(cont, next),
                    })
                }
                Err(e) => Err(e),
            },
            Stmt::FunctionDecl { .. } => Ok(None),
            Stmt::Return(e) => {
                let v = match e {
                    Some(e) => match self.eval_return_expr(e, env) {
                        Ok(v) => v,
                        Err(Error::Suspend { awaited, cont }) => {
                            let next: StmtsNext =
                                Rc::new(move |_, frag| Err(Error::Return(frag.unwrap_or(Value::Undefined))));
                            return Err(Error::Suspend {
                                awaited,
                                cont: chain_stmts(cont, next),
                            });
                        }
                        Err(e) => return Err(e),
                    },
                    None => Value::Undefined,
                };
                Err(Error::Return(v))
            }
            Stmt::Block(b) => {
                let scope = Rc::new(RefCell::new(Env::new_block(env.clone())));
                self.register_env(&scope);
                self.exec_stmts(b, &scope)
            }
            Stmt::Empty => Ok(None),
            Stmt::Break(label) => Err(Error::Break(label.clone(), None)),
            Stmt::Continue(label) => Err(Error::Continue(label.clone(), None)),
            Stmt::Throw(e) => {
                let v = match self.eval_expr(e, env) {
                    Ok(v) => v,
                    Err(Error::Suspend { awaited, cont }) => {
                        let next: StmtsNext =
                            Rc::new(move |_, frag| Err(Error::Runtime(frag.unwrap_or(Value::Undefined))));
                        return Err(Error::Suspend {
                            awaited,
                            cont: chain_stmts(cont, next),
                        });
                    }
                    Err(e) => return Err(e),
                };
                Err(Error::Runtime(v))
            }
            Stmt::If { cond, then, else_ } => {
                let cval = match self.eval_expr(cond, env) {
                    Ok(v) => v,
                    Err(Error::Suspend { awaited, cont }) => {
                        let then2 = then.clone();
                        let else2 = else_.clone();
                        let env2 = env.clone();
                        let next: StmtsNext = Rc::new(move |engine, frag| {
                            let res = if frag.unwrap_or(Value::Undefined).to_boolean() {
                                engine.exec_stmts(&then2, &env2)
                            } else if !else2.is_empty() {
                                engine.exec_stmts(&else2, &env2)
                            } else {
                                Ok(None)
                            };
                            engine.if_postprocess(res)
                        });
                        return Err(Error::Suspend {
                            awaited,
                            cont: chain_stmts(cont, next),
                        });
                    }
                    Err(e) => return Err(e),
                };
                let res = if cval.to_boolean() {
                    self.exec_stmts(then, env)
                } else if !else_.is_empty() {
                    self.exec_stmts(else_, env)
                } else {
                    Ok(None)
                };
                // Per spec the if statement's completion is
                // `UpdateEmpty(stmtCompletion, undefined)`: an empty value
                // (including an empty abrupt completion) becomes `undefined`.
                self.if_postprocess(res)
            }
            Stmt::Try { try_block, catch, finally } => {
                let res = self.exec_stmts(try_block, env);
                self.try_postprocess(res, catch, finally, env)
            }
            Stmt::Switch { discriminant, cases } => {
                self.exec_switch(discriminant, cases, env)
            }
            Stmt::ForIn { kind, name, expr, body } => {
                self.exec_for_in(None, *kind, name, expr, body, env)
            }
            Stmt::ForOf { kind, name, expr, body } => {
                self.exec_for_of(None, *kind, name, expr, body, env)
            }
            Stmt::For { init, cond, update, body } => {
                self.exec_for(None, init, cond, update, body, env)
            }
            Stmt::While { cond, body } => self.exec_while(None, cond, body, env),
            Stmt::DoWhile { cond, body } => self.exec_do_while(None, cond, body, env),
            Stmt::Labeled { label, body } => self.exec_labeled(label, body, env),
        }
    }

    /// Bind a `var`/`let`/`const` declarator list from index `start`.
    /// Suspension-aware: an `await` in initializer `i` suspends with a
    /// continuation that binds the resumed value and then the rest.
    fn eval_var_list(
        &self,
        list: &[(Pattern, Option<Expr>)],
        start: usize,
        is_const: bool,
        scope: &Rc<RefCell<Env>>,
        env: &Rc<RefCell<Env>>,
    ) -> Result<Option<Value>> {
        for (i, (pat, init)) in list.iter().enumerate().skip(start) {
            let val = match init {
                Some(e) => match self.eval_expr(e, env) {
                    Ok(v) => v,
                    Err(Error::Suspend { awaited, cont }) => {
                        let pat0 = pat.clone();
                        let rest: Vec<(Pattern, Option<Expr>)> = list[i + 1..].to_vec();
                        let scope2 = scope.clone();
                        let env2 = env.clone();
                        let next: StmtsNext = Rc::new(move |engine, frag| {
                            let v = frag.unwrap_or(Value::Undefined);
                            engine.bind_decl(&pat0, &v, &scope2, is_const)?;
                            for (p, init) in &rest {
                                let val = match init {
                                    Some(e) => engine.eval_expr(e, &env2)?,
                                    None => Value::Undefined,
                                };
                                engine.bind_decl(p, &val, &scope2, is_const)?;
                            }
                            Ok(None)
                        });
                        return Err(Error::Suspend {
                            awaited,
                            cont: chain_stmts(cont, next),
                        });
                    }
                    Err(e) => return Err(e),
                },
                None => Value::Undefined,
            };
            self.bind_decl(pat, &val, scope, is_const)?;
        }
        Ok(None)
    }

    /// `if` completion mapping (`UpdateEmpty` with `undefined`), applied to a
    /// branch outcome. A suspended branch resumes with the mapping re-attached.
    fn if_postprocess(&self, res: Result<Option<Value>>) -> Result<Option<Value>> {
        match res {
            Ok(Some(v)) => Ok(Some(v)),
            Ok(None) => Ok(Some(Value::Undefined)),
            Err(Error::Suspend { awaited, cont }) => {
                let next: StmtsNext =
                    Rc::new(move |_, frag| Ok(Some(frag.unwrap_or(Value::Undefined))));
                Err(Error::Suspend {
                    awaited,
                    cont: chain_stmts(cont, next),
                })
            }
            Err(Error::Break(l, None)) => Err(Error::Break(l, Some(Value::Undefined))),
            Err(Error::Continue(l, None)) => Err(Error::Continue(l, Some(Value::Undefined))),
            Err(e) => Err(e),
        }
    }

    /// `try` post-processing: route the try-block outcome through `catch`
    /// (on throws) and `finally` (always). Suspension-aware at every step.
    fn try_postprocess(
        &self,
        res: Result<Option<Value>>,
        catch: &Option<(Rc<str>, Vec<Stmt>)>,
        finally: &Option<Vec<Stmt>>,
        env: &Rc<RefCell<Env>>,
    ) -> Result<Option<Value>> {
        match res {
            Ok(v) => self.run_finally(Ok(v), finally, env),
            Err(Error::Runtime(thrown)) => {
                let cres = if let Some((name, cb)) = catch {
                    let ce = Rc::new(RefCell::new(Env::new_block(env.clone())));
                    self.register_env(&ce);
                    ce.borrow_mut().vars.insert(name.clone(), thrown);
                    match self.exec_stmts(cb, &ce) {
                        Err(Error::Suspend { awaited, cont }) => {
                            let fin2 = finally.clone();
                            let env2 = env.clone();
                            let next: OutcomeNext = Rc::new(move |engine, outcome| {
                                engine.run_finally(outcome, &fin2, &env2)
                            });
                            return Err(Error::Suspend {
                                awaited,
                                cont: chain_outcome(cont, next),
                            });
                        }
                        other => other,
                    }
                } else {
                    Err(Error::Runtime(thrown))
                };
                self.run_finally(cres, finally, env)
            }
            Err(Error::Suspend { awaited, cont }) => {
                let catch2 = catch.clone();
                let fin2 = finally.clone();
                let env2 = env.clone();
                let next: OutcomeNext = Rc::new(move |engine, outcome| {
                    engine.try_postprocess(outcome, &catch2, &fin2, &env2)
                });
                Err(Error::Suspend {
                    awaited,
                    cont: chain_outcome(cont, next),
                })
            }
            // `return`/`break`/`continue`/throws from anywhere still run `finally`.
            Err(e) => self.run_finally(Err(e), finally, env),
        }
    }

    /// Run the `finally` block, then yield `outcome` (a `finally` abrupt
    /// overrides it). A suspended `finally` resumes with the same rule.
    fn run_finally(
        &self,
        outcome: Result<Option<Value>>,
        finally: &Option<Vec<Stmt>>,
        env: &Rc<RefCell<Env>>,
    ) -> Result<Option<Value>> {
        if let Some(fb) = finally {
            match self.exec_stmts(fb, env) {
                Ok(_) => {}
                Err(Error::Suspend { awaited, cont }) => {
                    let outcome2 = outcome.clone();
                    let next: StmtsNext =
                        Rc::new(move |_, _| outcome2.clone());
                    return Err(Error::Suspend {
                        awaited,
                        cont: chain_stmts(cont, next),
                    });
                }
                Err(e) => return Err(e),
            }
        }
        outcome
    }

    /// Evaluate `e`, then feed the value into `next`. A suspension composes
    /// `next` onto the fragment's continuation.
    fn eval_expr_chain(
        &self,
        e: &Expr,
        env: &Rc<RefCell<Env>>,
        next: ValueNext,
    ) -> Result<Value> {
        match self.eval_expr(e, env) {
            Ok(v) => next(self, v),
            Err(Error::Suspend { awaited, cont }) => Err(Error::Suspend {
                awaited,
                cont: chain_value(cont, next),
            }),
            Err(e) => Err(e),
        }
    }

    /// Evaluate a call/new argument list from `idx`, accumulating into `argv`.
    fn eval_args_from(
        &self,
        args: &[Arg],
        env: &Rc<RefCell<Env>>,
        idx: usize,
        argv: Vec<Value>,
        next: ExprVecNext,
    ) -> Result<Value> {
        let mut argv = argv;
        for (i, a) in args.iter().enumerate().skip(idx) {
            match a {
                Arg::Expr(e) => match self.eval_expr(e, env) {
                    Ok(v) => argv.push(v),
                    Err(Error::Suspend { awaited, cont }) => {
                        let args2 = args.to_vec();
                        let env2 = env.clone();
                        let argv2 = argv.clone();
                        let next2 = next.clone();
                        let inner: ValueNext = Rc::new(move |engine, v| {
                            let mut argv3 = argv2.clone();
                            argv3.push(v);
                            engine.eval_args_from(&args2, &env2, i + 1, argv3, next2.clone())
                        });
                        return Err(Error::Suspend {
                            awaited,
                            cont: chain_value(cont, inner),
                        });
                    }
                    Err(e) => return Err(e),
                },
                Arg::Spread(s) => match self.eval_expr(s, env) {
                    Ok(sv) => {
                        for item in self.iterable_values(&sv)? {
                            argv.push(item);
                        }
                    }
                    Err(Error::Suspend { awaited, cont }) => {
                        let args2 = args.to_vec();
                        let env2 = env.clone();
                        let argv2 = argv.clone();
                        let next2 = next.clone();
                        let inner: ValueNext = Rc::new(move |engine, sv| {
                            let mut argv3 = argv2.clone();
                            for item in engine.iterable_values(&sv)? {
                                argv3.push(item);
                            }
                            engine.eval_args_from(&args2, &env2, i + 1, argv3, next2.clone())
                        });
                        return Err(Error::Suspend {
                            awaited,
                            cont: chain_value(cont, inner),
                        });
                    }
                    Err(e) => return Err(e),
                },
            }
        }
        next(self, argv)
    }

    /// Evaluate an expression list (sequence/tagged parts) from `idx`.
    fn eval_expr_list_from(
        &self,
        exprs: &[Expr],
        env: &Rc<RefCell<Env>>,
        idx: usize,
        acc: Vec<Value>,
        next: ExprVecNext,
    ) -> Result<Value> {
        let mut acc = acc;
        for (i, e) in exprs.iter().enumerate().skip(idx) {
            match self.eval_expr(e, env) {
                Ok(v) => acc.push(v),
                Err(Error::Suspend { awaited, cont }) => {
                    let exprs2 = exprs.to_vec();
                    let env2 = env.clone();
                    let acc2 = acc.clone();
                    let next2 = next.clone();
                    let inner: ValueNext = Rc::new(move |engine, v| {
                        let mut acc3 = acc2.clone();
                        acc3.push(v);
                        engine.eval_expr_list_from(&exprs2, &env2, i + 1, acc3, next2.clone())
                    });
                    return Err(Error::Suspend {
                        awaited,
                        cont: chain_value(cont, inner),
                    });
                }
                Err(e) => return Err(e),
            }
        }
        next(self, acc)
    }

    /// Evaluate a call's callee (resolving `this`) and arguments, then invoke.
    fn eval_call_callee(
        &self,
        callee: &Expr,
        args: &[Arg],
        env: &Rc<RefCell<Env>>,
    ) -> Result<Value> {
        if matches!(callee, Expr::Super) {
            let ctor = env.borrow().get("__super_ctor__").ok_or_else(|| {
                Error::Runtime(Value::String(Rc::from(
                    "SyntaxError: 'super' constructor call outside subclass",
                )))
            })?;
            // Inside a `new` construction this is a derived-class super call:
            // construct the parent with the active new-target and rebind
            // `this` to the result. Otherwise (e.g. a plain method call) keep
            // the legacy plain-call behavior.
            let new_target = self.active_new_target.borrow().clone();
            let env2 = env.clone();
            return self.eval_args_from(
                args,
                env,
                0,
                Vec::new(),
                Rc::new(move |engine, argv| match new_target.clone() {
                    Some(nt) => {
                        let v = engine.construct_with_new_target(&ctor, &argv, &nt)?;
                        env2.borrow_mut()
                            .vars
                            .insert(Rc::from("__this__"), v.clone());
                        Ok(v)
                    }
                    None => {
                        let this = env2
                            .borrow()
                            .get("__this__")
                            .unwrap_or_else(|| Value::Object(engine.global_object.clone()));
                        engine.call_function(&ctor, &this, &argv)
                    }
                }),
            );
        }
        match callee {
            Expr::Member { obj, prop, computed, .. } => {
                let prop_c = prop.clone();
                let computed_c = computed.clone();
                let args2 = args.to_vec();
                let env2 = env.clone();
                self.eval_expr_chain(obj, env, Rc::new(move |engine, base| {
                    match computed_c.clone() {
                        Some(idx) => {
                            let base2 = base.clone();
                            let args3 = args2.clone();
                            let env3 = env2.clone();
                            engine.eval_expr_chain(&idx, &env2, Rc::new(move |engine, k| {
                                let key = engine.to_property_key(&k)?;
                                let func = engine.get_member(&base2, &key)?;
                                let base3 = base2.clone();
                                engine.eval_args_from(
                                    &args3,
                                    &env3,
                                    0,
                                    Vec::new(),
                                    Rc::new(move |engine, argv| {
                                        engine.call_function(&func, &base3, &argv)
                                    }),
                                )
                            }))
                        }
                        None => {
                            let func = engine.get_member(&base, &prop_c)?;
                            let base2 = base.clone();
                            engine.eval_args_from(
                                &args2,
                                &env2,
                                0,
                                Vec::new(),
                                Rc::new(move |engine, argv| {
                                    engine.call_function(&func, &base2, &argv)
                                }),
                            )
                        }
                    }
                }))
            }
            _ => {
                let args2 = args.to_vec();
                let env2 = env.clone();
                self.eval_expr_chain(callee, env, Rc::new(move |engine, func| {
                    let this = Value::Object(engine.global_object.clone());
                    engine.eval_args_from(
                        &args2,
                        &env2,
                        0,
                        Vec::new(),
                        Rc::new(move |engine, argv| engine.call_function(&func, &this, &argv)),
                    )
                }))
            }
        }
    }

    /// Evaluate the argument of a `return` statement. A plain call in tail
    /// position (`return f(…)` / `return obj.m(…)`) whose callee is a regular
    /// user function becomes a proper tail call: the call frame is unwound
    /// and re-invoked without growing the stack.
    fn eval_return_expr(&self, e: &Expr, env: &Rc<RefCell<Env>>) -> Result<Value> {
        if let Expr::Call { callee, args, optional } = e {
            if !*optional && !matches!(&**callee, Expr::Super) {
                // Shared tail: build argv, then tail-call (sync targets) or invoke.
                fn finish(
                    engine: &Engine,
                    func: Value,
                    this: Value,
                    argv: Vec<Value>,
                ) -> Result<Value> {
                    if matches!(&func, Value::Function(f) if !f.borrow().generator && !f.borrow().is_async)
                    {
                        return Err(Error::TailCall { func, this, args: argv });
                    }
                    engine.call_function(&func, &this, &argv)
                }
                match &**callee {
                    Expr::Member { obj, prop, computed, .. } => {
                        let prop_c = prop.clone();
                        let computed_c = computed.clone();
                        let args2 = args.clone();
                        let env2 = env.clone();
                        return self.eval_expr_chain(obj, env, Rc::new(move |engine, base| {
                            match computed_c.clone() {
                                Some(idx) => {
                                    let base2 = base.clone();
                                    let args3 = args2.clone();
                                    let env3 = env2.clone();
                                    engine.eval_expr_chain(&idx, &env2, Rc::new(move |engine, k| {
                                        let key = engine.to_property_key(&k)?;
                                        let func = engine.get_member(&base2, &key)?;
                                        let func2 = func.clone();
                                        let base3 = base2.clone();
                                        engine.eval_args_from(
                                            &args3,
                                            &env3,
                                            0,
                                            Vec::new(),
                                            Rc::new(move |engine, argv| {
                                                finish(engine, func2.clone(), base3.clone(), argv)
                                            }),
                                        )
                                    }))
                                }
                                None => {
                                    let func = engine.get_member(&base, &prop_c)?;
                                    let base2 = base.clone();
                                    engine.eval_args_from(
                                        &args2,
                                        &env2,
                                        0,
                                        Vec::new(),
                                        Rc::new(move |engine, argv| {
                                            finish(engine, func.clone(), base2.clone(), argv)
                                        }),
                                    )
                                }
                            }
                        }));
                    }
                    _ => {
                        let args2 = args.clone();
                        let env2 = env.clone();
                        return self.eval_expr_chain(callee, env, Rc::new(move |engine, func| {
                            let this = Value::Object(engine.global_object.clone());
                            engine.eval_args_from(
                                &args2,
                                &env2,
                                0,
                                Vec::new(),
                                Rc::new(move |engine, argv| {
                                    finish(engine, func.clone(), this.clone(), argv)
                                }),
                            )
                        }));
                    }
                }
            }
        }
        self.eval_expr(e, env)
    }

    fn exec_labeled(
        &self,
        label: &Rc<str>,
        body: &Stmt,
        env: &Rc<RefCell<Env>>,
    ) -> Result<Option<Value>> {
        let res = match body {
            Stmt::For { init, cond, update, body } => {
                self.exec_for(Some(label), init, cond, update, body, env)
            }
            Stmt::ForIn { kind, name, expr, body } => {
                self.exec_for_in(Some(label), *kind, name, expr, body, env)
            }
            Stmt::ForOf { kind, name, expr, body } => {
                self.exec_for_of(Some(label), *kind, name, expr, body, env)
            }
            Stmt::While { cond, body } => self.exec_while(Some(label), cond, body, env),
            Stmt::DoWhile { cond, body } => self.exec_do_while(Some(label), cond, body, env),
            other => self.eval_stmt(other, env),
        };
        self.labeled_postprocess(res, label)
    }

    /// `lbl: stmt` completion: a `break`/`continue` targeting the label
    /// completes normally. A suspended body resumes with the same rule.
    fn labeled_postprocess(
        &self,
        res: Result<Option<Value>>,
        label: &Rc<str>,
    ) -> Result<Option<Value>> {
        match res {
            Err(Error::Suspend { awaited, cont }) => {
                let label2 = label.clone();
                let next: OutcomeNext = Rc::new(move |engine, outcome| {
                    engine.labeled_postprocess(outcome, &label2)
                });
                Err(Error::Suspend {
                    awaited,
                    cont: chain_outcome(cont, next),
                })
            }
            // A `break lbl` targeting this label completes the labelled
            // statement normally with its completion value (which may be
            // empty).
            Err(Error::Break(Some(l), v)) if &l == label => Ok(v),
            Err(Error::Continue(Some(l), v)) if &l == label => Ok(v),
            other => other,
        }
    }

    /// Shared loop-body outcome handling: fold a body outcome into the loop's
    /// accumulated value, deciding whether to continue iterating or finish.
    fn loop_body_step(
        &self,
        outcome: Result<Option<Value>>,
        v: Option<Value>,
        label: Option<&Rc<str>>,
    ) -> LoopStep {
        match outcome {
            Ok(val) => {
                let mut v = v;
                if val.is_some() {
                    v = val;
                }
                LoopStep::Next(v)
            }
            Err(Error::Break(b, val)) => {
                let val = val.or_else(|| v.clone());
                if b.is_none() || b.as_ref() == label {
                    LoopStep::Done(Ok(Some(val.unwrap_or(Value::Undefined))))
                } else {
                    LoopStep::Done(Err(Error::Break(b, val)))
                }
            }
            Err(Error::Continue(c, val)) => {
                if c.is_some() && c.as_ref() != label {
                    LoopStep::Done(Err(Error::Continue(c, val.or_else(|| v.clone()))))
                } else {
                    let mut v = v;
                    if val.is_some() {
                        v = val;
                    }
                    LoopStep::Next(v)
                }
            }
            Err(e) => LoopStep::Done(Err(e)),
        }
    }

    fn exec_switch(
        &self,
        discriminant: &Expr,
        cases: &[(Option<Expr>, Vec<Stmt>)],
        env: &Rc<RefCell<Env>>,
    ) -> Result<Option<Value>> {
        // The switch expression is evaluated in the *outer* lexical
        // environment; the case selectors and clause statements all share one
        // fresh declarative environment for the whole CaseBlock.
        let d = match self.eval_expr(discriminant, env) {
            Ok(v) => v,
            Err(Error::Suspend { awaited, cont }) => {
                let cases2 = cases.to_vec();
                let env2 = env.clone();
                let next: StmtsNext = Rc::new(move |engine, frag| {
                    engine.exec_switch_from(
                        &frag.unwrap_or(Value::Undefined),
                        &cases2,
                        &env2,
                        Some(Value::Undefined),
                    )
                });
                return Err(Error::Suspend {
                    awaited,
                    cont: chain_stmts(cont, next),
                });
            }
            Err(e) => return Err(e),
        };
        self.exec_switch_from(&d, cases, env, Some(Value::Undefined))
    }

    /// `switch` matching + clause execution on an already-evaluated
    /// discriminant.
    fn exec_switch_from(
        &self,
        d: &Value,
        cases: &[(Option<Expr>, Vec<Stmt>)],
        env: &Rc<RefCell<Env>>,
        v: Option<Value>,
    ) -> Result<Option<Value>> {
        let block_env = Rc::new(RefCell::new(Env::new_block(env.clone())));
        self.register_env(&block_env);
        let start = self.switch_start(d, cases, &block_env, 0, None)?;
        self.exec_switch_cases(cases, &block_env, start, v)
    }

    /// Run `switch` clauses from `start` (fall-through), accumulating `v`.
    fn exec_switch_cases(
        &self,
        cases: &[(Option<Expr>, Vec<Stmt>)],
        block_env: &Rc<RefCell<Env>>,
        start: Option<usize>,
        v: Option<Value>,
    ) -> Result<Option<Value>> {
        // `resultValue` starts as `undefined` (non-empty) per spec, so an
        // empty switch still produces an `undefined` completion value.
        let mut v = v;
        if let Some(start) = start {
            for i in start..cases.len() {
                match self.exec_stmts(&cases[i].1, block_env) {
                    Ok(val) => {
                        if val.is_some() {
                            v = val;
                        }
                    }
                    Err(Error::Suspend { awaited, cont }) => {
                        let cases2 = cases.to_vec();
                        let env2 = block_env.clone();
                        let v2 = v.clone();
                        let next: OutcomeNext = Rc::new(move |engine, outcome| {
                            let mut v = v2.clone();
                            match outcome {
                                Ok(val) => {
                                    if val.is_some() {
                                        v = val;
                                    }
                                }
                                Err(Error::Break(label, val)) => {
                                    let val = val.or_else(|| v.clone());
                                    if label.is_none() {
                                        return Ok(Some(val.unwrap_or(Value::Undefined)));
                                    }
                                    return Err(Error::Break(label, val));
                                }
                                Err(Error::Continue(label, val)) => {
                                    return Err(Error::Continue(
                                        label,
                                        val.or_else(|| v.clone()),
                                    ))
                                }
                                Err(e) => return Err(e),
                            }
                            engine.exec_switch_cases(&cases2, &env2, Some(i + 1), v)
                        });
                        return Err(Error::Suspend {
                            awaited,
                            cont: chain_outcome(cont, next),
                        });
                    }
                    // `break` completes the switch (spec `UpdateEmpty(R, V)`);
                    // an unlabeled break makes the switch's own completion a
                    // normal value.
                    Err(Error::Break(label, val)) => {
                        let val = val.or_else(|| v.clone());
                        if label.is_none() {
                            return Ok(Some(val.unwrap_or(Value::Undefined)));
                        }
                        return Err(Error::Break(label, val));
                    }
                    Err(Error::Continue(label, val)) => {
                        return Err(Error::Continue(label, val.or_else(|| v.clone())))
                    }
                    Err(e) => return Err(e),
                }
            }
        }
        Ok(v)
    }

    /// Match the discriminant against case tests from index `idx`.
    /// Suspension-aware: an `await` in test `j` suspends with the rest of the
    /// matching (plus clause execution) attached.
    fn switch_start(
        &self,
        d: &Value,
        cases: &[(Option<Expr>, Vec<Stmt>)],
        block_env: &Rc<RefCell<Env>>,
        idx: usize,
        default_idx: Option<usize>,
    ) -> Result<Option<usize>> {
        let mut start: Option<usize> = None;
        let mut default_idx = default_idx;
        for (i, (test, _)) in cases.iter().enumerate().skip(idx) {
            match test {
                Some(t) => {
                    let tv = match self.eval_expr(t, block_env) {
                        Ok(v) => v,
                        Err(Error::Suspend { awaited, cont }) => {
                            let d2 = d.clone();
                            let cases2 = cases.to_vec();
                            let env2 = block_env.clone();
                            let next: StmtsNext = Rc::new(move |engine, frag| {
                                let tv = frag.unwrap_or(Value::Undefined);
                                if Value::strict_eq(&d2, &tv) {
                                    return engine.exec_switch_cases(
                                        &cases2,
                                        &env2,
                                        Some(i),
                                        Some(Value::Undefined),
                                    );
                                }
                                let s = engine.switch_start(&d2, &cases2, &env2, i + 1, default_idx)?;
                                engine.exec_switch_cases(
                                    &cases2,
                                    &env2,
                                    s,
                                    Some(Value::Undefined),
                                )
                            });
                            return Err(Error::Suspend {
                                awaited,
                                cont: chain_stmts(cont, next),
                            });
                        }
                        Err(e) => return Err(e),
                    };
                    if Value::strict_eq(d, &tv) {
                        start = Some(i);
                        break;
                    }
                }
                None => {
                    if default_idx.is_none() {
                        default_idx = Some(i);
                    }
                }
            }
        }
        Ok(start.or(default_idx))
    }

    fn exec_for_in(
        &self,
        label: Option<&Rc<str>>,
        kind: VarKind,
        name: &Pattern,
        expr: &Expr,
        body: &[Stmt],
        env: &Rc<RefCell<Env>>,
    ) -> Result<Option<Value>> {
        let obj = match self.eval_expr(expr, env) {
            Ok(v) => v,
            Err(Error::Suspend { awaited, cont }) => {
                let kind2 = kind;
                let name2 = name.clone();
                let body2 = body.to_vec();
                let env2 = env.clone();
                let label2 = label.map(|l| l.clone());
                let next: StmtsNext = Rc::new(move |engine, frag| {
                    let obj = frag.unwrap_or(Value::Undefined);
                    let keys = engine.enumerable_keys(&obj);
                    engine.exec_for_in_keys(
                        label2.as_ref(),
                        kind2,
                        &name2,
                        &body2,
                        &env2,
                        &keys,
                        0,
                        Some(Value::Undefined),
                    )
                });
                return Err(Error::Suspend {
                    awaited,
                    cont: chain_stmts(cont, next),
                });
            }
            Err(e) => return Err(e),
        };
        let keys = self.enumerable_keys(&obj);
        self.exec_for_in_keys(label, kind, name, body, env, &keys, 0, Some(Value::Undefined))
    }

    /// `for-in` iterations from index `idx`, accumulating completion `v`.
    #[allow(clippy::too_many_arguments)]
    fn exec_for_in_keys(
        &self,
        label: Option<&Rc<str>>,
        kind: VarKind,
        name: &Pattern,
        body: &[Stmt],
        env: &Rc<RefCell<Env>>,
        keys: &[Rc<str>],
        idx: usize,
        v: Option<Value>,
    ) -> Result<Option<Value>> {
        let mut v = v;
        let is_var = matches!(kind, VarKind::Var);
        let var_scope = Env::function_scope(env);
        for (k, key) in keys.iter().enumerate().skip(idx) {
            let val = Value::String(key.clone());
            // `let`/`const` produce a fresh per-iteration binding so closures
            // capture that iteration's value; `var` binds once in the function.
            let per_iter: Option<Rc<RefCell<Env>>> = if is_var {
                self.bind_pattern(name, &val, &var_scope).map_err(|e| match e {
                    Error::Suspend { .. } => Error::Unimplemented(
                        "await in for-head pattern is not implemented".to_string(),
                    ),
                    other => other,
                })?;
                None
            } else {
                let pe = Rc::new(RefCell::new(Env::new_block(env.clone())));
                self.register_env(&pe);
                self.bind_pattern(name, &val, &pe).map_err(|e| match e {
                    Error::Suspend { .. } => Error::Unimplemented(
                        "await in for-head pattern is not implemented".to_string(),
                    ),
                    other => other,
                })?;
                if matches!(kind, VarKind::Const) {
                    mark_const_idents(name, &pe);
                }
                Some(pe)
            };
            let body_env: &Rc<RefCell<Env>> = per_iter.as_ref().unwrap_or(env);
            match self.exec_stmts(body, body_env) {
                Err(Error::Suspend { awaited, cont }) => {
                    let label2 = label.map(|l| l.clone());
                    let kind2 = kind;
                    let name2 = name.clone();
                    let body2 = body.to_vec();
                    let env2 = env.clone();
                    let keys2: Vec<Rc<str>> = keys.to_vec();
                    let v2 = v.clone();
                    let next: OutcomeNext = Rc::new(move |engine, outcome| {
                        match engine.loop_body_step(outcome, v2.clone(), label2.as_ref()) {
                            LoopStep::Next(v3) => engine.exec_for_in_keys(
                                label2.as_ref(),
                                kind2,
                                &name2,
                                &body2,
                                &env2,
                                &keys2,
                                k + 1,
                                v3,
                            ),
                            LoopStep::Done(r) => r,
                        }
                    });
                    return Err(Error::Suspend {
                        awaited,
                        cont: chain_outcome(cont, next),
                    });
                }
                other => match self.loop_body_step(other, v, label) {
                    LoopStep::Next(v2) => v = v2,
                    LoopStep::Done(r) => return r,
                },
            }
        }
        Ok(v)
    }

    fn exec_for_of(
        &self,
        label: Option<&Rc<str>>,
        kind: VarKind,
        name: &Pattern,
        expr: &Expr,
        body: &[Stmt],
        env: &Rc<RefCell<Env>>,
    ) -> Result<Option<Value>> {
        let obj = match self.eval_expr(expr, env) {
            Ok(v) => v,
            Err(Error::Suspend { awaited, cont }) => {
                let kind2 = kind;
                let name2 = name.clone();
                let body2 = body.to_vec();
                let env2 = env.clone();
                let label2 = label.map(|l| l.clone());
                let next: StmtsNext = Rc::new(move |engine, frag| {
                    let obj = frag.unwrap_or(Value::Undefined);
                    if let Value::Array(array) = obj {
                        return engine.exec_for_of_array(
                            label2.as_ref(), kind2, &name2, &body2, &env2, &array, 0,
                            Some(Value::Undefined),
                        );
                    }
                    let items = engine.iterable_values(&obj)?;
                    engine.exec_for_of_items(
                        label2.as_ref(),
                        kind2,
                        &name2,
                        &body2,
                        &env2,
                        &items,
                        0,
                        Some(Value::Undefined),
                    )
                });
                return Err(Error::Suspend {
                    awaited,
                    cont: chain_stmts(cont, next),
                });
            }
            Err(e) => return Err(e),
        };
        if let Value::Array(array) = obj {
            return self.exec_for_of_array(label, kind, name, body, env, &array, 0, Some(Value::Undefined));
        }
        let items = self.iterable_values(&obj)?;
        self.exec_for_of_items(label, kind, name, body, env, &items, 0, Some(Value::Undefined))
    }

    /// Stream an array's values directly from its backing storage. In
    /// particular, this keeps sparse arrays lazy and avoids allocating one
    /// `Value::Undefined` per hole before the first loop iteration.
    #[allow(clippy::too_many_arguments)]
    fn exec_for_of_array(
        &self,
        label: Option<&Rc<str>>,
        kind: VarKind,
        name: &Pattern,
        body: &[Stmt],
        env: &Rc<RefCell<Env>>,
        array: &Rc<RefCell<crate::value::ArrayData>>,
        idx: usize,
        v: Option<Value>,
    ) -> Result<Option<Value>> {
        let mut value = v;
        let is_var = matches!(kind, VarKind::Var);
        let var_scope = Env::function_scope(env);
        let mut k = idx;
        loop {
            // Array iterators observe length changes between `next()` calls.
            // Re-read it here while still avoiding any materialized value list.
            if k >= array.borrow().logical_len() {
                break;
            }
            let val = array.borrow().get_index(k).unwrap_or(Value::Undefined);
            let per_iter = if is_var {
                self.bind_pattern(name, &val, &var_scope).map_err(|e| match e {
                    Error::Suspend { .. } => Error::Unimplemented("await in for-head pattern is not implemented".to_string()),
                    other => other,
                })?;
                None
            } else {
                let pe = Rc::new(RefCell::new(Env::new_block(env.clone())));
                self.register_env(&pe);
                self.bind_pattern(name, &val, &pe).map_err(|e| match e {
                    Error::Suspend { .. } => Error::Unimplemented("await in for-head pattern is not implemented".to_string()),
                    other => other,
                })?;
                if matches!(kind, VarKind::Const) { mark_const_idents(name, &pe); }
                Some(pe)
            };
            let body_env = per_iter.as_ref().unwrap_or(env);
            match self.exec_stmts(body, body_env) {
                Err(Error::Suspend { awaited, cont }) => {
                    let label2 = label.cloned();
                    let name2 = name.clone();
                    let body2 = body.to_vec();
                    let env2 = env.clone();
                    let array2 = array.clone();
                    let value2 = value.clone();
                    let next: OutcomeNext = Rc::new(move |engine, outcome| {
                        match engine.loop_body_step(outcome, value2.clone(), label2.as_ref()) {
                            LoopStep::Next(v2) => engine.exec_for_of_array(
                                label2.as_ref(), kind, &name2, &body2, &env2, &array2, k + 1, v2,
                            ),
                            LoopStep::Done(r) => r,
                        }
                    });
                    return Err(Error::Suspend { awaited, cont: chain_outcome(cont, next) });
                }
                other => match self.loop_body_step(other, value, label) {
                    LoopStep::Next(v2) => value = v2,
                    LoopStep::Done(r) => return r,
                },
            }
            k += 1;
        }
        Ok(value)
    }

    /// `for-of` iterations from index `idx`, accumulating completion `v`.
    #[allow(clippy::too_many_arguments)]
    fn exec_for_of_items(
        &self,
        label: Option<&Rc<str>>,
        kind: VarKind,
        name: &Pattern,
        body: &[Stmt],
        env: &Rc<RefCell<Env>>,
        items: &[Value],
        idx: usize,
        v: Option<Value>,
    ) -> Result<Option<Value>> {
        let mut v = v;
        let is_var = matches!(kind, VarKind::Var);
        let var_scope = Env::function_scope(env);
        for (k, val) in items.iter().enumerate().skip(idx) {
            // `let`/`const` produce a fresh per-iteration binding so closures
            // capture that iteration's value; `var` binds once in the function.
            let per_iter: Option<Rc<RefCell<Env>>> = if is_var {
                self.bind_pattern(name, val, &var_scope).map_err(|e| match e {
                    Error::Suspend { .. } => Error::Unimplemented(
                        "await in for-head pattern is not implemented".to_string(),
                    ),
                    other => other,
                })?;
                None
            } else {
                let pe = Rc::new(RefCell::new(Env::new_block(env.clone())));
                self.register_env(&pe);
                self.bind_pattern(name, val, &pe).map_err(|e| match e {
                    Error::Suspend { .. } => Error::Unimplemented(
                        "await in for-head pattern is not implemented".to_string(),
                    ),
                    other => other,
                })?;
                if matches!(kind, VarKind::Const) {
                    mark_const_idents(name, &pe);
                }
                Some(pe)
            };
            let body_env: &Rc<RefCell<Env>> = per_iter.as_ref().unwrap_or(env);
            match self.exec_stmts(body, body_env) {
                Err(Error::Suspend { awaited, cont }) => {
                    let label2 = label.map(|l| l.clone());
                    let kind2 = kind;
                    let name2 = name.clone();
                    let body2 = body.to_vec();
                    let env2 = env.clone();
                    let items2: Vec<Value> = items.to_vec();
                    let v2 = v.clone();
                    let next: OutcomeNext = Rc::new(move |engine, outcome| {
                        match engine.loop_body_step(outcome, v2.clone(), label2.as_ref()) {
                            LoopStep::Next(v3) => engine.exec_for_of_items(
                                label2.as_ref(),
                                kind2,
                                &name2,
                                &body2,
                                &env2,
                                &items2,
                                k + 1,
                                v3,
                            ),
                            LoopStep::Done(r) => r,
                        }
                    });
                    return Err(Error::Suspend {
                        awaited,
                        cont: chain_outcome(cont, next),
                    });
                }
                other => match self.loop_body_step(other, v, label) {
                    LoopStep::Next(v2) => v = v2,
                    LoopStep::Done(r) => return r,
                },
            }
        }
        Ok(v)
    }

    fn exec_for(
        &self,
        label: Option<&Rc<str>>,
        init: &Option<Box<Stmt>>,
        cond: &Option<Expr>,
        update: &Option<Expr>,
        body: &[Stmt],
        env: &Rc<RefCell<Env>>,
    ) -> Result<Option<Value>> {
        // `for (let i = ...)` / `for (const x of ...)`: `let`/`const` names are
        // *per-iteration* bindings, so each iteration's body runs in a fresh
        // snapshot environment and closures capture per-iteration values.
        // The loop-scope env holds the "live" bindings for condition/update.
        let block_env = env;
        let mut loop_scope: Option<Rc<RefCell<Env>>> = None;
        let mut per_iter_names: Vec<Rc<str>> = Vec::new();
        if let Some(init) = init {
            match init.as_ref() {
                Stmt::Var(list, VarKind::Let | VarKind::Const) => {
                    let scope = Rc::new(RefCell::new(Env::new_block(block_env.clone())));
                    self.register_env(&scope);
                    match self.eval_stmt(init, &scope) {
                        Err(Error::Suspend { awaited, cont }) => {
                            let list2 = list.clone();
                            let scope2 = scope.clone();
                            let label2 = label.map(|l| l.clone());
                            let cond2 = cond.clone();
                            let update2 = update.clone();
                            let body2 = body.to_vec();
                            let next: StmtsNext = Rc::new(move |engine, _| {
                                let mut names: Vec<Rc<str>> = Vec::new();
                                for (pat, _) in &list2 {
                                    pattern_idents(pat, &mut names);
                                }
                                engine.exec_for_loop(
                                    label2.as_ref(),
                                    &cond2,
                                    &update2,
                                    &body2,
                                    &scope2,
                                    &names,
                                    Some(Value::Undefined),
                                )
                            });
                            return Err(Error::Suspend {
                                awaited,
                                cont: chain_stmts(cont, next),
                            });
                        }
                        Err(e) => return Err(e),
                        Ok(_) => {}
                    }
                    for (pat, _) in list {
                        pattern_idents(pat, &mut per_iter_names);
                    }
                    loop_scope = Some(scope);
                }
                other => match self.eval_stmt(other, block_env) {
                    Err(Error::Suspend { awaited, cont }) => {
                        let label2 = label.map(|l| l.clone());
                        let cond2 = cond.clone();
                        let update2 = update.clone();
                        let body2 = body.to_vec();
                        let env2 = block_env.clone();
                        let next: StmtsNext = Rc::new(move |engine, _| {
                            let names: Vec<Rc<str>> = Vec::new();
                            engine.exec_for_loop(
                                label2.as_ref(),
                                &cond2,
                                &update2,
                                &body2,
                                &env2,
                                &names,
                                Some(Value::Undefined),
                            )
                        });
                        return Err(Error::Suspend {
                            awaited,
                            cont: chain_stmts(cont, next),
                        });
                    }
                    Err(e) => return Err(e),
                    Ok(_) => {}
                },
            }
        }
        let loop_env: &Rc<RefCell<Env>> = loop_scope.as_ref().unwrap_or(block_env);
        self.exec_for_loop(label, cond, update, body, loop_env, &per_iter_names, Some(Value::Undefined))
    }

    /// One `for` body+update step: fold the body outcome, run the update.
    /// `Next` continues iterating (the caller loops); `Done` finishes.
    /// A suspended update resumes with the loop remainder attached.
    #[allow(clippy::too_many_arguments)]
    fn for_body_step(
        &self,
        outcome: Result<Option<Value>>,
        v: Option<Value>,
        label: Option<&Rc<str>>,
        cond: &Option<Expr>,
        update: &Option<Expr>,
        body: &[Stmt],
        loop_env: &Rc<RefCell<Env>>,
        per_iter_names: &[Rc<str>],
    ) -> Result<IterOut> {
        let v2 = match self.loop_body_step(outcome, v, label) {
            LoopStep::Next(v) => v,
            LoopStep::Done(r) => return Ok(IterOut::Done(r)),
        };
        if let Some(u) = update {
            match self.eval_expr(u, loop_env) {
                Err(Error::Suspend { awaited, cont }) => {
                    let label2 = label.map(|l| l.clone());
                    let cond2 = cond.clone();
                    let update2 = update.clone();
                    let body2 = body.to_vec();
                    let loop_env2 = loop_env.clone();
                    let names2: Vec<Rc<str>> = per_iter_names.to_vec();
                    let next: StmtsNext = Rc::new(move |engine, _| {
                        engine.exec_for_loop(
                            label2.as_ref(),
                            &cond2,
                            &update2,
                            &body2,
                            &loop_env2,
                            &names2,
                            v2.clone(),
                        )
                    });
                    return Err(Error::Suspend {
                        awaited,
                        cont: chain_stmts(cont, next),
                    });
                }
                Err(e) => return Err(e),
                Ok(_) => {}
            }
        }
        Ok(IterOut::Next(v2))
    }

    /// Fresh per-iteration snapshot of the loop bindings (or `None` when the
    /// loop has no `let`/`const` head names).
    fn for_snapshot(
        &self,
        loop_env: &Rc<RefCell<Env>>,
        per_iter_names: &[Rc<str>],
    ) -> Option<Rc<RefCell<Env>>> {
        if per_iter_names.is_empty() {
            return None;
        }
        let pe = Rc::new(RefCell::new(Env::new_block(loop_env.clone())));
        self.register_env(&pe);
        for name in per_iter_names {
            let val = loop_env.borrow().get(name).unwrap_or(Value::Undefined);
            pe.borrow_mut().vars.insert(name.clone(), val);
        }
        Some(pe)
    }

    /// `for` iterations with accumulated completion `v`. The synchronous path
    /// stays iterative (no stack growth); suspension remainders recurse (each
    /// resume is a fresh job, so depth stays bounded).
    #[allow(clippy::too_many_arguments)]
    fn exec_for_loop(
        &self,
        label: Option<&Rc<str>>,
        cond: &Option<Expr>,
        update: &Option<Expr>,
        body: &[Stmt],
        loop_env: &Rc<RefCell<Env>>,
        per_iter_names: &[Rc<str>],
        v: Option<Value>,
    ) -> Result<Option<Value>> {
        let mut v = v;
        loop {
            if let Some(c) = cond {
                match self.eval_expr(c, loop_env) {
                    Ok(cv) => {
                        if !cv.to_boolean() {
                            break;
                        }
                    }
                    Err(Error::Suspend { awaited, cont }) => {
                        let label2 = label.map(|l| l.clone());
                        let cond2 = cond.clone();
                        let update2 = update.clone();
                        let body2 = body.to_vec();
                        let loop_env2 = loop_env.clone();
                        let names2: Vec<Rc<str>> = per_iter_names.to_vec();
                        let v2 = v.clone();
                        let next: StmtsNext = Rc::new(move |engine, frag| {
                            if !frag.unwrap_or(Value::Undefined).to_boolean() {
                                return Ok(v2.clone());
                            }
                            let snap = engine.for_snapshot(&loop_env2, &names2);
                            let body_env2: &Rc<RefCell<Env>> =
                                snap.as_ref().unwrap_or(&loop_env2);
                            let bres = match engine.exec_stmts(&body2, body_env2) {
                                Err(Error::Suspend { awaited, cont }) => {
                                    let label3 = label2.clone();
                                    let cond3 = cond2.clone();
                                    let update3 = update2.clone();
                                    let body3 = body2.clone();
                                    let loop_env3 = loop_env2.clone();
                                    let names3 = names2.clone();
                                    let v2c = v2.clone();
                                    let next2: OutcomeNext = Rc::new(move |engine, outcome| {
                                        match engine.for_body_step(
                                            outcome,
                                            v2c.clone(),
                                            label3.as_ref(),
                                            &cond3,
                                            &update3,
                                            &body3,
                                            &loop_env3,
                                            &names3,
                                        )? {
                                            IterOut::Next(v3) => engine.exec_for_loop(
                                                label3.as_ref(),
                                                &cond3,
                                                &update3,
                                                &body3,
                                                &loop_env3,
                                                &names3,
                                                v3,
                                            ),
                                            IterOut::Done(r) => r,
                                        }
                                    });
                                    return Err(Error::Suspend {
                                        awaited,
                                        cont: chain_outcome(cont, next2),
                                    });
                                }
                                other => other,
                            };
                            match engine.for_body_step(
                                bres,
                                v2.clone(),
                                label2.as_ref(),
                                &cond2,
                                &update2,
                                &body2,
                                &loop_env2,
                                &names2,
                            )? {
                                IterOut::Next(v3) => engine.exec_for_loop(
                                    label2.as_ref(),
                                    &cond2,
                                    &update2,
                                    &body2,
                                    &loop_env2,
                                    &names2,
                                    v3,
                                ),
                                IterOut::Done(r) => r,
                            }
                        });
                        return Err(Error::Suspend {
                            awaited,
                            cont: chain_stmts(cont, next),
                        });
                    }
                    Err(e) => return Err(e),
                }
            }
            // Fresh per-iteration environment snapshotting the loop bindings.
            let snap = self.for_snapshot(loop_env, per_iter_names);
            let body_env: &Rc<RefCell<Env>> = snap.as_ref().unwrap_or(loop_env);
            match self.exec_stmts(body, body_env) {
                Err(Error::Suspend { awaited, cont }) => {
                    let label2 = label.map(|l| l.clone());
                    let cond2 = cond.clone();
                    let update2 = update.clone();
                    let body2 = body.to_vec();
                    let loop_env2 = loop_env.clone();
                    let names2: Vec<Rc<str>> = per_iter_names.to_vec();
                    let v2 = v.clone();
                    let next: OutcomeNext = Rc::new(move |engine, outcome| {
                        match engine.for_body_step(
                            outcome,
                            v2.clone(),
                            label2.as_ref(),
                            &cond2,
                            &update2,
                            &body2,
                            &loop_env2,
                            &names2,
                        )? {
                            IterOut::Next(v3) => engine.exec_for_loop(
                                label2.as_ref(),
                                &cond2,
                                &update2,
                                &body2,
                                &loop_env2,
                                &names2,
                                v3,
                            ),
                            IterOut::Done(r) => r,
                        }
                    });
                    return Err(Error::Suspend {
                        awaited,
                        cont: chain_outcome(cont, next),
                    });
                }
                other => match self.for_body_step(
                    other,
                    v,
                    label,
                    cond,
                    update,
                    body,
                    loop_env,
                    per_iter_names,
                )? {
                    IterOut::Next(v2) => v = v2,
                    IterOut::Done(r) => return r,
                },
            }
        }
        Ok(v)
    }

    fn exec_while(
        &self,
        label: Option<&Rc<str>>,
        cond: &Expr,
        body: &[Stmt],
        env: &Rc<RefCell<Env>>,
    ) -> Result<Option<Value>> {
        self.exec_while_loop(label, cond, body, env, Some(Value::Undefined))
    }

    /// `while` iterations with accumulated completion `v`.
    fn exec_while_loop(
        &self,
        label: Option<&Rc<str>>,
        cond: &Expr,
        body: &[Stmt],
        env: &Rc<RefCell<Env>>,
        v: Option<Value>,
    ) -> Result<Option<Value>> {
        let mut v = v;
        loop {
            match self.eval_expr(cond, env) {
                Ok(cv) => {
                    if !cv.to_boolean() {
                        break;
                    }
                }
                Err(Error::Suspend { awaited, cont }) => {
                    let label2 = label.map(|l| l.clone());
                    let cond2 = cond.clone();
                    let body2 = body.to_vec();
                    let env2 = env.clone();
                    let v2 = v.clone();
                    let next: StmtsNext = Rc::new(move |engine, frag| {
                        if !frag.unwrap_or(Value::Undefined).to_boolean() {
                            return Ok(v2.clone());
                        }
                        match engine.exec_stmts(&body2, &env2) {
                            Err(Error::Suspend { awaited, cont }) => {
                                let label3 = label2.clone();
                                let cond3 = cond2.clone();
                                let body3 = body2.clone();
                                let env3 = env2.clone();
                                let v2c = v2.clone();
                                let next2: OutcomeNext = Rc::new(move |engine, outcome| {
                                    match engine.loop_body_step(outcome, v2c.clone(), label3.as_ref()) {
                                        LoopStep::Next(v3) => engine.exec_while_loop(
                                            label3.as_ref(),
                                            &cond3,
                                            &body3,
                                            &env3,
                                            v3,
                                        ),
                                        LoopStep::Done(r) => r,
                                    }
                                });
                                Err(Error::Suspend {
                                    awaited,
                                    cont: chain_outcome(cont, next2),
                                })
                            }
                            other => {
                                match engine.loop_body_step(other, v2.clone(), label2.as_ref()) {
                                    LoopStep::Next(v3) => engine.exec_while_loop(
                                        label2.as_ref(),
                                        &cond2,
                                        &body2,
                                        &env2,
                                        v3,
                                    ),
                                    LoopStep::Done(r) => r,
                                }
                            }
                        }
                    });
                    return Err(Error::Suspend {
                        awaited,
                        cont: chain_stmts(cont, next),
                    });
                }
                Err(e) => return Err(e),
            }
            match self.exec_stmts(body, env) {
                Err(Error::Suspend { awaited, cont }) => {
                    let label2 = label.map(|l| l.clone());
                    let cond2 = cond.clone();
                    let body2 = body.to_vec();
                    let env2 = env.clone();
                    let v2 = v.clone();
                    let next: OutcomeNext = Rc::new(move |engine, outcome| {
                        match engine.loop_body_step(outcome, v2.clone(), label2.as_ref()) {
                            LoopStep::Next(v3) => engine.exec_while_loop(
                                label2.as_ref(),
                                &cond2,
                                &body2,
                                &env2,
                                v3,
                            ),
                            LoopStep::Done(r) => r,
                        }
                    });
                    return Err(Error::Suspend {
                        awaited,
                        cont: chain_outcome(cont, next),
                    });
                }
                other => match self.loop_body_step(other, v, label) {
                    LoopStep::Next(v2) => v = v2,
                    LoopStep::Done(r) => return r,
                },
            }
        }
        Ok(v)
    }

    fn exec_do_while(
        &self,
        label: Option<&Rc<str>>,
        cond: &Expr,
        body: &[Stmt],
        env: &Rc<RefCell<Env>>,
    ) -> Result<Option<Value>> {
        self.exec_do_while_loop(label, cond, body, env, Some(Value::Undefined))
    }

    /// `do-while` iterations with accumulated completion `v`.
    fn exec_do_while_loop(
        &self,
        label: Option<&Rc<str>>,
        cond: &Expr,
        body: &[Stmt],
        env: &Rc<RefCell<Env>>,
        v: Option<Value>,
    ) -> Result<Option<Value>> {
        let mut v = v;
        loop {
            match self.exec_stmts(body, env) {
                Err(Error::Suspend { awaited, cont }) => {
                    let label2 = label.map(|l| l.clone());
                    let cond2 = cond.clone();
                    let body2 = body.to_vec();
                    let env2 = env.clone();
                    let v2 = v.clone();
                    let next: OutcomeNext = Rc::new(move |engine, outcome| {
                        let v3 = match engine.loop_body_step(outcome, v2.clone(), label2.as_ref()) {
                            LoopStep::Next(v) => v,
                            LoopStep::Done(r) => return r,
                        };
                        match engine.eval_expr(&cond2, &env2) {
                            Ok(cv) => {
                                if !cv.to_boolean() {
                                    return Ok(v3);
                                }
                            }
                            Err(Error::Suspend { awaited, cont }) => {
                                let label3 = label2.clone();
                                let cond3 = cond2.clone();
                                let body3 = body2.clone();
                                let env3 = env2.clone();
                                let v3c = v3.clone();
                                let next2: StmtsNext = Rc::new(move |engine, frag| {
                                    if !frag.unwrap_or(Value::Undefined).to_boolean() {
                                        return Ok(v3c.clone());
                                    }
                                    engine.exec_do_while_loop(
                                        label3.as_ref(),
                                        &cond3,
                                        &body3,
                                        &env3,
                                        v3c.clone(),
                                    )
                                });
                                return Err(Error::Suspend {
                                    awaited,
                                    cont: chain_stmts(cont, next2),
                                });
                            }
                            Err(e) => return Err(e),
                        }
                        engine.exec_do_while_loop(
                            label2.as_ref(),
                            &cond2,
                            &body2,
                            &env2,
                            v3,
                        )
                    });
                    return Err(Error::Suspend {
                        awaited,
                        cont: chain_outcome(cont, next),
                    });
                }
                other => match self.loop_body_step(other, v, label) {
                    LoopStep::Next(v2) => v = v2,
                    LoopStep::Done(r) => return r,
                },
            }
            match self.eval_expr(cond, env) {
                Ok(cv) => {
                    if !cv.to_boolean() {
                        break;
                    }
                }
                Err(Error::Suspend { awaited, cont }) => {
                    let label2 = label.map(|l| l.clone());
                    let cond2 = cond.clone();
                    let body2 = body.to_vec();
                    let env2 = env.clone();
                    let v2 = v.clone();
                    let next: StmtsNext = Rc::new(move |engine, frag| {
                        if !frag.unwrap_or(Value::Undefined).to_boolean() {
                            return Ok(v2.clone());
                        }
                        engine.exec_do_while_loop(
                            label2.as_ref(),
                            &cond2,
                            &body2,
                            &env2,
                            v2.clone(),
                        )
                    });
                    return Err(Error::Suspend {
                        awaited,
                        cont: chain_stmts(cont, next),
                    });
                }
                Err(e) => return Err(e),
            }
        }
        Ok(v)
    }

    // --- expression evaluation ---

    /// Syntactic suspension check used to select the allocation-free CPS fast
    /// path. Function bodies are intentionally not inspected: creating a
    /// function never executes its body.
    fn expr_may_suspend(expr: &Expr) -> bool {
        match expr {
            Expr::Await(_) | Expr::Yield { .. } => true,
            Expr::Member { obj, computed, .. } => {
                Self::expr_may_suspend(obj)
                    || computed.as_deref().is_some_and(Self::expr_may_suspend)
            }
            Expr::Call { callee, args, .. } | Expr::New { callee, args } => {
                Self::expr_may_suspend(callee)
                    || args.iter().any(|a| match a {
                        Arg::Expr(e) | Arg::Spread(e) => Self::expr_may_suspend(e),
                    })
            }
            Expr::Sequence(es) => es.iter().any(Self::expr_may_suspend),
            Expr::InstanceOf { left, right }
            | Expr::Binary { left, right, .. }
            | Expr::Logical { left, right, .. } => {
                Self::expr_may_suspend(left) || Self::expr_may_suspend(right)
            }
            Expr::Unary { operand, .. } | Expr::ToString(operand) => Self::expr_may_suspend(operand),
            Expr::Ternary { cond, then, else_ } => {
                Self::expr_may_suspend(cond)
                    || Self::expr_may_suspend(then)
                    || Self::expr_may_suspend(else_)
            }
            Expr::Assignment { target, value } => {
                let target_suspends = match target {
                    AssignTarget::Expr(e) => Self::expr_may_suspend(e),
                    AssignTarget::Pattern(_) => false,
                };
                target_suspends || Self::expr_may_suspend(value)
            }
            Expr::Array(es) => es.iter().any(|e| match e {
                ArrayElem::Expr(x) | ArrayElem::Spread(x) => Self::expr_may_suspend(x),
                ArrayElem::Elision => false,
            }),
            Expr::Object(ps) => ps.iter().any(|p| match p {
                Prop::Init { key, value } => {
                    matches!(key, PropKey::Computed(e) if Self::expr_may_suspend(e))
                        || Self::expr_may_suspend(value)
                }
                Prop::Method { key, .. } | Prop::Accessor { key, .. } => {
                    matches!(key, PropKey::Computed(e) if Self::expr_may_suspend(e))
                }
                Prop::Spread(e) => Self::expr_may_suspend(e),
            }),
            Expr::Tagged { tag, cooked, subs, .. } => {
                Self::expr_may_suspend(tag)
                    || cooked.iter().any(Self::expr_may_suspend)
                    || subs.iter().any(Self::expr_may_suspend)
            }
            Expr::Function { .. } | Expr::Arrow { .. } | Expr::Class(_) => false,
            Expr::Lit(_) | Expr::Ident(_) | Expr::This | Expr::Super => false,
        }
    }

    fn eval_args_sync(&self, args: &[Arg], env: &Rc<RefCell<Env>>) -> Result<Vec<Value>> {
        let mut argv = Vec::with_capacity(args.len());
        for arg in args {
            match arg {
                Arg::Expr(e) => argv.push(self.eval_expr_sync(e, env)?),
                Arg::Spread(e) => {
                    let value = self.eval_expr_sync(e, env)?;
                    if let Value::Array(a) = &value {
                        argv.extend(self.array_values_iter(a.clone()));
                    } else {
                        argv.extend(self.iterable_values(&value)?);
                    }
                }
            }
        }
        Ok(argv)
    }

    fn eval_call_sync(
        &self,
        callee: &Expr,
        args: &[Arg],
        env: &Rc<RefCell<Env>>,
        optional: bool,
    ) -> Result<Value> {
        if matches!(callee, Expr::Super) {
            let ctor = env.borrow().get("__super_ctor__").ok_or_else(|| {
                Error::Runtime(Value::String(Rc::from(
                    "SyntaxError: 'super' constructor call outside subclass",
                )))
            })?;
            let argv = self.eval_args_sync(args, env)?;
            let active_target = self.active_new_target.borrow().clone();
            if let Some(nt) = active_target {
                let v = self.construct_with_new_target(&ctor, &argv, &nt)?;
                env.borrow_mut().vars.insert(Rc::from("__this__"), v.clone());
                return Ok(v);
            }
            let this = env
                .borrow()
                .get("__this__")
                .unwrap_or_else(|| Value::Object(self.global_object.clone()));
            return self.call_function(&ctor, &this, &argv);
        }
        let (func, this) = match callee {
            Expr::Member { obj, prop, computed, optional: member_optional } => {
                let base = self.eval_expr_sync(obj, env)?;
                if (optional || *member_optional) && is_nullish(&base) {
                    return Ok(Value::Undefined);
                }
                let key = match computed {
                    Some(e) => self.to_property_key(&self.eval_expr_sync(e, env)?)?,
                    None => prop.clone(),
                };
                let f = self.get_member(&base, &key)?;
                (f, base)
            }
            _ => {
                let f = self.eval_expr_sync(callee, env)?;
                if optional && is_nullish(&f) {
                    return Ok(Value::Undefined);
                }
                (f, Value::Object(self.global_object.clone()))
            }
        };
        let argv = self.eval_args_sync(args, env)?;
        self.call_function(&func, &this, &argv)
    }

    /// Evaluate non-suspending expressions directly, without allocating CPS
    /// closures. The fallback is retained for expression forms whose normal
    /// evaluator already has no suspension points (function/class literals).
    fn eval_expr_sync(&self, expr: &Expr, env: &Rc<RefCell<Env>>) -> Result<Value> {
        match expr {
            Expr::Lit(l) => Ok(lit_to_value(l)),
            Expr::Ident(name) => env
                .borrow()
                .get(name)
                .or_else(|| self.global_object.borrow().props.get(name).map(|p| p.value.clone()))
                .ok_or_else(|| Error::Runtime(Value::String(Rc::from(format!("ReferenceError: {name} is not defined"))))),
            Expr::This => Ok(env.borrow().get("__this__").unwrap_or_else(|| Value::Object(self.global_object.clone()))),
            Expr::Member { obj, prop, computed, optional } => {
                let base = self.eval_expr_sync(obj, env)?;
                if *optional && is_nullish(&base) { return Ok(Value::Undefined); }
                let key = match computed {
                    Some(e) => self.to_property_key(&self.eval_expr_sync(e, env)?)?,
                    None => prop.clone(),
                };
                self.get_member(&base, &key)
            }
            Expr::Call { callee, args, optional } => self.eval_call_sync(callee, args, env, *optional),
            Expr::New { callee, args } => {
                let f = self.eval_expr_sync(callee, env)?;
                let argv = self.eval_args_sync(args, env)?;
                self.construct(&f, &argv)
            }
            Expr::Sequence(es) => {
                let mut value = Value::Undefined;
                for e in es { value = self.eval_expr_sync(e, env)?; }
                Ok(value)
            }
            Expr::InstanceOf { left, right } => {
                let l = self.eval_expr_sync(left, env)?;
                let r = self.eval_expr_sync(right, env)?;
                self.eval_instance_of(&l, &r)
            }
            Expr::Binary { op, left, right } => {
                let l = self.eval_expr_sync(left, env)?;
                let r = self.eval_expr_sync(right, env)?;
                self.eval_binary_value(*op, &l, &r)
            }
            Expr::Logical { op, left, right } => {
                let l = self.eval_expr_sync(left, env)?;
                let take_right = match op {
                    LogicalOp::And => l.to_boolean(),
                    LogicalOp::Or => !l.to_boolean(),
                    LogicalOp::Coalesce => is_nullish(&l),
                };
                if take_right { self.eval_expr_sync(right, env) } else { Ok(l) }
            }
            Expr::Unary { op, operand } => {
                let v = self.eval_expr_sync(operand, env)?;
                self.eval_unary_value(*op, &v, operand, env)
            }
            Expr::Ternary { cond, then, else_ } => {
                if self.eval_expr_sync(cond, env)?.to_boolean() { self.eval_expr_sync(then, env) } else { self.eval_expr_sync(else_, env) }
            }
            Expr::Assignment { target, value } => {
                let v = self.eval_expr_sync(value, env)?;
                self.assign_to_target(target, v, env)
            }
            Expr::ToString(e) => {
                let v = self.eval_expr_sync(e, env)?;
                Ok(Value::String(self.to_string_fallible(&v)?))
            }
            _ => self.eval_expr(expr, env),
        }
    }

    fn eval_expr(&self, expr: &Expr, env: &Rc<RefCell<Env>>) -> Result<Value> {
        match expr {
            Expr::Lit(l) => Ok(lit_to_value(l)),
            Expr::Ident(name) => {
                if let Some(v) = env.borrow().get(name) {
                    Ok(v)
                } else if let Some(p) = self.global_object.borrow().props.get(name) {
                    Ok(p.value.clone())
                } else {
                    Err(Error::Runtime(Value::String(Rc::from(format!(
                        "ReferenceError: {name} is not defined"
                    )))))
                }
            }
            Expr::This => Ok(env
                .borrow()
                .get("__this__")
                .unwrap_or_else(|| Value::Object(self.global_object.clone()))),
            Expr::Member { obj, prop, computed, optional } => {
                let prop_c = prop.clone();
                let computed_c = computed.clone();
                let optional_c = *optional;
                let env2 = env.clone();
                self.eval_expr_chain(obj, env, Rc::new(move |engine, base| {
                    if optional_c && is_nullish(&base) {
                        return Ok(Value::Undefined);
                    }
                    match computed_c.clone() {
                        Some(idx) => {
                            let base2 = base.clone();
                            let prop2 = prop_c.clone();
                            let _ = prop2;
                            engine.eval_expr_chain(&idx, &env2, Rc::new(move |engine, k| {
                                let key = engine.to_property_key(&k)?;
                                engine.get_member(&base2, &key)
                            }))
                        }
                        None => engine.get_member(&base, &prop_c),
                    }
                }))
            }
            Expr::Call { callee, args, optional } => {
                if !Self::expr_may_suspend(expr) {
                    return self.eval_call_sync(callee, args, env, *optional);
                }
                let callee_c = callee.clone();
                let args_c = args.clone();
                let optional_c = *optional;
                let env2 = env.clone();
                // Optional-chain short-circuit checks (each evaluates its base).
                let pre = match &*callee_c {
                    _ if !optional_c => None,
                    Expr::Member { obj, .. } => Some(obj.clone()),
                    Expr::Call { callee: inner, .. } => match &**inner {
                        Expr::Member { obj, .. } => Some(obj.clone()),
                        _ => None,
                    },
                    _ => None,
                };
                match pre {
                    Some(check) => {
                        let callee3 = callee_c.clone();
                        let args3 = args_c.clone();
                        let env3 = env2.clone();
                        self.eval_expr_chain(&check, env, Rc::new(move |engine, b| {
                            if is_nullish(&b) {
                                return Ok(Value::Undefined);
                            }
                            engine.eval_call_callee(&callee3, &args3, &env3)
                        }))
                    }
                    None => self.eval_call_callee(&callee_c, &args_c, env),
                }
            }
            Expr::Function { name, params, body } => {
                // A named function expression binds its own name in a fresh
                // immutable scope wrapping the closure.
                let closure = match name {
                    Some(n) => {
                        let scope = Rc::new(RefCell::new(Env::new_block(env.clone())));
                        self.register_env(&scope);
                        let f = self.make_function(
                            n.clone(),
                            params.clone(),
                            body.clone(),
                            scope.clone(),
                            None,
                        );
                        scope.borrow_mut().vars.insert(n.clone(), f.clone());
                        return Ok(f);
                    }
                    None => env.clone(),
                };
                Ok(self.make_function(
                    Rc::from(""),
                    params.clone(),
                    body.clone(),
                    closure,
                    None,
                ))
            }
            Expr::Arrow { params, body } => {
                let this = env
                    .borrow()
                    .get("__this__")
                    .unwrap_or_else(|| Value::Object(self.global_object.clone()));
                Ok(self.make_function(
                    Rc::from(""),
                    params.clone(),
                    body.clone(),
                    env.clone(),
                    Some(this),
                ))
            }
            Expr::New { callee, args } => {
                if !Self::expr_may_suspend(expr) {
                    let func = self.eval_expr_sync(callee, env)?;
                    let argv = self.eval_args_sync(args, env)?;
                    return self.construct(&func, &argv);
                }
                let args2 = args.clone();
                let env2 = env.clone();
                self.eval_expr_chain(callee, env, Rc::new(move |engine, func| {
                    engine.eval_args_from(
                        &args2,
                        &env2,
                        0,
                        Vec::new(),
                        Rc::new(move |engine, argv| engine.construct(&func, &argv)),
                    )
                }))
            }
            Expr::InstanceOf { left, right } => {
                let env2 = env.clone();
                let right2 = right.clone();
                self.eval_expr_chain(left, env, Rc::new(move |engine, obj| {
                    let obj2 = obj.clone();
                    engine.eval_expr_chain(&right2, &env2, Rc::new(move |engine, ctor| {
                        engine.eval_instance_of(&obj2, &ctor)
                    }))
                }))
            }
            Expr::Binary { op, left, right } => {
                let op_c = *op;
                let right_c = right.clone();
                let env2 = env.clone();
                self.eval_expr_chain(left, env, Rc::new(move |engine, l| {
                    let l2 = l.clone();
                    engine.eval_expr_chain(&right_c, &env2, Rc::new(move |engine, r| {
                        engine.eval_binary_value(op_c, &l2, &r)
                    }))
                }))
            }
            Expr::Logical { op, left, right } => {
                let op_c = *op;
                let right_c = right.clone();
                let env2 = env.clone();
                self.eval_expr_chain(left, env, Rc::new(move |engine, l| {
                    match op_c {
                        LogicalOp::And => {
                            if l.to_boolean() {
                                engine.eval_expr(&right_c, &env2)
                            } else {
                                Ok(l.clone())
                            }
                        }
                        LogicalOp::Or => {
                            if l.to_boolean() {
                                Ok(l.clone())
                            } else {
                                engine.eval_expr(&right_c, &env2)
                            }
                        }
                        LogicalOp::Coalesce => {
                            if is_nullish(&l) {
                                engine.eval_expr(&right_c, &env2)
                            } else {
                                Ok(l.clone())
                            }
                        }
                    }
                }))
            }
            Expr::Unary { op, operand } => {
                // `typeof undeclaredVar` short-circuits to `"undefined"`
                // without evaluating (no `ReferenceError`).
                if matches!(op, UnaryOp::Typeof) {
                    if let Expr::Ident(name) = &**operand {
                        let defined = env.borrow().get(name).is_some()
                            || self.global_object.borrow().props.contains_key(name);
                        if !defined {
                            return Ok(Value::String(Rc::from("undefined")));
                        }
                    }
                }
                let op_c = *op;
                let operand_c = operand.clone();
                let env2 = env.clone();
                self.eval_expr_chain(operand, env, Rc::new(move |engine, v| {
                    engine.eval_unary_value(op_c, &v, &operand_c, &env2)
                }))
            }
            Expr::Ternary { cond, then, else_ } => {
                let then_c = then.clone();
                let else_c = else_.clone();
                let env2 = env.clone();
                self.eval_expr_chain(cond, env, Rc::new(move |engine, c| {
                    if c.to_boolean() {
                        engine.eval_expr(&then_c, &env2)
                    } else {
                        engine.eval_expr(&else_c, &env2)
                    }
                }))
            }
            Expr::Assignment { target, value } => {
                let target_c = target.clone();
                let env2 = env.clone();
                self.eval_expr_chain(value, env, Rc::new(move |engine, v| {
                    engine.assign_to_target(&target_c, v, &env2)
                }))
            }
            Expr::Array(elems) => {
                let elems2 = elems.clone();
                self.eval_array_elems(&elems2, env, 0, Vec::new())
            }
            Expr::Object(props) => {
                let obj = self.new_object();
                let props2 = props.clone();
                self.eval_object_props(&props2, &obj, env, 0)?;
                Ok(Value::Object(obj))
            }
            Expr::Class(c) => self.eval_class(c, env),
            Expr::Super => Ok(env
                .borrow()
                .get("__super__")
                .unwrap_or(Value::Undefined)),
            Expr::Tagged {
                tag,
                cooked,
                raws: _,
                subs,
            } => {
                let cooked2 = cooked.clone();
                let subs2 = subs.clone();
                let env2 = env.clone();
                self.eval_expr_chain(tag, env, Rc::new(move |engine, func| {
                    let func2 = func.clone();
                    let cooked3 = cooked2.clone();
                    let subs3 = subs2.clone();
                    let env3 = env2.clone();
                    let env3c = env3.clone();
                    engine.eval_expr_list_from(
                        &cooked3,
                        &env3,
                        0,
                        Vec::new(),
                        Rc::new(move |engine, strings| {
                            let cooked_arr = Value::Array(engine.new_array(strings));
                            let func4 = func2.clone();
                            let subs4 = subs3.clone();
                            let env4 = env3c.clone();
                            engine.eval_expr_list_from(
                                &subs4,
                                &env4,
                                0,
                                Vec::new(),
                                Rc::new(move |engine, subs| {
                                    let mut argv = vec![cooked_arr.clone()];
                                    argv.extend(subs);
                                    engine.call_function(&func4, &Value::Undefined, &argv)
                                }),
                            )
                        }),
                    )
                }))
            }
            Expr::Sequence(exprs) => {
                let exprs2 = exprs.clone();
                self.eval_expr_list_from(
                    &exprs2,
                    env,
                    0,
                    Vec::new(),
                    Rc::new(move |_, mut vals| Ok(vals.pop().unwrap_or(Value::Undefined))),
                )
            }
            Expr::ToString(e) => self.eval_expr_chain(e, env, Rc::new(move |engine, v| {
                Ok(Value::String(engine.to_string_fallible(&v)?))
            })),
            Expr::Await(e) => {
                // `await` is only valid inside an `async` function (tracked
                // lexically). Outside, report unimplemented rather than a
                // `SyntaxError`: top-level `await` (empty stack) and `await`
                // in a sync function are both engine limitations.
                let stack = self.func_async_stack.borrow();
                let in_async = stack.last().copied().unwrap_or(false);
                let enclosed = !stack.is_empty();
                drop(stack);
                if !in_async {
                    if !enclosed {
                        return Err(Error::Unimplemented(
                            "top-level await is not implemented".to_string(),
                        ));
                    }
                    return Err(Error::Unimplemented(
                        "await is only valid in async functions".to_string(),
                    ));
                }
                fn await_cont() -> SuspendCont {
                    Rc::new(move |_, s: Settlement| match s {
                        Settlement::Fulfilled(v) => Ok(Some(v)),
                        Settlement::Rejected(e) => Err(Error::Runtime(e)),
                    })
                }
                match self.eval_expr(e, env) {
                    Ok(operand) => Err(Error::Suspend {
                        awaited: operand,
                        cont: await_cont(),
                    }),
                    Err(Error::Suspend { awaited, cont }) => {
                        let inner: ValueNext = Rc::new(move |_, v| {
                            Err(Error::Suspend {
                                awaited: v,
                                cont: await_cont(),
                            })
                        });
                        Err(Error::Suspend {
                            awaited,
                            cont: chain_value(cont, inner),
                        })
                    }
                    Err(e) => Err(e),
                }
            }
            Expr::Yield { expr, delegated: _ } => {
                let v = match expr {
                    Some(e) => self.eval_expr(e, env)?,
                    None => Value::Undefined,
                };
                // Inside an eagerly-executed generator body, `yield` records
                // the value for the generator object instead of returning.
                if let Some(sink) = self.yield_sink.borrow().clone() {
                    sink.borrow_mut().push(v);
                    return Ok(Value::Undefined);
                }
                Ok(v)
            }
        }
    }

    /// Evaluate array literal elements from `idx`, accumulating into `v`.
    fn eval_array_elems(
        &self,
        elems: &[ArrayElem],
        env: &Rc<RefCell<Env>>,
        idx: usize,
        v: Vec<Value>,
    ) -> Result<Value> {
        let mut v = v;
        for (i, e) in elems.iter().enumerate().skip(idx) {
            match e {
                ArrayElem::Expr(ex) => match self.eval_expr(ex, env) {
                    Ok(val) => v.push(val),
                    Err(Error::Suspend { awaited, cont }) => {
                        let elems2 = elems.to_vec();
                        let env2 = env.clone();
                        let v0 = v.clone();
                        let inner: ValueNext = Rc::new(move |engine, val| {
                            let mut v2 = v0.clone();
                            v2.push(val);
                            engine.eval_array_elems(&elems2, &env2, i + 1, v2)
                        });
                        return Err(Error::Suspend {
                            awaited,
                            cont: chain_value(cont, inner),
                        });
                    }
                    Err(e) => return Err(e),
                },
                ArrayElem::Spread(s) => match self.eval_expr(s, env) {
                    Ok(sv) => {
                        for item in self.iterable_values(&sv)? {
                            v.push(item);
                        }
                    }
                    Err(Error::Suspend { awaited, cont }) => {
                        let elems2 = elems.to_vec();
                        let env2 = env.clone();
                        let v0 = v.clone();
                        let inner: ValueNext = Rc::new(move |engine, sv| {
                            let mut v2 = v0.clone();
                            for item in engine.iterable_values(&sv)? {
                                v2.push(item);
                            }
                            engine.eval_array_elems(&elems2, &env2, i + 1, v2)
                        });
                        return Err(Error::Suspend {
                            awaited,
                            cont: chain_value(cont, inner),
                        });
                    }
                    Err(e) => return Err(e),
                },
                ArrayElem::Elision => v.push(Value::Undefined),
            }
        }
        Ok(Value::Array(self.new_array(v)))
    }

    /// Evaluate object-literal properties from `idx` into `obj`.
    fn eval_object_props(
        &self,
        props: &[Prop],
        obj: &Rc<RefCell<Object>>,
        env: &Rc<RefCell<Env>>,
        idx: usize,
    ) -> Result<()> {
        for (i, p) in props.iter().enumerate().skip(idx) {
            match p {
                Prop::Init { key, value } => {
                    let v = match self.eval_expr(value, env) {
                        Ok(v) => v,
                        Err(Error::Suspend { awaited, cont }) => {
                            let props2 = props.to_vec();
                            let obj2 = obj.clone();
                            let env2 = env.clone();
                            let key2 = key.clone();
                            let inner: ValueNext = Rc::new(move |engine, v| {
                                let k = engine.eval_prop_key(&key2, &env2)?;
                                obj2.borrow_mut().props.insert(
                                    Rc::from(k.as_ref()),
                                    Property::new(v),
                                );
                                engine.eval_object_props(&props2, &obj2, &env2, i + 1)?;
                                Ok(Value::Undefined)
                            });
                            return Err(Error::Suspend {
                                awaited,
                                cont: chain_value(cont, inner),
                            });
                        }
                        Err(e) => return Err(e),
                    };
                    // The key may itself suspend; chain the store + rest.
                    match self.eval_prop_key(key, env) {
                        Ok(k) => {
                            obj.borrow_mut()
                                .props
                                .insert(Rc::from(k.as_ref()), Property::new(v));
                        }
                        Err(Error::Suspend { awaited, cont }) => {
                            let props2 = props.to_vec();
                            let obj2 = obj.clone();
                            let env2 = env.clone();
                            let v2 = v.clone();
                            let inner: ValueNext = Rc::new(move |engine, k| {
                                let key = engine.to_property_key(&k)?;
                                obj2.borrow_mut().props.insert(
                                    Rc::from(key.as_ref()),
                                    Property::new(v2.clone()),
                                );
                                engine.eval_object_props(&props2, &obj2, &env2, i + 1)?;
                                Ok(Value::Undefined)
                            });
                            return Err(Error::Suspend {
                                awaited,
                                cont: chain_value(cont, inner),
                            });
                        }
                        Err(e) => return Err(e),
                    }
                }
                Prop::Method { key, params, body } => {
                    let k = match self.eval_prop_key(key, env) {
                        Ok(k) => k,
                        Err(Error::Suspend { awaited, cont }) => {
                            let props2 = props.to_vec();
                            let obj2 = obj.clone();
                            let env2 = env.clone();
                            let params2 = params.clone();
                            let body2 = body.clone();
                            let inner: ValueNext = Rc::new(move |engine, k| {
                                let key = engine.to_property_key(&k)?;
                                let f = engine.make_function(
                                    key.clone(),
                                    params2.clone(),
                                    body2.clone(),
                                    env2.clone(),
                                    None,
                                );
                                obj2.borrow_mut().props.insert(
                                    Rc::from(key.as_ref()),
                                    Property::new(f),
                                );
                                engine.eval_object_props(&props2, &obj2, &env2, i + 1)?;
                                Ok(Value::Undefined)
                            });
                            return Err(Error::Suspend {
                                awaited,
                                cont: chain_value(cont, inner),
                            });
                        }
                        Err(e) => return Err(e),
                    };
                    let f = self.make_function(
                        k.clone(),
                        params.clone(),
                        body.clone(),
                        env.clone(),
                        None,
                    );
                    obj.borrow_mut()
                        .props
                        .insert(Rc::from(k.as_ref()), Property::new(f));
                }
                Prop::Accessor { get, key, params, body } => {
                    let k = match self.eval_prop_key(key, env) {
                        Ok(k) => k,
                        Err(Error::Suspend { awaited, cont }) => {
                            let props2 = props.to_vec();
                            let obj2 = obj.clone();
                            let env2 = env.clone();
                            let params2 = params.clone();
                            let body2 = body.clone();
                            let get2 = *get;
                            let inner: ValueNext = Rc::new(move |engine, k| {
                                let key = engine.to_property_key(&k)?;
                                let f = engine.make_function(
                                    key.clone(),
                                    params2.clone(),
                                    body2.clone(),
                                    env2.clone(),
                                    None,
                                );
                                if get2 {
                                    engine.define_accessor(&obj2, &key, Some(f), None);
                                } else {
                                    engine.define_accessor(&obj2, &key, None, Some(f));
                                }
                                engine.eval_object_props(&props2, &obj2, &env2, i + 1)?;
                                Ok(Value::Undefined)
                            });
                            return Err(Error::Suspend {
                                awaited,
                                cont: chain_value(cont, inner),
                            });
                        }
                        Err(e) => return Err(e),
                    };
                    let f = self.make_function(
                        k.clone(),
                        params.clone(),
                        body.clone(),
                        env.clone(),
                        None,
                    );
                    if *get {
                        self.define_accessor(obj, &k, Some(f), None);
                    } else {
                        self.define_accessor(obj, &k, None, Some(f));
                    }
                }
                Prop::Spread(s) => {
                    let sv = match self.eval_expr(s, env) {
                        Ok(v) => v,
                        Err(Error::Suspend { awaited, cont }) => {
                            let props2 = props.to_vec();
                            let obj2 = obj.clone();
                            let env2 = env.clone();
                            let inner: ValueNext = Rc::new(move |engine, sv| {
                                let src = engine.to_object(&sv)?;
                                {
                                    let mut guard = obj2.borrow_mut();
                                    for (k, p) in src.borrow().props.iter() {
                                        if p.enumerable && k.as_ref() != "__value__" {
                                            guard.props.insert(
                                                k.clone(),
                                                Property::new(p.value.clone()),
                                            );
                                        }
                                    }
                                }
                                engine.eval_object_props(&props2, &obj2, &env2, i + 1)?;
                                Ok(Value::Undefined)
                            });
                            return Err(Error::Suspend {
                                awaited,
                                cont: chain_value(cont, inner),
                            });
                        }
                        Err(e) => return Err(e),
                    };
                    let src = self.to_object(&sv)?;
                    let mut guard = obj.borrow_mut();
                    for (k, p) in src.borrow().props.iter() {
                        if p.enumerable && k.as_ref() != "__value__" {
                            guard
                                .props
                                .insert(k.clone(), Property::new(p.value.clone()));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Evaluate a `class` expression into a constructor function value.
    /// Suspension-aware: `await` in the heritage expression, computed keys
    /// and static field initializers suspends with the rest of the class
    /// construction attached as a continuation.
    fn eval_class(&self, class: &Class, env: &Rc<RefCell<Env>>) -> Result<Value> {
        match &class.extends {
            Some(e) => {
                let class2 = class.clone();
                let env2 = env.clone();
                self.eval_expr_chain(e, env, Rc::new(move |engine, parent| {
                    engine.eval_class_with_parent(&class2, Some(parent), &env2)
                }))
            }
            None => self.eval_class_with_parent(class, None, env),
        }
    }

    /// Class construction with an already-evaluated heritage value.
    fn eval_class_with_parent(
        &self,
        class: &Class,
        parent: Option<Value>,
        env: &Rc<RefCell<Env>>,
    ) -> Result<Value> {
        let parent_proto = match &parent {
            Some(p) => self.proto_of(p),
            None => None,
        };
        let base_proto = parent_proto.unwrap_or_else(|| self.object_prototype.clone());
        let proto = self.make_object(base_proto.clone());

        let mut ctor_params: Vec<Pattern> = Vec::new();
        let mut ctor_body: Vec<Stmt> = Vec::new();
        let mut has_ctor = false;
        let mut instance_members: Vec<ClassElem> = Vec::new();
        let mut static_members: Vec<ClassElem> = Vec::new();
        let mut instance_fields: Vec<(PropKey, Option<Expr>)> = Vec::new();
        let mut static_fields: Vec<(PropKey, Option<Expr>)> = Vec::new();

        for el in &class.elements {
            match el {
                ClassElem::Constructor { params, body } => {
                    ctor_params = params.clone();
                    ctor_body = body.clone();
                    has_ctor = true;
                }
                ClassElem::Method { is_static, .. } => {
                    if *is_static {
                        static_members.push(el.clone());
                    } else {
                        instance_members.push(el.clone());
                    }
                }
                ClassElem::Field { is_static, key, init } => {
                    let i = init.clone();
                    if *is_static {
                        static_fields.push((key.clone(), i));
                    } else {
                        instance_fields.push((key.clone(), i));
                    }
                }
                ClassElem::AccessorProp { is_static, key } => {
                    let k = key.clone();
                    if *is_static {
                        static_fields.push((k, None));
                    } else {
                        instance_fields.push((k, None));
                    }
                }
            }
        }

        // A derived class without an explicit constructor gets a default one
        // that forwards to `super(...)`. Our AST has no rest parameters, so
        // forward the `arguments` object with spread instead.
        if parent.is_some() && !has_ctor {
            ctor_body = vec![Stmt::Expr(Expr::Call {
                callee: Box::new(Expr::Super),
                args: vec![Arg::Spread(Expr::Ident(Rc::from("arguments")))],
                optional: false,
            })];
        }

        let name = class.name.clone();
        let env2 = env.clone();
        let base_proto2 = base_proto.clone();
        let proto2 = proto.clone();
        let parent2 = parent.clone();
        self.eval_class_field_keys(
            &instance_fields,
            env,
            0,
            Vec::new(),
            Rc::new(move |engine, inst_keys| {
                engine.eval_class_finish(
                    name.clone(),
                    parent2.clone(),
                    base_proto2.clone(),
                    proto2.clone(),
                    ctor_params.clone(),
                    ctor_body.clone(),
                    inst_keys,
                    instance_members.clone(),
                    static_members.clone(),
                    static_fields.clone(),
                    &env2,
                )
            }),
        )
    }

    /// Evaluate instance-field computed keys from `idx` (initializers stay as
    /// AST for constructor injection), then continue with `next`.
    fn eval_class_field_keys(
        &self,
        fields: &[(PropKey, Option<Expr>)],
        env: &Rc<RefCell<Env>>,
        idx: usize,
        acc: Vec<(Rc<str>, Option<Expr>)>,
        next: ClassKeysNext,
    ) -> Result<Value> {
        let mut acc = acc;
        for (i, (key, init)) in fields.iter().enumerate().skip(idx) {
            let k = match key {
                PropKey::Computed(e) => match self.eval_expr(e, env) {
                    Ok(v) => self.to_property_key(&v)?,
                    Err(Error::Suspend { awaited, cont }) => {
                        let fields2 = fields.to_vec();
                        let env2 = env.clone();
                        let acc2 = acc.clone();
                        let next2 = next.clone();
                        let inner: ValueNext = Rc::new(move |engine, v| {
                            let k = engine.to_property_key(&v)?;
                            let mut acc3 = acc2.clone();
                            acc3.push((k, fields2[i].1.clone()));
                            engine.eval_class_field_keys(&fields2, &env2, i + 1, acc3, next2.clone())
                        });
                        return Err(Error::Suspend {
                            awaited,
                            cont: chain_value(cont, inner),
                        });
                    }
                    Err(e) => return Err(e),
                },
                PropKey::Ident(s) | PropKey::Str(s) => s.clone(),
            };
            acc.push((k, init.clone()));
        }
        next(self, acc)
    }

    /// Install one instance method value (shared by sync and resume paths).
    fn install_class_method(
        &self,
        el: &ClassElem,
        k: &Rc<str>,
        proto: &Rc<RefCell<Object>>,
        base_proto: &Rc<RefCell<Object>>,
        env: &Rc<RefCell<Env>>,
    ) {
        if let ClassElem::Method {
            get, params, body, ..
        } = el
        {
            let f =
                self.make_function(k.clone(), params.clone(), body.clone(), env.clone(), None);
            if let Value::Function(fr) = &f {
                fr.borrow_mut().super_proto = Some(base_proto.clone());
            }
            if *get {
                self.define_accessor(proto, k, Some(f), None);
            } else {
                proto
                    .borrow_mut()
                    .props
                    .insert(Rc::from(k.as_ref()), Property::new(f));
            }
        }
    }

    /// Install instance methods from `idx` onto `proto`.
    fn eval_class_inst_methods(
        &self,
        members: &[ClassElem],
        proto: &Rc<RefCell<Object>>,
        base_proto: &Rc<RefCell<Object>>,
        env: &Rc<RefCell<Env>>,
        idx: usize,
    ) -> Result<()> {
        for (i, el) in members.iter().enumerate().skip(idx) {
            if let ClassElem::Method { key, .. } = el {
                match self.eval_prop_key(key, env) {
                    Ok(k) => self.install_class_method(el, &k, proto, base_proto, env),
                    Err(Error::Suspend { awaited, cont }) => {
                        let members2 = members.to_vec();
                        let proto2 = proto.clone();
                        let base2 = base_proto.clone();
                        let env2 = env.clone();
                        let inner: ValueNext = Rc::new(move |engine, kv| {
                            let k: Rc<str> = kv.to_string();
                            engine.install_class_method(
                                &members2[i],
                                &k,
                                &proto2,
                                &base2,
                                &env2,
                            );
                            engine.eval_class_inst_methods(
                                &members2, &proto2, &base2, &env2, i + 1,
                            )?;
                            Ok(Value::Undefined)
                        });
                        return Err(Error::Suspend {
                            awaited,
                            cont: chain_value(cont, inner),
                        });
                    }
                    Err(e) => return Err(e),
                }
            }
        }
        Ok(())
    }

    /// Evaluate static field keys/initializers from `idx`, installing on `ctor`.
    fn eval_class_static_fields(
        &self,
        fields: &[(PropKey, Option<Expr>)],
        ctor: &Rc<RefCell<Function>>,
        env: &Rc<RefCell<Env>>,
        idx: usize,
    ) -> Result<()> {
        for (i, (key, init)) in fields.iter().enumerate().skip(idx) {
            let k = match key {
                PropKey::Computed(e) => match self.eval_expr(e, env) {
                    Ok(v) => self.to_property_key(&v)?,
                    Err(Error::Suspend { awaited, cont }) => {
                        let fields2 = fields.to_vec();
                        let ctor2 = ctor.clone();
                        let env2 = env.clone();
                        let inner: ValueNext = Rc::new(move |engine, v| {
                            let k = engine.to_property_key(&v)?;
                            let value = match &fields2[i].1 {
                                Some(e) => engine.eval_expr(e, &env2)?,
                                None => Value::Undefined,
                            };
                            ctor2
                                .borrow_mut()
                                .props
                                .insert(Rc::from(k.as_ref()), Property::new(value));
                            engine.eval_class_static_fields(&fields2, &ctor2, &env2, i + 1)?;
                            Ok(Value::Undefined)
                        });
                        return Err(Error::Suspend {
                            awaited,
                            cont: chain_value(cont, inner),
                        });
                    }
                    Err(e) => return Err(e),
                },
                PropKey::Ident(s) | PropKey::Str(s) => s.clone(),
            };
            let value = match init {
                Some(e) => match self.eval_expr(e, env) {
                    Ok(v) => v,
                    Err(Error::Suspend { awaited, cont }) => {
                        let fields2 = fields.to_vec();
                        let ctor2 = ctor.clone();
                        let env2 = env.clone();
                        let k2 = k.clone();
                        let inner: ValueNext = Rc::new(move |engine, v| {
                            ctor2
                                .borrow_mut()
                                .props
                                .insert(Rc::from(k2.as_ref()), Property::new(v));
                            engine.eval_class_static_fields(&fields2, &ctor2, &env2, i + 1)?;
                            Ok(Value::Undefined)
                        });
                        return Err(Error::Suspend {
                            awaited,
                            cont: chain_value(cont, inner),
                        });
                    }
                    Err(e) => return Err(e),
                },
                None => Value::Undefined,
            };
            ctor.borrow_mut()
                .props
                .insert(Rc::from(k.as_ref()), Property::new(value));
        }
        Ok(())
    }

    /// Install static methods from `idx` onto `ctor`.
    fn eval_class_static_methods(
        &self,
        members: &[ClassElem],
        ctor: &Rc<RefCell<Function>>,
        env: &Rc<RefCell<Env>>,
        idx: usize,
    ) -> Result<()> {
        for (i, el) in members.iter().enumerate().skip(idx) {
            if let ClassElem::Method {
                get,
                key,
                params,
                body,
                ..
            } = el
            {
                let k = match self.eval_prop_key(key, env) {
                    Ok(k) => k,
                    Err(Error::Suspend { awaited, cont }) => {
                        let members2 = members.to_vec();
                        let ctor2 = ctor.clone();
                        let env2 = env.clone();
                        let inner: ValueNext = Rc::new(move |engine, kv| {
                            let k: Rc<str> = kv.to_string();
                            if let ClassElem::Method {
                                get,
                                params,
                                body,
                                ..
                            } = &members2[i]
                            {
                                let f = engine.make_function(
                                    k.clone(),
                                    params.clone(),
                                    body.clone(),
                                    env2.clone(),
                                    None,
                                );
                                if *get {
                                    engine.define_accessor_ctor(&ctor2, &k, Some(f), None);
                                } else {
                                    ctor2
                                        .borrow_mut()
                                        .props
                                        .insert(Rc::from(k.as_ref()), Property::new(f));
                                }
                            }
                            engine.eval_class_static_methods(&members2, &ctor2, &env2, i + 1)?;
                            Ok(Value::Undefined)
                        });
                        return Err(Error::Suspend {
                            awaited,
                            cont: chain_value(cont, inner),
                        });
                    }
                    Err(e) => return Err(e),
                };
                let f =
                    self.make_function(k.clone(), params.clone(), body.clone(), env.clone(), None);
                if *get {
                    self.define_accessor_ctor(ctor, &k, Some(f), None);
                } else {
                    ctor.borrow_mut()
                        .props
                        .insert(Rc::from(k.as_ref()), Property::new(f));
                }
            }
        }
        Ok(())
    }

    /// Finish class construction: inject instance fields into the constructor
    /// body, build the constructor, then install methods and static fields.
    #[allow(clippy::too_many_arguments)]
    fn eval_class_finish(
        &self,
        name: Option<Rc<str>>,
        parent: Option<Value>,
        base_proto: Rc<RefCell<Object>>,
        proto: Rc<RefCell<Object>>,
        ctor_params: Vec<Pattern>,
        ctor_body: Vec<Stmt>,
        inst_keys: Vec<(Rc<str>, Option<Expr>)>,
        instance_members: Vec<ClassElem>,
        static_members: Vec<ClassElem>,
        static_fields: Vec<(PropKey, Option<Expr>)>,
        env: &Rc<RefCell<Env>>,
    ) -> Result<Value> {
        // Inject instance field initializers at the top of the constructor body.
        let mut full_ctor_body = Vec::new();
        for (k, init) in &inst_keys {
            let value_expr = match init {
                Some(e) => e.clone(),
                None => Expr::Lit(Lit::Undefined),
            };
            full_ctor_body.push(Stmt::Expr(Expr::Assignment {
                target: AssignTarget::Expr(Box::new(Expr::Member {
                    obj: Box::new(Expr::This),
                    prop: k.clone(),
                    computed: None,
                    optional: false,
                })),
                value: Box::new(value_expr),
            }));
        }
        full_ctor_body.extend(ctor_body);

        let ctor = Rc::new(RefCell::new(Function {
            name: name.clone().unwrap_or_else(|| Rc::from("")),
            params: ctor_params,
            body: Rc::new(full_ctor_body),
            closure: env.clone(),
            props: crate::value::new_props(),
            proto: Some(self.function_prototype.clone()),
            super_proto: Some(base_proto.clone()),
            super_ctor: parent.clone(),
            this_capture: None,
            generator: false,
            is_async: false,
            is_derived: parent.is_some(),
            realm: None,
        }));
        proto.borrow_mut().props.insert(
            Rc::from("constructor"),
            Property::new(Value::Function(ctor.clone())),
        );
        ctor.borrow_mut().props.insert(
            Rc::from("prototype"),
            Property::new(Value::Object(proto.clone())),
        );
        ctor.borrow_mut().props.insert(
            Rc::from("name"),
            Property::new(Value::String(
                name.clone().unwrap_or_else(|| Rc::from("")),
            )),
        );
        self.register_function(&ctor);

        self.eval_class_inst_methods(&instance_members, &proto, &base_proto, env, 0)?;
        // Static fields/members live on the constructor itself.
        self.eval_class_static_fields(&static_fields, &ctor, env, 0)?;
        self.eval_class_static_methods(&static_members, &ctor, env, 0)?;
        Ok(Value::Function(ctor))
    }


    /// Define a getter/setter property on `obj`, merging with any existing
    /// accessor already present under the same key.
    fn define_accessor(
        &self,
        obj: &Rc<RefCell<Object>>,
        key: &str,
        get: Option<Value>,
        set: Option<Value>,
    ) {
        let (g, s) = match obj.borrow().props.get(key).cloned() {
            Some(p) if p.is_accessor() => (p.get.clone().or(get), p.set.clone().or(set)),
            _ => (get, set),
        };
        obj.borrow_mut()
            .props
            .insert(Rc::from(key), Property::accessor(g, s));
    }

    /// Define a getter/setter property on a constructor function, merging
    /// with any existing accessor already present under the same key.
    fn define_accessor_ctor(
        &self,
        ctor: &Rc<RefCell<Function>>,
        key: &str,
        get: Option<Value>,
        set: Option<Value>,
    ) {
        let (g, s) = match ctor.borrow().props.get(key).cloned() {
            Some(p) if p.is_accessor() => (p.get.clone().or(get), p.set.clone().or(set)),
            _ => (get, set),
        };
        ctor.borrow_mut()
            .props
            .insert(Rc::from(key), Property::accessor(g, s));
    }

    fn eval_prop_key(&self, key: &PropKey, env: &Rc<RefCell<Env>>) -> Result<Rc<str>> {
        Ok(match key {
            PropKey::Ident(s) | PropKey::Str(s) => s.clone(),
            PropKey::Computed(e) => match self.eval_expr(e, env) {
                Ok(v) => self.to_property_key(&v)?,
                Err(Error::Suspend { awaited, cont }) => {
                    let inner: ValueNext = Rc::new(move |engine, k| {
                        engine
                            .to_property_key(&k)
                            .map(|s| Value::String(s))
                    });
                    return Err(Error::Suspend {
                        awaited,
                        cont: chain_value(cont, inner),
                    });
                }
                Err(e) => return Err(e),
            },
        })
    }

    /// `instanceof` on already-evaluated operands (pure prototype walk).
    fn eval_instance_of(&self, obj: &Value, ctor: &Value) -> Result<Value> {
        let proto = self.proto_of(ctor);
        let proto = match proto {
            Some(p) => p,
            None => return Ok(Value::Boolean(false)),
        };
        let mut cur: Option<Rc<RefCell<Object>>> = match obj {
            Value::Object(o) => Some(o.clone()),
            Value::Array(a) => a.borrow().proto.clone(),
            Value::Function(f) => f.borrow().proto.clone(),
            Value::NativeFunction(nf) => nf.borrow().proto.clone(),
            Value::Map(_) => Some(self.map_prototype.clone()),
            Value::Set(_) => Some(self.set_prototype.clone()),
            Value::WeakMap(_) => Some(self.weakmap_prototype.clone()),
            Value::WeakSet(_) => Some(self.weakset_prototype.clone()),
            Value::Promise(p) => p
                .borrow()
                .proto
                .clone()
                .or_else(|| Some(self.promise_prototype.clone())),
            Value::Regex(_) => Some(self.regexp_prototype.clone()),
            _ => None,
        };
        while let Some(o) = cur {
            if Rc::ptr_eq(&o, &proto) {
                return Ok(Value::Boolean(true));
            }
            cur = o.borrow().proto.clone();
        }
        Ok(Value::Boolean(false))
    }

    /// Apply a unary operator to an already-evaluated value. The inc/dec and
    /// `delete` paths re-enter member evaluation (suspension-aware).
    fn eval_unary_value(
        &self,
        op: UnaryOp,
        v: &Value,
        operand: &Expr,
        env: &Rc<RefCell<Env>>,
    ) -> Result<Value> {
        match op {
            UnaryOp::Neg => {
                if Self::bigint_direct(v) {
                    let n = self.to_numeric(v)?;
                    return match n {
                        Value::BigInt(s) => {
                            let b = self.big_from_value(&Value::BigInt(s))?;
                            Ok(self.big_to_value(b.negated()))
                        }
                        Value::Number(x) => Ok(Value::Number(-x)),
                        _ => Ok(Value::Number(f64::NAN)),
                    };
                }
                Ok(Value::Number(-v.to_number()))
            }
            UnaryOp::Plus => {
                if Self::bigint_direct(v) {
                    return Err(Error::Runtime(self.make_type_error(
                        "Cannot convert a BigInt to a number",
                    )));
                }
                Ok(Value::Number(v.to_number()))
            }
            UnaryOp::Not => Ok(Value::Boolean(!v.to_boolean())),
            UnaryOp::Void => Ok(Value::Undefined),
            UnaryOp::Typeof => {
                if let Expr::Ident(name) = operand {
                    let defined = env.borrow().get(name).is_some()
                        || self.global_object.borrow().props.contains_key(name);
                    if !defined {
                        return Ok(Value::String(Rc::from("undefined")));
                    }
                }
                Ok(Value::String(Rc::from(v.type_of())))
            }
            UnaryOp::BitNot => {
                if Self::bigint_direct(v) {
                    let n = self.to_numeric(v)?;
                    return match n {
                        Value::BigInt(s) => {
                            let b = self.big_from_value(&Value::BigInt(s))?;
                            let one = Big::from_decimal("1").unwrap();
                            Ok(self.big_to_value(b.negated().sub(&one)))
                        }
                        Value::Number(x) => Ok(Value::Number((!to_int32(x)) as f64)),
                        _ => Ok(Value::Number(f64::NAN)),
                    };
                }
                Ok(Value::Number((!to_int32(v.to_number())) as f64))
            }
            UnaryOp::PreInc => {
                let n = self.incdec(v, true)?;
                self.assign_unary_result(operand, n.clone(), env)?;
                Ok(n)
            }
            UnaryOp::PreDec => {
                let n = self.incdec(v, false)?;
                self.assign_unary_result(operand, n.clone(), env)?;
                Ok(n)
            }
            UnaryOp::PostInc => {
                let old = v.clone();
                let n = self.incdec(v, true)?;
                self.assign_unary_result(operand, n, env)?;
                Ok(old)
            }
            UnaryOp::PostDec => {
                let old = v.clone();
                let n = self.incdec(v, false)?;
                self.assign_unary_result(operand, n, env)?;
                Ok(old)
            }
            UnaryOp::Delete => {
                match operand {
                    Expr::Member { obj, prop, computed, .. } => {
                        let prop_c = prop.clone();
                        let computed_c = computed.clone();
                        let env2 = env.clone();
                        return self.eval_expr_chain(obj, env, Rc::new(move |engine, base| {
                            match computed_c.clone() {
                                Some(idx) => {
                                    let base2 = base.clone();
                                    engine.eval_expr_chain(&idx, &env2, Rc::new(move |engine, k| {
                                        let key = engine.to_property_key(&k)?;
                                        Ok(Value::Boolean(engine.delete_property(&base2, &key)?))
                                    }))
                                }
                                None => Ok(Value::Boolean(engine.delete_property(&base, &prop_c)?)),
                            }
                        }));
                    }
                    Expr::Ident(name) => {
                        // Identifier bindings (variables, functions, globals) are
                        // not deletable: `delete x` evaluates to `false`.
                        let _ = name;
                        Ok(Value::Boolean(false))
                    }
                    _ => Ok(Value::Boolean(true)),
                }
            }
        }
    }

    /// Write-back for `++`/`--`: run the assignment (suspension-aware) and
    /// yield the precomputed `n`.
    fn assign_unary_result(
        &self,
        operand: &Expr,
        n: Value,
        env: &Rc<RefCell<Env>>,
    ) -> Result<Value> {
        match self.assign_to(operand, n.clone(), env) {
            Ok(_) => Ok(n.clone()),
            Err(Error::Suspend { awaited, cont }) => {
                let n2 = n.clone();
                let next: ValueNext = Rc::new(move |_, _| Ok(n2.clone()));
                Err(Error::Suspend {
                    awaited,
                    cont: chain_value(cont, next),
                })
            }
            Err(e) => Err(e),
        }
    }

    fn delete_property(&self, base: &Value, key: &str) -> Result<bool> {
        match base {
            Value::Object(o) => {
                let mut b = o.borrow_mut();
                match b.props.get(key) {
                    Some(p) if p.configurable => {
                        b.props.remove(key);
                        Ok(true)
                    }
                    Some(_) if self.strict.get() => {
                        Err(Error::Runtime(self.make_type_error("Cannot delete property")))
                    }
                    Some(_) => Ok(false),
                    None => Ok(true),
                }
            }
            Value::Function(f) => {
                let mut b = f.borrow_mut();
                match b.props.get(key) {
                    Some(p) if p.configurable => {
                        b.props.remove(key);
                        Ok(true)
                    }
                    Some(_) if self.strict.get() => {
                        Err(Error::Runtime(self.make_type_error("Cannot delete property")))
                    }
                    Some(_) => Ok(false),
                    None => Ok(true),
                }
            }
            Value::NativeFunction(nf) => {
                let mut b = nf.borrow_mut();
                match b.props.get(key) {
                    Some(p) if p.configurable => {
                        b.props.remove(key);
                        Ok(true)
                    }
                    Some(_) if self.strict.get() => {
                        Err(Error::Runtime(self.make_type_error("Cannot delete property")))
                    }
                    Some(_) => Ok(false),
                    None => Ok(true),
                }
            }
            Value::Array(a) => {
                let mut b = a.borrow_mut();
                match b.props.get(key) {
                    Some(p) if p.configurable => {
                        b.props.remove(key);
                        Ok(true)
                    }
                    Some(_) if self.strict.get() => {
                        Err(Error::Runtime(self.make_type_error("Cannot delete property")))
                    }
                    Some(_) => Ok(false),
                    None => {
                        // Numeric-index deletion creates a hole. Sparse entries
                        // can be removed without touching the dense prefix.
                        if let Ok(i) = key.parse::<usize>() {
                            b.sparse.remove(&i);
                            if i < b.elems.len() {
                                b.elems[i] = Value::Undefined;
                            }
                        }
                        Ok(true)
                    }
                }
            }
            _ => Ok(true),
        }
    }

    fn assign_to(&self, target: &Expr, val: Value, env: &Rc<RefCell<Env>>) -> Result<Value> {
        match target {
            Expr::Ident(name) => {
                if Env::binding_is_const(name.as_ref(), env) {
                    return Err(Error::Runtime(self.make_type_error(
                        "Assignment to constant variable",
                    )));
                }
                // Assignment to a non-writable global (e.g. `undefined`, `NaN`,
                // `Infinity`) is a silent no-op in sloppy mode.
                let global_writable = self
                    .global_object
                    .borrow()
                    .props
                    .get(name)
                    .map(|p| p.writable)
                    .unwrap_or(true);
                if !global_writable {
                    return Ok(val);
                }
                env.borrow_mut().set(name, val.clone());
                Ok(val)
            }
            Expr::Member { obj, prop, computed, .. } => {
                let prop_c = prop.clone();
                let computed_c = computed.clone();
                let env2 = env.clone();
                let val2 = val.clone();
                return self.eval_expr_chain(obj, env, Rc::new(move |engine, base| {
                    match computed_c.clone() {
                        Some(idx) => {
                            let base2 = base.clone();
                            let prop2 = prop_c.clone();
                            let val3 = val2.clone();
                            let _ = prop2;
                            engine.eval_expr_chain(&idx, &env2, Rc::new(move |engine, k| {
                                let key = engine.to_property_key(&k)?;
                                if is_nullish(&base2) {
                                    return Err(Error::Runtime(engine.make_type_error(&format!(
                                        "Cannot set properties of {} (setting '{}')",
                                        if matches!(base2, Value::Null) { "null" } else { "undefined" },
                                        key,
                                    ))));
                                }
                                engine.set_property(&base2, &key, val3.clone())?;
                                Ok(val3.clone())
                            }))
                        }
                        None => {
                            if is_nullish(&base) {
                                return Err(Error::Runtime(engine.make_type_error(&format!(
                                    "Cannot set properties of {} (setting '{}')",
                                    if matches!(base, Value::Null) { "null" } else { "undefined" },
                                    prop_c,
                                ))));
                            }
                            engine.set_property(&base, &prop_c, val2.clone())?;
                            Ok(val2.clone())
                        }
                    }
                }));
            }
            _ => Err(Error::Runtime(Value::String(Rc::from(
                "invalid assignment target",
            )))),
        }
    }

    fn assign_to_target(
        &self,
        target: &AssignTarget,
        val: Value,
        env: &Rc<RefCell<Env>>,
    ) -> Result<Value> {
        match target {
            AssignTarget::Expr(e) => self.assign_to(e, val, env),
            AssignTarget::Pattern(p) => {
                self.assign_pattern(p, &val, env)?;
                Ok(val)
            }
        }
    }

    /// Bind a destructuring pattern as part of a declaration/parameter/loop.
    fn bind_pattern(&self, pat: &Pattern, val: &Value, env: &Rc<RefCell<Env>>) -> Result<()> {
        match pat {
            Pattern::Ident(name) => {
                // Declarations bind into the *given* scope directly, never
                // climbing the `outer` chain (which `Env::set` would do).
                env.borrow_mut().vars.insert(name.clone(), val.clone());
                Ok(())
            }
            Pattern::Array { elems, rest } => {
                let items = self.to_indexed(val)?;
                let mut i = 0usize;
                for e in elems {
                    if let Some(p) = e {
                        let v = items.get(i).cloned().unwrap_or(Value::Undefined);
                        self.bind_pattern(p, &v, env)?;
                    }
                    i += 1;
                }
                if let Some(r) = rest {
                    let rest_items = if i < items.len() {
                        items[i..].to_vec()
                    } else {
                        Vec::new()
                    };
                    let rest_val = Value::Array(self.new_array(rest_items));
                    self.bind_pattern(r, &rest_val, env)?;
                }
                Ok(())
            }
            Pattern::Object { props, rest } => {
                let src = self.to_object(val)?;
                self.bind_object_props(props, &src, rest, env, 0)
            }
            Pattern::Member(e) => {
                self.assign_to(e, val.clone(), env)?;
                Ok(())
            }
            Pattern::Default { inner, default } => {
                if matches!(val, Value::Undefined) {
                    let inner2 = inner.clone();
                    match self.eval_expr(default, env) {
                        Ok(dv) => self.bind_pattern(&inner2, &dv, env),
                        Err(Error::Suspend { awaited, cont }) => {
                            let inner3 = inner.clone();
                            let env3 = env.clone();
                            let next: ValueNext = Rc::new(move |engine, dv| {
                                engine.bind_pattern(&inner3, &dv, &env3)?;
                                Ok(Value::Undefined)
                            });
                            Err(Error::Suspend {
                                awaited,
                                cont: chain_value(cont, next),
                            })
                        }
                        Err(e) => Err(e),
                    }
                } else {
                    self.bind_pattern(inner, val, env)
                }
            }
        }
    }

    /// Bind object-pattern properties from index `idx`, then the rest pattern.
    /// Suspension-aware over computed keys and nested patterns.
    fn bind_object_props(
        &self,
        props: &[(PropKey, Pattern)],
        src: &Rc<RefCell<Object>>,
        rest: &Option<Box<Pattern>>,
        env: &Rc<RefCell<Env>>,
        idx: usize,
    ) -> Result<()> {
        for (i, (key, p)) in props.iter().enumerate().skip(idx) {
            let k = match key {
                PropKey::Computed(e) => match self.eval_expr(e, env) {
                    Ok(v) => self.to_property_key(&v)?,
                    Err(Error::Suspend { awaited, cont }) => {
                        let props2 = props.to_vec();
                        let src2 = src.clone();
                        let rest2 = rest.clone();
                        let env2 = env.clone();
                        let next: ValueNext = Rc::new(move |engine, v| {
                            let k = engine.to_property_key(&v)?;
                            let pv = engine.get_property(&Value::Object(src2.clone()), &k);
                            match engine.bind_pattern(&props2[i].1, &pv, &env2) {
                                Err(Error::Suspend { awaited, cont }) => {
                                    let props3 = props2.clone();
                                    let src3 = src2.clone();
                                    let rest3 = rest2.clone();
                                    let env3 = env2.clone();
                                    let next2: ValueNext = Rc::new(move |engine, _| {
                                        engine.bind_object_props(
                                            &props3, &src3, &rest3, &env3, i + 1,
                                        )?;
                                        Ok(Value::Undefined)
                                    });
                                    return Err(Error::Suspend {
                                        awaited,
                                        cont: chain_value(cont, next2),
                                    });
                                }
                                other => other?,
                            }
                            engine.bind_object_props(&props2, &src2, &rest2, &env2, i + 1)?;
                            Ok(Value::Undefined)
                        });
                        return Err(Error::Suspend {
                            awaited,
                            cont: chain_value(cont, next),
                        });
                    }
                    Err(e) => return Err(e),
                },
                PropKey::Ident(s) | PropKey::Str(s) => s.clone(),
            };
            let v = self.get_property(&Value::Object(src.clone()), &k);
            match self.bind_pattern(p, &v, env) {
                Ok(()) => {}
                Err(Error::Suspend { awaited, cont }) => {
                    let props2 = props.to_vec();
                    let src2 = src.clone();
                    let rest2 = rest.clone();
                    let env2 = env.clone();
                    let next: ValueNext = Rc::new(move |engine, _| {
                        engine.bind_object_props(&props2, &src2, &rest2, &env2, i + 1)?;
                        Ok(Value::Undefined)
                    });
                    return Err(Error::Suspend {
                        awaited,
                        cont: chain_value(cont, next),
                    });
                }
                Err(e) => return Err(e),
            }
        }
        if let Some(r) = rest {
            let mut rest_obj = Object::with_proto(self.object_prototype.clone());
            for (k, p) in src.borrow().props.iter() {
                if p.enumerable && !props.iter().any(|(pk, _)| key_matches(pk, k)) {
                    rest_obj
                        .props
                        .insert(k.clone(), Property::new(p.value.clone()));
                }
            }
            self.bind_pattern(
                r,
                &Value::Object(Rc::new(RefCell::new(rest_obj))),
                env,
            )?;
        }
        Ok(())
    }
    /// Bind a declaration pattern, tracking `const` bindings so that later
    /// reassignment can be rejected.
    fn bind_decl(&self, pat: &Pattern, val: &Value, env: &Rc<RefCell<Env>>, is_const: bool) -> Result<()> {
        self.bind_pattern(pat, val, env)?;
        if is_const {
            mark_const_idents(pat, env);
        }
        Ok(())
    }

    /// Assign a destructuring pattern (used for `lhs = rhs`).
    fn assign_pattern(&self, pat: &Pattern, val: &Value, env: &Rc<RefCell<Env>>) -> Result<()> {
        match pat {
            Pattern::Ident(name) => {
                env.borrow_mut().set(name, val.clone());
                Ok(())
            }
            Pattern::Array { elems, rest } => {
                let items = self.to_indexed(val)?;
                let mut i = 0usize;
                for e in elems {
                    if let Some(p) = e {
                        let v = items.get(i).cloned().unwrap_or(Value::Undefined);
                        self.assign_pattern(p, &v, env)?;
                    }
                    i += 1;
                }
                if let Some(r) = rest {
                    let rest_items = if i < items.len() {
                        items[i..].to_vec()
                    } else {
                        Vec::new()
                    };
                    let rest_val = Value::Array(self.new_array(rest_items));
                    self.assign_pattern(r, &rest_val, env)?;
                }
                Ok(())
            }
            Pattern::Object { props, rest } => {
                let src = self.to_object(val)?;
                self.assign_object_props(props, &src, rest, env, 0)
            }
            Pattern::Member(e) => {
                self.assign_to(e, val.clone(), env)?;
                Ok(())
            }
            Pattern::Default { inner, default } => {
                if matches!(val, Value::Undefined) {
                    let inner2 = inner.clone();
                    match self.eval_expr(default, env) {
                        Ok(dv) => self.assign_pattern(&inner2, &dv, env),
                        Err(Error::Suspend { awaited, cont }) => {
                            let inner3 = inner.clone();
                            let env3 = env.clone();
                            let next: ValueNext = Rc::new(move |engine, dv| {
                                engine.assign_pattern(&inner3, &dv, &env3)?;
                                Ok(Value::Undefined)
                            });
                            Err(Error::Suspend {
                                awaited,
                                cont: chain_value(cont, next),
                            })
                        }
                        Err(e) => Err(e),
                    }
                } else {
                    self.assign_pattern(inner, val, env)
                }
            }
        }
    }

    /// Assign object-pattern properties from index `idx`, then the rest pattern.
    fn assign_object_props(
        &self,
        props: &[(PropKey, Pattern)],
        src: &Rc<RefCell<Object>>,
        rest: &Option<Box<Pattern>>,
        env: &Rc<RefCell<Env>>,
        idx: usize,
    ) -> Result<()> {
        for (i, (key, p)) in props.iter().enumerate().skip(idx) {
            let k = match key {
                PropKey::Computed(e) => match self.eval_expr(e, env) {
                    Ok(v) => self.to_property_key(&v)?,
                    Err(Error::Suspend { awaited, cont }) => {
                        let props2 = props.to_vec();
                        let src2 = src.clone();
                        let rest2 = rest.clone();
                        let env2 = env.clone();
                        let next: ValueNext = Rc::new(move |engine, v| {
                            let k = engine.to_property_key(&v)?;
                            let pv = engine.get_property(&Value::Object(src2.clone()), &k);
                            match engine.assign_pattern(&props2[i].1, &pv, &env2) {
                                Err(Error::Suspend { awaited, cont }) => {
                                    let props3 = props2.clone();
                                    let src3 = src2.clone();
                                    let rest3 = rest2.clone();
                                    let env3 = env2.clone();
                                    let next2: ValueNext = Rc::new(move |engine, _| {
                                        engine.assign_object_props(
                                            &props3, &src3, &rest3, &env3, i + 1,
                                        )?;
                                        Ok(Value::Undefined)
                                    });
                                    return Err(Error::Suspend {
                                        awaited,
                                        cont: chain_value(cont, next2),
                                    });
                                }
                                other => other?,
                            }
                            engine.assign_object_props(&props2, &src2, &rest2, &env2, i + 1)?;
                            Ok(Value::Undefined)
                        });
                        return Err(Error::Suspend {
                            awaited,
                            cont: chain_value(cont, next),
                        });
                    }
                    Err(e) => return Err(e),
                },
                PropKey::Ident(s) | PropKey::Str(s) => s.clone(),
            };
            let v = self.get_property(&Value::Object(src.clone()), &k);
            match self.assign_pattern(p, &v, env) {
                Ok(()) => {}
                Err(Error::Suspend { awaited, cont }) => {
                    let props2 = props.to_vec();
                    let src2 = src.clone();
                    let rest2 = rest.clone();
                    let env2 = env.clone();
                    let next: ValueNext = Rc::new(move |engine, _| {
                        engine.assign_object_props(&props2, &src2, &rest2, &env2, i + 1)?;
                        Ok(Value::Undefined)
                    });
                    return Err(Error::Suspend {
                        awaited,
                        cont: chain_value(cont, next),
                    });
                }
                Err(e) => return Err(e),
            }
        }
        if let Some(r) = rest {
            let mut rest_obj = Object::with_proto(self.object_prototype.clone());
            for (k, p) in src.borrow().props.iter() {
                if p.enumerable && !props.iter().any(|(pk, _)| key_matches(pk, k)) {
                    rest_obj
                        .props
                        .insert(k.clone(), Property::new(p.value.clone()));
                }
            }
            self.assign_pattern(
                r,
                &Value::Object(Rc::new(RefCell::new(rest_obj))),
                env,
            )?;
        }
        Ok(())
    }

    pub(crate) fn call_function(&self, func: &Value, this: &Value, args: &[Value]) -> Result<Value> {
        // Promise resolver functions (created for `new Promise(executor)` and
        // thenable assimilation) settle their capability instead of running code.
        if let Value::NativeFunction(nf) = func {
            if nf.borrow().props.contains_key("__promise_cap__") {
                return self.call_promise_resolver(func, args);
            }
            // `NewPromiseCapability` executors record their resolve/reject pair.
            if nf.borrow().props.contains_key("__cap_exec__") {
                if let Some(r) = self.try_call_cap_executor(func, args) {
                    return r;
                }
            }
        }
        // Unwrap a bound function (created by `Function.prototype.bind`).
        if let Value::NativeFunction(nf) = func {
            if nf.borrow().props.contains_key("__target__") {
                let b = nf.borrow();
                let target = b.props.get("__target__").unwrap().value.clone();
                let this_arg = b
                    .props
                    .get("__this__")
                    .map(|p| p.value.clone())
                    .unwrap_or(Value::Undefined);
                let mut bound = match &b.props.get("__args__").map(|p| &p.value) {
                    Some(Value::Array(a)) => a.borrow().elems.clone(),
                    _ => Vec::new(),
                };
                drop(b);
                bound.extend_from_slice(args);
                return self.call_function(&target, &this_arg, &bound);
            }
        }
        // Calling a generator function produces a generator object without
        // executing the body; the body runs (eagerly) on the first `next()`.
        // Calling an async function returns a promise; the body runs now
        // (synchronously until the first `await`) and settles the promise.
        if let Value::Function(u) = func {
            if u.borrow().generator {
                return Ok(self.make_generator_object(u.clone(), this, args));
            }
            if u.borrow().is_async {
                return Ok(self.call_async_function(u.clone(), this, args));
            }
        }
        self.call_function_normal(func, this, args)
    }

    /// Call an `async` function: create the result promise, bind parameters,
    /// and run the body. `return v` resolves, a throw rejects, and an `await`
    /// on a pending promise suspends (resuming via a reaction job).
    fn call_async_function(
        &self,
        u: Rc<RefCell<Function>>,
        this: &Value,
        args: &[Value],
    ) -> Value {
        let cap = self.new_promise();
        let out = Value::Promise(cap.clone());
        // Fresh child environment with `this`/super/`arguments` wired
        // synchronously; parameters bind (suspension-aware) below.
        let closure_env = u.borrow().closure.clone();
        let child = Rc::new(RefCell::new(Env::new_child(closure_env)));
        self.register_env(&child);
        let (this_val, super_proto, super_ctor) = {
            let f = u.borrow();
            (
                f.this_capture.clone().unwrap_or_else(|| this.clone()),
                f.super_proto.clone(),
                f.super_ctor.clone(),
            )
        };
        child
            .borrow_mut()
            .vars
            .insert(Rc::from("__this__"), this_val);
        if let Some(sp) = super_proto {
            child
                .borrow_mut()
                .vars
                .insert(Rc::from("__super__"), Value::Object(sp));
        }
        if let Some(sc) = super_ctor {
            child
                .borrow_mut()
                .vars
                .insert(Rc::from("__super_ctor__"), sc);
        }
        let args_obj = self.make_object(self.object_prototype.clone());
        for (i, av) in args.iter().enumerate() {
            args_obj
                .borrow_mut()
                .props
                .insert(self.intern_key(&i.to_string()), Property::new(av.clone()));
        }
        args_obj.borrow_mut().props.insert(
            Rc::from("length"),
            Property::new(Value::Number(args.len() as f64)),
        );
        child
            .borrow_mut()
            .vars
            .insert(Rc::from("arguments"), Value::Object(args_obj));
        let params = u.borrow().params.clone();
        let body = u.borrow().body.clone();
        let args_vec = args.to_vec();
        self.func_async_stack.borrow_mut().push(true);
        self.async_depth.set(self.async_depth.get() + 1);
        let result = self.bind_params_then_body(&params, &args_vec, &child, 0, &body);
        self.async_depth.set(self.async_depth.get().saturating_sub(1));
        self.func_async_stack.borrow_mut().pop();
        // A tail call on the async boundary executes inline and settles.
        let result = match result {
            Err(Error::TailCall { func, this, args }) => {
                self.call_function(&func, &this, &args).map(Some)
            }
            other => other,
        };
        match result {
            Ok(opt) => self.resolve_value(&cap, opt.unwrap_or(Value::Undefined)),
            Err(Error::Return(v)) => self.resolve_value(&cap, v),
            Err(Error::Runtime(e)) => self.reject_promise(&cap, e),
            Err(Error::Suspend { awaited, cont }) => self.await_suspend(awaited, cont, &cap),
            Err(Error::Unimplemented(m)) => self.reject_promise(
                &cap,
                self.make_type_error(&format!("async is not implemented: {m}")),
            ),
            Err(_) => self.reject_promise(
                &cap,
                self.make_type_error("async function failed"),
            ),
        }
        out
    }

    /// Bind parameters from `idx`, then run the body. An `await` in a default
    /// value suspends with the remaining bindings plus the body attached.
    fn bind_params_then_body(
        &self,
        params: &[Pattern],
        args: &[Value],
        child: &Rc<RefCell<Env>>,
        idx: usize,
        body: &[Stmt],
    ) -> Result<Option<Value>> {
        for (i, p) in params.iter().enumerate().skip(idx) {
            let v = args.get(i).cloned().unwrap_or(Value::Undefined);
            match self.bind_pattern(p, &v, child) {
                Ok(()) => {}
                Err(Error::Suspend { awaited, cont }) => {
                    let params2 = params.to_vec();
                    let args2 = args.to_vec();
                    let child2 = child.clone();
                    let body2 = body.to_vec();
                    let next: ValueNext = Rc::new(move |engine, _| {
                        let opt = engine.bind_params_then_body(
                            &params2, &args2, &child2, i + 1, &body2,
                        )?;
                        Ok(opt.unwrap_or(Value::Undefined))
                    });
                    return Err(Error::Suspend {
                        awaited,
                        cont: chain_value(cont, next),
                    });
                }
                Err(e) => return Err(e),
            }
        }
        self.exec_stmts(body, child)
    }

    /// Build the child lexical environment for a user-function call: bind
    /// parameters, `__this__`, super bindings and the `arguments` object.
    /// Shared by sync and async calls (async suspends propagate to the async
    /// boundary through the returned `Err`, with parameter bindings before the
    /// suspension point already applied — matching sync evaluation order).
    fn bind_call_env(
        &self,
        u: &Rc<RefCell<Function>>,
        this: &Value,
        args: &[Value],
    ) -> Result<Rc<RefCell<Env>>> {
        let f = u.borrow();
        let closure_env = f.closure.clone();
        let child = Rc::new(RefCell::new(Env::new_child(closure_env)));
        self.register_env(&child);
        let this_val = f
            .this_capture
            .clone()
            .unwrap_or_else(|| this.clone());
        // Clone the small pieces we need so the borrow ends before binding
        // (binding evaluates default expressions, which may re-enter).
        let params = f.params.clone();
        let super_proto = f.super_proto.clone();
        let super_ctor = f.super_ctor.clone();
        drop(f);
        for (i, p) in params.iter().enumerate() {
            let v = args.get(i).cloned().unwrap_or(Value::Undefined);
            self.bind_pattern(p, &v, &child)?;
        }
        child
            .borrow_mut()
            .vars
            .insert(Rc::from("__this__"), this_val);
        if let Some(sp) = super_proto {
            child
                .borrow_mut()
                .vars
                .insert(Rc::from("__super__"), Value::Object(sp));
        }
        if let Some(sc) = super_ctor {
            child
                .borrow_mut()
                .vars
                .insert(Rc::from("__super_ctor__"), sc);
        }
        // `arguments` pseudo-array.
        let args_obj = self.make_object(self.object_prototype.clone());
        for (i, av) in args.iter().enumerate() {
            args_obj
                .borrow_mut()
                .props
                .insert(self.intern_key(&i.to_string()), Property::new(av.clone()));
        }
        args_obj.borrow_mut().props.insert(
            Rc::from("length"),
            Property::new(Value::Number(args.len() as f64)),
        );
        child
            .borrow_mut()
            .vars
            .insert(Rc::from("arguments"), Value::Object(args_obj));
        Ok(child)
    }

    /// A generator object for `fn_value`: an object carrying the function, its
    /// `this`/arguments and (after the first `next`) the eagerly-collected
    /// yield values. Iteration is served sequentially from that list.
    pub(crate) fn make_generator_object(
        &self,
        fn_value: Rc<RefCell<Function>>,
        this: &Value,
        args: &[Value],
    ) -> Value {
        let obj = self.make_object(self.generator_prototype.clone());
        let mut b = obj.borrow_mut();
        b.props.insert(
            Rc::from("__gen_fn__"),
            Property::new(Value::Function(fn_value)),
        );
        b.props
            .insert(Rc::from("__gen_this__"), Property::new(this.clone()));
        let argv: Vec<Value> = args.to_vec();
        b.props.insert(
            Rc::from("__gen_args__"),
            Property::new(Value::Array(self.new_array(argv))),
        );
        b.props
            .insert(Rc::from("__gen_started__"), Property::new(Value::Boolean(false)));
        b.props
            .insert(Rc::from("__gen_values__"), Property::new(Value::Undefined));
        b.props
            .insert(Rc::from("__gen_index__"), Property::new(Value::Number(0.0)));
        drop(b);
        Value::Object(obj)
    }

    /// Execute a generator body eagerly, collecting every `yield`ed value.
    /// The body runs exactly once, on the first `next()` call.
    pub(crate) fn run_generator_body(&self, gen_obj: &Value) -> Result<()> {
        let fn_value = self.get_property(gen_obj, "__gen_fn__");
        let this = self.get_property(gen_obj, "__gen_this__");
        let args: Vec<Value> = match self.get_property(gen_obj, "__gen_args__") {
            Value::Array(a) => a.borrow().elems.clone(),
            _ => Vec::new(),
        };
        if !matches!(&fn_value, Value::Function(_)) {
            return Err(Error::Runtime(self.make_type_error(
                "generator function is not callable",
            )));
        }
        let (result, values) = {
            let sink = Rc::new(RefCell::new(Vec::new()));
            let prev = self.yield_sink.borrow().clone();
            *self.yield_sink.borrow_mut() = Some(sink.clone());
            let r = self.call_function_normal(&fn_value, &this, &args);
            *self.yield_sink.borrow_mut() = prev;
            (r, Rc::try_unwrap(sink).map(|s| s.into_inner()).unwrap_or_default())
        };
        let _ = result; // the completion value is unused; `return v` in a
                        // generator body contributes no yielded value
        if let Value::Object(obj) = gen_obj {
            let mut b = obj.borrow_mut();
            b.props.insert(
                Rc::from("__gen_values__"),
                Property::new(Value::Array(self.new_array(values))),
            );
            b.props
                .insert(Rc::from("__gen_started__"), Property::new(Value::Boolean(true)));
        }
        Ok(())
    }

    fn call_function_normal(&self, func: &Value, this: &Value, args: &[Value]) -> Result<Value> {
        let mut cur_func = func.clone();
        let mut cur_this = this.clone();
        let mut cur_args: Vec<Value> = args.to_vec();
        loop {
            // A tail call may target an async function: run it through the
            // general path, which returns its promise.
            if let Value::Function(u) = &cur_func {
                if u.borrow().is_async {
                    return self.call_function(&cur_func, &cur_this, &cur_args);
                }
            }
            let result = match &cur_func {
                Value::NativeFunction(f) => {
                    let prev_ctor = self.active_native_ctor.borrow().clone();
                    *self.active_native_ctor.borrow_mut() = Some(cur_func.clone());
                    let r = (f.borrow().func)(self, &cur_this, &cur_args, false);
                    *self.active_native_ctor.borrow_mut() = prev_ctor;
                    return r;
                }
                Value::Function(u) => {
                    // Track lexical async context for `await` validity: sync
                    // bodies (including generator bodies run eagerly) see
                    // `false`, so `await` inside them is an unimplemented
                    // error rather than a suspension.
                    self.func_async_stack.borrow_mut().push(false);
                    let r = (|| {
                        let child = self.bind_call_env(u, &cur_this, &cur_args)?;
                        let body = u.borrow().body.clone();
                        self.exec_stmts(&body, &child)
                    })();
                    self.func_async_stack.borrow_mut().pop();
                    r
                }
                _ => {
                    return Err(Error::Runtime(self.make_type_error(
                        "called value is not a function",
                    )))
                }
            };
            match result {
                Ok(v) => return Ok(v.unwrap_or(Value::Undefined)),
                Err(Error::Return(rv)) => return Ok(rv),
                // A proper tail call: unwind this frame and invoke the target
                // function without growing the stack.
                Err(Error::TailCall { func, this, args }) => {
                    cur_func = func;
                    cur_this = this;
                    cur_args = args;
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Whether `v` is callable as a constructor (`new` → object).
    pub(crate) fn is_constructor(&self, v: &Value) -> bool {
        match v {
            // Async functions (like generators) are not constructable.
            Value::Function(f) => !f.borrow().is_async,
            Value::NativeFunction(nf) => nf.borrow().constructable,
            _ => false,
        }
    }

    fn construct(&self, func: &Value, args: &[Value]) -> Result<Value> {
        self.construct_with_new_target(func, args, func)
    }

    /// Perform `new func(...args)` where the instance prototype is taken from
    /// `new_target`'s `prototype` property (per `Reflect.construct`).
    pub(crate) fn construct_with_new_target(&self, func: &Value, args: &[Value], new_target: &Value) -> Result<Value> {
        if let Value::Function(f) = func {
            if f.borrow().is_async {
                return Err(Error::Runtime(self.make_type_error("async function is not a constructor")));
            }
        }
        let proto = self.proto_of(new_target).or_else(|| self.default_proto(new_target, func));
        let proto = proto.unwrap_or_else(|| self.object_prototype.clone());
        let inst = self.make_object(proto);
        let this_val = Value::Object(inst.clone());
        // Derived class constructors run with their own environment so the
        // `this` rebound by `super()` can be read back: the constructed value
        // is that rebound `this` (or an explicit object return).
        if let Value::Function(u) = func {
            if u.borrow().is_derived {
                return self.construct_derived(u.clone(), &this_val, args, new_target);
            }
        }
        let prev_target = self.active_new_target.borrow().clone();
        *self.active_new_target.borrow_mut() = Some(new_target.clone());
        let res = match func {
            Value::NativeFunction(nf) => {
                if !nf.borrow().constructable {
                    *self.active_new_target.borrow_mut() = prev_target;
                    return Err(Error::Runtime(self.make_type_error("not a constructor")));
                }
                let prev_realm = self.native_realm.borrow().clone();
                *self.native_realm.borrow_mut() = nf.borrow().realm.clone();
                let prev_ctor = self.active_native_ctor.borrow().clone();
                *self.active_native_ctor.borrow_mut() = Some(func.clone());
                let r = (nf.borrow().func)(self, &this_val, args, true);
                *self.active_native_ctor.borrow_mut() = prev_ctor;
                *self.native_realm.borrow_mut() = prev_realm;
                r
            }
            _ => self.call_function(func, &this_val, args),
        };
        *self.active_new_target.borrow_mut() = prev_target;
        let res = res?;
        match res {
            Value::Object(_)
            | Value::Array(_)
            | Value::Function(_)
            | Value::NativeFunction(_)
            | Value::Map(_)
            | Value::Set(_)
            | Value::WeakMap(_)
            | Value::WeakSet(_)
            | Value::Promise(_)
            | Value::Regex(_)
            | Value::Symbol(_) => Ok(res),
            _ => Ok(Value::Object(inst)),
        }
    }

    /// Run a derived class constructor: `super()` (via the `Super` call path)
    /// rebinds `__this__` in the call environment; the constructed value is
    /// that rebound binding, unless the body explicitly returns an object.
    fn construct_derived(
        &self,
        u: Rc<RefCell<Function>>,
        placeholder: &Value,
        args: &[Value],
        new_target: &Value,
    ) -> Result<Value> {
        let prev_target = self.active_new_target.borrow().clone();
        *self.active_new_target.borrow_mut() = Some(new_target.clone());
        self.func_async_stack.borrow_mut().push(false);
        let r = (|| -> Result<Value> {
            let child = self.bind_call_env(&u, placeholder, args)?;
            let body = u.borrow().body.clone();
            let r = self.exec_stmts(&body, &child);
            // A tail call in constructor position runs inline (rare).
            let r = match r {
                Err(Error::TailCall { func, this, args }) => {
                    self.call_function(&func, &this, &args).map(Some)
                }
                other => other,
            };
            match r {
                Err(Error::Return(v)) if Self::is_object_value(&v) => Ok(v),
                Err(Error::Return(_)) | Ok(_) => Ok(child
                    .borrow()
                    .get("__this__")
                    .unwrap_or_else(|| placeholder.clone())),
                Err(e) => Err(e),
            }
        })();
        self.func_async_stack.borrow_mut().pop();
        *self.active_new_target.borrow_mut() = prev_target;
        r
    }

    /// `GetPrototypeFromConstructor`: the default prototype for `new_target`
    /// when its own `.prototype` is not an object. The fallback resolves the
    /// intrinsic named by the *target* constructor in the realm of the new
    /// target (`GetFunctionRealm(new_target)`).
    fn default_proto(&self, new_target: &Value, target: &Value) -> Option<Rc<RefCell<Object>>> {
        let realm = match new_target {
            Value::Function(f) => f.borrow().realm.clone(),
            Value::NativeFunction(nf) => nf.borrow().realm.clone(),
            _ => None,
        }
        .unwrap_or_else(|| self.realm.clone());
        let kind = match target {
            Value::NativeFunction(nf) => nf
                .borrow()
                .intrinsic_proto
                .unwrap_or(crate::value::RealmProto::Object),
            _ => crate::value::RealmProto::Object,
        };
        let fallback = || realm.object_prototype.clone();
        Some(match kind {
            crate::value::RealmProto::Object => fallback(),
            crate::value::RealmProto::Number => realm.number_prototype.clone(),
            crate::value::RealmProto::String => realm.string_prototype.clone(),
            crate::value::RealmProto::Boolean => realm.boolean_prototype.clone(),
            crate::value::RealmProto::Promise => realm.promise_prototype.clone(),
            crate::value::RealmProto::Error => {
                realm.error_prototype_for("Error").unwrap_or_else(fallback)
            }
            crate::value::RealmProto::NativeError(name) => {
                realm.error_prototype_for(name).unwrap_or_else(fallback)
            }
        })
    }

}
/// Record every identifier bound by a `const` pattern as immutable in `env`.
fn mark_const_idents(pat: &Pattern, env: &Rc<RefCell<Env>>) {
    match pat {
        Pattern::Ident(name) => {
            env.borrow_mut().constants.insert(name.clone());
        }
        Pattern::Array { elems, rest } => {
            for e in elems {
                if let Some(p) = e {
                    mark_const_idents(p, env);
                }
            }
            if let Some(r) = rest {
                mark_const_idents(r, env);
            }
        }
        Pattern::Object { props, rest } => {
            for (_, p) in props {
                mark_const_idents(p, env);
            }
            if let Some(r) = rest {
                mark_const_idents(r, env);
            }
        }
        Pattern::Default { inner, .. } => mark_const_idents(inner, env),
        Pattern::Member(_) => {}
    }
}

/// Collect every identifier name bound by a declaration pattern.
fn pattern_idents(pat: &Pattern, out: &mut Vec<Rc<str>>) {
    match pat {
        Pattern::Ident(name) => out.push(name.clone()),
        Pattern::Array { elems, rest } => {
            for e in elems {
                if let Some(p) = e {
                    pattern_idents(p, out);
                }
            }
            if let Some(r) = rest {
                pattern_idents(r, out);
            }
        }
        Pattern::Object { props, rest } => {
            for (_, p) in props {
                pattern_idents(p, out);
            }
            if let Some(r) = rest {
                pattern_idents(r, out);
            }
        }
        Pattern::Default { inner, .. } => pattern_idents(inner, out),
        Pattern::Member(_) => {}
    }
}

/// Collect every `var`-declared name within `stmts` (recursing into blocks and
/// control-flow bodies, but not into nested function bodies), so they can be
/// hoisted to the nearest function/global scope.
fn collect_var_names(stmts: &[Stmt], out: &mut Vec<Rc<str>>) {
    for s in stmts {
        match s {
            Stmt::Var(list, VarKind::Var) => {
                for (pat, _) in list {
                    pattern_idents(pat, out);
                }
            }
            Stmt::Block(b) => collect_var_names(b, out),
            Stmt::If { then, else_, .. } => {
                collect_var_names(then, out);
                collect_var_names(else_, out);
            }
            Stmt::For { init, body, .. } => {
                if let Some(init) = init {
                    collect_var_names(core::slice::from_ref(init.as_ref()), out);
                }
                collect_var_names(body, out);
            }
            Stmt::ForIn { body, .. } | Stmt::ForOf { body, .. } => collect_var_names(body, out),
            Stmt::While { body, .. } | Stmt::DoWhile { body, .. } => collect_var_names(body, out),
            Stmt::Try {
                try_block,
                catch,
                finally,
            } => {
                collect_var_names(try_block, out);
                if let Some((_, c)) = catch {
                    collect_var_names(c, out);
                }
                if let Some(f) = finally {
                    collect_var_names(f, out);
                }
            }
            Stmt::Switch { cases, .. } => {
                for (_, body) in cases {
                    collect_var_names(body, out);
                }
            }
            Stmt::Labeled { body, .. } => collect_var_names(core::slice::from_ref(body.as_ref()), out),
            _ => {}
        }
    }
}

impl Engine {
    /// Whether the value is a BigInt primitive or a BigInt wrapper object
    /// (a cheap check that runs no user code).
    pub(crate) fn bigint_direct(v: &Value) -> bool {
        match v {
            Value::BigInt(_) => true,
            Value::Object(o) => matches!(
                o.borrow().props.get("__value__").map(|p| &p.value),
                Some(Value::BigInt(_))
            ),
            _ => false,
        }
    }

    /// ECMAScript `ToPropertyKey`: `Symbol`s map to their internal key string;
    /// everything else goes through `ToPrimitive` (string hint) + `ToString`.
    pub(crate) fn to_property_key(&self, v: &Value) -> Result<Rc<str>> {
        match v {
            Value::Symbol(s) => Ok(s.id.clone()),
            other => {
                let prim = self.to_primitive_hint(other, PrimitiveHint::String)?;
                if let Value::Symbol(s) = &prim {
                    return Ok(s.id.clone());
                }
                Ok(prim.to_string())
            }
        }
    }

    /// ECMAScript `ToNumeric`: `ToPrimitive` (number hint), then BigInt stays
    /// BigInt and every other primitive becomes a Number.
    fn to_numeric(&self, v: &Value) -> Result<Value> {
        let prim = self.to_primitive_hint(v, PrimitiveHint::Number)?;
        match prim {
            Value::BigInt(_) => Ok(prim),
            Value::Symbol(_) => Err(Error::Runtime(
                self.make_type_error("Cannot convert a Symbol value to a number"),
            )),
            other => Ok(Value::Number(other.to_number())),
        }
    }

    /// Binary operator evaluation that is BigInt-aware; falls back to the pure
    /// (legacy) evaluation when neither operand involves BigInt.
    fn eval_binary_value(&self, op: BinaryOp, l: &Value, r: &Value) -> Result<Value> {
        use BinaryOp::*;
        if !Self::bigint_direct(l) && !Self::bigint_direct(r) {
            return Ok(eval_binary(op, l, r));
        }
        match op {
            Add => {
                let lp = self.to_primitive_hint(l, PrimitiveHint::Default)?;
                let rp = self.to_primitive_hint(r, PrimitiveHint::Default)?;
                if matches!(lp, Value::String(_)) || matches!(rp, Value::String(_)) {
                    let ls = self.to_string_fallible(&lp)?;
                    let rs = self.to_string_fallible(&rp)?;
                    let mut s = alloc::string::String::with_capacity(ls.len() + rs.len());
                    s.push_str(&ls);
                    s.push_str(&rs);
                    return Ok(Value::String(Rc::from(s.as_str())));
                }
                let ln = self.to_numeric(&lp)?;
                let rn = self.to_numeric(&rp)?;
                self.numeric_binary(op, ln, rn)
            }
            Sub | Mul | Div | Rem | Exp | BitAnd | BitOr | BitXor | Shl | Shr | Ushr => {
                let ln = self.to_numeric(l)?;
                let rn = self.to_numeric(r)?;
                self.numeric_binary(op, ln, rn)
            }
            Lt | Gt | Le | Ge => {
                let ln = self.to_numeric(l)?;
                let rn = self.to_numeric(r)?;
                let ord = self.compare_numeric(&ln, &rn)?;
                use core::cmp::Ordering::*;
                let b = match (op, ord) {
                    (Lt, Some(Less))
                    | (Gt, Some(Greater))
                    | (Le, Some(Less) | Some(Equal))
                    | (Ge, Some(Greater) | Some(Equal)) => true,
                    _ => false,
                };
                Ok(Value::Boolean(b))
            }
            Eq | Ne => {
                let eq = self.loose_eq_value(l, r)?;
                Ok(Value::Boolean(match op {
                    Eq => eq,
                    _ => !eq,
                }))
            }
            Seq | Sne => {
                let eq = if Self::bigint_direct(l) || Self::bigint_direct(r) {
                    match (l, r) {
                        (Value::BigInt(a), Value::BigInt(b)) => a == b,
                        _ => false,
                    }
                } else {
                    Value::strict_eq(l, r)
                };
                Ok(Value::Boolean(match op {
                    Seq => eq,
                    _ => !eq,
                }))
            }
            _ => Ok(eval_binary(op, l, r)),
        }
    }

    /// Apply an arithmetic/bitwise operator to two `ToNumeric` results.
    fn numeric_binary(&self, op: BinaryOp, ln: Value, rn: Value) -> Result<Value> {
        use BinaryOp::*;
        let has_big = matches!(ln, Value::BigInt(_)) || matches!(rn, Value::BigInt(_));
        let both_big = matches!(ln, Value::BigInt(_)) && matches!(rn, Value::BigInt(_));
        if has_big && !both_big {
            return Err(Error::Runtime(self.make_type_error(
                "Cannot mix BigInt and other types, use explicit conversions",
            )));
        }
        if !both_big {
            return Ok(eval_binary(op, &ln, &rn));
        }
        let a = self.big_from_value(&ln)?;
        let b = self.big_from_value(&rn)?;
        let r = match op {
            Add => a.add(&b),
            Sub => a.sub(&b),
            Mul => a.mul(&b),
            Div => {
                if b.is_zero() {
                    return Err(Error::Runtime(
                        self.make_range_error("BigInt division by zero"),
                    ));
                }
                a.divmod(&b).0
            }
            Rem => {
                if b.is_zero() {
                    return Err(Error::Runtime(
                        self.make_range_error("BigInt division by zero"),
                    ));
                }
                a.divmod(&b).1
            }
            Exp => {
                if b.neg {
                    return Err(Error::Runtime(
                        self.make_range_error("Exponent must be non-negative"),
                    ));
                }
                if b.bit_length() > 30 {
                    return Err(Error::Runtime(
                        self.make_range_error("Maximum BigInt size exceeded"),
                    ));
                }
                let exp = b.to_decimal_string().parse::<u64>().unwrap_or(0);
                a.pow(exp)
            }
            Shl => {
                let k = self.shift_amount(&b)?;
                a.shl(k)
            }
            Shr => {
                let k = self.shift_amount(&b)?;
                a.shr(k)
            }
            Ushr => {
                return Err(Error::Runtime(self.make_type_error(
                    "BigInts have no unsigned right shift, use >> instead",
                )))
            }
            BitAnd => bigint_bitop(&a, &b, BitOp::And),
            BitOr => bigint_bitop(&a, &b, BitOp::Or),
            BitXor => bigint_bitop(&a, &b, BitOp::Xor),
            _ => return Ok(eval_binary(op, &ln, &rn)),
        };
        Ok(self.big_to_value(r))
    }

    /// `++` / `--` on a value: BigInt-aware (returns the new value).
    fn incdec(&self, v: &Value, add: bool) -> Result<Value> {
        if Self::bigint_direct(v) {
            let n = self.to_numeric(v)?;
            return match n {
                Value::BigInt(s) => {
                    let b = self.big_from_value(&Value::BigInt(s))?;
                    let one = Big::from_decimal("1").unwrap();
                    Ok(self.big_to_value(if add { b.add(&one) } else { b.sub(&one) }))
                }
                Value::Number(x) => Ok(Value::Number(if add { x + 1.0 } else { x - 1.0 })),
                _ => Err(Error::Runtime(
                    self.make_type_error("invalid increment/decrement operand"),
                )),
            };
        }
        let x = v.to_number();
        Ok(Value::Number(if add { x + 1.0 } else { x - 1.0 }))
    }

    /// The shift count for BigInt shifts.
    fn shift_amount(&self, b: &Big) -> Result<u64> {
        if b.neg {
            return Ok(0); // a negative shift amount is a no-op for u64 counts
        }
        if b.bit_length() > 32 {
            return Err(Error::Runtime(
                self.make_range_error("Maximum BigInt size exceeded"),
            ));
        }
        Ok(b.to_decimal_string().parse::<u64>().unwrap_or(0))
    }

    fn big_from_value(&self, v: &Value) -> Result<Big> {
        match v {
            Value::BigInt(s) => Big::from_decimal(s).ok_or_else(|| {
                Error::Runtime(self.make_type_error("invalid BigInt internal representation"))
            }),
            _ => Err(Error::Runtime(self.make_type_error("not a BigInt"))),
        }
    }

    fn big_to_value(&self, b: Big) -> Value {
        Value::BigInt(Rc::from(b.to_decimal_string().as_str()))
    }

    /// Exact numeric comparison for `ToNumeric` values (BigInt/Number mix ok).
    /// `None` means the comparison is "undefined" (a NaN operand).
    fn compare_numeric(&self, ln: &Value, rn: &Value) -> Result<Option<core::cmp::Ordering>> {
        use core::cmp::Ordering;
        match (ln, rn) {
            (Value::BigInt(_), Value::BigInt(_)) => {
                let a = self.big_from_value(ln)?;
                let b = self.big_from_value(rn)?;
                Ok(Some(a.cmp(&b)))
            }
            (Value::Number(x), Value::Number(y)) => Ok(if x.is_nan() || y.is_nan() {
                None
            } else {
                Some(x.partial_cmp(y).unwrap_or(Ordering::Equal))
            }),
            (Value::Number(x), Value::BigInt(_)) => {
                if x.is_nan() {
                    return Ok(None);
                }
                let b = self.big_from_value(rn)?;
                Ok(cmp_number_bigint(*x, &b))
            }
            (Value::BigInt(_), Value::Number(y)) => {
                if y.is_nan() {
                    return Ok(None);
                }
                let a = self.big_from_value(ln)?;
                Ok(cmp_number_bigint(*y, &a).map(|o| o.reverse()))
            }
            _ => Ok(None),
        }
    }

    /// Abstract equality with BigInt support (fallible due to `ToPrimitive`).
    fn loose_eq_value(&self, l: &Value, r: &Value) -> Result<bool> {
        use super::ops::loose_eq;
        if !Self::bigint_direct(l) && !Self::bigint_direct(r) {
            return Ok(loose_eq(l, r));
        }
        // Convert objects to primitives first (default hint, per spec).
        let lp = self.to_primitive_hint(l, PrimitiveHint::Default)?;
        let rp = self.to_primitive_hint(r, PrimitiveHint::Default)?;
        match (&lp, &rp) {
            (Value::BigInt(_), Value::BigInt(_)) => {
                let a = self.big_from_value(&lp)?;
                let b = self.big_from_value(&rp)?;
                Ok(a == b)
            }
            (Value::BigInt(_), Value::Number(y)) => {
                let a = self.big_from_value(&lp)?;
                Ok(number_equals_bigint(*y, &a))
            }
            (Value::Number(x), Value::BigInt(_)) => {
                let b = self.big_from_value(&rp)?;
                Ok(number_equals_bigint(*x, &b))
            }
            (Value::BigInt(_), Value::String(s)) => {
                let a = self.big_from_value(&lp)?;
                Ok(Big::from_decimal(s).map(|b| a == b).unwrap_or(false))
            }
            (Value::String(s), Value::BigInt(_)) => {
                let b = self.big_from_value(&rp)?;
                Ok(Big::from_decimal(s).map(|a| a == b).unwrap_or(false))
            }
            (Value::BigInt(_), Value::Boolean(_)) => {
                let n = self.to_numeric(&rp)?;
                self.loose_eq_value(&lp, &n)
            }
            (Value::Boolean(_), Value::BigInt(_)) => {
                let n = self.to_numeric(&lp)?;
                self.loose_eq_value(&n, &rp)
            }
            _ => Ok(loose_eq(&lp, &rp)),
        }
    }
}

/// Exact comparison of a finite Number against a BigInt.
fn cmp_number_bigint(x: f64, b: &Big) -> Option<core::cmp::Ordering> {
    use core::cmp::Ordering;
    if x.is_nan() {
        return None;
    }
    if x.is_infinite() {
        return Some(if x > 0.0 { Ordering::Greater } else { Ordering::Less });
    }
    let trunc = x.trunc();
    let xb = Big::from_decimal(&alloc::format!("{:.0}", trunc))?;
    let c = xb.cmp(b);
    if x == trunc {
        Some(c)
    } else {
        // x is non-integral: x < y iff x < ceil; y == floor(x) → y < x.
        Some(match c {
            Ordering::Less => Ordering::Less,
            Ordering::Equal => Ordering::Less,
            Ordering::Greater => Ordering::Greater,
        })
    }
}

/// Whether a finite Number equals a BigInt exactly.
fn number_equals_bigint(x: f64, b: &Big) -> bool {
    if x.is_nan() || x.is_infinite() || x.fract() != 0.0 {
        return false;
    }
    cmp_number_bigint(x, b) == Some(core::cmp::Ordering::Equal)
}

enum BitOp {
    And,
    Or,
    Xor,
}

/// BigInt bitwise operations via fixed-width two's complement.
fn bigint_bitop(a: &Big, b: &Big, op: BitOp) -> Big {
    let width = a.bit_length().max(b.bit_length()) + 2;
    let limb_count = (width / 32 + 2) as usize;
    let aw = twos_complement(a, limb_count);
    let bw = twos_complement(b, limb_count);
    let mut out = alloc::vec![0u32; limb_count];
    for i in 0..limb_count {
        out[i] = match op {
            BitOp::And => aw[i] & bw[i],
            BitOp::Or => aw[i] | bw[i],
            BitOp::Xor => aw[i] ^ bw[i],
        };
    }
    let negative = out[limb_count - 1] >> 31 == 1;
    if negative {
        // Two's-complement negate the magnitude, then mark negative.
        for l in out.iter_mut() {
            *l = !*l;
        }
        let mut carry = 1u64;
        for l in out.iter_mut() {
            let cur = *l as u64 + carry;
            *l = cur as u32;
            carry = cur >> 32;
        }
        Big::from_sign_abs(true, out)
    } else {
        Big::from_sign_abs(false, out)
    }
}

/// The `limb_count`-limb two's complement representation of a BigInt.
fn twos_complement(a: &Big, limb_count: usize) -> alloc::vec::Vec<u32> {
    let mut v = alloc::vec![0u32; limb_count];
    if !a.neg {
        for (i, l) in a.mag.iter().enumerate().take(limb_count) {
            v[i] = *l;
        }
    } else {
        // -|a| → invert |a| then add one.
        for (i, l) in a.mag.iter().enumerate().take(limb_count) {
            v[i] = !*l;
        }
        let mut carry = 1u64;
        for l in v.iter_mut() {
            let cur = *l as u64 + carry;
            *l = cur as u32;
            carry = cur >> 32;
        }
    }
    v
}
