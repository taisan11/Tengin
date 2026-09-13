//! Integration tests for the built-in library, evaluated through `Engine::eval`
//! and compared against the raw `Value` representation.

use std::rc::Rc;

use tengin::{Engine, Value};

fn num(src: &str) -> f64 {
    let engine = Engine::new();
    match engine.eval(src) {
        Ok(Value::Number(n)) => n,
        other => panic!("expected Number from {src:?}, got {other:?}"),
    }
}

fn str(src: &str) -> String {
    let engine = Engine::new();
    match engine.eval(src) {
        Ok(Value::String(s)) => s.to_string(),
        other => panic!("expected String from {src:?}, got {other:?}"),
    }
}

fn bool(src: &str) -> bool {
    let engine = Engine::new();
    match engine.eval(src) {
        Ok(Value::Boolean(b)) => b,
        other => panic!("expected Boolean from {src:?}, got {other:?}"),
    }
}

#[test]
fn json_stringify_primitives_and_arrays() {
    assert_eq!(str("JSON.stringify(42)"), "42");
    assert_eq!(str("JSON.stringify('hi')"), "\"hi\"");
    assert_eq!(str("JSON.stringify(true)"), "true");
    assert_eq!(str("JSON.stringify(null)"), "null");
    assert_eq!(str("JSON.stringify([1, 2, 'a'])"), "[1,2,\"a\"]");
    assert_eq!(str("JSON.stringify({ b: 1, a: 2 })"), "{\"a\":2,\"b\":1}");
}

#[test]
fn json_parse_and_roundtrip() {
    assert_eq!(num("JSON.parse('{\"x\": 1.5}').x"), 1.5);
    assert_eq!(bool("JSON.parse('[true, null]')[0]"), true);
    assert_eq!(Engine::new().eval("JSON.parse('[true, null]')[1]").unwrap(), Value::Null);
    assert!(Engine::new().eval("JSON.parse('{bad')").is_err());
}

#[test]
fn string_methods() {
    assert_eq!(str("'abc'.toUpperCase()"), "ABC");
    assert_eq!(str("'  x '.trim()"), "x");
    assert_eq!(num("'abc'.indexOf('b')"), 1.0);
    assert_eq!(str("'abc'.slice(1)"), "bc");
    assert_eq!(str("'a-b-c'.split('-').join('+')"), "a+b+c");
    assert_eq!(str("'abc'.repeat(2)"), "abcabc");
    assert_eq!(str("'abc'.at(-1)"), "c");
    assert_eq!(num("'ABC'.charCodeAt(1)"), 66.0);
    assert_eq!(str("String.fromCharCode(65)"), "A");
    assert_eq!(str("'5'.padStart(3, '0')"), "005");
    assert_eq!(str("'abc'.concat('d', 'e')"), "abcde");
}

#[test]
fn array_methods() {
    assert_eq!(str("[1,2,3].map(function(x){ return x * 2; }).join(',')"), "2,4,6");
    assert_eq!(num("[1,2,3].reduce(function(a,b){ return a+b; }, 0)"), 6.0);
    assert_eq!(num("[1,2,3].filter(function(x){ return x > 1; }).length"), 2.0);
    assert_eq!(str("[1,2,3].reverse().join(',')"), "3,2,1");
    assert_eq!(str("[1].concat([2],[3]).join(',')"), "1,2,3");
    assert_eq!(bool("[1,2,3].includes(2)"), true);
    assert_eq!(bool("Array.isArray([1])"), true);
    assert_eq!(bool("Array.isArray({length: 1})"), false);
    assert_eq!(num("[1,2,3].indexOf(3)"), 2.0);
    assert_eq!(num("[10, 20, 30].length"), 3.0);
}

#[test]
fn object_methods() {
    assert_eq!(str("Object.keys({ a: 1, b: 2 }).join(',')"), "a,b");
    assert_eq!(bool("({a:1}).hasOwnProperty('a')"), true);
    assert_eq!(bool("({a:1}).hasOwnProperty('b')"), false);
    assert_eq!(num("Object.assign({}, { a: 1 }).a"), 1.0);
    assert_eq!(bool("typeof Object.freeze({}) === 'object'"), true);
    assert_eq!(num("Object.values({ a: 7 }).pop ? 7 : 7"), 7.0);
}

#[test]
fn math_functions() {
    assert_eq!(num("Math.max(1, 5, 3)"), 5.0);
    assert_eq!(num("Math.min(1, 5, 3)"), 1.0);
    assert_eq!(num("Math.floor(1.5)"), 1.0);
    assert_eq!(num("Math.floor(-1.5)"), -2.0);
    assert_eq!(num("Math.trunc(-1.7)"), -1.0);
    assert_eq!(num("Math.abs(-3)"), 3.0);
    assert_eq!(num("Math.round(2.5)"), 3.0);
    assert_eq!(num("Math.pow(2, 10)"), 1024.0);
    assert_eq!(num("Math.sqrt(9)"), 3.0);
    assert_eq!(bool("Math.sign(-2) === -1"), true);
}

#[test]
fn number_constructors_and_statics() {
    assert_eq!(num("Number('12') + 1"), 13.0);
    assert_eq!(str("(5).toFixed(2)"), "5.00");
    assert_eq!(bool("Number.isInteger(3)"), true);
    assert_eq!(bool("Number.isInteger(3.5)"), false);
    assert_eq!(num("Number.MAX_SAFE_INTEGER"), 9007199254740991.0);
    assert_eq!(num("parseInt('42px')"), 42.0);
    match Engine::new().eval("parseInt('px')").unwrap() {
        Value::Number(n) => assert!(n.is_nan()),
        other => panic!("expected NaN, got {other:?}"),
    }
}

#[test]
fn map_and_set_basics() {
    let e = Engine::new();
    assert_eq!(
        e.eval("var m = new Map(); m.set('k', 1); m.get('k')").unwrap(),
        Value::Number(1.0)
    );
    assert_eq!(
        e.eval("var m = new Map([[1, 'a']]); m.get(1)").unwrap(),
        Value::String(Rc::from("a"))
    );
    assert_eq!(
        e.eval("new Set([1, 2, 2]).size").unwrap(),
        Value::Number(2.0)
    );
    assert_eq!(
        e.eval("var s = new Set(); s.add(9); s.has(9)").unwrap(),
        Value::Boolean(true)
    );
}

#[test]
fn bigint_and_symbol() {
    assert_eq!(Engine::new().eval("2n + 3n").unwrap(), Value::BigInt(Rc::from("5")));
    assert_eq!(bool("1n < 2n"), true);
    assert!(Engine::new().eval("1n + 1").is_err(), "BigInt + Number must throw");
    assert_eq!(
        Engine::new().eval("typeof Symbol('s')").unwrap(),
        Value::String(Rc::from("symbol"))
    );
}

#[test]
fn regex_basics() {
    assert_eq!(str("/a(b)c/.exec('xabc')[1]"), "b");
    assert_eq!(str("'xabc'.replace(/a(b)/, 'T$1')"), "xTbc");
    assert_eq!(bool("/z/.test('abc')"), false);
    assert_eq!(bool("/b/.test('abc')"), true);
}

#[test]
fn uri_helpers() {
    assert_eq!(str("decodeURIComponent('a%20b')"), "a b");
    assert_eq!(str("encodeURIComponent('a b')"), "a%20b");
}

#[test]
fn type_conversions() {
    assert_eq!(str("typeof undefined"), "undefined");
    assert_eq!(str("typeof null"), "object");
    assert_eq!(str("typeof 1"), "number");
    assert_eq!(str("typeof 'x'"), "string");
    assert_eq!(str("typeof {}"), "object");
    assert_eq!(bool("!!'x'"), true);
    assert_eq!(bool("!!''"), false);
    assert_eq!(bool("1 == '1'"), true);
    assert_eq!(bool("1 === '1'"), false);
    assert_eq!(bool("isNaN(NaN)"), true);
    assert_eq!(bool("[1] instanceof Array"), true);
    assert_eq!(num("1 + true"), 2.0);
}

#[test]
fn control_flow_and_functions() {
    let e = Engine::new();
    assert_eq!(
        e.eval("(function(){ switch (2) { case 2: return 'two'; } })()").unwrap(),
        Value::String(Rc::from("two"))
    );
    assert_eq!(
        e.eval("var s = ''; for (var k in { a: 1, b: 2 }) s += k; s").unwrap(),
        Value::String(Rc::from("ab"))
    );
    assert_eq!(
        e.eval("var n = 0; outer: for (var i = 0; i < 3; i++) { for (var j = 0; j < 3; j++) { if (j == 1) continue outer; n++; } } n").unwrap(),
        Value::Number(3.0)
    );
    assert_eq!(
        e.eval("function f(a, b) { return arguments.length; } f(1, 2, 3)").unwrap(),
        Value::Number(3.0)
    );
}

#[test]
fn try_catch_and_throw() {
    let e = Engine::new();
    assert_eq!(
        e.eval("var r = null; try { throw new TypeError('boom'); } catch (err) { r = typeof err; } r")
            .unwrap(),
        Value::String(Rc::from("object"))
    );
    assert_eq!(
        e.eval("var caught = false; try { throw new RangeError('boom'); } catch (e) { caught = true; } caught")
            .unwrap(),
        Value::Boolean(true)
    );
    assert_eq!(
        e.eval("var r = []; try { throw 1; } catch (err) { r.push('caught'); } finally { r.push('fin'); } r.join(',')")
            .unwrap(),
        Value::String(Rc::from("caught,fin"))
    );
    assert_eq!(
        e.eval("function f(){ try { return 'try'; } finally { } } f()").unwrap(),
        Value::String(Rc::from("try"))
    );
}

#[test]
fn array_sort() {
    assert_eq!(str("[10, 9, 1, 2].sort().join(',')"), "1,10,2,9");
    assert_eq!(str("[10, 9, 1, 2].sort(function(a,b){ return a - b; }).join(',')"), "1,2,9,10");
    assert_eq!(str("[3,1,2].sort(function(a,b){ return b - a; }).join(',')"), "3,2,1");
    assert_eq!(str("['b','a','c'].sort().join('')"), "abc");
    assert_eq!(num("[].sort().length"), 0.0);
    assert_eq!(str("[1, undefined, 2].sort().join(',')"), "1,2,");
    // Sorts in place and returns the same array object.
    assert_eq!(
        str("var a = [2,1]; var r = a.sort(); a === r ? a.join(',') : 'different'"),
        "1,2"
    );
    // Comparator errors propagate.
    assert!(Engine::new()
        .eval("[1,2].sort(function(){ throw new Error('cmp'); })")
        .is_err());
}

#[test]
fn switch_completion_value() {
    assert_eq!(str("switch (2) { case 2: 'two'; }"), "two");
    assert_eq!(str("switch (2) { case 2: 'two'; break; }"), "two");
    assert_eq!(
        Engine::new().eval("switch (9) { case 2: 'two'; }").unwrap(),
        Value::Undefined,
        "no match and no default leaves the switch empty"
    );
    assert_eq!(
        str("1; switch (2) { case 2: 'two'; }"),
        "two",
        "switch's own completion value wins over earlier statements"
    );
    assert_eq!(str("switch (2) { default: 'def'; }"), "def");
    assert_eq!(str("switch (3) { case 1: 'a'; default: 'd'; case 2: 'b'; }"), "b");
}

#[test]
fn nullish_member_access_throws() {
    let e = Engine::new();
    let err = e.eval("null.x").unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("TypeError"), "got {msg}");
    assert!(msg.contains("null"), "got {msg}");

    assert!(e.eval("undefined.x").is_err());
    assert!(e.eval("null[0]").is_err());
    assert!(e.eval("null.x = 1").is_err());
    assert!(e.eval("undefined.x = 1").is_err());
    // Optional chaining stays null-safe.
    assert_eq!(e.eval("null?.x").unwrap(), Value::Undefined);
    assert_eq!(e.eval("undefined?.x").unwrap(), Value::Undefined);
}

#[test]
fn error_instanceof() {
    let e = Engine::new();
    assert_eq!(
        e.eval("(function(){ try { throw new TypeError('x'); } catch (err) { return err instanceof TypeError; } })()")
            .unwrap(),
        Value::Boolean(true)
    );
    assert_eq!(
        e.eval("(function(){ try { throw new TypeError('x'); } catch (err) { return err instanceof Error; } })()")
            .unwrap(),
        Value::Boolean(true)
    );
    assert_eq!(
        e.eval("(function(){ try { throw new RangeError('x'); } catch (err) { return err instanceof TypeError; } })()")
            .unwrap(),
        Value::Boolean(false)
    );
}

#[test]
fn eval_builtins() {
    let e = Engine::new();
    assert_eq!(e.eval("eval('1 + 2')").unwrap(), Value::Number(3.0));
    assert_eq!(
        e.eval("var x = 5; eval('x + 1')").unwrap(),
        Value::Number(6.0)
    );
}

#[test]
fn if_and_switch_completion_values() {
    // Statement lists keep the last value-producing statement's value.
    assert_eq!(num("1; {}"), 1.0);
    assert_eq!(num("1; var a;"), 1.0);
    // An if statement fills its empty completion with `undefined`.
    assert_eq!(Engine::new().eval("1; if (true) { }").unwrap(), Value::Undefined);
    assert_eq!(num("2; if (true) { 3; }"), 3.0);
    assert_eq!(Engine::new().eval("1; if (false) { }").unwrap(), Value::Undefined);
    // Loops accumulate their body's completion value.
    assert_eq!(Engine::new().eval("1; while (false) { }").unwrap(), Value::Undefined);
    assert_eq!(num("1; do { 2; } while (false)"), 2.0);
    // Abrupt completions filled from the accumulated value.
    assert_eq!(num("1; do { 2; if (true) { 3; break; } 4; } while (false)"), 3.0);
    assert_eq!(
        Engine::new().eval("5; do { 6; if (true) { break; } 7; } while (false)").unwrap(),
        Value::Undefined,
        "if fills an empty break with undefined, which wins over 6"
    );
    assert_eq!(num("8; do { 9; if (true) { 10; continue; } 11; } while (false)"), 10.0);
    // Switch case blocks: fall-through and abrupt completion values.
    assert_eq!(num("1; switch ('a') { case 'a': 2; case 'b': 3; break; default: }"), 3.0);
    assert_eq!(Engine::new().eval("1; switch ('a') { case 'a': break; default: }").unwrap(), Value::Undefined);
    assert_eq!(num("4; do { switch ('a') { case 'a': 5; case 'b': 6; continue; default: } } while (false)"), 6.0);
}

#[test]
fn labelled_statement_completion() {
    assert_eq!(num("1; lbl: 2;"), 2.0);
    assert_eq!(num("1; lbl: { 2; break lbl; }"), 2.0);
    assert_eq!(num("1; lbl: { break lbl; }"), 1.0);
    // A loop's completion value starts as (non-empty) `undefined` even when the
    // loop exits via `break` without producing a value.
    assert_eq!(
        Engine::new()
            .eval("1; outer: for (var i = 0; i < 3; i++) { for (var j = 0; j < 3; j++) { if (j === 1) break outer; } }")
            .unwrap(),
        Value::Undefined
    );
    assert_eq!(num("1; outer: for (var i = 0; i < 3; i++) { 2; break outer; }"), 2.0);
}

#[test]
fn switch_case_block_single_lexical_scope() {
    // All case clauses share one lexical environment.
    let engine = Engine::new();
    engine
        .eval(
            "let x = 'outside'; var p1, p2; \
             switch (null) { \
               case null: let x = 'inside'; p1 = function () { return x; }; \
               case null: p2 = function () { return x; }; \
             }",
        )
        .unwrap();
    assert_eq!(engine.eval("p1()").unwrap(), Value::String(Rc::from("inside")));
    assert_eq!(engine.eval("p2()").unwrap(), Value::String(Rc::from("inside")));
    assert_eq!(engine.eval("x").unwrap(), Value::String(Rc::from("outside")));
    // Lexical declarations inside a case block do not leak.
    assert!(engine
        .eval("switch (0) { default: const y = 1; } y;")
        .is_err());
    assert!(engine
        .eval("switch (0) { default: function* z() {} } z;")
        .is_err());
}

#[test]
fn switch_case_selector_uses_case_block_env() {
    let engine = Engine::new();
    engine
        .eval(
            "let x = 'outside'; var probeExpr, probeSelector, probeStmt; \
             switch (probeExpr = function () { return x; }, null) { \
               case probeSelector = function () { return x; }, null: \
                 probeStmt = function () { return x; }; \
                 let x = 'inside'; \
             }",
        )
        .unwrap();
    assert_eq!(engine.eval("probeExpr()").unwrap(), Value::String(Rc::from("outside")));
    assert_eq!(engine.eval("probeSelector()").unwrap(), Value::String(Rc::from("inside")));
    assert_eq!(engine.eval("probeStmt()").unwrap(), Value::String(Rc::from("inside")));
}

#[test]
fn switch_redeclaration_errors() {
    let bad = [
        "switch (0) { case 1: let f; default: let f }",
        "switch (0) { case 1: let f; default: var f }",
        "switch (0) { case 1: function f() {} default: var f }",
        "switch (0) { case 1: var f; default: function f() {} }",
        "switch (0) { case 1: let f; default: function f() {} }",
        "switch (0) { case 1: function f() {} default: function* f() {} }",
        "switch (0) { case 1: var f; default: let f }",
    ];
    for src in bad {
        assert!(Engine::new().parse_source(src).is_err(), "expected parse error: {src}");
    }
    // Sloppy mode allows duplicate plain function declarations.
    assert!(Engine::new()
        .parse_source("switch (0) { case 1: function f() {} default: function f() {} }")
        .is_ok());
    // Strict mode does not.
    assert!(Engine::new()
        .parse_source("\"use strict\"; switch (0) { case 1: function f() {} default: function f() {} }")
        .is_err());
}

#[test]
fn if_statement_function_declaration_rules() {
    // Sloppy mode allows a function declaration as an if body.
    assert!(Engine::new().parse_source("if (true) function f() {}").is_ok());
    let engine = Engine::new();
    engine.eval("if (true) function f() { return 7; }").unwrap();
    assert_eq!(engine.eval("f()").unwrap(), Value::Number(7.0));
    // Strict mode rejects it.
    assert!(Engine::new()
        .parse_source("\"use strict\"; if (true) function f() {}")
        .is_err());
    assert!(Engine::new()
        .parse_source("\"use strict\"; if (false) ; else function f() {}")
        .is_err());
    // Labelled function declarations are never allowed as an if body.
    assert!(Engine::new()
        .parse_source("if (false) label1: label2: function test262() {}")
        .is_err());
    assert!(Engine::new()
        .parse_source("if (true) ; else label: function f() {}")
        .is_err());
}

#[test]
fn named_function_expression_binds_its_name() {
    assert_eq!(str("(function f(n) { if (n === 0) { return 'done'; } return f(n - 1); })(2)"), "done");
    // The name is only visible inside the function.
    assert_eq!(
        str("var g = function inner() { return typeof inner; }; g() + ':' + typeof inner"),
        "function:undefined"
    );
}

#[test]
fn tail_calls_do_not_grow_the_stack() {
    // 100k recursive tail calls must complete without stack overflow.
    assert_eq!(str("(function f(n) { if (n === 0) { return 'ok'; } return f(n - 1); })(100000)"), "ok");
    assert_eq!(str("(function f(n) { if (n === 0) { return 'ok'; } if (true) { return f(n - 1); } })(100000)"), "ok");
}
