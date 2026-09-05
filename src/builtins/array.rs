use alloc::rc::Rc;
use alloc::vec::Vec;
use core::cell::RefCell;

use crate::error::Error;
use crate::interpreter::Engine;
use crate::value::{ArrayData, Value};

use super::helpers::fract;

pub(crate) fn as_array(this: &Value) -> Option<Rc<RefCell<ArrayData>>> {
    if let Value::Array(a) = this {
        Some(a.clone())
    } else {
        None
    }
}

pub(crate) fn arr_push(_e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let arr = as_array(this).ok_or_else(|| Error::Runtime(Value::String(Rc::from("not an array"))))?;
    for v in a {
        arr.borrow_mut().elems.push(v.clone());
    }
    Ok(Value::Number(arr.borrow().elems.len() as f64))
}

pub(crate) fn arr_pop(_e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    let arr = as_array(this).ok_or_else(|| Error::Runtime(Value::String(Rc::from("not an array"))))?;
    Ok(arr.borrow_mut().elems.pop().unwrap_or(Value::Undefined))
}

pub(crate) fn arr_shift(_e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    let arr = as_array(this).ok_or_else(|| Error::Runtime(Value::String(Rc::from("not an array"))))?;
    if arr.borrow().elems.is_empty() {
        return Ok(Value::Undefined);
    }
    Ok(arr.borrow_mut().elems.remove(0))
}

pub(crate) fn arr_unshift(_e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let arr = as_array(this).ok_or_else(|| Error::Runtime(Value::String(Rc::from("not an array"))))?;
    let mut elems = arr.borrow_mut();
    for v in a.iter().rev() {
        elems.elems.insert(0, v.clone());
    }
    Ok(Value::Number(elems.elems.len() as f64))
}

pub(crate) fn arr_for_each(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let arr = as_array(this).ok_or_else(|| Error::Runtime(Value::String(Rc::from("not an array"))))?;
    let cb = a.first().cloned().unwrap_or(Value::Undefined);
    let elems = arr.borrow().elems.clone();
    for (i, v) in elems.iter().enumerate() {
        e.call_value(&cb, &Value::Undefined, &[v.clone(), Value::Number(i as f64), this.clone()])?;
    }
    Ok(Value::Undefined)
}

pub(crate) fn arr_map(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let arr = as_array(this).ok_or_else(|| Error::Runtime(Value::String(Rc::from("not an array"))))?;
    let cb = a.first().cloned().unwrap_or(Value::Undefined);
    let elems = arr.borrow().elems.clone();
    let mut out = Vec::new();
    for (i, v) in elems.iter().enumerate() {
        let r = e.call_value(&cb, &Value::Undefined, &[v.clone(), Value::Number(i as f64), this.clone()])?;
        out.push(r);
    }
    Ok(Value::Array(Rc::new(RefCell::new(ArrayData::new(out, Some(e.array_prototype.clone()))))))
}

pub(crate) fn arr_filter(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let arr = as_array(this).ok_or_else(|| Error::Runtime(Value::String(Rc::from("not an array"))))?;
    let cb = a.first().cloned().unwrap_or(Value::Undefined);
    let elems = arr.borrow().elems.clone();
    let mut out = Vec::new();
    for (i, v) in elems.iter().enumerate() {
        let keep = e.call_value(&cb, &Value::Undefined, &[v.clone(), Value::Number(i as f64), this.clone()])?;
        if keep.to_boolean() {
            out.push(v.clone());
        }
    }
    Ok(Value::Array(Rc::new(RefCell::new(ArrayData::new(out, Some(e.array_prototype.clone()))))))
}

pub(crate) fn arr_index_of(_e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let arr = as_array(this).ok_or_else(|| Error::Runtime(Value::String(Rc::from("not an array"))))?;
    let target = a.first().cloned().unwrap_or(Value::Undefined);
    let from = a.get(1).and_then(|v| {
        if *v == Value::Undefined { None } else { Some(v.to_number() as usize) }
    }).unwrap_or(0);
    let elems = arr.borrow().elems.clone();
    for (i, v) in elems.iter().enumerate().skip(from) {
        if Value::strict_eq(v, &target) {
            return Ok(Value::Number(i as f64));
        }
    }
    Ok(Value::Number(-1.0))
}

pub(crate) fn arr_last_index_of(_e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let arr = as_array(this).ok_or_else(|| Error::Runtime(Value::String(Rc::from("not an array"))))?;
    let target = a.first().cloned().unwrap_or(Value::Undefined);
    let elems = arr.borrow().elems.clone();
    for i in (0..elems.len()).rev() {
        if Value::strict_eq(&elems[i], &target) {
            return Ok(Value::Number(i as f64));
        }
    }
    Ok(Value::Number(-1.0))
}

pub(crate) fn arr_includes(_e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let arr = as_array(this).ok_or_else(|| Error::Runtime(Value::String(Rc::from("not an array"))))?;
    let target = a.first().cloned().unwrap_or(Value::Undefined);
    let elems = arr.borrow().elems.clone();
    for v in &elems {
        if Value::same_value(v, &target) {
            return Ok(Value::Boolean(true));
        }
    }
    Ok(Value::Boolean(false))
}

pub(crate) fn arr_slice(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let arr = as_array(this).ok_or_else(|| Error::Runtime(Value::String(Rc::from("not an array"))))?;
    let elems = arr.borrow().elems.clone();
    let len = elems.len();
    let mut start = a.first().map(|v| v.to_number() as isize).unwrap_or(0);
    if start < 0 {
        start += len as isize;
    }
    start = start.clamp(0, len as isize);
    let mut end = a.get(1).map(|v| v.to_number() as isize).unwrap_or(len as isize);
    if end < 0 {
        end += len as isize;
    }
    end = end.clamp(0, len as isize);
    let out: Vec<Value> = elems[start as usize..end as usize].to_vec();
    Ok(Value::Array(Rc::new(RefCell::new(ArrayData::new(out, Some(e.array_prototype.clone()))))))
}

pub(crate) fn arr_splice(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let arr = as_array(this).ok_or_else(|| Error::Runtime(Value::String(Rc::from("not an array"))))?;
    if a.is_empty() {
        return Ok(Value::Array(Rc::new(RefCell::new(ArrayData::new(Vec::new(), Some(e.array_prototype.clone()))))));
    }
    let len = arr.borrow().elems.len();
    let start = a[0].to_number() as isize;
    let start = if start < 0 { (len as isize + start).clamp(0, len as isize) } else { start.clamp(0, len as isize) } as usize;
    let delete = if a.len() > 1 { a[1].to_number() as usize } else { len - start };
    let mut elems = arr.borrow_mut();
    let removed: Vec<Value> = elems.elems.drain(start..(start + delete).min(len)).collect();
    for item in a[2..].iter().rev() {
        elems.elems.insert(start, item.clone());
    }
    Ok(Value::Array(Rc::new(RefCell::new(ArrayData::new(removed, Some(e.array_prototype.clone()))))))
}

pub(crate) fn arr_concat(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let mut out = Vec::new();
    if let Some(arr) = as_array(this) {
        out.extend(arr.borrow().elems.clone());
    } else {
        out.push(this.clone());
    }
    for v in a {
        if let Some(arr) = as_array(v) {
            out.extend(arr.borrow().elems.clone());
        } else {
            out.push(v.clone());
        }
    }
    Ok(Value::Array(Rc::new(RefCell::new(ArrayData::new(out, Some(e.array_prototype.clone()))))))
}

pub(crate) fn arr_join(_e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let arr = as_array(this).ok_or_else(|| Error::Runtime(Value::String(Rc::from("not an array"))))?;
    let sep = match a.first() {
        Some(Value::String(s)) => s.to_string(),
        Some(Value::Undefined) | None => ",".to_string(),
        Some(v) => v.to_string().to_string(),
    };
    let elems = arr.borrow().elems.clone();
    let parts: Vec<alloc::string::String> = elems
        .iter()
        .map(|v| if *v == Value::Undefined || *v == Value::Null { alloc::string::String::new() } else { v.to_string().to_string() })
        .collect();
    Ok(Value::String(Rc::from(parts.join(&sep))))
}

pub(crate) fn arr_reverse(_e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    let arr = as_array(this).ok_or_else(|| Error::Runtime(Value::String(Rc::from("not an array"))))?;
    arr.borrow_mut().elems.reverse();
    Ok(this.clone())
}

pub(crate) fn arr_fill(_e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let arr = as_array(this).ok_or_else(|| Error::Runtime(Value::String(Rc::from("not an array"))))?;
    let val = a.first().cloned().unwrap_or(Value::Undefined);
    let len = arr.borrow().elems.len();
    let mut start = a.get(1).map(|v| v.to_number() as usize).unwrap_or(0).min(len);
    let mut end = a.get(2).map(|v| v.to_number() as usize).unwrap_or(len).min(len);
    if let Some(Value::Number(n)) = a.get(1) {
        if n.is_nan() { start = 0; }
    }
    if let Some(Value::Number(n)) = a.get(2) {
        if n.is_nan() { end = 0; }
    }
    for i in start..end {
        arr.borrow_mut().elems[i] = val.clone();
    }
    Ok(this.clone())
}

pub(crate) fn arr_find(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let arr = as_array(this).ok_or_else(|| Error::Runtime(Value::String(Rc::from("not an array"))))?;
    let cb = a.first().cloned().unwrap_or(Value::Undefined);
    let elems = arr.borrow().elems.clone();
    for (i, v) in elems.iter().enumerate() {
        let r = e.call_value(&cb, &Value::Undefined, &[v.clone(), Value::Number(i as f64), this.clone()])?;
        if r.to_boolean() {
            return Ok(v.clone());
        }
    }
    Ok(Value::Undefined)
}

pub(crate) fn arr_find_index(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let arr = as_array(this).ok_or_else(|| Error::Runtime(Value::String(Rc::from("not an array"))))?;
    let cb = a.first().cloned().unwrap_or(Value::Undefined);
    let elems = arr.borrow().elems.clone();
    for (i, v) in elems.iter().enumerate() {
        let r = e.call_value(&cb, &Value::Undefined, &[v.clone(), Value::Number(i as f64), this.clone()])?;
        if r.to_boolean() {
            return Ok(Value::Number(i as f64));
        }
    }
    Ok(Value::Number(-1.0))
}

pub(crate) fn arr_some(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let arr = as_array(this).ok_or_else(|| Error::Runtime(Value::String(Rc::from("not an array"))))?;
    let cb = a.first().cloned().unwrap_or(Value::Undefined);
    let elems = arr.borrow().elems.clone();
    for (i, v) in elems.iter().enumerate() {
        let r = e.call_value(&cb, &Value::Undefined, &[v.clone(), Value::Number(i as f64), this.clone()])?;
        if r.to_boolean() {
            return Ok(Value::Boolean(true));
        }
    }
    Ok(Value::Boolean(false))
}

pub(crate) fn arr_every(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let arr = as_array(this).ok_or_else(|| Error::Runtime(Value::String(Rc::from("not an array"))))?;
    let cb = a.first().cloned().unwrap_or(Value::Undefined);
    let elems = arr.borrow().elems.clone();
    for (i, v) in elems.iter().enumerate() {
        let r = e.call_value(&cb, &Value::Undefined, &[v.clone(), Value::Number(i as f64), this.clone()])?;
        if !r.to_boolean() {
            return Ok(Value::Boolean(false));
        }
    }
    Ok(Value::Boolean(true))
}

pub(crate) fn arr_reduce(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let arr = as_array(this).ok_or_else(|| Error::Runtime(Value::String(Rc::from("not an array"))))?;
    let cb = a.first().cloned().unwrap_or(Value::Undefined);
    let mut acc = a.get(1).cloned().unwrap_or(Value::Undefined);
    let elems = arr.borrow().elems.clone();
    for (i, v) in elems.iter().enumerate() {
        acc = e.call_value(&cb, &Value::Undefined, &[acc, v.clone(), Value::Number(i as f64), this.clone()])?;
    }
    Ok(acc)
}

pub(crate) fn arr_at(_e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let arr = as_array(this).ok_or_else(|| Error::Runtime(Value::String(Rc::from("not an array"))))?;
    let len = arr.borrow().elems.len();
    let mut i = a.first().map(|v| v.to_number() as isize).unwrap_or(0);
    if i < 0 {
        i += len as isize;
    }
    if i < 0 || i >= len as isize {
        return Ok(Value::Undefined);
    }
    Ok(arr.borrow().elems[i as usize].clone())
}

// --- Array constructor & statics ---

pub(crate) fn array_ctor(e: &Engine, _this: &Value, a: &[Value], _construct: bool) -> Result<Value, Error> {
    if a.len() == 1 && matches!(a[0], Value::Number(_)) {
        let n = a[0].to_number() as usize;
        if fract(a[0].to_number()) != 0.0 || n > 1_000_000_000 {
            return Err(Error::Runtime(Value::String(Rc::from("Invalid array length"))));
        }
        return Ok(Value::Array(Rc::new(RefCell::new(ArrayData::new(
            alloc::vec![Value::Undefined; n],
            Some(e.array_prototype.clone()),
        )))));
    }
    let elems = a.to_vec();
    Ok(Value::Array(Rc::new(RefCell::new(ArrayData::new(
        elems,
        Some(e.array_prototype.clone()),
    )))))
}

pub(crate) fn arr_is_array(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Boolean(matches!(a.first(), Some(Value::Array(_)))))
}

pub(crate) fn arr_from(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let src = a.first().cloned().unwrap_or(Value::Undefined);
    let mut out = Vec::new();
    if let Value::Array(arr) = &src {
        out.extend(arr.borrow().elems.clone());
    } else if let Value::Object(o) = &src {
        if let Some(len) = o.borrow().props.get("length") {
            let n = len.value.to_number() as usize;
            for i in 0..n {
                let key = Rc::from(i.to_string());
                if let Some(p) = o.borrow().props.get(&key) {
                    out.push(p.value.clone());
                } else {
                    out.push(Value::Undefined);
                }
            }
        }
    }
    let cb = a.get(1).cloned().unwrap_or(Value::Undefined);
    if cb != Value::Undefined {
        let mapped: Result<Vec<Value>, Error> = out
            .iter()
            .enumerate()
            .map(|(i, v)| e.call_value(&cb, &Value::Undefined, &[v.clone(), Value::Number(i as f64)]))
            .collect();
        out = mapped?;
    }
    Ok(Value::Array(Rc::new(RefCell::new(ArrayData::new(out, Some(e.array_prototype.clone()))))))
}

pub(crate) fn arr_of(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(Value::Array(Rc::new(RefCell::new(ArrayData::new(
        a.to_vec(),
        Some(e.array_prototype.clone()),
    )))))
}
