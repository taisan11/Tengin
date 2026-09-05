use alloc::string::ToString;
use alloc::format;
use alloc::rc::Rc;
use alloc::vec::Vec;
use core::cell::RefCell;

use crate::error::Error;
use crate::interpreter::Engine;
use crate::value::{ArrayData, RegexData, Value};

use super::regexp::{regexp_match, regexp_replace, regexp_search, regexp_split};

pub(crate) fn as_string(this: &Value, e: &Engine) -> alloc::string::String {
    e.this_primitive(this).to_string().to_string()
}

/// Coerce `this` to a string, throwing a `TypeError` when it is `null` or
/// `undefined` (as required by `String.prototype` methods used via `.call()`).
fn str_this(e: &Engine, this: &Value) -> Result<alloc::string::String, Error> {
    match this {
        Value::Undefined | Value::Null => Err(Error::Runtime(e.make_type_error(
            "String.prototype method called on null or undefined",
        ))),
        _ => Ok(e.this_primitive(this).to_string().to_string()),
    }
}

/// ECMAScript `ToIntegerOrInfinity`: `NaN` → 0, otherwise truncate toward zero.
fn to_integer(n: f64) -> f64 {
    if n.is_nan() {
        0.0
    } else {
        n.trunc()
    }
}

/// Escape an attribute value per Annex B `CreateHTML`: `&` → `&amp;`, `"` → `&quot;`.
fn escape_html(s: &str) -> alloc::string::String {
    let mut out = alloc::string::String::new();
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// Build an HTML wrapper tag per Annex B `CreateHTML`.
fn html_wrap(
    e: &Engine,
    this: &Value,
    tag: &str,
    attr: Option<&str>,
    attr_val: Option<Value>,
) -> Result<Value, Error> {
    let body = str_this(e, this)?;
    let mut out = alloc::format!("<{tag}");
    if let (Some(attr), Some(v)) = (attr, attr_val) {
        let escaped = escape_html(&v.to_string());
        out = alloc::format!("<{tag} {attr}=\"{escaped}\"");
    }
    out.push('>');
    out.push_str(&body);
    out.push_str(&alloc::format!("</{tag}>"));
    Ok(Value::String(Rc::from(out)))
}

pub(crate) fn str_anchor(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    html_wrap(e, this, "a", Some("name"), a.first().cloned())
}

pub(crate) fn str_link(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    html_wrap(e, this, "a", Some("href"), a.first().cloned())
}

pub(crate) fn str_fontcolor(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    html_wrap(e, this, "font", Some("color"), a.first().cloned())
}

pub(crate) fn str_fontsize(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    html_wrap(e, this, "font", Some("size"), a.first().cloned())
}

pub(crate) fn str_big(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    html_wrap(e, this, "big", None, None)
}

pub(crate) fn str_blink(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    html_wrap(e, this, "blink", None, None)
}

pub(crate) fn str_bold(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    html_wrap(e, this, "b", None, None)
}

pub(crate) fn str_fixed(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    html_wrap(e, this, "tt", None, None)
}

pub(crate) fn str_italics(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    html_wrap(e, this, "i", None, None)
}

pub(crate) fn str_small(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    html_wrap(e, this, "small", None, None)
}

pub(crate) fn str_strike(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    html_wrap(e, this, "strike", None, None)
}

pub(crate) fn str_sub(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    html_wrap(e, this, "sub", None, None)
}

pub(crate) fn str_sup(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    html_wrap(e, this, "sup", None, None)
}

/// Legacy `String.prototype.substr(start, length)` (Annex B).
pub(crate) fn str_substr(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = str_this(e, this)?;
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len() as f64;
    let mut start = a.first().map(|v| to_integer(v.to_number())).unwrap_or(0.0);
    if start < 0.0 {
        start = (len + start).max(0.0);
    }
    let start = start.min(len).max(0.0) as usize;
    let end = match a.get(1) {
        Some(l) => {
            let l = to_integer(l.to_number());
            if l < 0.0 {
                return Ok(Value::String(Rc::from("")));
            }
            (start as f64 + l).min(len) as usize
        }
        None => len as usize,
    };
    let out: alloc::string::String = chars[start..end.max(start)].iter().collect();
    Ok(Value::String(Rc::from(out)))
}

pub(crate) fn str_char_at(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = as_string(this, e);
    let i = a.first().map(|v| v.to_number() as isize).unwrap_or(0);
    if i < 0 || i >= s.chars().count() as isize {
        return Ok(Value::String(Rc::from("")));
    }
    let ch = s.chars().nth(i as usize).unwrap();
    Ok(Value::String(Rc::from(ch.to_string())))
}

pub(crate) fn str_char_code_at(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = as_string(this, e);
    let i = a.first().map(|v| v.to_number() as isize).unwrap_or(0);
    if i < 0 || i >= s.chars().count() as isize {
        return Ok(Value::Number(f64::NAN));
    }
    let ch = s.chars().nth(i as usize).unwrap();
    Ok(Value::Number(ch as u32 as f64))
}

pub(crate) fn str_code_point_at(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    str_char_code_at(e, this, a, _c)
}

pub(crate) fn str_index_of(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = as_string(this, e);
    let pat = a.first().map(|v| v.to_string().to_string()).unwrap_or_default();
    let from = a.get(1).map(|v| v.to_number() as usize).unwrap_or(0);
    if from > 0 {
        let prefix: alloc::string::String = s.chars().take(from).collect();
        if let Some(idx) = s[prefix.len()..].find(&pat) {
            return Ok(Value::Number((prefix.chars().count() + idx) as f64));
        }
        return Ok(Value::Number(-1.0));
    }
    match s.find(&pat) {
        Some(i) => Ok(Value::Number(s[..i].chars().count() as f64)),
        None => Ok(Value::Number(-1.0)),
    }
}

pub(crate) fn str_last_index_of(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = as_string(this, e);
    let pat = a.first().map(|v| v.to_string().to_string()).unwrap_or_default();
    match s.rfind(&pat) {
        Some(i) => Ok(Value::Number(s[..i].chars().count() as f64)),
        None => Ok(Value::Number(-1.0)),
    }
}

pub(crate) fn str_includes(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = as_string(this, e);
    let pat = a.first().map(|v| v.to_string().to_string()).unwrap_or_default();
    Ok(Value::Boolean(s.contains(&pat)))
}

pub(crate) fn str_starts_with(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = as_string(this, e);
    let pat = a.first().map(|v| v.to_string().to_string()).unwrap_or_default();
    Ok(Value::Boolean(s.starts_with(&pat)))
}

pub(crate) fn str_ends_with(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = as_string(this, e);
    let pat = a.first().map(|v| v.to_string().to_string()).unwrap_or_default();
    Ok(Value::Boolean(s.ends_with(&pat)))
}

pub(crate) fn str_slice(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = as_string(this, e);
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    let mut start = a.first().map(|v| v.to_number() as isize).unwrap_or(0);
    if start < 0 { start += len as isize; }
    start = start.clamp(0, len as isize);
    let mut end = a.get(1).map(|v| v.to_number() as isize).unwrap_or(len as isize);
    if end < 0 { end += len as isize; }
    end = end.clamp(0, len as isize);
    let out: alloc::string::String = chars[start as usize..end as usize].iter().collect();
    Ok(Value::String(Rc::from(out)))
}

pub(crate) fn str_substring(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = as_string(this, e);
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    let mut start = a.first().map(|v| v.to_number()).unwrap_or(0.0);
    if start < 0.0 || start.is_nan() { start = 0.0; }
    let mut end = a.get(1).map(|v| v.to_number()).unwrap_or(len as f64);
    if end < 0.0 || end.is_nan() { end = 0.0; }
    let (lo, hi) = if start > end { (end as usize, start as usize) } else { (start as usize, end as usize) };
    let hi = hi.min(len);
    let out: alloc::string::String = chars[lo..hi].iter().collect();
    Ok(Value::String(Rc::from(out)))
}

pub(crate) fn str_upper(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = as_string(this, e);
    Ok(Value::String(Rc::from(s.to_uppercase())))
}

pub(crate) fn str_lower(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = as_string(this, e);
    Ok(Value::String(Rc::from(s.to_lowercase())))
}

pub(crate) fn str_trim(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = as_string(this, e);
    Ok(Value::String(Rc::from(s.trim())))
}

pub(crate) fn str_trim_start(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = as_string(this, e);
    Ok(Value::String(Rc::from(s.trim_start())))
}

pub(crate) fn str_trim_end(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = as_string(this, e);
    Ok(Value::String(Rc::from(s.trim_end())))
}

pub(crate) fn str_concat(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let mut s = as_string(this, e);
    for v in a {
        s.push_str(&v.to_string());
    }
    Ok(Value::String(Rc::from(s)))
}

pub(crate) fn str_split(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = as_string(this, e);
    let limit = a
        .get(1)
        .map(|v| v.to_number())
        .map(|n| if n.is_nan() || n < 0.0 { 0 } else { n as usize });
    if let Some(Value::Regex(rx)) = a.first() {
        return Ok(regexp_split(e, rx, &s, limit));
    }
    let pat = a.first().map(|v| v.to_string().to_string()).unwrap_or_default();
    let parts: Vec<Value> = if pat.is_empty() {
        s.chars().map(|c| Value::String(Rc::from(c.to_string()))).collect()
    } else {
        s.split(&pat).map(|p| Value::String(Rc::from(p))).collect()
    };
    let parts = match limit {
        Some(l) => parts.into_iter().take(l).collect(),
        None => parts,
    };
    Ok(Value::Array(Rc::new(RefCell::new(ArrayData::new(parts, Some(e.array_prototype.clone()))))))
}

pub(crate) fn str_repeat(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = as_string(this, e);
    let n = a.first().map(|v| v.to_number()).unwrap_or(0.0);
    if n < 0.0 || n.is_nan() {
        return Err(Error::Runtime(Value::String(Rc::from("Invalid count value"))));
    }
    let n = n as usize;
    if n > 1_000_000_000 {
        return Err(Error::Runtime(Value::String(Rc::from("too many repeats"))));
    }
    Ok(Value::String(Rc::from(s.repeat(n))))
}

pub(crate) fn str_replace(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = as_string(this, e);
    let search = a.first().cloned().unwrap_or(Value::String(Rc::from("")));
    let rep = a.get(1).cloned().unwrap_or(Value::String(Rc::from("")));
    if let Value::Regex(rx) = &search {
        return Ok(Value::String(Rc::from(regexp_replace(e, rx, &s, &rep))));
    }
    let pat = search.to_string().to_string();
    let rep = rep.to_string().to_string();
    Ok(Value::String(Rc::from(s.replacen(&pat, &rep, 1))))
}

pub(crate) fn str_replace_all(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = str_this(e, this)?;
    let search = a.first().cloned().unwrap_or(Value::String(Rc::from("")));
    if let Value::Regex(_) = &search {
        return Err(Error::Runtime(e.make_type_error(
            "String.prototype.replaceAll: searchValue is a RegExp",
        )));
    }
    let search_str = search.to_string();
    let repl = a.get(1).cloned().unwrap_or(Value::String(Rc::from("")));

    let len = s.len();
    let search_len = search_str.len();
    let mut out = alloc::string::String::new();
    let mut i = 0usize;
    loop {
        if i > len {
            break;
        }
        let rel = if search_len == 0 {
            Some(0usize)
        } else {
            s[i..].find(search_str.as_ref())
        };
        let Some(rel_byte) = rel else { break };
        let match_start = i + rel_byte;
        let match_end = match_start + search_len;
        out.push_str(&s[i..match_start]);
        let matched = &s[match_start..match_end];
        match &repl {
            Value::Function(_) | Value::NativeFunction(_) => {
                let args = vec![
                    Value::String(Rc::from(matched)),
                    Value::Number(match_start as f64),
                    Value::String(Rc::from(s.as_str())),
                ];
                let res = e.call_function(&repl, &Value::Undefined, &args)?;
                out.push_str(&res.to_string());
            }
            _ => {
                let rep_str = repl.to_string();
                out.push_str(&get_substitution(&s, match_start, match_end, &rep_str));
            }
        }
        if search_len == 0 {
            let next = s[match_start..].chars().next().map(|c| c.len_utf8()).unwrap_or(0);
            if next == 0 {
                break;
            }
            i = match_start + next;
        } else {
            i = match_end;
        }
    }
    out.push_str(&s[i.min(len)..]);
    Ok(Value::String(Rc::from(out)))
}

/// Annex-B/Edition `GetSubstitution` for a string (non-regexp) replacement:
/// expand `$$`/`$&`/`$``/`$'` (and leave `$n` literal since there are no
/// captured groups).
fn get_substitution(str_: &str, start: usize, end: usize, replacement: &str) -> alloc::string::String {
    let header = &str_[..start];
    let tail = &str_[end..];
    let matched = &str_[start..end];
    let mut out = alloc::string::String::new();
    let mut chars = replacement.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '$' {
            match chars.peek().copied() {
                Some('$') => {
                    out.push('$');
                    chars.next();
                }
                Some('&') => {
                    out.push_str(matched);
                    chars.next();
                }
                Some('`') => {
                    out.push_str(header);
                    chars.next();
                }
                Some('\'') => {
                    out.push_str(tail);
                    chars.next();
                }
                Some(d) if d.is_ascii_digit() && d != '0' => {
                    // No captures; the `$n` is left as-is.
                    out.push('$');
                    out.push(d);
                    chars.next();
                }
                _ => out.push('$'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

pub(crate) fn str_match(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = as_string(this, e);
    let arg = a.first().cloned().unwrap_or(Value::Undefined);
    match arg {
        Value::Regex(rx) => Ok(regexp_match(e, &rx, &s)),
        Value::Undefined | Value::Null => {
            Ok(regexp_match(e, &RegexData::new(Rc::from(""), Rc::from("")), &s))
        }
        other => {
            let pat = other.to_string();
            Ok(regexp_match(e, &RegexData::new(pat, Rc::from("")), &s))
        }
    }
}

pub(crate) fn str_search(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = as_string(this, e);
    let arg = a.first().cloned().unwrap_or(Value::Undefined);
    match arg {
        Value::Regex(rx) => Ok(regexp_search(e, &rx, &s)),
        other => {
            let pat = other.to_string();
            Ok(regexp_search(e, &RegexData::new(pat, Rc::from("")), &s))
        }
    }
}

pub(crate) fn str_pad_start(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    str_pad(this, e, a, true)
}

pub(crate) fn str_pad_end(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    str_pad(this, e, a, false)
}

pub(crate) fn str_pad(this: &Value, e: &Engine, a: &[Value], start: bool) -> Result<Value, Error> {
    let s = as_string(this, e);
    let target = a.first().map(|v| v.to_number()).unwrap_or(0.0) as usize;
    let pad = a.get(1).map(|v| v.to_string().to_string()).unwrap_or_else(|| " ".to_string());
    if s.chars().count() >= target || pad.is_empty() {
        return Ok(Value::String(Rc::from(s)));
    }
    let need = target - s.chars().count();
    let mut fill = alloc::string::String::new();
    while fill.chars().count() < need {
        fill.push_str(&pad);
    }
    let fill: alloc::string::String = fill.chars().take(need).collect();
    let out = if start { format!("{fill}{s}") } else { format!("{s}{fill}") };
    Ok(Value::String(Rc::from(out)))
}

pub(crate) fn str_value_of(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(e.this_primitive(this))
}

pub(crate) fn str_raw(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let template = a.first().cloned().unwrap_or(Value::Undefined);
    let raw = e.get_property(&template, "raw");
    let len = e.get_property(&raw, "length").to_number();
    let len = if len.is_nan() || len < 0.0 { 0 } else { len as usize };
    let subs = &a[1..];
    let mut out = alloc::string::String::new();
    for i in 0..len {
        out.push_str(&e.get_property(&raw, &i.to_string()).to_string());
        if i < subs.len() {
            out.push_str(&subs[i].to_string());
        }
    }
    Ok(Value::String(Rc::from(out)))
}

pub(crate) fn string_ctor(_e: &Engine, this: &Value, a: &[Value], construct: bool) -> Result<Value, Error> {
    let s = if a.is_empty() {
        alloc::string::String::new()
    } else {
        a[0].to_string().to_string()
    };
    if construct {
        super::helpers::set_primitive(this, Value::String(Rc::from(s)));
        Ok(this.clone())
    } else {
        Ok(Value::String(Rc::from(s)))
    }
}

pub(crate) fn str_from_char_code(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let mut s = alloc::string::String::new();
    let mut i = 0;
    let len = a.len();
    while i < len {
        let code = a[i].to_number();
        if code.is_nan() {
            return Err(Error::Runtime(Value::String(Rc::from(
                "String.fromCharCode: invalid code point",
            ))));
        }
        let u = (code as i64) as u16;
        if (0xD800..=0xDBFF).contains(&u) {
            // A high surrogate: combine with an immediately following low
            // surrogate into a single code point, otherwise treat it as malformed
            // (this engine stores strings as Unicode code points, so a lone
            // surrogate cannot be represented and is reported as a URIError).
            if i + 1 < len {
                let next = a[i + 1].to_number();
                let next_u = (next as i64) as u16;
                if (0xDC00..=0xDFFF).contains(&next_u) {
                    let cp = 0x10000
                        + ((u as u32 - 0xD800) << 10)
                        + (next_u as u32 - 0xDC00);
                    s.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
                    i += 2;
                    continue;
                }
            }
            return Err(Error::Runtime(e.make_uri_error("URIError: URI malformed")));
        } else if (0xDC00..=0xDFFF).contains(&u) {
            // A lone low surrogate cannot be represented in a UTF-8 string.
            return Err(Error::Runtime(e.make_uri_error("URIError: URI malformed")));
        }
        s.push(char::from_u32(u as u32).unwrap_or('\u{FFFD}'));
        i += 1;
    }
    Ok(Value::String(Rc::from(s)))
}

pub(crate) fn str_from_code_point(_e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let mut s = alloc::string::String::new();
    for v in a {
        let code = v.to_number();
        if code.is_nan() || !(code.fract() == 0.0) || code < 0.0 || code > 0x10FFFF as f64 {
            return Err(Error::Runtime(Value::String(Rc::from(
                "String.fromCodePoint: invalid code point",
            ))));
        }
        let u = code as u32;
        if (0xD800..=0xDFFF).contains(&u) {
            return Err(Error::Runtime(Value::String(Rc::from(
                "String.fromCodePoint: invalid code point",
            ))));
        }
        s.push(char::from_u32(u).unwrap_or('\u{FFFD}'));
    }
    Ok(Value::String(Rc::from(s)))
}

pub(crate) fn str_locale_compare(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = str_this(e, this)?;
    let other = a.first().cloned().unwrap_or(Value::Undefined).to_string();
    let cmp = s.cmp(&other.to_string());
    Ok(Value::Number(match cmp {
        core::cmp::Ordering::Less => -1.0,
        core::cmp::Ordering::Equal => 0.0,
        core::cmp::Ordering::Greater => 1.0,
    }))
}

pub(crate) fn str_at(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = str_this(e, this)?;
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len() as isize;
    let mut idx = a.first().map(|v| to_integer(v.to_number()) as isize).unwrap_or(0);
    if idx < 0 {
        idx += len;
    }
    if idx < 0 || idx >= len {
        return Ok(Value::Undefined);
    }
    Ok(Value::String(Rc::from(chars[idx as usize].to_string().as_str())))
}

pub(crate) fn str_is_well_formed(_e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = e_this_str(this)?;
    let mut it = s.chars().peekable();
    let wf = loop {
        match it.next() {
            Some(c) => {
                if (0xD800..=0xDBFF).contains(&(c as u32)) {
                    match it.next() {
                        Some(next) if (0xDC00..=0xDFFF).contains(&(next as u32)) => continue,
                        _ => break false,
                    }
                } else if (0xDC00..=0xDFFF).contains(&(c as u32)) {
                    break false;
                }
            }
            None => break true,
        }
    };
    Ok(Value::Boolean(wf))
}

/// `String.prototype.toWellFormed()`.
pub(crate) fn str_to_well_formed(_e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    let s = e_this_str(this)?;
    let mut out = alloc::string::String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        let cu = c as u32;
        if (0xD800..=0xDBFF).contains(&cu) {
            match chars.peek() {
                Some(next) if (0xDC00..=0xDFFF).contains(&(*next as u32)) => {
                    out.push(c);
                    out.push(chars.next().unwrap());
                }
                _ => out.push('\u{FFFD}'),
            }
        } else if (0xDC00..=0xDFFF).contains(&cu) {
            out.push('\u{FFFD}');
        } else {
            out.push(c);
        }
    }
    Ok(Value::String(Rc::from(out)))
}

fn e_this_str(this: &Value) -> Result<alloc::string::String, Error> {
    match this {
        Value::Undefined | Value::Null => Err(Error::Runtime(Value::String(Rc::from(
            "String method called on null or undefined",
        )))),
        _ => Ok(this.primitive_value().unwrap_or_else(|| this.clone()).to_string().to_string()),
    }
}
