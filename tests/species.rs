//! Promise subclass / `Symbol.species` integration tests.

use tengin::{Engine, Value};

fn str_val(e: &Engine, src: &str) -> String {
    e.eval(src).unwrap().to_string().to_string()
}

#[test]
fn subclass_construction() {
    let e = Engine::new();
    e.eval(
        "class P extends Promise { constructor(e){ super(e); } } \
         var p = new P(function(res){ res(1); }); \
         var isP = p instanceof P; var isQ = p instanceof Promise; \
         var seen = -1; p.then(function(v){ seen = v; });",
    )
    .unwrap();
    assert_eq!(e.eval("isP").unwrap(), Value::Boolean(true));
    assert_eq!(e.eval("isQ").unwrap(), Value::Boolean(true));
    assert_eq!(e.eval("seen").unwrap(), Value::Number(1.0));
    // Prototype linkage: instance proto is P.prototype.
    assert_eq!(
        e.eval("Object.getPrototypeOf(p) === P.prototype").unwrap(),
        Value::Boolean(true)
    );
    assert_eq!(e.eval("p.constructor === P").unwrap(), Value::Boolean(true));
}

#[test]
fn subclass_default_ctor() {
    let e = Engine::new();
    e.eval(
        "class Q extends Promise {} \
         var q = new Q(function(res){ res(2); }); \
         var seen = -1; q.then(function(v){ seen = v; });",
    )
    .unwrap();
    assert_eq!(e.eval("q instanceof Q").unwrap(), Value::Boolean(true));
    assert_eq!(e.eval("seen").unwrap(), Value::Number(2.0));
}

#[test]
fn subclass_executor_throw_rejects() {
    let e = Engine::new();
    e.eval(
        "class P extends Promise {} \
         var r = ''; \
         new P(function(){ throw 'boom'; }).then(null, function(e){ r = e; });",
    )
    .unwrap();
    assert_eq!(str_val(&e, "r"), "boom");
}

#[test]
fn then_returns_subclass_instance() {
    let e = Engine::new();
    e.eval(
        "class P extends Promise {} \
         var p = new P(function(res){ res(1); }); \
         var q = p.then(function(v){ return v + 1; }); \
         var fin = -1; q.then(function(v){ fin = v; });",
    )
    .unwrap();
    assert_eq!(e.eval("q instanceof P").unwrap(), Value::Boolean(true));
    assert_eq!(
        e.eval("Object.getPrototypeOf(q) === P.prototype").unwrap(),
        Value::Boolean(true)
    );
    assert_eq!(e.eval("fin").unwrap(), Value::Number(2.0));
}

#[test]
fn catch_and_finally_use_species() {
    let e = Engine::new();
    e.eval(
        "class P extends Promise {} \
         var p = Promise.reject.call(P, 'e'); \
         var c = p.catch(function(x){ return x; }); \
         var f = p.finally(function(){}); \
         var cv = '', fv = ''; \
         c.then(function(v){ cv = v; }); f.then(null, function(v){ fv = v; });",
    )
    .unwrap();
    assert_eq!(e.eval("c instanceof P").unwrap(), Value::Boolean(true));
    assert_eq!(e.eval("f instanceof P").unwrap(), Value::Boolean(true));
    assert_eq!(str_val(&e, "cv"), "e");
    assert_eq!(str_val(&e, "fv"), "e");
}

#[test]
fn custom_species_returns_base() {
    let e = Engine::new();
    e.eval(
        "class P extends Promise { static get [Symbol.species]() { return Promise; } } \
         var p = new P(function(res){ res(1); }); \
         var q = p.then(function(v){ return v; });",
    )
    .unwrap();
    assert_eq!(e.eval("q instanceof P").unwrap(), Value::Boolean(false));
    assert_eq!(
        e.eval("q instanceof Promise").unwrap(),
        Value::Boolean(true)
    );
    assert_eq!(
        e.eval("Object.getPrototypeOf(q) === Promise.prototype").unwrap(),
        Value::Boolean(true)
    );
}

#[test]
fn species_getter_throw_propagates() {
    let e = Engine::new();
    let r = e.eval(
        "class P extends Promise { static get [Symbol.species]() { throw 'sp-boom'; } } \
         var p = new P(function(res){ res(1); }); p.then(function(v){ return v; });",
    );
    assert!(matches!(r, Err(tengin::Error::Runtime(_))));
    // The engine still works afterwards.
    assert_eq!(e.eval("40 + 2").unwrap(), Value::Number(42.0));
}

#[test]
fn species_non_constructor_throws() {
    let e = Engine::new();
    let r = e.eval(
        "class P extends Promise { static get [Symbol.species]() { return {}; } } \
         var p = new P(function(res){ res(1); }); p.then(function(v){ return v; });",
    );
    assert!(matches!(r, Err(tengin::Error::Runtime(_))));
}

#[test]
fn species_inherited_default() {
    let e = Engine::new();
    e.eval("class P extends Promise {}")
        .unwrap();
    assert_eq!(
        e.eval("P[Symbol.species] === P").unwrap(),
        Value::Boolean(true)
    );
}

#[test]
fn resolve_identity_and_subclass() {
    let e = Engine::new();
    e.eval(
        "var p = Promise.resolve(1); \
         var same = Promise.resolve(p) === p; \
         class P extends Promise {} \
         var q = new P(function(res){ res(2); }); \
         var adopted = Promise.resolve(q) === q; \
         var r = Promise.resolve.call(P, 7); \
         var rv = -1; r.then(function(v){ rv = v; });",
    )
    .unwrap();
    assert_eq!(e.eval("same").unwrap(), Value::Boolean(true));
    // Different constructor: adopts rather than returning identical.
    assert_eq!(e.eval("adopted").unwrap(), Value::Boolean(false));
    assert_eq!(e.eval("r instanceof P").unwrap(), Value::Boolean(true));
    assert_eq!(e.eval("rv").unwrap(), Value::Number(7.0));
}

#[test]
fn statics_use_this_constructor() {
    let e = Engine::new();
    e.eval(
        "class P extends Promise {} \
         var a = Promise.all.call(P, [1, 2]); \
         var vals = ''; a.then(function(v){ vals = v.join(','); }); \
         var r = Promise.race.call(P, [3]); \
         var s = Promise.allSettled.call(P, [1]); \
         var w = Promise.withResolvers.call(P); w.resolve(9); \
         var wv = -1; w.promise.then(function(v){ wv = v; });",
    )
    .unwrap();
    assert_eq!(e.eval("a instanceof P").unwrap(), Value::Boolean(true));
    assert_eq!(str_val(&e, "vals"), "1,2");
    assert_eq!(e.eval("r instanceof P").unwrap(), Value::Boolean(true));
    assert_eq!(e.eval("s instanceof P").unwrap(), Value::Boolean(true));
    assert_eq!(e.eval("w.promise instanceof P").unwrap(), Value::Boolean(true));
    assert_eq!(e.eval("wv").unwrap(), Value::Number(9.0));
}

#[test]
fn statics_reject_non_constructor_this() {
    let e = Engine::new();
    assert!(e.eval("Promise.all.call({}, []);").is_err());
    assert!(e.eval("Promise.resolve.call(1, 2);").is_err());
    // A plain function that ignores its executor cannot provide resolve/reject.
    assert!(e.eval("Promise.all.call(function(){}, []);").is_err());
}

#[test]
fn custom_constructor_executors_run() {
    let e = Engine::new();
    // Plain-function constructor used as species: executor runs synchronously.
    e.eval(
        "var calls = 0; \
         function C(exec){ calls++; exec(function(){}, function(){}); } \
         var r = Promise.all.call(C, []);",
    )
    .unwrap();
    assert_eq!(e.eval("calls").unwrap(), Value::Number(1.0));
    // Throwing executor propagates synchronously.
    assert!(e.eval("Promise.all.call(function(){ throw 'ct'; }, []);").is_err());
}
