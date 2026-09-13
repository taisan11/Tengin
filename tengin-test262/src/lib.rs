//! A small test262 harness runner for Tengin.
//!
//! It parses the `/*--- ... ---*/` frontmatter of a test262 test, assembles the
//! required harness sources (always `assert.js` + `sta.js`, plus anything listed
//! under `includes:`), and evaluates the test in the appropriate mode(s)
//! (non-strict, strict, or both) reporting a [`Status`] per mode.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock};

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
    is_async: bool,
    includes: Vec<String>,
    features: Vec<String>,
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
                    "async" => meta.is_async = true,
                    _ => {}
                },
                Some("includes") => meta.includes.push(item.to_string()),
                Some("features") => meta.features.push(item.to_string()),
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
                            "async" => meta.is_async = true,
                            _ => {}
                        },
                        "includes" => meta.includes.push(it.to_string()),
                        "features" => meta.features.push(it.to_string()),
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

/// Cache of harness file contents, keyed by the resolved absolute path.
///
/// The same handful of harness files (`assert.js`, `sta.js`, …) are referenced
/// by tens of thousands of tests, so reading them once and reusing the buffer
/// avoids a massive amount of redundant disk I/O during a full run.
static HARNESS_CACHE: OnceLock<Mutex<HashMap<PathBuf, Option<Arc<String>>>>> = OnceLock::new();

/// Locate a harness file by walking up from `test_path`, then falling back to
/// `vendor/test262/harness`.
///
/// Only directories that also contain `assert.js` are treated as the canonical
/// harness root. This avoids accidentally picking up the harness *tests* that
/// live under `test/harness/` (which contain self-checks rather than the real
/// implementations).
///
/// The resolved file's contents are cached so subsequent tests pay no I/O cost.
fn find_harness(name: &str, test_path: &Path) -> Option<String> {
    let path = resolve_harness_path(name, test_path)?;
    harness_content(&path)
}

/// Resolve the on-disk path of a harness file (see [`find_harness`] for the
/// lookup rules) without reading its contents.
fn resolve_harness_path(name: &str, test_path: &Path) -> Option<PathBuf> {
    let mut dir = test_path.parent();
    while let Some(d) = dir {
        let hdir = d.join("harness");
        if hdir.join("assert.js").exists() {
            let p = hdir.join(name);
            if p.exists() {
                return Some(p);
            }
        }
        dir = d.parent();
    }
    let fallback = Path::new("vendor/test262/harness").join(name);
    if fallback.exists() {
        return Some(fallback);
    }
    None
}

/// Read and cache the contents of a harness file.
fn harness_content(path: &Path) -> Option<String> {
    let cache = HARNESS_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cache.lock().unwrap();
    if let Some(v) = guard.get(path) {
        return v.as_ref().map(|s| s.to_string());
    }
    let content = fs::read_to_string(path).ok().map(Arc::new);
    guard.insert(path.to_path_buf(), content.clone());
    content.map(|s| s.to_string())
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

std::thread_local! {
    /// Async-test `$DONE` state for the currently running test on this thread:
    /// `(called, error_detail)`. Reset before each mode evaluation.
    static DONE_STATE: RefCell<(bool, Option<String>)> = RefCell::new((false, None));
}

fn native_done(
    _e: &Engine,
    _this: &Value,
    a: &[Value],
    _c: bool,
) -> Result<Value, tengin::Error> {
    let err = a.first().cloned().unwrap_or(Value::Undefined);
    let detail = match &err {
        Value::Undefined => None,
        other => Some(other.error_message().to_string()),
    };
    DONE_STATE.with(|s| *s.borrow_mut() = (true, detail));
    Ok(Value::Undefined)
}

/// Register the async-test `$DONE` callback for one evaluation.
fn register_async(engine: &mut Engine) {
    DONE_STATE.with(|s| *s.borrow_mut() = (false, None));
    engine.register_global("$DONE", native(native_done));
}

fn native_create_realm(
    _e: &Engine,
    _this: &Value,
    _a: &[Value],
    _c: bool,
) -> Result<Value, tengin::Error> {
    // A fresh realm is its own `Engine` (with the full built-in set). The
    // engine is leaked for the process lifetime; only its global object is
    // exposed, and the strong references held by that object's property map
    // keep the realm's constructors (and their prototypes) reachable.
    let realm = Box::leak(Box::new(Engine::new()));
    register_host(realm);
    let global_value = realm.global_object_value();
    let mut o = Object::new();
    o.props
        .insert(Rc::from("global"), tengin::value::Property::new(global_value.clone()));
    o.props
        .insert(Rc::from("globalThis"), tengin::value::Property::new(global_value));
    Ok(Value::Object(Rc::new(RefCell::new(o))))
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

    // Run the rest inside a panic catcher so a single misbehaving test cannot
    // abort the whole suite.
    let path = test_path.to_path_buf();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_test_inner(&path, &source, &meta)
    }));
    match result {
        Ok(outcomes) => outcomes,
        Err(_) => vec![TestOutcome {
            path: test_path.to_string_lossy().to_string(),
            strict: false,
            status: Status::Fail,
            detail: "panic while running test".to_string(),
        }],
    }
}

fn run_test_inner(test_path: &Path, source: &str, meta: &Meta) -> Vec<TestOutcome> {

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
        if meta.is_async {
            register_async(&mut engine);
        }

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
            source.to_string()
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
            Ok(_) => {
                // Unhandled rejections are only checked for async tests: sync
                // tests routinely construct (and inspect) rejected promises
                // without attaching handlers.
                if meta.is_async {
                    return check_async_completion(engine);
                }
                (Status::Pass, String::new())
            }
            Err(e) => (Status::Fail, format!("{e}")),
        },
    }
}

/// After an async test's evaluation (jobs drained): require `$DONE`, surface
/// a `$DONE(error)`, and report unhandled rejections.
fn check_async_completion(engine: &Engine) -> (Status, String) {
    let (called, err) = DONE_STATE.with(|s| s.borrow().clone());
    if !called {
        return (
            Status::Fail,
            "async test did not call $DONE".to_string(),
        );
    }
    if let Some(detail) = err {
        return (Status::Fail, format!("$DONE called with error: {detail}"));
    }
    check_unhandled(engine)
}

/// Report unhandled promise rejections / microtask exceptions, if any.
fn check_unhandled(engine: &Engine) -> (Status, String) {
    let unhandled = engine.take_unhandled();
    if unhandled.is_empty() {
        (Status::Pass, String::new())
    } else {
        let first = unhandled
            .first()
            .map(|v| v.error_message().to_string())
            .unwrap_or_default();
        (
            Status::Fail,
            format!("unhandled rejection ({}): {first}", unhandled.len()),
        )
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

/// Recursively collect every `*.js` test file beneath `root`.
pub fn collect_test_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect_test_files_inner(root, &mut out);
    out
}

fn collect_test_files_inner(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_test_files_inner(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("js") {
            out.push(path);
        }
    }
}

/// Parse an `excludelist.xml` file, returning the set of excluded test path
/// substrings (the `id` attributes). An empty or missing file yields an empty
/// list.
pub fn load_excludelist(path: &Path) -> Vec<String> {
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for line in content.lines() {
        if let Some(rest) = line.trim().strip_prefix("<test") {
            if let Some(start) = rest.find("id=\"") {
                let after = &rest[start + 4..];
                if let Some(end) = after.find('"') {
                    out.push(after[..end].to_string());
                }
            }
        }
    }
    out
}

/// Returns `true` if `path` matches any excluded-id prefix.
pub fn is_excluded(path: &Path, excludes: &[String]) -> bool {
    let s = path.to_string_lossy();
    excludes.iter().any(|e| s.contains(e.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_temp(name: &str, contents: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("tengin-test262-tests");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn frontmatter_parses_flags_in_block_and_inline_form() {
        let meta = parse_frontmatter(
            "/*---\nflags: [onlyStrict, module]\n---*/\n0;",
        );
        assert!(meta.only_strict);
        assert!(meta.module);
        assert!(!meta.no_strict);

        let meta = parse_frontmatter(
            "/*---\nflags:\n  - noStrict\n  - raw\n---*/\n0;",
        );
        assert!(meta.no_strict);
        assert!(meta.raw);
    }

    #[test]
    fn frontmatter_parses_includes_and_negative() {
        let meta = parse_frontmatter(
            "/*---\nincludes: [compareArray.js]\nnegative:\n  phase: runtime\n  type: RangeError\n---*/\n0;",
        );
        assert_eq!(meta.includes, vec!["compareArray.js".to_string()]);
        assert_eq!(meta.negative_phase.as_deref(), Some("runtime"));
        assert_eq!(meta.negative_type.as_deref(), Some("RangeError"));
    }

    #[test]
    fn frontmatter_without_block_is_default() {
        let meta = parse_frontmatter("// no frontmatter here");
        assert!(meta.includes.is_empty());
        assert_eq!(meta.negative_phase, None);
        assert!(!meta.raw);
        assert!(!meta.module);
        assert!(!meta.only_strict);
        assert!(!meta.no_strict);
    }

    #[test]
    fn excludelist_parsing_and_matching() {
        let path = write_temp(
            "excludelist.xml",
            "<excludeList>\n<test id=\"test/built-ins/Math/abs\"><![CDATA[x]]></test>\n<test id=\"test/foo/bar\"/>\n</excludeList>",
        );
        let excludes = load_excludelist(&path);
        assert_eq!(excludes.len(), 2);
        assert!(excludes.contains(&"test/built-ins/Math/abs".to_string()));
        assert!(excludes.contains(&"test/foo/bar".to_string()));

        assert!(is_excluded(Path::new("vendor/test262/test/built-ins/Math/abs/x.js"), &excludes));
        assert!(!is_excluded(Path::new("test/built-ins/JSON/stringify.js"), &excludes));
    }

    #[test]
    fn missing_excludelist_is_empty() {
        assert!(load_excludelist(Path::new("/nonexistent/excludelist.xml")).is_empty());
    }

    #[test]
    fn summarize_counts_outcomes() {
        let outcomes = vec![
            TestOutcome { path: String::new(), strict: false, status: Status::Pass, detail: String::new() },
            TestOutcome { path: String::new(), strict: true, status: Status::Fail, detail: String::new() },
            TestOutcome { path: String::new(), strict: false, status: Status::Skip, detail: String::new() },
            TestOutcome { path: String::new(), strict: false, status: Status::Pass, detail: String::new() },
        ];
        assert_eq!(summarize(&outcomes), (2, 1, 1));
        assert_eq!(summarize(&[]), (0, 0, 0));
    }

    #[test]
    fn collect_test_files_finds_js_recursively() {
        let root = std::env::temp_dir().join("tengin-test262-tests/collect");
        let nested = root.join("a/b");
        fs::create_dir_all(&nested).unwrap();
        fs::write(root.join("top.js"), "").unwrap();
        fs::write(nested.join("deep.js"), "").unwrap();
        fs::write(root.join("readme.txt"), "").unwrap();

        let mut files = collect_test_files(&root);
        files.sort();
        let names: Vec<String> =
            files.iter().map(|p| p.file_name().unwrap().to_string_lossy().to_string()).collect();
        assert_eq!(names, vec!["deep.js".to_string(), "top.js".to_string()]);
    }

    #[test]
    fn missing_test_file_reports_failure_not_panic() {
        let outcomes = run_test_file(Path::new("/nonexistent/tengin-test-case.js"));
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].status, Status::Fail);
    }
}
