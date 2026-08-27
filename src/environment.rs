use alloc::rc::Rc;
use alloc::collections::BTreeMap;
use core::cell::RefCell;

use crate::value::Value;

/// A lexical scope. Variable resolution walks the `outer` chain.
#[derive(Debug, Clone)]
pub struct Env {
    pub vars: BTreeMap<Rc<str>, Value>,
    pub outer: Option<Rc<RefCell<Env>>>,
}

impl Env {
    pub fn new_global() -> Self {
        Env {
            vars: BTreeMap::new(),
            outer: None,
        }
    }

    #[allow(dead_code)]
    pub fn new_child(outer: Rc<RefCell<Env>>) -> Self {
        Env {
            vars: BTreeMap::new(),
            outer: Some(outer),
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
