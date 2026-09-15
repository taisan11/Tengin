use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::format;
use core::cell::RefCell;

use regex::Captures;

use crate::error::Error;
use crate::interpreter::Engine;
use crate::value::{
    compile_regex, native, NativeFn, Object, Property, RegexData, Value,
};

use super::helpers::{proto_data, proto_method, reg_ctor};

// --- index helpers -----------------------------------------------------------

/// Convert a byte offset into `input` to a character index (matching how the
/// rest of the engine indexes strings, e.g. `charAt`/`indexOf`).
fn byte_to_char(input: &str, byte: usize) -> usize {
    input[..byte].chars().count()
}

/// Convert a character index in `input` back to the byte offset used by the
/// `regex` crate. A character index past the end maps to the end of the string.
fn char_to_byte(input: &str, chars: usize) -> usize {
    input
        .char_indices()
        .nth(chars)
        .map(|(b, _)| b)
        .unwrap_or(input.len())
}

fn this_regex<'r>(this: &'r Value, e: &Engine) -> Result<&'r RegexData, Error> {
    match this {
        Value::Regex(rx) => Ok(rx),
        _ => Err(Error::Runtime(
            e.make_type_error("RegExp.prototype method called on an incompatible receiver"),
        )),
    }
}

/// What a `RegExp.prototype` accessor getter was invoked with. Accessors accept
/// either a real RegExp (read its flags) or the `RegExp.prototype` object itself
/// (return the spec-mandated defaults); any other object is a TypeError.
enum GetterTarget<'r> {
    Data(&'r RegexData),
    Default,
}

fn getter_target<'r>(e: &Engine, this: &'r Value) -> Result<GetterTarget<'r>, Error> {
    if let Value::Regex(rx) = this {
        return Ok(GetterTarget::Data(rx));
    }
    if let Value::Object(o) = this {
        if Rc::ptr_eq(o, &e.regexp_prototype) {
            return Ok(GetterTarget::Default);
        }
    }
    Err(Error::Runtime(
        e.make_type_error("RegExp.prototype accessor called on an incompatible receiver"),
    ))
}

// --- matching ----------------------------------------------------------------

/// Find the next match of `rx` in `input`, honouring the `g`/`y` `lastIndex`
/// semantics. Returns the captures and its start byte offset; `None` when there
/// is no match (or compilation failed).
///
/// On a failed search with `g` or `y`, `lastIndex` is reset to 0. On a
/// successful search with `g` or `y`, `lastIndex` is advanced (an empty match
/// advances by one character to avoid an infinite loop).
fn get_match<'h>(rx: &RegexData, input: &'h str) -> Option<(Captures<'h>, usize)> {
    let re = rx.re.borrow().clone();
    let global = rx.has_flag('g');
    let sticky = rx.has_flag('y');
    let start_char = *rx.last_index.borrow();
    let start_byte = char_to_byte(input, start_char);

    let caps = match re {
        Some(re) => re.captures_at(input, start_byte),
        None => None,
    };
    let caps = match caps {
        Some(c) => c,
        None => {
            // A failed search with `g`/`y` resets `lastIndex` to 0.
            if global || sticky {
                *rx.last_index.borrow_mut() = 0;
            }
            return None;
        }
    };

    let m = caps.get(0).unwrap();
    let (s, en) = (m.start(), m.end());
    if sticky && s != start_byte {
        if global || sticky {
            *rx.last_index.borrow_mut() = 0;
        }
        return None;
    }
    if global || sticky {
        let end_char = byte_to_char(input, en);
        *rx.last_index.borrow_mut() = if en == s { start_char + 1 } else { end_char };
    }
    Some((caps, s))
}

/// Build the `exec` result: an array of `[full, ...captures]` carrying `index`,
/// `input`, `groups` and `lastIndex` own properties.
fn make_result(
    e: &Engine,
    rx: &RegexData,
    caps: &Captures,
    input: &str,
    start_byte: usize,
) -> Value {
    let mut elems = Vec::new();
    let full = caps.get(0).unwrap();
    elems.push(Value::String(Rc::from(full.as_str())));
    for i in 1..caps.len() {
        elems.push(match caps.get(i) {
            Some(m) => Value::String(Rc::from(m.as_str())),
            None => Value::Undefined,
        });
    }
    let arr = e.new_array(elems);
    let index = byte_to_char(input, start_byte);

    // Determine named capture groups (if any existed on the pattern).
    let mut groups_obj: Option<Rc<RefCell<Object>>> = None;
    let re = rx.re.borrow().clone();
    if let Some(re) = re {
        for (i, name) in re.capture_names().enumerate() {
            if i == 0 {
                continue;
            }
            if let Some(nm) = name {
                let val = match caps.name(nm) {
                    Some(m) => Value::String(Rc::from(m.as_str())),
                    None => Value::Undefined,
                };
                if groups_obj.is_none() {
                    groups_obj = Some(e.new_object());
                }
                groups_obj
                    .as_ref()
                    .unwrap()
                    .borrow_mut()
                    .props
                    .insert(Rc::from(nm), Property::new(val));
            }
        }
    }
    let groups = groups_obj.map(Value::Object).unwrap_or(Value::Undefined);

    {
        let mut b = arr.borrow_mut();
        b.props
            .insert(Rc::from("index"), Property::new(Value::Number(index as f64)));
        b.props
            .insert(Rc::from("input"), Property::new(Value::String(Rc::from(input))));
        b.props.insert(Rc::from("groups"), Property::new(groups));
        b.props.insert(
            Rc::from("lastIndex"),
            Property::new(Value::Number(*rx.last_index.borrow() as f64)),
        );
    }
    Value::Array(arr)
}

// --- flag validation ---------------------------------------------------------

fn validate_flags(flags: &str) -> Result<(), String> {
    let allowed = ['d', 'g', 'i', 'm', 's', 'u', 'v', 'y'];
    let mut seen = Vec::new();
    for c in flags.chars() {
        if !allowed.contains(&c) {
            return Err(format!("invalid regular expression flag: {c}"));
        }
        if seen.contains(&c) {
            return Err(format!("duplicate regular expression flag: {c}"));
        }
        seen.push(c);
    }
    Ok(())
}

fn syntax_error(e: &Engine, msg: &str) -> Error {
    let obj = e.make_object(e.syntax_error_proto.clone());
    {
        let mut b = obj.borrow_mut();
        b.props
            .insert(Rc::from("name"), Property::new(Value::String(Rc::from("SyntaxError"))));
        b.props
            .insert(Rc::from("message"), Property::new(Value::String(Rc::from(msg))));
    }
    Error::Runtime(Value::Object(obj))
}

// --- constructor and prototype methods --------------------------------------

pub(crate) fn regexp_ctor(e: &Engine, _this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let pattern_arg = a.first().cloned().unwrap_or(Value::Undefined);
    let flags_arg = a.get(1).cloned().unwrap_or(Value::Undefined);

    // `RegExp(re)` / `new RegExp(re)` with no flags reuses the same pattern.
    if let Value::Regex(_) = pattern_arg {
        if matches!(flags_arg, Value::Undefined) {
            return Ok(pattern_arg);
        }
    }

    let pattern = match pattern_arg {
        Value::Undefined => String::new(),
        v => v.to_string().to_string(),
    };
    let flags = match flags_arg {
        Value::Undefined => String::new(),
        v => v.to_string().to_string(),
    };

    if let Err(msg) = validate_flags(&flags) {
        return Err(syntax_error(e, &msg));
    }

    let compiled = match compile_regex(&pattern, &flags) {
        Ok(re) => re,
        Err(msg) => return Err(syntax_error(e, &format!("invalid regular expression: {msg}"))),
    };

    Ok(Value::Regex(Rc::new(RegexData {
        pattern: Rc::from(pattern.as_str()),
        flags: Rc::from(flags.as_str()),
        re: RefCell::new(Some(compiled)),
        last_index: RefCell::new(0),
    })))
}

pub(crate) fn regexp_exec(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let rx = this_regex(this, e)?;
    let input = a.first().map(|v| v.to_string().to_string()).unwrap_or_default();
    Ok(match get_match(rx, &input) {
        Some((caps, s)) => make_result(e, rx, &caps, &input, s),
        None => Value::Null,
    })
}

pub(crate) fn regexp_test(e: &Engine, this: &Value, a: &[Value], _c: bool) -> Result<Value, Error> {
    let rx = this_regex(this, e)?;
    let input = a.first().map(|v| v.to_string().to_string()).unwrap_or_default();
    Ok(Value::Boolean(get_match(rx, &input).is_some()))
}

fn regexp_to_string(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    let rx = this_regex(this, e)?;
    Ok(Value::String(Rc::from(
        format!("/{}/{}", escape_source(&rx.source()), rx.flag_string()).as_str(),
    )))
}

/// Escape `/` and line terminators for the `source`/`toString` representation.
fn escape_source(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '/' => out.push_str("\\/"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            _ => out.push(c),
        }
    }
    out
}

// --- accessor getters --------------------------------------------------------

fn regexp_source(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    Ok(match getter_target(e, this)? {
        GetterTarget::Data(rx) => Value::String(rx.source()),
        GetterTarget::Default => Value::String(Rc::from("(?:)")),
    })
}

/// `RegExp.prototype.flags` getter. Per spec it reads the individual flag
/// properties off `this` and coerces each to a boolean, so a non-RegExp object
/// with mock flag properties is accepted.
fn regexp_flags(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    // Per spec `RegExp.prototype.flags` requires `this` to be an object; any
    // primitive (including `null`/`undefined`) throws a TypeError.
    if !matches!(
        this,
        Value::Object(_)
            | Value::Array(_)
            | Value::Function(_)
            | Value::NativeFunction(_)
            | Value::Map(_)
            | Value::Set(_)
            | Value::WeakMap(_)
            | Value::WeakSet(_)
            | Value::Regex(_)
    ) {
        return Err(Error::Runtime(e.make_type_error("RegExp.prototype.flags called on a non-object")));
    }
    let has_indices = e.get_property(this, "hasIndices").to_boolean();
    let global = e.get_property(this, "global").to_boolean();
    let ignore_case = e.get_property(this, "ignoreCase").to_boolean();
    let multiline = e.get_property(this, "multiline").to_boolean();
    let dot_all = e.get_property(this, "dotAll").to_boolean();
    let unicode = e.get_property(this, "unicode").to_boolean();
    let unicode_sets = e.get_property(this, "unicodeSets").to_boolean();
    let sticky = e.get_property(this, "sticky").to_boolean();
    let mut out = String::new();
    if has_indices {
        out.push('d');
    }
    if global {
        out.push('g');
    }
    if ignore_case {
        out.push('i');
    }
    if multiline {
        out.push('m');
    }
    if dot_all {
        out.push('s');
    }
    if unicode {
        out.push('u');
    }
    if unicode_sets {
        out.push('v');
    }
    if sticky {
        out.push('y');
    }
    Ok(Value::String(Rc::from(out.as_str())))
}

fn flag_getter(e: &Engine, this: &Value, c: char) -> Result<Value, Error> {
    Ok(match getter_target(e, this)? {
        GetterTarget::Data(rx) => Value::Boolean(rx.has_flag(c)),
        GetterTarget::Default => Value::Boolean(false),
    })
}

fn regexp_global(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    flag_getter(e, this, 'g')
}

fn regexp_ignore_case(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    flag_getter(e, this, 'i')
}

fn regexp_multiline(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    flag_getter(e, this, 'm')
}

fn regexp_dotall(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    flag_getter(e, this, 's')
}

fn regexp_unicode(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    flag_getter(e, this, 'u')
}

fn regexp_unicode_sets(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    flag_getter(e, this, 'v')
}

fn regexp_sticky(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    flag_getter(e, this, 'y')
}

fn regexp_has_indices(e: &Engine, this: &Value, _a: &[Value], _c: bool) -> Result<Value, Error> {
    flag_getter(e, this, 'd')
}

// --- regexp-aware String methods --------------------------------------------

/// `String.prototype.match` when the argument is a RegExp.
pub(crate) fn regexp_match(e: &Engine, rx: &RegexData, input: &str) -> Value {
    if rx.has_flag('g') {
        let mut elems = Vec::new();
        let re = rx.re.borrow().clone();
        if let Some(re) = re {
            for m in re.find_iter(input) {
                elems.push(Value::String(Rc::from(m.as_str())));
            }
        }
        Value::Array(e.new_array(elems))
    } else {
        match get_match(rx, input) {
            Some((caps, s)) => make_result(e, rx, &caps, input, s),
            None => Value::Null,
        }
    }
}

/// `String.prototype.search` for a RegExp: returns the character index of the
/// first match, or `-1`.
pub(crate) fn regexp_search(_e: &Engine, rx: &RegexData, input: &str) -> Value {
    let re = rx.re.borrow().clone();
    match re {
        Some(re) => match re.find(input) {
            Some(m) => Value::Number(byte_to_char(input, m.start()) as f64),
            None => Value::Number(-1.0),
        },
        None => Value::Number(-1.0),
    }
}

/// `String.prototype.replace` for a RegExp. Replacement may be a string
/// (`$&`, `$1`…`$99`, `$<name>`, `$$`) or a function invoked with the captures.
pub(crate) fn regexp_replace(
    e: &Engine,
    rx: &RegexData,
    input: &str,
    replacement: &Value,
) -> String {
    let re = rx.re.borrow().clone();
    let Some(re) = re else {
        return input.to_string();
    };
    let global = rx.has_flag('g');
    let mut out = String::new();
    if global {
        let mut last = 0;
        for caps in re.captures_iter(input) {
            let m = caps.get(0).unwrap();
            out.push_str(&input[last..m.start()]);
            out.push_str(&expand_replacement(e, &caps, replacement, input, m.start()));
            last = m.end();
        }
        out.push_str(&input[last..]);
    } else if let Some(caps) = re.captures(input) {
        let m = caps.get(0).unwrap();
        out.push_str(&input[..m.start()]);
        out.push_str(&expand_replacement(e, &caps, replacement, input, m.start()));
        out.push_str(&input[m.end()..]);
    } else {
        return input.to_string();
    }
    out
}

/// `String.prototype.split` for a RegExp separator.
pub(crate) fn regexp_split(
    e: &Engine,
    rx: &RegexData,
    input: &str,
    limit: Option<usize>,
) -> Value {
    let mut parts: Vec<Value> = Vec::new();
    let re = rx.re.borrow().clone();
    let has_limit = limit.is_some() && limit.unwrap() > 0;
    let limit = limit.unwrap_or(usize::MAX);
    if let Some(re) = re {
        let mut last = 0;
        for caps in re.captures_iter(input) {
            if has_limit && parts.len() >= limit {
                break;
            }
            let m = caps.get(0).unwrap();
            parts.push(Value::String(Rc::from(&input[last..m.start()])));
            for i in 1..caps.len() {
                parts.push(match caps.get(i) {
                    Some(c) => Value::String(Rc::from(c.as_str())),
                    None => Value::Undefined,
                });
            }
            last = m.end();
        }
        if !has_limit || parts.len() < limit {
            parts.push(Value::String(Rc::from(&input[last..])));
        }
    } else {
        parts.push(Value::String(Rc::from(input)));
    }
    if has_limit && parts.len() > limit {
        parts.truncate(limit);
    }
    Value::Array(e.new_array(parts))
}

/// Expand a replacement string (`$&`, `` $` ``, `$'`, `$1`…`$99`, `$<name>`,
/// `$$`) or invoke the replacement function with the capture array.
fn expand_replacement(
    e: &Engine,
    caps: &Captures,
    replacement: &Value,
    input: &str,
    start_byte: usize,
) -> String {
    if matches!(replacement, Value::Function(_) | Value::NativeFunction(_)) {
        let mut elems = Vec::new();
        let full = caps.get(0).unwrap();
        elems.push(Value::String(Rc::from(full.as_str())));
        for i in 1..caps.len() {
            elems.push(match caps.get(i) {
                Some(m) => Value::String(Rc::from(m.as_str())),
                None => Value::Undefined,
            });
        }
        let arr = e.new_array(elems);
        let res = e
            .call_value(replacement, &Value::Undefined, &[Value::Array(arr)])
            .unwrap_or(Value::Undefined);
        return res.to_string().to_string();
    }

    let rep = replacement.to_string().to_string();
    let chars: Vec<char> = rep.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c != '$' {
            out.push(c);
            i += 1;
            continue;
        }
        let Some(&n) = chars.get(i + 1) else {
            out.push('$');
            break;
        };
        match n {
            '$' => {
                out.push('$');
                i += 2;
            }
            '&' => {
                out.push_str(caps.get(0).unwrap().as_str());
                i += 2;
            }
            '`' => {
                out.push_str(&input[..start_byte]);
                i += 2;
            }
            '\'' => {
                let m = caps.get(0).unwrap();
                out.push_str(&input[m.end()..]);
                i += 2;
            }
            '0'..='9' => {
                let mut gi = 0usize;
                let mut digits = 0usize;
                while let Some(&d) = chars.get(i + 1 + digits) {
                    if d.is_ascii_digit() {
                        gi = gi * 10 + (d as usize - '0' as usize);
                        digits += 1;
                        if digits > 2 {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                i += 1 + digits;
                if gi == 0 {
                    out.push_str(caps.get(0).unwrap().as_str());
                } else if let Some(m) = caps.get(gi) {
                    out.push_str(m.as_str());
                }
            }
            '<' => {
                let Some(rest) = rep.get(i + 2..) else {
                    out.push('$');
                    i += 1;
                    continue;
                };
                if let Some(end) = rest.find('>') {
                    let name = &rest[..end];
                    if let Some(m) = caps.name(name) {
                        out.push_str(m.as_str());
                    }
                    i += 2 + end + 1;
                } else {
                    out.push('$');
                    i += 1;
                }
            }
            _ => {
                out.push('$');
                i += 1;
            }
        }
    }
    out
}

// --- install ----------------------------------------------------------------

/// Install `RegExp` and its prototype into the global environment.
pub(crate) fn register_regexp(engine: &mut Engine) {
    let proto = engine.regexp_prototype.clone();

    proto_method(&proto, "exec", regexp_exec);
    proto_method(&proto, "test", regexp_test);
    proto_method(&proto, "toString", regexp_to_string);

    let getters: &[(&str, NativeFn)] = &[
        ("source", regexp_source),
        ("flags", regexp_flags),
        ("global", regexp_global),
        ("ignoreCase", regexp_ignore_case),
        ("multiline", regexp_multiline),
        ("dotAll", regexp_dotall),
        ("unicode", regexp_unicode),
        ("unicodeSets", regexp_unicode_sets),
        ("sticky", regexp_sticky),
        ("hasIndices", regexp_has_indices),
    ];
    for (name, f) in getters {
        // Spec: accessor functions are named `get <property>` with `length` 0,
        // and the `RegExp.prototype` accessors are all non-enumerable.
        let getter = native(*f);
        if let Value::NativeFunction(nf) = &getter {
            let mut b = nf.borrow_mut();
            b.props.insert(
                Rc::from("name"),
                Property::config(Value::String(Rc::from(format!("get {name}")))),
            );
            b.props
                .insert(Rc::from("length"), Property::config(Value::Number(0.0)));
        }
        proto.borrow_mut().props.insert(
            Rc::from(*name),
            Property::accessor(Some(getter), None),
        );
    }

    let ctor = reg_ctor(engine, "RegExp", regexp_ctor, proto.clone());
    proto_data(&proto, "constructor", ctor.clone());
}
