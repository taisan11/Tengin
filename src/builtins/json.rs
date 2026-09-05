use alloc::string::ToString;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;

use crate::error::Error;
use crate::interpreter::Engine;
use crate::value::{ArrayData, Object, Property, Value};

pub(crate) fn json_stringify(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    Ok(Value::String(Rc::from(json_to_string(e, &v))))
}

pub(crate) fn json_to_string(e: &Engine, v: &Value) -> String {
    match v {
        Value::Undefined | Value::Function(_) | Value::NativeFunction(_) => "null".to_string(),
        Value::Symbol(_) => "null".to_string(),
        Value::Map(_) | Value::Set(_) | Value::WeakMap(_) | Value::WeakSet(_) => "{}".to_string(),
        Value::BigInt(s) => s.as_ref().to_string(),
        Value::Regex(_) => "null".to_string(),
        Value::Null => "null".to_string(),
        Value::Boolean(b) => if *b { "true".to_string() } else { "false".to_string() },
        Value::Number(n) => {
            if n.is_nan() || n.is_infinite() {
                "null".to_string()
            } else {
                n.to_string()
            }
        }
        Value::String(s) => format!("\"{}\"", s.as_ref()),
        Value::Array(arr) => {
            let parts: Vec<String> = arr
                .borrow()
                .elems
                .iter()
                .map(|x| json_to_string(e, x))
                .collect();
            format!("[{}]", parts.join(","))
        }
        Value::Object(o) => {
            let mut parts = Vec::new();
            for (k, p) in o.borrow().props.iter() {
                if k.as_ref() == "__value__" {
                    continue;
                }
                parts.push(format!("\"{}\":{}", k.as_ref(), json_to_string(e, &p.value)));
            }
            format!("{{{}}}", parts.join(","))
        }
    }
}

pub(crate) fn json_parse(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = a.first().cloned().unwrap_or(Value::Undefined).to_string();
    parse_json(&s).ok_or_else(|| Error::Runtime(Value::String(Rc::from("JSON.parse failed"))))
}

/// Minimal JSON parser supporting objects, arrays, strings, numbers, and literals.
pub(crate) fn parse_json(s: &str) -> Option<Value> {
    let mut p = JsonParser { chars: s.trim().chars().collect(), pos: 0 };
    p.parse_value()
}

struct JsonParser {
    chars: Vec<char>,
    pos: usize,
}

impl JsonParser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }
    fn next(&mut self) -> Option<char> {
        let c = self.chars.get(self.pos).copied();
        self.pos += 1;
        c
    }
    fn skip_ws(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_whitespace() {
                self.pos += 1;
            } else {
                break;
            }
        }
    }
    fn parse_value(&mut self) -> Option<Value> {
        self.skip_ws();
        match self.peek()? {
            '{' => self.parse_object(),
            '[' => self.parse_array(),
            '"' => Some(Value::String(self.parse_string())),
            't' | 'f' => self.parse_bool(),
            'n' => self.parse_null(),
            _ => self.parse_number(),
        }
    }
    fn parse_object(&mut self) -> Option<Value> {
        self.next()?;
        let o = crate::value::new_props();
        let obj = Value::Object(Rc::new(RefCell::new(Object {
            props: o,
            proto: None,
            ctor: None,
        })));
        self.skip_ws();
        if self.peek()? == '}' {
            self.next();
            return Some(obj);
        }
        loop {
            self.skip_ws();
            let key = self.parse_string();
            self.skip_ws();
            if self.next()? != ':' {
                return None;
            }
            let val = self.parse_value()?;
            if let Value::Object(o) = &obj {
                o.borrow_mut()
                    .props
                    .insert(key.clone(), Property::new(val));
            }
            self.skip_ws();
            match self.next()? {
                ',' => continue,
                '}' => break,
                _ => return None,
            }
        }
        Some(obj)
    }
    fn parse_array(&mut self) -> Option<Value> {
        self.next()?;
        let mut elems = Vec::new();
        self.skip_ws();
        if self.peek()? == ']' {
            self.next();
            return Some(Value::Array(Rc::new(RefCell::new(ArrayData::new(elems, None)))));
        }
        loop {
            let val = self.parse_value()?;
            elems.push(val);
            self.skip_ws();
            match self.next()? {
                ',' => continue,
                ']' => break,
                _ => return None,
            }
        }
        Some(Value::Array(Rc::new(RefCell::new(ArrayData::new(elems, None)))))
    }
    fn parse_string(&mut self) -> Rc<str> {
        self.next();
        let mut s = String::new();
        while let Some(c) = self.next() {
            if c == '"' {
                break;
            }
            if c == '\\' {
                if let Some(e) = self.next() {
                    match e {
                        '"' => s.push('"'),
                        '\\' => s.push('\\'),
                        '/' => s.push('/'),
                        'n' => s.push('\n'),
                        't' => s.push('\t'),
                        'r' => s.push('\r'),
                        _ => s.push(e),
                    }
                }
            } else {
                s.push(c);
            }
        }
        Rc::from(s)
    }
    fn parse_bool(&mut self) -> Option<Value> {
        let start = self.pos;
        let word: String = (0..5).filter_map(|_| self.next()).collect();
        if word.starts_with("true") {
            self.pos = start + 4;
            Some(Value::Boolean(true))
        } else if word.starts_with("false") {
            self.pos = start + 5;
            Some(Value::Boolean(false))
        } else {
            None
        }
    }
    fn parse_null(&mut self) -> Option<Value> {
        let start = self.pos;
        let word: String = (0..4).filter_map(|_| self.next()).collect();
        if word.starts_with("null") {
            self.pos = start + 4;
            Some(Value::Null)
        } else {
            None
        }
    }
    fn parse_number(&mut self) -> Option<Value> {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() || c == '-' || c == '+' || c == '.' || c == 'e' || c == 'E' {
                self.pos += 1;
            } else {
                break;
            }
        }
        let s: String = self.chars[start..self.pos].iter().collect();
        s.parse::<f64>().ok().map(Value::Number)
    }
}
