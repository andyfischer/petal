use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::value::Value;

#[derive(Debug, Clone)]
pub struct Environment {
    inner: Rc<RefCell<EnvInner>>,
}

#[derive(Debug, Clone)]
struct EnvInner {
    values: HashMap<String, Value>,
    parent: Option<Environment>,
}

impl Environment {
    pub fn new() -> Self {
        Self {
            inner: Rc::new(RefCell::new(EnvInner {
                values: HashMap::new(),
                parent: None,
            })),
        }
    }

    pub fn with_parent(parent: &Environment) -> Self {
        Self {
            inner: Rc::new(RefCell::new(EnvInner {
                values: HashMap::new(),
                parent: Some(parent.clone()),
            })),
        }
    }

    pub fn get(&self, name: &str) -> Option<Value> {
        let inner = self.inner.borrow();
        if let Some(val) = inner.values.get(name) {
            Some(val.clone())
        } else if let Some(ref parent) = inner.parent {
            parent.get(name)
        } else {
            None
        }
    }

    pub fn set(&self, name: &str, value: Value) {
        self.inner
            .borrow_mut()
            .values
            .insert(name.to_string(), value);
    }

    /// Set a variable in the scope where it's already defined, or in current scope if new
    pub fn assign(&self, name: &str, value: Value) -> bool {
        {
            let inner = self.inner.borrow();
            if inner.values.contains_key(name) {
                drop(inner);
                self.inner
                    .borrow_mut()
                    .values
                    .insert(name.to_string(), value);
                return true;
            }
        }
        let inner = self.inner.borrow();
        if let Some(ref parent) = inner.parent {
            parent.assign(name, value)
        } else {
            false
        }
    }
}
