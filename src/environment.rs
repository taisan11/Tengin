use alloc::rc::Rc;
use core::cell::RefCell;
use std::collections::{HashMap, HashSet};

use crate::value::Value;

/// A lexical scope. Variable resolution walks the `outer` chain.
#[derive(Debug, Clone)]
pub struct Env {
    pub vars: HashMap<Rc<str>, Value>,
    pub outer: Option<Rc<RefCell<Env>>>,
    /// Whether `var` declarations bind inside this scope (`true` for function
    /// and global scopes) rather than being hoisted to a containing function.
    pub function_scope: bool,
    /// Names bound as `const` in this scope (reassignment is a TypeError).
    pub constants: HashSet<Rc<str>>,
}

impl Env {
    pub fn new_global() -> Self {
        Env {
            vars: HashMap::new(),
            outer: None,
            function_scope: true,
            constants: HashSet::new(),
        }
    }

    /// Create a new function (or catch-free) scope whose `var`s bind locally.
    pub fn new_child(outer: Rc<RefCell<Env>>) -> Self {
        Env {
            vars: HashMap::new(),
            outer: Some(outer),
            function_scope: true,
            constants: HashSet::new(),
        }
    }

    /// Create a block scope (`let`/`const` bind here; `var` climbs out).
    pub fn new_block(outer: Rc<RefCell<Env>>) -> Self {
        Env {
            vars: HashMap::new(),
            outer: Some(outer),
            function_scope: false,
            constants: HashSet::new(),
        }
    }

    /// The nearest enclosing function/global scope, used to place `var`s.
    ///
    /// Walks the strong `outer` chain until a scope with
    /// `function_scope == true` is found (the global scope always is).
    pub fn function_scope(env: &Rc<RefCell<Env>>) -> Rc<RefCell<Env>> {
        let mut cur = env.clone();
        loop {
            let (is_fn, next): (bool, Option<Rc<RefCell<Env>>>) = {
                let c = cur.borrow();
                (c.function_scope, c.outer.clone())
            };
            if is_fn {
                return cur;
            }
            match next {
                Some(o) => cur = o,
                None => return cur,
            }
        }
    }

    /// Whether `name` is bound as a `const` in the scope chain that owns it.
    pub fn binding_is_const(name: &str, env: &Rc<RefCell<Env>>) -> bool {
        let mut cur = env.clone();
        loop {
            let (found, is_const, next): (bool, bool, Option<Rc<RefCell<Env>>>) = {
                let g = cur.borrow();
                (g.vars.contains_key(name), g.constants.contains(name), g.outer.clone())
            };
            if found {
                return is_const;
            }
            match next {
                Some(o) => cur = o,
                None => return false,
            }
        }
    }

    pub fn get(&self, name: &str) -> Option<Value> {
        if let Some(v) = self.vars.get(name) {
            return Some(v.clone());
        }
        if let Some(o) = &self.outer {
            return o.borrow().get(name);
        }
        None
    }

    pub fn set(&mut self, name: &str, val: Value) {
        if self.vars.contains_key(name) {
            self.vars.insert(Rc::from(name), val);
            return;
        }
        if let Some(o) = &self.outer {
            o.borrow_mut().set(name, val);
            return;
        }
        self.vars.insert(Rc::from(name), val);
    }
}
