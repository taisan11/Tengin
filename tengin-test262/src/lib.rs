//! A small test262 harness runner for Tengin.
//!
//! It parses the `/*--- ... ---*/` frontmatter of a test262 test, assembles the
//! required harness sources (always `assert.js` + `sta.js`, plus anything listed
//! under `includes:`), and evaluates the test in the appropriate mode(s)
//! (non-strict, strict, or both) reporting a [`Status`] per mode.

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use tengin::value::{native, Object, Value};
use tengin::Engine;

/// Outcome of running a single test in a single mode.
#[derive(Debug, Clone)]
pub struct TestOutcome {
    pub path: String,
    pub strict: bool,
    pub status: Status,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Pass,
    Fail,
    Skip,
}

impl Status {
    pub fn is_pass(&self) -> bool {
        *self == Status::Pass
    }
}

#[derive(Debug, Clone, Default)]
struct Meta {
    negative_phase: Option<String>,
    negative_type: Option<String>,
    only_strict: bool,
    no_strict: bool,
    raw: bool,
    module: bool,
    includes: Vec<String>,
}

/// Parse the `/*--- ... ---*/` YAML-ish frontmatter of a test262 test.
fn parse_frontmatter(src: &str) -> Meta {
    let Some(start) = src.find("/*---") else {
        return Meta::default();
    };
    let rest = &src[start + 5..];
    let Some(end) = rest.find("---*/") else {
        return Meta::default();
    };
    let block = &rest[..end];

    let mut meta = Meta::default();
    let mut section: Option<&str> = None;
    let mut neg_phase = String::new();
    let mut neg_type = String::new();

    for raw_line in block.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        // List item `- value`
        if let Some(item) = line.strip_prefix("- ") {
            let item = item.trim();
            match section {
                Some("flags") => match item {
                    "onlyStrict" => meta.only_strict = true,
                    "noStrict" => meta.no_strict = true,
                    "raw" => meta.raw = true,
                    "module" => meta.module = true,
                    _ => {}
                },
                Some("includes") => meta.includes.push(item.to_string()),
                _ => {}
            }
            continue;
        }

        // Inline list `key: [a, b, c]`
        if let Some((key, after)) = line.split_once(':') {
            let key = key.trim();
            let val = after.trim();
            if let Some(inner) = val
                .strip_prefix('[')
                .and_then(|s| s.strip_suffix(']'))
            {
                for item in inner.split(',') {
                    let it = item.trim().trim_matches('"').trim_matches('\'').trim();
                    if it.is_empty() {
                        continue;
                    }
                    match key {
                        "flags" => match it {
                            "onlyStrict" => meta.only_strict = true,
                            "noStrict" => meta.no_strict = true,
                            "raw" => meta.raw = true,
                            "module" => meta.module = true,
                            _ => {}
                        },
                        "includes" => meta.includes.push(it.to_string()),
                        _ => {}
                    }
                }
                section = None;
                continue;
            }
            // Block scalar `key: |`
            if val == "|" || val == ">" {
                section = None;
                continue;
            }
            // Nested keys inside `negative:`
            if section == Some("negative") {
                match key {
                    "phase" => neg_phase = val.to_string(),
                    "type" => neg_type = val.to_string(),
                    _ => {}
                }
                continue;
            }
            match key {
                "negative" => section = Some("negative"),
                "flags" => section = Some("flags"),
                "includes" => section = Some("includes"),
                "features" => section = Some("features"),
                _ => section = None,
            }
        }
    }

    if !neg_phase.is_empty() || !neg_type.is_empty() {
        meta.negative_phase = Some(neg_phase);
        meta.negative_type = Some(neg_type);
    }
    meta
}

/// Locate a harness file by walking up from `test_path`, then falling back to
/// `vendor/test262/harness`.
fn find_harness(name: &str, test_path: &Path) -> Option<String> {
    let mut dir = test_path.parent();
    while let Some(d) = dir {
        let p = d.join("harness").join(name);
        if p.exists() {
            if let Ok(s) = fs::read_to_string(&p) {
                return Some(s);
            }
        }
        dir = d.parent();
    }
    let fallback = Path::new("vendor/test262/harness").join(name);
    if fallback.exists() {
        if let Ok(s) = fs::read_to_string(&fallback) {
            return Some(s);
        }
    }
    None
}

fn native_print(
    _e: &Engine,
    _this: &Value,
    args: &[Value],
    _c: bool,
) -> Result<Value, tengin::Error> {
    let parts: Vec<String> = args.iter().map(|v| v.to_string().to_string()).collect();
    println!("{}", parts.join(" "));
    Ok(Value::Undefined)
}

fn native_create_realm(
    e: &Engine,
    _this: &Value,
    _a: &[Value],
    _c: bool,
) -> Result<Value, tengin::Error> {
    let global = Rc::new(RefCell::new(Object::new()));
    let mut realm = Object::new();
    realm
        .props
        .insert(Rc::from("global"), tengin::value::Property::new(Value::Object(global.clone())));
    realm
        .props
        .insert(Rc::from("globalThis"), tengin::value::Property::new(Value::Object(global)));
    let _ = e;
    Ok(Value::Object(Rc::new(RefCell::new(realm))))
}

/// Build the `$262` host object.
fn make_262(engine: &Engine) -> Value {
    let mut o = Object::new();
    o.props.insert(
        Rc::from("global"),
        tengin::value::Property::new(engine.global_object_value()),
    );
    o.props.insert(
        Rc::from("globalThis"),
        tengin::value::Property::new(engine.global_object_value()),
    );
    o.props.insert(
        Rc::from("eval"),
        tengin::value::Property::new(native(|e, _this, a, _c| {
            let src = a.first().cloned().unwrap_or(Value::Undefined).to_string();
            e.eval(&src)
        })),
    );
    o.props.insert(
        Rc::from("createRealm"),
        tengin::value::Property::new(native(native_create_realm)),
    );
    Value::Object(Rc::new(RefCell::new(o)))
}

fn register_host(engine: &mut Engine) {
    engine.register_global("print", native(native_print));
    engine.register_global("$262", make_262(engine));
}

/// Run a single test file, returning one [`TestOutcome`] per mode evaluated.
pub fn run_test_file(test_path: &Path) -> Vec<TestOutcome> {
    let source = match fs::read_to_string(test_path) {
        Ok(s) => s,
        Err(e) => {
            return vec![TestOutcome {
                path: test_path.to_string_lossy().to_string(),
                strict: false,
                status: Status::Fail,
                detail: format!("cannot read file: {e}"),
            }]
        }
    };

    let meta = parse_frontmatter(&source);

    if meta.module {
        return vec![TestOutcome {
            path: test_path.to_string_lossy().to_string(),
            strict: false,
            status: Status::Skip,
            detail: "modules are not supported".to_string(),
        }];
    }

    // Assemble harness sources.
    let mut harness = String::new();
    if !meta.raw {
        match find_harness("assert.js", test_path) {
            Some(s) => harness.push_str(&s),
            None => {
                return vec![harness_missing(test_path, "assert.js")];
            }
        }
        match find_harness("sta.js", test_path) {
            Some(s) => harness.push_str(&s),
            None => return vec![harness_missing(test_path, "sta.js")],
        }
        for inc in &meta.includes {
            match find_harness(inc, test_path) {
                Some(s) => harness.push_str(&s),
                None => {
                    return vec![harness_missing(test_path, inc)];
                }
            }
        }
    }

    let modes: Vec<bool> = if meta.only_strict {
        vec![true]
    } else if meta.no_strict {
        vec![false]
    } else if meta.raw {
        vec![false]
    } else {
        vec![false, true]
    };

    let mut outcomes = Vec::new();
    for strict in modes {
        let mut engine = Engine::new();
        register_host(&mut engine);

        if !harness.is_empty() {
            if let Err(e) = engine.eval(&harness) {
                outcomes.push(TestOutcome {
                    path: test_path.to_string_lossy().to_string(),
                    strict,
                    status: Status::Fail,
                    detail: format!("harness failed: {e}"),
                });
                continue;
            }
        }

        let body = if strict {
            format!("\"use strict\";\n{source}")
        } else {
            source.clone()
        };

        let status = evaluate(&engine, &body, &meta);
        outcomes.push(TestOutcome {
            path: test_path.to_string_lossy().to_string(),
            strict,
            status: status.0,
            detail: status.1,
        });
    }
    outcomes
}

fn harness_missing(test_path: &Path, name: &str) -> TestOutcome {
    TestOutcome {
        path: test_path.to_string_lossy().to_string(),
        strict: false,
        status: Status::Fail,
        detail: format!("harness file '{name}' not found"),
    }
}

/// Decide pass/fail for one execution.
fn evaluate(engine: &Engine, body: &str, meta: &Meta) -> (Status, String) {
    match &meta.negative_phase {
        Some(phase) if phase == "parse" => {
            if engine.parse_source(body).is_err() {
                (Status::Pass, String::new())
            } else {
                (Status::Fail, "expected a parse error".to_string())
            }
        }
        Some(phase) if phase == "runtime" => {
            if engine.eval(body).is_err() {
                (Status::Pass, String::new())
            } else {
                (Status::Fail, "expected an exception to be thrown".to_string())
            }
        }
        // `early` / `resolution` negatives cannot be detected by this interpreter.
        Some(_) => (Status::Skip, "negative phase not detectable".to_string()),
        None => match engine.eval(body) {
            Ok(_) => (Status::Pass, String::new()),
            Err(e) => (Status::Fail, format!("{e}")),
        },
    }
}

/// Run a list of test files, returning all outcomes.
pub fn run_files(paths: &[PathBuf]) -> Vec<TestOutcome> {
    let mut out = Vec::new();
    for p in paths {
        out.extend(run_test_file(p));
    }
    out
}

/// Convenience: count how many outcomes passed / failed / skipped.
pub fn summarize(outcomes: &[TestOutcome]) -> (usize, usize, usize) {
    let mut pass = 0;
    let mut fail = 0;
    let mut skip = 0;
    for o in outcomes {
        match o.status {
            Status::Pass => pass += 1,
            Status::Fail => fail += 1,
            Status::Skip => skip += 1,
        }
    }
    (pass, fail, skip)
}
