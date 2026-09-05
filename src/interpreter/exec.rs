//! The tree-walking evaluator: statement execution and expression evaluation.
//! All functions are [`crate::interpreter::Engine`] methods so they can share
//! the engine's allocator and lexical environments.

use alloc::rc::Rc;
use alloc::vec::Vec;
use core::cell::RefCell;

use crate::ast::*;
use crate::environment::Env;
use crate::error::{Error, Result};
use crate::value::{CtorRef, Function, Object, Property, Value};

use super::ops::{eval_binary, is_nullish, key_matches, key_of, lit_to_value, to_int32};
use super::Engine;

impl Engine {
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
        let f = Rc::new(RefCell::new(Function {
            name: name.clone(),
            params,
            body,
            closure,
            props: crate::value::new_props(),
            proto: None,
            super_proto: None,
            super_ctor: None,
            this_capture,
            realm: None,
        }));
        let proto = self.new_object();        proto
            .borrow_mut()
            .ctor = Some(CtorRef::Func(Rc::downgrade(&f)));
        f.borrow_mut()
            .props
            .insert(Rc::from("prototype"), Property::new(Value::Object(proto)));
        f.borrow_mut()
            .props
            .insert(Rc::from("name"), Property::config(Value::String(name)));
        f.borrow_mut()
            .props
            .insert(Rc::from("length"), Property::config(Value::Number(params_count as f64)));
        f.borrow_mut().proto = Some(self.function_prototype.clone());
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
    pub(crate) fn exec_stmts(&self, stmts: &[Stmt], env: &Rc<RefCell<Env>>) -> Result<Value> {
        self.hoist(stmts, env);
        let mut last = Value::Undefined;
        for s in stmts {
            last = self.eval_stmt(s, env)?;
        }
        Ok(last)
    }

    fn eval_stmt(&self, stmt: &Stmt, env: &Rc<RefCell<Env>>) -> Result<Value> {
        match stmt {
            Stmt::Var(list, kind) => {
                let is_const = matches!(kind, VarKind::Const);
                // `var` binds in the nearest function/global scope; `let`/`const`
                // bind in the current (block) scope.
                let scope = match kind {
                    VarKind::Var => Env::function_scope(env),
                    _ => env.clone(),
                };
                for (pat, init) in list {
                    let val = match init {
                        Some(e) => self.eval_expr(e, env)?,
                        None => Value::Undefined,
                    };
                    self.bind_decl(pat, &val, &scope, is_const)?;
                }
                Ok(Value::Undefined)
            }
            Stmt::Expr(e) => self.eval_expr(e, env),
            Stmt::FunctionDecl { .. } => Ok(Value::Undefined),
            Stmt::Return(e) => {
                let v = match e {
                    Some(e) => self.eval_expr(e, env)?,
                    None => Value::Undefined,
                };
                Err(Error::Return(v))
            }
            Stmt::Block(b) => {
                let scope = Rc::new(RefCell::new(Env::new_block(env.clone())));
                self.register_env(&scope);
                self.exec_stmts(b, &scope)
            }
            Stmt::Empty => Ok(Value::Undefined),
            Stmt::Break(label) => Err(Error::Break(label.clone())),
            Stmt::Continue(label) => Err(Error::Continue(label.clone())),
            Stmt::Throw(e) => {
                let v = self.eval_expr(e, env)?;
                Err(Error::Runtime(v))
            }
            Stmt::If { cond, then, else_ } => {
                if self.eval_expr(cond, env)?.to_boolean() {
                    self.exec_stmts(then, env)
                } else if !else_.is_empty() {
                    self.exec_stmts(else_, env)
                } else {
                    Ok(Value::Undefined)
                }
            }
            Stmt::Try { try_block, catch, finally } => {
                let res = self.exec_stmts(try_block, env);
                let result = match res {
                    Ok(v) => Ok(v),
                    Err(Error::Runtime(thrown)) => {
                        if let Some((name, cb)) = catch {
                            let ce = Rc::new(RefCell::new(Env::new_block(env.clone())));
                            self.register_env(&ce);
                            ce.borrow_mut().vars.insert(name.clone(), thrown);
                            self.exec_stmts(cb, &ce)
                        } else {
                            Err(Error::Runtime(thrown))
                        }
                    }
                    Err(e) => Err(e),
                };
                if let Some(fb) = finally {
                    if let Err(e) = self.exec_stmts(fb, env) {
                        return Err(e);
                    }
                }
                result
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

    fn exec_labeled(
        &self,
        label: &Rc<str>,
        body: &Stmt,
        env: &Rc<RefCell<Env>>,
    ) -> Result<Value> {
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
        match res {
            Err(Error::Break(Some(l))) if &l == label => Ok(Value::Undefined),
            Err(Error::Continue(Some(l))) if &l == label => Ok(Value::Undefined),
            other => other,
        }
    }

    fn exec_switch(
        &self,
        discriminant: &Expr,
        cases: &[(Option<Expr>, Vec<Stmt>)],
        env: &Rc<RefCell<Env>>,
    ) -> Result<Value> {
        let d = self.eval_expr(discriminant, env)?;
        let mut start: Option<usize> = None;
        let mut default_idx: Option<usize> = None;
        for (i, (test, _)) in cases.iter().enumerate() {
            match test {
                Some(t) => {
                    let tv = self.eval_expr(t, env)?;
                    if Value::strict_eq(&d, &tv) {
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
        if start.is_none() {
            start = default_idx;
        }
        if let Some(start) = start {
            for i in start..cases.len() {
                match self.exec_stmts(&cases[i].1, env) {
                    Ok(_) => {}
                    Err(Error::Break(_)) => break,
                    Err(Error::Return(v)) => return Err(Error::Return(v)),
                    Err(e) => return Err(e),
                }
            }
        }
        Ok(Value::Undefined)
    }

    fn exec_for_in(
        &self,
        label: Option<&Rc<str>>,
        kind: VarKind,
        name: &Pattern,
        expr: &Expr,
        body: &[Stmt],
        env: &Rc<RefCell<Env>>,
    ) -> Result<Value> {
        let obj = self.eval_expr(expr, env)?;
        let keys = self.enumerable_keys(&obj);
        let is_var = matches!(kind, VarKind::Var);
        let var_scope = Env::function_scope(env);
        for key in keys {
            let val = Value::String(key.clone());
            // `let`/`const` produce a fresh per-iteration binding so closures
            // capture that iteration's value; `var` binds once in the function.
            let per_iter: Option<Rc<RefCell<Env>>> = if is_var {
                self.bind_pattern(name, &val, &var_scope)?;
                None
            } else {
                let pe = Rc::new(RefCell::new(Env::new_block(env.clone())));
                self.register_env(&pe);
                self.bind_pattern(name, &val, &pe)?;
                if matches!(kind, VarKind::Const) {
                    mark_const_idents(name, &pe);
                }
                Some(pe)
            };
            let body_env: &Rc<RefCell<Env>> = per_iter.as_ref().unwrap_or(env);
            match self.exec_stmts(body, body_env) {
                Ok(_) => {}
                Err(Error::Break(b)) => {
                    if b.is_none() || b.as_ref() == label {
                        break;
                    } else {
                        return Err(Error::Break(b));
                    }
                }
                Err(Error::Continue(c)) => {
                    if c.is_some() && c.as_ref() != label {
                        return Err(Error::Continue(c));
                    }
                }
                Err(Error::Return(v)) => return Err(Error::Return(v)),
                Err(e) => return Err(e),
            }
        }
        Ok(Value::Undefined)
    }

    fn exec_for_of(
        &self,
        label: Option<&Rc<str>>,
        kind: VarKind,
        name: &Pattern,
        expr: &Expr,
        body: &[Stmt],
        env: &Rc<RefCell<Env>>,
    ) -> Result<Value> {
        let obj = self.eval_expr(expr, env)?;
        let items = self.iterable_values(&obj)?;
        let is_var = matches!(kind, VarKind::Var);
        let var_scope = Env::function_scope(env);
        for val in items {
            // `let`/`const` produce a fresh per-iteration binding so closures
            // capture that iteration's value; `var` binds once in the function.
            let per_iter: Option<Rc<RefCell<Env>>> = if is_var {
                self.bind_pattern(name, &val, &var_scope)?;
                None
            } else {
                let pe = Rc::new(RefCell::new(Env::new_block(env.clone())));
                self.register_env(&pe);
                self.bind_pattern(name, &val, &pe)?;
                if matches!(kind, VarKind::Const) {
                    mark_const_idents(name, &pe);
                }
                Some(pe)
            };
            let body_env: &Rc<RefCell<Env>> = per_iter.as_ref().unwrap_or(env);
            match self.exec_stmts(body, body_env) {
                Ok(_) => {}
                Err(Error::Break(b)) => {
                    if b.is_none() || b.as_ref() == label {
                        break;
                    } else {
                        return Err(Error::Break(b));
                    }
                }
                Err(Error::Continue(c)) => {
                    if c.is_some() && c.as_ref() != label {
                        return Err(Error::Continue(c));
                    }
                }
                Err(Error::Return(v)) => return Err(Error::Return(v)),
                Err(e) => return Err(e),
            }
        }
        Ok(Value::Undefined)
    }

    fn exec_for(
        &self,
        label: Option<&Rc<str>>,
        init: &Option<Box<Stmt>>,
        cond: &Option<Expr>,
        update: &Option<Expr>,
        body: &[Stmt],
        env: &Rc<RefCell<Env>>,
    ) -> Result<Value> {
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
                    self.eval_stmt(init, &scope)?;
                    for (pat, _) in list {
                        pattern_idents(pat, &mut per_iter_names);
                    }
                    loop_scope = Some(scope);
                }
                other => {
                    self.eval_stmt(other, block_env)?;
                }
            }
        }
        let loop_env: &Rc<RefCell<Env>> = loop_scope.as_ref().unwrap_or(block_env);
        loop {
            if let Some(c) = cond {
                if !self.eval_expr(c, loop_env)?.to_boolean() {
                    break;
                }
            }
            // Fresh per-iteration environment snapshotting the loop bindings.
            let mut per_iter: Option<Rc<RefCell<Env>>> = None;
            if !per_iter_names.is_empty() {
                let pe = Rc::new(RefCell::new(Env::new_block(loop_env.clone())));
                self.register_env(&pe);
                for name in &per_iter_names {
                    let val = loop_env.borrow().get(name).unwrap_or(Value::Undefined);
                    pe.borrow_mut().vars.insert(name.clone(), val);
                }
                per_iter = Some(pe);
            }
            let body_env: &Rc<RefCell<Env>> = per_iter.as_ref().unwrap_or(loop_env);
            match self.exec_stmts(body, body_env) {
                Ok(_) => {}
                Err(Error::Break(b)) => {
                    if b.is_none() || b.as_ref() == label {
                        break;
                    } else {
                        return Err(Error::Break(b));
                    }
                }
                Err(Error::Continue(c)) => {
                    if c.is_some() && c.as_ref() != label {
                        return Err(Error::Continue(c));
                    }
                }
                Err(Error::Return(v)) => return Err(Error::Return(v)),
                Err(e) => return Err(e),
            }
            if let Some(u) = update {
                self.eval_expr(u, loop_env)?;
            }
        }
        Ok(Value::Undefined)
    }

    fn exec_while(
        &self,
        label: Option<&Rc<str>>,
        cond: &Expr,
        body: &[Stmt],
        env: &Rc<RefCell<Env>>,
    ) -> Result<Value> {
        while self.eval_expr(cond, env)?.to_boolean() {
            match self.exec_stmts(body, env) {
                Ok(_) => {}
                Err(Error::Break(b)) => {
                    if b.is_none() || b.as_ref() == label {
                        break;
                    } else {
                        return Err(Error::Break(b));
                    }
                }
                Err(Error::Continue(c)) => {
                    if c.is_some() && c.as_ref() != label {
                        return Err(Error::Continue(c));
                    }
                }
                Err(Error::Return(v)) => return Err(Error::Return(v)),
                Err(e) => return Err(e),
            }
        }
        Ok(Value::Undefined)
    }

    fn exec_do_while(
        &self,
        label: Option<&Rc<str>>,
        cond: &Expr,
        body: &[Stmt],
        env: &Rc<RefCell<Env>>,
    ) -> Result<Value> {
        loop {
            match self.exec_stmts(body, env) {
                Ok(_) => {}
                Err(Error::Break(b)) => {
                    if b.is_none() || b.as_ref() == label {
                        break;
                    } else {
                        return Err(Error::Break(b));
                    }
                }
                Err(Error::Continue(c)) => {
                    if c.is_some() && c.as_ref() != label {
                        return Err(Error::Continue(c));
                    }
                }
                Err(Error::Return(v)) => return Err(Error::Return(v)),
                Err(e) => return Err(e),
            }
            if !self.eval_expr(cond, env)?.to_boolean() {
                break;
            }
        }
        Ok(Value::Undefined)
    }

    // --- expression evaluation ---

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
                let base = self.eval_expr(obj, env)?;
                if *optional && is_nullish(&base) {
                    return Ok(Value::Undefined);
                }
                let key = match computed {
                    Some(idx) => key_of(&self.eval_expr(idx, env)?),
                    None => prop.clone(),
                };
                Ok(self.get_property(&base, &key))
            }
            Expr::Call { callee, args, optional } => {
                if *optional {
                    if let Expr::Member { obj, .. } = &**callee {
                        let b = self.eval_expr(obj, env)?;
                        if is_nullish(&b) {
                            return Ok(Value::Undefined);
                        }
                    } else if let Expr::Call { callee: inner, .. } = &**callee {
                        if let Expr::Member { obj, .. } = &**inner {
                            let b = self.eval_expr(obj, env)?;
                            if is_nullish(&b) {
                                return Ok(Value::Undefined);
                            }
                        }
                    }
                }
                if matches!(&**callee, Expr::Super) {
                    let ctor = env
                        .borrow()
                        .get("__super_ctor__")
                        .ok_or_else(|| {
                            Error::Runtime(Value::String(Rc::from(
                                "SyntaxError: 'super' constructor call outside subclass",
                            )))
                        })?;
                    let this = env
                        .borrow()
                        .get("__this__")
                        .unwrap_or_else(|| Value::Object(self.global_object.clone()));
                    let mut argv = Vec::new();
                    for a in args {
                        self.push_arg(&mut argv, a, env)?;
                    }
                    return self.call_function(&ctor, &this, &argv);
                }
                let (func, this) = match &**callee {
                    Expr::Member { obj, prop, computed, .. } => {
                        let base = self.eval_expr(obj, env)?;
                        let func = match computed {
                            Some(idx) => {
                                let k = key_of(&self.eval_expr(idx, env)?);
                                self.get_property(&base, &k)
                            }
                            None => self.get_property(&base, prop),
                        };
                        (func, base)
                    }
                    _ => {
                        let func = self.eval_expr(callee, env)?;
                        (func, Value::Object(self.global_object.clone()))
                    }
                };
                let mut argv = Vec::new();
                for a in args {
                    self.push_arg(&mut argv, a, env)?;
                }
                self.call_function(&func, &this, &argv)
            }
            Expr::Function { params, body } => Ok(self.make_function(
                Rc::from(""),
                params.clone(),
                body.clone(),
                env.clone(),
                None,
            )),
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
                let func = self.eval_expr(callee, env)?;
                let mut argv = Vec::new();
                for a in args {
                    self.push_arg(&mut argv, a, env)?;
                }
                self.construct(&func, &argv)
            }
            Expr::InstanceOf { left, right } => {
                let obj = self.eval_expr(left, env)?;
                let ctor = self.eval_expr(right, env)?;
                let proto = self.proto_of(&ctor);
                let proto = match proto {
                    Some(p) => p,
                    None => return Ok(Value::Boolean(false)),
                };
                let mut cur: Option<Rc<RefCell<Object>>> = match &obj {
                    Value::Object(o) => Some(o.clone()),
                    Value::Array(a) => a.borrow().proto.clone(),
                    Value::Function(f) => f.borrow().proto.clone(),
                    Value::NativeFunction(nf) => nf.borrow().proto.clone(),
                    Value::Map(_) => Some(self.map_prototype.clone()),
                    Value::Set(_) => Some(self.set_prototype.clone()),
                    Value::WeakMap(_) => Some(self.weakmap_prototype.clone()),
                    Value::WeakSet(_) => Some(self.weakset_prototype.clone()),
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
            Expr::Binary { op, left, right } => {
                let l = self.eval_expr(left, env)?;
                let r = self.eval_expr(right, env)?;
                Ok(eval_binary(*op, &l, &r))
            }
            Expr::Logical { op, left, right } => {
                let l = self.eval_expr(left, env)?;
                match op {
                    LogicalOp::And => {
                        if l.to_boolean() {
                            self.eval_expr(right, env)
                        } else {
                            Ok(l)
                        }
                    }
                    LogicalOp::Or => {
                        if l.to_boolean() {
                            Ok(l)
                        } else {
                            self.eval_expr(right, env)
                        }
                    }
                    LogicalOp::Coalesce => {
                        if is_nullish(&l) {
                            self.eval_expr(right, env)
                        } else {
                            Ok(l)
                        }
                    }
                }
            }
            Expr::Unary { op, operand } => self.eval_unary(*op, operand, env),
            Expr::Ternary { cond, then, else_ } => {
                let c = self.eval_expr(cond, env)?;
                if c.to_boolean() {
                    self.eval_expr(then, env)
                } else {
                    self.eval_expr(else_, env)
                }
            }
            Expr::Assignment { target, value } => {
                let v = self.eval_expr(value, env)?;
                self.assign_to_target(target, v, env)
            }
            Expr::Array(elems) => {
                let mut v = Vec::new();
                for e in elems {
                    match e {
                        ArrayElem::Expr(e) => v.push(self.eval_expr(e, env)?),
                        ArrayElem::Spread(s) => {
                            let sv = self.eval_expr(s, env)?;
                            for item in self.iterable_values(&sv)? {
                                v.push(item);
                            }
                        }
                        ArrayElem::Elision => v.push(Value::Undefined),
                    }
                }
                Ok(Value::Array(self.new_array(v)))
            }
            Expr::Object(props) => {
                let obj = self.new_object();
                for p in props {
                    match p {
                        Prop::Init { key, value } => {
                            let v = self.eval_expr(value, env)?;
                            let k = self.eval_prop_key(key, env)?;
                            obj.borrow_mut()
                                .props
                                .insert(Rc::from(k.as_ref()), Property::new(v));
                        }
                        Prop::Method { key, params, body } => {
                            let k = match key {
                                PropKey::Computed(e) => self.eval_expr(e, env)?.to_string(),
                                PropKey::Ident(s) | PropKey::Str(s) => s.clone(),
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
                            let k = match key {
                                PropKey::Computed(e) => self.eval_expr(e, env)?.to_string(),
                                PropKey::Ident(s) | PropKey::Str(s) => s.clone(),
                            };
                            let f = self.make_function(
                                k.clone(),
                                params.clone(),
                                body.clone(),
                                env.clone(),
                                None,
                            );
                            if *get {
                                self.define_accessor(&obj, &k, Some(f), None);
                            } else {
                                self.define_accessor(&obj, &k, None, Some(f));
                            }
                        }
                        Prop::Spread(s) => {
                            let sv = self.eval_expr(s, env)?;
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
                let func = self.eval_expr(tag, env)?;
                let mut strings = Vec::new();
                for c in cooked {
                    strings.push(self.eval_expr(c, env)?);
                }
                let cooked_arr = Value::Array(self.new_array(strings));
                let mut argv = vec![cooked_arr];
                for s in subs {
                    argv.push(self.eval_expr(s, env)?);
                }
                self.call_function(&func, &Value::Undefined, &argv)
            }
            Expr::Await(e) => self.eval_expr(e, env),
            Expr::Yield { expr, delegated: _ } => match expr {
                Some(e) => self.eval_expr(e, env),
                None => Ok(Value::Undefined),
            },
        }
    }

    fn push_arg(&self, argv: &mut Vec<Value>, arg: &Arg, env: &Rc<RefCell<Env>>) -> Result<()> {
        match arg {
            Arg::Expr(e) => argv.push(self.eval_expr(e, env)?),
            Arg::Spread(s) => {
                let sv = self.eval_expr(s, env)?;
                for item in self.iterable_values(&sv)? {
                    argv.push(item);
                }
            }
        }
        Ok(())
    }

    /// Evaluate a `class` expression into a constructor function value.
    fn eval_class(&self, class: &Class, env: &Rc<RefCell<Env>>) -> Result<Value> {
        let parent = match &class.extends {
            Some(e) => Some(self.eval_expr(e, env)?),
            None => None,
        };
        let parent_proto = match &parent {
            Some(p) => self.proto_of(p),
            None => None,
        };
        let base_proto = parent_proto.unwrap_or_else(|| self.object_prototype.clone());
        let proto = self.make_object(base_proto.clone());

        let mut ctor_params: Vec<Pattern> = Vec::new();
        let mut ctor_body: Vec<Stmt> = Vec::new();
        let mut instance_members: Vec<&ClassElem> = Vec::new();
        let mut static_members: Vec<&ClassElem> = Vec::new();
        let mut instance_fields: Vec<(PropKey, Option<Expr>)> = Vec::new();
        let mut static_fields: Vec<(PropKey, Option<Expr>)> = Vec::new();

        for el in &class.elements {
            match el {
                ClassElem::Constructor { params, body } => {
                    ctor_params = params.clone();
                    ctor_body = body.clone();
                }
                ClassElem::Method { is_static, .. } => {
                    if *is_static {
                        static_members.push(el);
                    } else {
                        instance_members.push(el);
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

        // Inject instance field initializers at the top of the constructor body.
        let mut full_ctor_body = Vec::new();
        for (key, init) in &instance_fields {
            let k = match key {
                PropKey::Computed(e) => self.eval_expr(e, env)?.to_string(),
                PropKey::Ident(s) | PropKey::Str(s) => s.clone(),
            };
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
            name: class.name.clone().unwrap_or_else(|| Rc::from("")),
            params: ctor_params,
            body: full_ctor_body,
            closure: env.clone(),
            props: crate::value::new_props(),
            proto: Some(self.function_prototype.clone()),
            super_proto: Some(base_proto.clone()),
            super_ctor: parent.clone(),
            this_capture: None,
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
                class.name.clone().unwrap_or_else(|| Rc::from("")),
            )),
        );
        self.register_function(&ctor);

        for el in instance_members {
            if let ClassElem::Method { get, key, params, body, .. } = el {
                let k = self.eval_prop_key(key, env)?;
                let f = self.make_function(k.clone(), params.clone(), body.clone(), env.clone(), None);
                if let Value::Function(fr) = &f {
                    fr.borrow_mut().super_proto = Some(base_proto.clone());
                }
                if *get {
                    self.define_accessor(&proto, &k, Some(f), None);
                } else {
                    proto
                        .borrow_mut()
                        .props
                        .insert(Rc::from(k.as_ref()), Property::new(f));
                }
            }
        }
        // Static fields/members live on the constructor itself.
        for (key, init) in &static_fields {
            let k = match key {
                PropKey::Computed(e) => self.eval_expr(e, env)?.to_string(),
                PropKey::Ident(s) | PropKey::Str(s) => s.clone(),
            };
            let value = match init {
                Some(e) => self.eval_expr(e, env)?,
                None => Value::Undefined,
            };
            ctor.borrow_mut()
                .props
                .insert(Rc::from(k.as_ref()), Property::new(value));
        }
        for el in static_members {
            if let ClassElem::Method { key, params, body, .. } = el {
                let k = self.eval_prop_key(key, env)?;
                let f = self.make_function(k.clone(), params.clone(), body.clone(), env.clone(), None);
                ctor
                    .borrow_mut()
                    .props
                    .insert(Rc::from(k.as_ref()), Property::new(f));
            }
        }
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

    fn eval_prop_key(&self, key: &PropKey, env: &Rc<RefCell<Env>>) -> Result<Rc<str>> {
        Ok(match key {
            PropKey::Ident(s) | PropKey::Str(s) => s.clone(),
            PropKey::Computed(e) => key_of(&self.eval_expr(e, env)?),
        })
    }

    fn eval_unary(&self, op: UnaryOp, operand: &Expr, env: &Rc<RefCell<Env>>) -> Result<Value> {
        match op {
            UnaryOp::Neg => Ok(Value::Number(-self.eval_expr(operand, env)?.to_number())),
            UnaryOp::Plus => Ok(Value::Number(self.eval_expr(operand, env)?.to_number())),
            UnaryOp::Not => Ok(Value::Boolean(!self.eval_expr(operand, env)?.to_boolean())),
            UnaryOp::Void => {
                self.eval_expr(operand, env)?;
                Ok(Value::Undefined)
            }
            UnaryOp::Typeof => {
                if let Expr::Ident(name) = operand {
                    let defined = env.borrow().get(name).is_some()
                        || self.global_object.borrow().props.contains_key(name);
                    if !defined {
                        return Ok(Value::String(Rc::from("undefined")));
                    }
                }
                let v = self.eval_expr(operand, env)?;
                Ok(Value::String(Rc::from(v.type_of())))
            }
            UnaryOp::BitNot => {
                let v = self.eval_expr(operand, env)?;
                Ok(Value::Number((!to_int32(v.to_number())) as f64))
            }
            UnaryOp::PreInc => {
                let v = self.eval_expr(operand, env)?;
                let n = Value::Number(v.to_number() + 1.0);
                self.assign_to(operand, n.clone(), env)?;
                Ok(n)
            }
            UnaryOp::PreDec => {
                let v = self.eval_expr(operand, env)?;
                let n = Value::Number(v.to_number() - 1.0);
                self.assign_to(operand, n.clone(), env)?;
                Ok(n)
            }
            UnaryOp::PostInc => {
                let v = self.eval_expr(operand, env)?;
                let n = Value::Number(v.to_number() + 1.0);
                self.assign_to(operand, n, env)?;
                Ok(v)
            }
            UnaryOp::PostDec => {
                let v = self.eval_expr(operand, env)?;
                let n = Value::Number(v.to_number() - 1.0);
                self.assign_to(operand, n, env)?;
                Ok(v)
            }
            UnaryOp::Delete => {
                match operand {
                    Expr::Member { obj, prop, computed, .. } => {
                        let base = self.eval_expr(obj, env)?;
                        let key = match computed {
                            Some(idx) => key_of(&self.eval_expr(idx, env)?),
                            None => prop.clone(),
                        };
                        return Ok(Value::Boolean(self.delete_property(&base, &key)?));
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
                        // Numeric-index deletion creates a hole; the engine uses a
                        // dense vector, so report success without a value change.
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
                let base = self.eval_expr(obj, env)?;
                let key = match computed {
                    Some(idx) => key_of(&self.eval_expr(idx, env)?),
                    None => prop.clone(),
                };
                self.set_property(&base, &key, val.clone())?;
                Ok(val)
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
                for (key, p) in props {
                    let k = match key {
                        PropKey::Computed(e) => key_of(&self.eval_expr(e, env)?),
                        PropKey::Ident(s) | PropKey::Str(s) => s.clone(),
                    };
                    let v = self.get_property(&Value::Object(src.clone()), &k);
                    self.bind_pattern(p, &v, env)?;
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
            Pattern::Member(e) => {
                self.assign_to(e, val.clone(), env)?;
                Ok(())
            }
            Pattern::Default { inner, default } => {
                let v = if matches!(val, Value::Undefined) {
                    self.eval_expr(default, env)?
                } else {
                    val.clone()
                };
                self.bind_pattern(inner, &v, env)
            }
        }
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
                for (key, p) in props {
                    let k = match key {
                        PropKey::Computed(e) => key_of(&self.eval_expr(e, env)?),
                        PropKey::Ident(s) | PropKey::Str(s) => s.clone(),
                    };
                    let v = self.get_property(&Value::Object(src.clone()), &k);
                    self.assign_pattern(p, &v, env)?;
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
            Pattern::Member(e) => {
                self.assign_to(e, val.clone(), env)?;
                Ok(())
            }
            Pattern::Default { inner, default } => {
                let v = if matches!(val, Value::Undefined) {
                    self.eval_expr(default, env)?
                } else {
                    val.clone()
                };
                self.assign_pattern(inner, &v, env)
            }
        }
    }

    pub(crate) fn call_function(&self, func: &Value, this: &Value, args: &[Value]) -> Result<Value> {
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
        match func {
            Value::NativeFunction(f) => (f.borrow().func)(self, this, args, false),
            Value::Function(u) => {
                let f = u.borrow();
                let closure_env = f.closure.clone();
                let child = Rc::new(RefCell::new(Env::new_child(closure_env)));
                self.register_env(&child);
                let this_val = f
                    .this_capture
                    .clone()
                    .unwrap_or_else(|| this.clone());
                for (i, p) in f.params.iter().enumerate() {
                    let v = args.get(i).cloned().unwrap_or(Value::Undefined);
                    self.bind_pattern(p, &v, &child)?;
                }
                child
                    .borrow_mut()
                    .vars
                    .insert(Rc::from("__this__"), this_val);
                if let Some(sp) = f.super_proto.clone() {
                    child
                        .borrow_mut()
                        .vars
                        .insert(Rc::from("__super__"), Value::Object(sp));
                }
                if let Some(sc) = f.super_ctor.clone() {
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
                        .insert(Rc::from(i.to_string()), Property::new(av.clone()));
                }
                args_obj.borrow_mut().props.insert(
                    Rc::from("length"),
                    Property::new(Value::Number(args.len() as f64)),
                );
                child
                    .borrow_mut()
                    .vars
                    .insert(Rc::from("arguments"), Value::Object(args_obj));
                match self.exec_stmts(&f.body, &child) {
                    Ok(v) => Ok(v),
                    Err(Error::Return(rv)) => Ok(rv),
                    Err(e) => Err(e),
                }
            }
            _ => Err(Error::Runtime(self.make_type_error(
                "called value is not a function",
            ))),
        }
    }

    /// Whether `v` is callable as a constructor (`new` → object).
    pub(crate) fn is_constructor(&self, v: &Value) -> bool {
        match v {
            Value::Function(_) => true,
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
        let proto = self.proto_of(new_target).or_else(|| self.default_proto(new_target, func));
        let proto = proto.unwrap_or_else(|| self.object_prototype.clone());
        let inst = self.make_object(proto);
        let this_val = Value::Object(inst.clone());
        let res = match func {
            Value::NativeFunction(nf) => {
                if !nf.borrow().constructable {
                    return Err(Error::Runtime(self.make_type_error("not a constructor")));
                }
                let prev = self.native_realm.borrow().clone();
                *self.native_realm.borrow_mut() = nf.borrow().realm.clone();
                let r = (nf.borrow().func)(self, &this_val, args, true);
                *self.native_realm.borrow_mut() = prev;
                r
            }
            _ => self.call_function(func, &this_val, args),
        }?;
        match res {
            Value::Object(_)
            | Value::Array(_)
            | Value::Function(_)
            | Value::NativeFunction(_)
            | Value::Map(_)
            | Value::Set(_)
            | Value::WeakMap(_)
            | Value::WeakSet(_)
            | Value::Regex(_)
            | Value::Symbol(_) => Ok(res),
            _ => Ok(Value::Object(inst)),
        }
    }

    /// `GetPrototypeFromConstructor`: the default prototype for `new_target` when
    /// its own `.prototype` is not an object. The fallback is the intrinsic
    /// `%default%` prototype of the new target's realm (matching the target's kind).
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
        Some(match kind {
            crate::value::RealmProto::Object => realm.object_prototype.clone(),
            crate::value::RealmProto::Number => realm.number_prototype.clone(),
            crate::value::RealmProto::String => realm.string_prototype.clone(),
            crate::value::RealmProto::Boolean => realm.boolean_prototype.clone(),
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
