use alloc::collections::BTreeSet;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::ToString;
use alloc::vec::Vec;
use core::cell::RefCell;

use crate::ast::*;
use crate::environment::Env;
use crate::error::{Error, Result};
use crate::parser;
use crate::value::{ArrayData, Function, Object, Property, Value};

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
}

impl Engine {
    pub fn new() -> Self {
        let object_prototype = Rc::new(RefCell::new(Object::new_root()));
        let global_object = Rc::new(RefCell::new(Object::with_proto(object_prototype.clone())));
        let global_env = Rc::new(RefCell::new(Env::new_global()));
        global_env
            .borrow_mut()
            .vars
            .insert(Rc::from("__this__"), Value::Object(global_object.clone()));
        let function_prototype = Rc::new(RefCell::new(Object::with_proto(object_prototype.clone())));
        let array_prototype = Rc::new(RefCell::new(Object::with_proto(object_prototype.clone())));
        let string_prototype = Rc::new(RefCell::new(Object::with_proto(object_prototype.clone())));
        let number_prototype = Rc::new(RefCell::new(Object::with_proto(object_prototype.clone())));
        let boolean_prototype = Rc::new(RefCell::new(Object::with_proto(object_prototype.clone())));
        let error_prototype = Rc::new(RefCell::new(Object::with_proto(object_prototype.clone())));
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
        };
        engine.register_builtins();
        engine
    }

    /// Allocate a fresh plain object linked to `Object.prototype`.
    pub(crate) fn new_object(&self) -> Rc<RefCell<Object>> {
        Rc::new(RefCell::new(Object::with_proto(self.object_prototype.clone())))
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
            .insert(Rc::from(name), val);
    }

    pub fn parse_source(&self, src: &str) -> Result<Program> {
        parser::parse(src)
    }

    pub fn eval(&self, src: &str) -> Result<Value> {
        let prog = parser::parse(src)?;
        self.exec_stmts(&prog.stmts, &self.global_env)
    }

    // --- function values ---

    fn make_function(
        &self,
        name: Rc<str>,
        params: Vec<Rc<str>>,
        body: Vec<Stmt>,
        closure: Rc<RefCell<Env>>,
    ) -> Value {
        let f = Rc::new(RefCell::new(Function {
            name: name.clone(),
            params,
            body,
            closure,
            props: crate::value::new_props(),
            proto: None,
        }));
        let proto = self.new_object();
        proto
            .borrow_mut()
            .props
            .insert(Rc::from("constructor"), Property::new(Value::Function(f.clone())));
        f.borrow_mut()
            .props
            .insert(Rc::from("prototype"), Property::new(Value::Object(proto)));
        f.borrow_mut()
            .props
            .insert(Rc::from("name"), Property::new(Value::String(name)));
        f.borrow_mut().proto = Some(self.function_prototype.clone());
        Value::Function(f)
    }

    fn register_builtins(&mut self) {
        crate::builtins::register_builtins(self);
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
                );
                env.borrow_mut().vars.insert(name.clone(), f);
            }
        }
    }

    fn exec_stmts(&self, stmts: &[Stmt], env: &Rc<RefCell<Env>>) -> Result<Value> {
        self.hoist(stmts, env);
        let mut last = Value::Undefined;
        for s in stmts {
            last = self.eval_stmt(s, env)?;
        }
        Ok(last)
    }

    fn eval_stmt(&self, stmt: &Stmt, env: &Rc<RefCell<Env>>) -> Result<Value> {
        match stmt {
            Stmt::Var(list) => {
                for (name, init) in list {
                    let val = match init {
                        Some(e) => self.eval_expr(e, env)?,
                        None => Value::Undefined,
                    };
                    env.borrow_mut().set(name, val);
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
            Stmt::Block(b) => self.exec_stmts(b, env),
            Stmt::Empty => Ok(Value::Undefined),
            Stmt::Break => Err(Error::Break),
            Stmt::Continue => Err(Error::Continue),
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
                            let ce = Rc::new(RefCell::new(Env::new_child(env.clone())));
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
                            Err(Error::Break) => break,
                            Err(Error::Return(v)) => return Err(Error::Return(v)),
                            Err(Error::Continue) => {}
                            Err(e) => return Err(e),
                        }
                    }
                }
                Ok(Value::Undefined)
            }
            Stmt::ForIn { var, name, expr, body } => {
                let obj = self.eval_expr(expr, env)?;
                let keys = self.enumerable_keys(&obj);
                for key in keys {
                    let val = Value::String(key.clone());
                    if *var {
                        env.borrow_mut().set(name, val);
                    } else {
                        self.assign_to(&Expr::Ident(name.clone()), val, env)?;
                    }
                    match self.exec_stmts(body, env) {
                        Ok(_) => {}
                        Err(Error::Break) => break,
                        Err(Error::Continue) => {}
                        Err(Error::Return(v)) => return Err(Error::Return(v)),
                        Err(e) => return Err(e),
                    }
                }
                Ok(Value::Undefined)
            }
            Stmt::For { init, cond, update, body } => {
                if let Some(init) = init {
                    self.eval_stmt(init, env)?;
                }
                loop {
                    if let Some(c) = cond {
                        if !self.eval_expr(c, env)?.to_boolean() {
                            break;
                        }
                    }
                    match self.exec_stmts(body, env) {
                        Ok(_) => {}
                        Err(Error::Break) => break,
                        Err(Error::Continue) => {}
                        Err(Error::Return(v)) => return Err(Error::Return(v)),
                        Err(e) => return Err(e),
                    }
                    if let Some(u) = update {
                        self.eval_expr(u, env)?;
                    }
                }
                Ok(Value::Undefined)
            }
        }
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
            Expr::Member { obj, prop, computed } => {
                let base = self.eval_expr(obj, env)?;
                let key = match computed {
                    Some(idx) => key_of(&self.eval_expr(idx, env)?),
                    None => prop.clone(),
                };
                Ok(self.get_property(&base, &key))
            }
            Expr::Call { callee, args } => {
                let (func, this) = match &**callee {
                    Expr::Member { obj, prop, computed } => {
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
                    argv.push(self.eval_expr(a, env)?);
                }
                self.call_function(&func, &this, &argv)
            }
            Expr::Function { params, body } => {
                Ok(self.make_function(Rc::from(""), params.clone(), body.clone(), env.clone()))
            }
            Expr::New { callee, args } => {
                let func = self.eval_expr(callee, env)?;
                let mut argv = Vec::new();
                for a in args {
                    argv.push(self.eval_expr(a, env)?);
                }
                self.construct(&func, &argv)
            }
            Expr::InstanceOf { left, right } => {
                let obj = self.eval_expr(left, env)?;
                let ctor = self.eval_expr(right, env)?;
                let proto = match &ctor {
                    Value::Function(f) => f
                        .borrow()
                        .props
                        .get("prototype")
                        .and_then(|p| {
                            if let Value::Object(o) = &p.value {
                                Some(o.clone())
                            } else {
                                None
                            }
                        }),
                    Value::NativeFunction(nf) => nf
                        .borrow()
                        .props
                        .get("prototype")
                        .and_then(|p| {
                            if let Value::Object(o) = &p.value {
                                Some(o.clone())
                            } else {
                                None
                            }
                        })
                        .or_else(|| nf.borrow().proto.clone()),
                    _ => None,
                };
                let proto = match proto {
                    Some(p) => p,
                    None => return Ok(Value::Boolean(false)),
                };
                let mut cur = match &obj {
                    Value::Object(o) => Some(o.clone()),
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
                self.assign_to(target, v, env)
            }
            Expr::Array(elems) => {
                let mut v = Vec::new();
                for e in elems {
                    v.push(self.eval_expr(e, env)?);
                }
                Ok(Value::Array(Rc::new(RefCell::new(ArrayData::new(
                    v,
                    Some(self.array_prototype.clone()),
                )))))
            }
            Expr::Object(props) => {
                let obj = Rc::new(RefCell::new(Object::with_proto(
                    self.object_prototype.clone(),
                )));
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
                            );
                            obj.borrow_mut()
                                .props
                                .insert(Rc::from(k.as_ref()), Property::new(f));
                        }
                    }
                }
                Ok(Value::Object(obj))
            }
        }
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
        }
    }

    fn assign_to(&self, target: &Expr, val: Value, env: &Rc<RefCell<Env>>) -> Result<Value> {
        match target {
            Expr::Ident(name) => {
                env.borrow_mut().set(name, val.clone());
                Ok(val)
            }
            Expr::Member { obj, prop, computed } => {
                let base = self.eval_expr(obj, env)?;
                let key = match computed {
                    Some(idx) => key_of(&self.eval_expr(idx, env)?),
                    None => prop.clone(),
                };
                self.set_property(&base, &key, val.clone());
                Ok(val)
            }
            _ => Err(Error::Runtime(Value::String(Rc::from(
                "invalid assignment target",
            )))),
        }
    }

    fn call_function(&self, func: &Value, this: &Value, args: &[Value]) -> Result<Value> {
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
                let child = Rc::new(RefCell::new(Env::new_child(f.closure.clone())));
                for (i, p) in f.params.iter().enumerate() {
                    let v = args.get(i).cloned().unwrap_or(Value::Undefined);
                    child.borrow_mut().vars.insert(p.clone(), v);
                }
                child
                    .borrow_mut()
                    .vars
                    .insert(Rc::from("__this__"), this.clone());
                // `arguments` pseudo-array.
                let mut args_obj = Object::with_proto(self.object_prototype.clone());
                for (i, av) in args.iter().enumerate() {
                    args_obj
                        .props
                        .insert(Rc::from(i.to_string()), Property::new(av.clone()));
                }
                args_obj.props.insert(
                    Rc::from("length"),
                    Property::new(Value::Number(args.len() as f64)),
                );
                child
                    .borrow_mut()
                    .vars
                    .insert(Rc::from("arguments"), Value::Object(Rc::new(RefCell::new(args_obj))));
                match self.exec_stmts(&f.body, &child) {
                    Ok(v) => Ok(v),
                    Err(Error::Return(rv)) => Ok(rv),
                    Err(e) => Err(e),
                }
            }
            _ => Err(Error::Runtime(Value::String(Rc::from(
                "TypeError: called value is not a function",
            )))),
        }
    }

    fn construct(&self, func: &Value, args: &[Value]) -> Result<Value> {
        let proto = match func {
            Value::Function(f) => f
                .borrow()
                .props
                .get("prototype")
                .and_then(|p| {
                    if let Value::Object(o) = &p.value {
                        Some(o.clone())
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| self.object_prototype.clone()),
            Value::NativeFunction(nf) => nf
                .borrow()
                .proto
                .clone()
                .unwrap_or_else(|| self.object_prototype.clone()),
            _ => self.object_prototype.clone(),
        };
        let inst = Rc::new(RefCell::new(Object::with_proto(proto)));
        let this_val = Value::Object(inst.clone());
        let res = match func {
            Value::NativeFunction(nf) => (nf.borrow().func)(self, &this_val, args, true),
            _ => self.call_function(func, &this_val, args),
        }?;
        match res {
            Value::Object(_) | Value::Array(_) | Value::Function(_) => Ok(res),
            _ => Ok(Value::Object(inst)),
        }
    }

    /// Resolve a property on `base`, following the prototype chain. Primitives are
    /// auto-boxed to their corresponding prototype objects for the lookup.
    pub(crate) fn get_property(&self, base: &Value, key: &str) -> Value {
        match base {
            Value::Object(o) => {
                if let Some(pv) = o.borrow().props.get("__value__") {
                    if key == "length" {
                        return Value::Number(pv.value.to_string().len() as f64);
                    }
                }
                if let Some(p) = o.borrow().props.get(key) {
                    return p.value.clone();
                }
                let mut cur = o.borrow().proto.clone();
                while let Some(c) = cur {
                    if let Some(p) = c.borrow().props.get(key) {
                        return p.value.clone();
                    }
                    cur = c.borrow().proto.clone();
                }
                Value::Undefined
            }
            Value::Array(a) => {
                if key == "length" {
                    return Value::Number(a.borrow().elems.len() as f64);
                }
                if let Ok(i) = key.parse::<usize>() {
                    return a.borrow().elems.get(i).cloned().unwrap_or(Value::Undefined);
                }
                return self.lookup_proto(&self.array_prototype, key);
            }
            Value::Function(f) => {
                if let Some(p) = f.borrow().props.get(key) {
                    return p.value.clone();
                }
                let mut cur = f.borrow().proto.clone();
                while let Some(c) = cur {
                    if let Some(p) = c.borrow().props.get(key) {
                        return p.value.clone();
                    }
                    cur = c.borrow().proto.clone();
                }
                Value::Undefined
            }
            Value::String(s) => {
                if key == "length" {
                    return Value::Number(s.len() as f64);
                }
                self.lookup_proto(&self.string_prototype, key)
            }
            Value::Number(_) => self.lookup_proto(&self.number_prototype, key),
            Value::Boolean(_) => self.lookup_proto(&self.boolean_prototype, key),
            Value::NativeFunction(nf) => {
                if let Some(p) = nf.borrow().props.get(key) {
                    return p.value.clone();
                }
                let mut cur = nf.borrow().proto.clone();
                while let Some(c) = cur {
                    if let Some(p) = c.borrow().props.get(key) {
                        return p.value.clone();
                    }
                    cur = c.borrow().proto.clone();
                }
                Value::Undefined
            }
            _ => Value::Undefined,
        }
    }

    /// Look up `key` on a prototype chain rooted at `start`.
    fn lookup_proto(&self, start: &Rc<RefCell<Object>>, key: &str) -> Value {
        let mut cur = Some(start.clone());
        while let Some(c) = cur {
            if let Some(p) = c.borrow().props.get(key) {
                return p.value.clone();
            }
            cur = c.borrow().proto.clone();
        }
        Value::Undefined
    }

    /// If `this` is a primitive wrapper object, return the wrapped primitive;
    /// otherwise return `this` unchanged.
    pub(crate) fn this_primitive(&self, this: &Value) -> Value {
        if let Value::Object(o) = this {
            if let Some(pv) = o.borrow().props.get("__value__") {
                return pv.value.clone();
            }
        }
        this.clone()
    }

    /// Collect the enumerable own + inherited string-keyed properties of `base`,
    /// in prototype-chain order (closest first). Used by `for…in`.
    pub(crate) fn enumerable_keys(&self, base: &Value) -> Vec<Rc<str>> {
        let mut seen: BTreeSet<Rc<str>> = BTreeSet::new();
        match base {
            Value::Array(a) => {
                let len = a.borrow().elems.len();
                for i in 0..len {
                    seen.insert(Rc::from(i.to_string()));
                }
            }
            Value::String(s) => {
                for (i, _) in s.chars().enumerate() {
                    seen.insert(Rc::from(i.to_string()));
                }
            }
            Value::Object(o) => {
                let mut cur = Some(o.clone());
                while let Some(c) = cur {
                    for (k, p) in c.borrow().props.iter() {
                        if k.as_ref() == "__value__" {
                            continue;
                        }
                        if p.enumerable {
                            seen.insert(k.clone());
                        }
                    }
                    cur = c.borrow().proto.clone();
                }
            }
            _ => {}
        }
        seen.into_iter().collect()
    }

    pub(crate) fn set_property(&self, base: &Value, key: &str, val: Value) {
        match base {
            Value::Object(o) => {
                o.borrow_mut()
                    .props
                    .insert(Rc::from(key), Property::new(val));
            }
            Value::Array(a) => {
                if key == "length" {
                    let n = val.to_number() as usize;
                    a.borrow_mut().elems.resize(n, Value::Undefined);
                } else if let Ok(i) = key.parse::<usize>() {
                    let mut arr = a.borrow_mut();
                    while arr.elems.len() <= i {
                        arr.elems.push(Value::Undefined);
                    }
                    arr.elems[i] = val;
                }
            }
            Value::Function(f) => {
                f.borrow_mut()
                    .props
                    .insert(Rc::from(key), Property::new(val));
            }
            Value::NativeFunction(nf) => {
                nf.borrow_mut()
                    .props
                    .insert(Rc::from(key), Property::new(val));
            }
            _ => {}
        }
    }
}

impl Default for Engine {
    fn default() -> Self {
        Engine::new()
    }
}

// --- helpers ---

fn lit_to_value(l: &Lit) -> Value {
    match l {
        Lit::Number(n) => Value::Number(*n),
        Lit::String(s) => Value::String(s.clone()),
        Lit::Bool(b) => Value::Boolean(*b),
        Lit::Undefined => Value::Undefined,
        Lit::Null => Value::Null,
    }
}

fn key_of(v: &Value) -> Rc<str> {
    match v {
        Value::Number(n) => Rc::from(alloc::format!("{}", *n as i64)),
        Value::String(s) => s.clone(),
        _ => v.to_string(),
    }
}

/// ECMAScript `Abstract Equality Comparison` (`==` / `!=`).
fn loose_eq(a: &Value, b: &Value) -> bool {
    use core::mem::discriminant;

    // Same type: fall back to strict equality.
    if discriminant(a) == discriminant(b) {
        return Value::strict_eq(a, b);
    }

    match (a, b) {
        (Value::Null, Value::Undefined) | (Value::Undefined, Value::Null) => true,
        (Value::Number(_), Value::String(_)) => loose_eq(a, &Value::Number(b.to_number())),
        (Value::String(_), Value::Number(_)) => loose_eq(&Value::Number(a.to_number()), b),
        (Value::Boolean(_), _) => loose_eq(&Value::Number(a.to_number()), b),
        (_, Value::Boolean(_)) => loose_eq(a, &Value::Number(b.to_number())),
        (
            Value::String(_) | Value::Number(_),
            Value::Object(_) | Value::Array(_) | Value::Function(_) | Value::NativeFunction(_),
        ) => loose_eq(a, &Value::String(b.to_string())),
        (
            Value::Object(_) | Value::Array(_) | Value::Function(_) | Value::NativeFunction(_),
            Value::String(_) | Value::Number(_),
        ) => loose_eq(&Value::String(a.to_string()), b),
        _ => false,
    }
}

fn eval_binary(op: BinaryOp, l: &Value, r: &Value) -> Value {
    match op {
        BinaryOp::Add => {
            if matches!(l, Value::String(_)) || matches!(r, Value::String(_)) {
                let s = alloc::format!("{}{}", l.to_string(), r.to_string());
                Value::String(Rc::from(s.as_str()))
            } else {
                Value::Number(l.to_number() + r.to_number())
            }
        }
        BinaryOp::Sub => Value::Number(l.to_number() - r.to_number()),
        BinaryOp::Mul => Value::Number(l.to_number() * r.to_number()),
        BinaryOp::Div => Value::Number(l.to_number() / r.to_number()),
        BinaryOp::Rem => Value::Number(l.to_number() % r.to_number()),
        BinaryOp::Eq => Value::Boolean(loose_eq(l, r)),
        BinaryOp::Ne => Value::Boolean(!loose_eq(l, r)),
        BinaryOp::Seq => Value::Boolean(Value::strict_eq(l, r)),
        BinaryOp::Sne => Value::Boolean(!Value::strict_eq(l, r)),
        BinaryOp::Lt => Value::Boolean(l.to_number() < r.to_number()),
        BinaryOp::Gt => Value::Boolean(l.to_number() > r.to_number()),
        BinaryOp::Le => Value::Boolean(l.to_number() <= r.to_number()),
        BinaryOp::Ge => Value::Boolean(l.to_number() >= r.to_number()),
    }
}
