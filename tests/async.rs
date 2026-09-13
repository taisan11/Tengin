//! Promise + async/await integration tests.
//!
//! The engine drains its microtask queue at the end of every top-level
//! [`Engine::eval`], so each test drives async work to completion with plain
//! `eval` calls and then observes the settled state with `Value` comparisons.

use tengin::{Engine, Value};

fn str_val(e: &Engine, src: &str) -> String {
    e.eval(src).unwrap().to_string().to_string()
}

#[test]
fn promise_then_runs_as_microtask_in_order() {
    let e = Engine::new();
    e.eval(
        "var log = []; var outcome = 'unset'; \
         Promise.resolve(1).then(function(v){ log.push(v); return v + 1; }) \
             .then(function(v){ log.push(v); outcome = log.join(','); }); \
         log.push(0);",
    )
    .unwrap();
    assert_eq!(str_val(&e, "outcome"), "0,1,2");
}

#[test]
fn promise_basic_fulfill_and_reject() {
    let e = Engine::new();
    e.eval(
        "var out = []; \
         new Promise(function(res){ res(10); }).then(function(v){ out.push(v); }); \
         new Promise(function(_, rej){ rej('bad'); }).then(null, function(r){ out.push(r); }); \
         Promise.resolve(7).then(function(v){ out.push(v); }); \
         Promise.reject('x').catch(function(r){ out.push(r); });",
    )
    .unwrap();
    assert_eq!(str_val(&e, "out.join(',')"), "10,bad,7,x");
}

#[test]
fn promise_executor_throw_rejects() {
    let e = Engine::new();
    e.eval("var got = ''; new Promise(function(){ throw 'oops'; }).then(null, function(r){ got = r; });")
        .unwrap();
    assert_eq!(str_val(&e, "got"), "oops");
}

#[test]
fn promise_ctor_requires_new_and_function() {
    let e = Engine::new();
    assert!(e.eval("Promise(function(){});").is_err());
    assert!(e.eval("new Promise(1);").is_err());
}

#[test]
fn promise_then_returns_new_promise() {
    let e = Engine::new();
    let chained = e
        .eval("var p = Promise.resolve(1); var q = p.then(function(v){ return v + 1; }); [p instanceof Promise, q instanceof Promise, p === q]")
        .unwrap();
    assert_eq!(str_val(&e, "JSON.stringify([p instanceof Promise, q instanceof Promise, p === q])"), "[true,true,false]");
    let _ = chained;
    e.eval("var seen = -1; q.then(function(v){ seen = v; });").unwrap();
    assert_eq!(e.eval("seen").unwrap(), Value::Number(2.0));
}

#[test]
fn promise_resolve_identity() {
    let e = Engine::new();
    e.eval("var p = Promise.resolve(1); var same = Promise.resolve(p) === p;")
        .unwrap();
    assert_eq!(e.eval("same").unwrap(), Value::Boolean(true));
}

#[test]
fn promise_self_resolution_rejects() {
    let e = Engine::new();
    e.eval(
        "var got = 'none'; var p2 = Promise.resolve().then(function(){ return p2; }); \
         p2.then(null, function(e){ got = e instanceof TypeError ? 'type-error' : 'other'; });",
    )
    .unwrap();
    assert_eq!(str_val(&e, "got"), "type-error");
}

#[test]
fn promise_thenable_assimilation() {
    let e = Engine::new();
    e.eval(
        "var got = -1; \
         Promise.resolve({ then: function(res){ res(42); } }).then(function(v){ got = v; });",
    )
    .unwrap();
    assert_eq!(e.eval("got").unwrap(), Value::Number(42.0));
}

#[test]
fn promise_then_getter_throw_rejects() {
    let e = Engine::new();
    e.eval(
        "var got = 'none'; \
         var evil = {}; Object.defineProperty(evil, 'then', { get: function(){ throw 'getter-boom'; } }); \
         Promise.resolve(evil).then(null, function(r){ got = r; });",
    )
    .unwrap();
    assert_eq!(str_val(&e, "got"), "getter-boom");
}

#[test]
fn promise_finally_passthrough() {
    let e = Engine::new();
    e.eval(
        "var log = []; var fin1, fin2; \
         Promise.resolve(5).finally(function(){ log.push('fin'); }).then(function(v){ fin1 = v; }); \
         Promise.reject('e').finally(function(){ log.push('fin2'); }).then(null, function(r){ fin2 = r; });",
    )
    .unwrap();
    assert_eq!(e.eval("fin1").unwrap(), Value::Number(5.0));
    assert_eq!(str_val(&e, "fin2"), "e");
    assert_eq!(str_val(&e, "log.join(',')"), "fin,fin2");
}

#[test]
fn promise_finally_throw_overrides() {
    let e = Engine::new();
    e.eval(
        "var got = 'none'; \
         Promise.resolve(1).finally(function(){ throw 'fin-throw'; }).then(null, function(r){ got = r; });",
    )
    .unwrap();
    assert_eq!(str_val(&e, "got"), "fin-throw");
}

#[test]
fn promise_all_basic() {
    let e = Engine::new();
    e.eval(
        "var got = ''; \
         Promise.all([1, Promise.resolve(2), Promise.resolve(3).then(function(x){ return x * 10; })]) \
             .then(function(a){ got = a.join(','); });",
    )
    .unwrap();
    assert_eq!(str_val(&e, "got"), "1,2,30");
}

#[test]
fn promise_all_empty_and_reject() {
    let e = Engine::new();
    e.eval(
        "var empty = -1; Promise.all([]).then(function(a){ empty = a.length; }); \
         var rej = 'none'; \
         Promise.all([Promise.resolve(1), Promise.reject('bad'), Promise.resolve(3)]) \
             .then(null, function(r){ rej = r; });",
    )
    .unwrap();
    assert_eq!(e.eval("empty").unwrap(), Value::Number(0.0));
    assert_eq!(str_val(&e, "rej"), "bad");
}

#[test]
fn promise_all_non_iterable_rejects() {
    let e = Engine::new();
    e.eval("var ok = false; Promise.all(123).then(null, function(){ ok = true; });")
        .unwrap();
    assert_eq!(e.eval("ok").unwrap(), Value::Boolean(true));
}

#[test]
fn promise_race_first_wins() {
    let e = Engine::new();
    e.eval(
        "var v1 = ''; Promise.race([new Promise(function(){}), Promise.resolve('fast')]) \
             .then(function(v){ v1 = v; }); \
         var v2 = ''; Promise.race([Promise.reject('e1'), Promise.resolve('late')]) \
             .then(null, function(r){ v2 = r; });",
    )
    .unwrap();
    assert_eq!(str_val(&e, "v1"), "fast");
    assert_eq!(str_val(&e, "v2"), "e1");
}

#[test]
fn promise_all_settled() {
    let e = Engine::new();
    e.eval(
        "var s0, v0, s1, r1; \
         Promise.allSettled([1, Promise.reject('e')]).then(function(a){ \
             s0 = a[0].status; v0 = a[0].value; s1 = a[1].status; r1 = a[1].reason; });",
    )
    .unwrap();
    assert_eq!(str_val(&e, "s0"), "fulfilled");
    assert_eq!(e.eval("v0").unwrap(), Value::Number(1.0));
    assert_eq!(str_val(&e, "s1"), "rejected");
    assert_eq!(str_val(&e, "r1"), "e");
}

#[test]
fn promise_any_first_fulfillment() {
    let e = Engine::new();
    e.eval(
        "var v = -1; Promise.any([Promise.reject(1), 2, Promise.resolve(3)]) \
             .then(function(x){ v = x; });",
    )
    .unwrap();
    assert_eq!(e.eval("v").unwrap(), Value::Number(2.0));
}

#[test]
fn promise_any_all_reject_gives_aggregate_error() {
    let e = Engine::new();
    e.eval(
        "var isAgg = false; var errs = ''; \
         Promise.any([Promise.reject('a'), Promise.reject('b')]).then(null, function(e){ \
             isAgg = e instanceof AggregateError; errs = e.errors.join(','); });",
    )
    .unwrap();
    assert_eq!(e.eval("isAgg").unwrap(), Value::Boolean(true));
    assert_eq!(str_val(&e, "errs"), "a,b");
}

#[test]
fn promise_with_resolvers() {
    let e = Engine::new();
    e.eval(
        "var wr = Promise.withResolvers(); var v = -1; \
         wr.promise.then(function(x){ v = x; }); wr.resolve('w');",
    )
    .unwrap();
    assert_eq!(str_val(&e, "v"), "w");
}

#[test]
fn promise_prototype_shape() {
    let e = Engine::new();
    e.eval("var tag = Object.prototype.toString.call(Promise.resolve());")
        .unwrap();
    assert_eq!(str_val(&e, "tag"), "[object Promise]");
    assert_eq!(e.eval("typeof Promise").unwrap().to_string().to_string(), "function");
}

#[test]
fn queue_microtask_runs_after_sync() {
    let e = Engine::new();
    e.eval("var log = []; queueMicrotask(function(){ log.push('micro'); }); log.push('sync');")
        .unwrap();
    assert_eq!(str_val(&e, "log.join(',')"), "sync,micro");
}

#[test]
fn async_function_returns_promise_and_awaits() {
    let e = Engine::new();
    e.eval(
        "var log = []; \
         async function f(a) { log.push('f' + a); var b = await a * 2; log.push('g' + b); return b + 1; } \
         var p = f(21); \
         var isP = p instanceof Promise; \
         log.push('sync'); \
         var fin = ''; p.then(function(v){ fin = v; });",
    )
    .unwrap();
    assert_eq!(e.eval("isP").unwrap(), Value::Boolean(true));
    assert_eq!(str_val(&e, "log.join(',')"), "f21,sync,g42");
    assert_eq!(e.eval("fin").unwrap(), Value::Number(43.0));
}

#[test]
fn await_pending_gate_resumes_later() {
    let e = Engine::new();
    e.eval(
        "var resolveX; var gate = new Promise(function(r){ resolveX = r; }); \
         var order = []; \
         async function f(){ order.push('a'); await gate; order.push('b'); return 7; } \
         var p = f(); order.push('c'); resolveX(1); order.push('d'); \
         var tail = -1; p.then(function(v){ tail = v; order.push('e'); });",
    )
    .unwrap();
    assert_eq!(str_val(&e, "order.join(',')"), "a,c,d,b,e");
    assert_eq!(e.eval("tail").unwrap(), Value::Number(7.0));
}

#[test]
fn await_rejection_throws_into_async() {
    let e = Engine::new();
    e.eval(
        "var out = ''; \
         async function g(){ try { await Promise.reject(new Error('boom')); return 'no'; } \
             catch(e) { return 'caught:' + e.message; } } \
         g().then(function(v){ out = v; }); \
         async function h(){ await Promise.reject('unhandled-x'); } \
         var hr = ''; h().then(null, function(r){ hr = r; });",
    )
    .unwrap();
    assert_eq!(str_val(&e, "out"), "caught:boom");
    assert_eq!(str_val(&e, "hr"), "unhandled-x");
}

#[test]
fn await_in_loops_and_conditionals() {
    let e = Engine::new();
    e.eval(
        "async function s(){ var t = 0; for (var i = 1; i <= 3; i++){ t += await i; } return t; } \
         var r1 = -1; s().then(function(v){ r1 = v; }); \
         async function o(){ var a = []; for (var x of [1, 2, 3]){ a.push(await x * 10); } return a.join(','); } \
         var r2 = ''; o().then(function(v){ r2 = v; }); \
         async function w(){ var n = 3, acc = ''; while (n > 0){ acc += await n; n--; } return acc; } \
         var r3 = ''; w().then(function(v){ r3 = v; }); \
         async function c(x){ if (await x) { return await 'yes'; } return 'no'; } \
         var r4 = ''; c(1).then(function(v){ r4 = v; });",
    )
    .unwrap();
    assert_eq!(e.eval("r1").unwrap(), Value::Number(6.0));
    assert_eq!(str_val(&e, "r2"), "10,20,30");
    assert_eq!(str_val(&e, "r3"), "321");
    assert_eq!(str_val(&e, "r4"), "yes");
}

#[test]
fn await_in_try_finally() {
    let e = Engine::new();
    e.eval(
        "var log = []; \
         async function f(){ try { await 1; log.push('try'); return 'ret'; } \
             finally { await 2; log.push('fin'); } } \
         var r = ''; f().then(function(v){ r = v; });",
    )
    .unwrap();
    assert_eq!(str_val(&e, "r"), "ret");
    assert_eq!(str_val(&e, "log.join(',')"), "try,fin");
}

#[test]
fn async_arrow_and_method() {
    let e = Engine::new();
    e.eval(
        "var f = async (x) => await x * 2; var r1 = -1; f(21).then(function(v){ r1 = v; }); \
         var o = { async m(x) { return await x + 1; } }; var r2 = -1; o.m(41).then(function(v){ r2 = v; });",
    )
    .unwrap();
    assert_eq!(e.eval("r1").unwrap(), Value::Number(42.0));
    assert_eq!(e.eval("r2").unwrap(), Value::Number(42.0));
}

#[test]
fn async_throw_rejects() {
    let e = Engine::new();
    e.eval(
        "async function f(){ throw new Error('nope'); } \
         var r = ''; f().then(null, function(e){ r = e.message; });",
    )
    .unwrap();
    assert_eq!(str_val(&e, "r"), "nope");
}

#[test]
fn async_not_constructable() {
    let e = Engine::new();
    assert!(e.eval("new (async function(){})();").is_err());
    // Async functions have no `prototype` property.
    assert_eq!(
        e.eval("(async function(){}).hasOwnProperty('prototype')").unwrap(),
        Value::Boolean(false)
    );
}

#[test]
fn top_level_await_is_an_error() {
    let e = Engine::new();
    assert!(e.eval("await 1;").is_err());
}

#[test]
fn await_in_sync_function_is_an_error() {
    let e = Engine::new();
    assert!(e.eval("function f(){ return await 1; } f();").is_err());
}

#[test]
fn async_generators_are_unimplemented() {
    let e = Engine::new();
    let err = e.eval("async function* g(){}").unwrap_err();
    assert!(matches!(err, tengin::Error::Unimplemented(_)));
}

#[test]
fn for_await_is_unimplemented() {
    let e = Engine::new();
    let err = e
        .eval("async function f(){ for await (const x of []); }")
        .unwrap_err();
    assert!(matches!(err, tengin::Error::Unimplemented(_)));
}

#[test]
fn unhandled_rejections_are_reported() {
    let e = Engine::new();
    e.eval("Promise.reject('lost');").unwrap();
    let unhandled = e.take_unhandled();
    assert_eq!(unhandled.len(), 1);
    let e2 = Engine::new();
    e2.eval("Promise.reject('x').catch(function(){});").unwrap();
    assert!(e2.take_unhandled().is_empty());
}

#[test]
fn async_function_intrinsics() {
    let e = Engine::new();
    e.eval("var f = async function(){};").unwrap();
    assert_eq!(
        str_val(&e, "Object.prototype.toString.call(f)"),
        "[object AsyncFunction]"
    );
    assert_eq!(
        e.eval("Object.getPrototypeOf(f) === Function.prototype").unwrap(),
        Value::Boolean(false)
    );
    assert_eq!(
        e.eval("f instanceof Function").unwrap(),
        Value::Boolean(true)
    );
    assert_eq!(
        e.eval("f.constructor === AsyncFunction").unwrap(),
        Value::Boolean(true)
    );
    assert_eq!(
        e.eval("AsyncFunction.prototype.constructor === AsyncFunction").unwrap(),
        Value::Boolean(true)
    );
    assert_eq!(str_val(&e, "AsyncFunction.name"), "AsyncFunction");
    assert_eq!(e.eval("typeof AsyncFunction.prototype").unwrap().to_string().to_string(), "object");
}

#[test]
fn async_function_constructor() {
    let e = Engine::new();
    e.eval(
        "var f = new AsyncFunction('x', 'return await x + 1;'); \
         var r = -1; f(41).then(function(v){ r = v; });",
    )
    .unwrap();
    assert_eq!(e.eval("r").unwrap(), Value::Number(42.0));
    assert_eq!(str_val(&e, "f.name"), "anonymous");
    assert_eq!(
        e.eval("f instanceof AsyncFunction").unwrap(),
        Value::Boolean(true)
    );
}

#[test]
fn await_in_class_heritage() {
    let e = Engine::new();
    e.eval(
        "async function f(){ return class C extends (await Promise.resolve(Object)) {}; } \
         var r = false; f().then(function(C){ r = new C() instanceof C; });",
    )
    .unwrap();
    assert_eq!(e.eval("r").unwrap(), Value::Boolean(true));
}

#[test]
fn await_in_class_computed_keys() {
    let e = Engine::new();
    e.eval(
        "async function f(){ \
             var k = await Promise.resolve('dyn'); \
             return class { static [k] = 42; }; \
         } \
         var r = -1; f().then(function(C){ r = C.dyn; }); \
         async function g(){ \
             var m = await Promise.resolve('m'); \
             return class { [m]() { return 7; } }; \
         } \
         var r2 = -1; g().then(function(C){ r2 = new C().m(); });",
    )
    .unwrap();
    assert_eq!(e.eval("r").unwrap(), Value::Number(42.0));
    assert_eq!(e.eval("r2").unwrap(), Value::Number(7.0));
}

#[test]
fn await_in_static_field_init_is_syntax_error() {
    // Class field initializers are not async contexts, even lexically inside
    // an async function: the parser rejects `await` there.
    let e = Engine::new();
    assert!(
        e.eval("async function f(){ return class { static v = await Promise.resolve(9); }; }")
            .is_err()
    );
}
