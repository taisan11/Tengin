//! Timer (`setTimeout`/`setInterval`) integration tests on the virtual clock.

use tengin::{Engine, Value};

fn str_val(e: &Engine, src: &str) -> String {
    e.eval(src).unwrap().to_string().to_string()
}

#[test]
fn timeout_runs_after_microtasks_in_delay_order() {
    let e = Engine::new();
    e.eval(
        "var log = []; \
         setTimeout(function(){ log.push('t100'); }, 100); \
         setTimeout(function(){ log.push('t10'); }, 10); \
         Promise.resolve().then(function(){ log.push('micro'); }); \
         log.push('sync');",
    )
    .unwrap();
    assert_eq!(str_val(&e, "log.join(',')"), "sync,micro,t10,t100");
}

#[test]
fn timeout_ids_and_clear() {
    let e = Engine::new();
    e.eval(
        "var log = []; \
         var id = setTimeout(function(){ log.push('never'); }, 10); \
         var isNum = typeof id === 'number'; \
         clearTimeout(id); \
         setTimeout(function(){ log.push('yes'); }, 5);",
    )
    .unwrap();
    assert_eq!(e.eval("isNum").unwrap(), Value::Boolean(true));
    assert_eq!(str_val(&e, "log.join(',')"), "yes");
}

#[test]
fn timeout_extra_args() {
    let e = Engine::new();
    e.eval("var got = ''; setTimeout(function(a, b){ got = a + b; }, 0, 'x', 'y');")
        .unwrap();
    assert_eq!(str_val(&e, "got"), "xy");
}

#[test]
fn timeout_non_function_throws() {
    let e = Engine::new();
    assert!(e.eval("setTimeout(123, 0);").is_err());
    assert!(e.eval("setInterval(null);").is_err());
}

#[test]
fn interval_repeats_until_cleared() {
    let e = Engine::new();
    e.eval(
        "var n = 0; \
         var id = setInterval(function(){ n++; if (n >= 3) { clearInterval(id); } }, 5);",
    )
    .unwrap();
    assert_eq!(e.eval("n").unwrap(), Value::Number(3.0));
}

#[test]
fn interval_cleared_from_outside() {
    let e = Engine::new();
    e.eval(
        "var n = 0; \
         var id = setInterval(function(){ n++; }, 5); \
         setTimeout(function(){ clearInterval(id); }, 12);",
    )
    .unwrap();
    // Fires at 5, 10; cleared at 12 before the 15ms firing.
    assert_eq!(e.eval("n").unwrap(), Value::Number(2.0));
}

#[test]
fn timers_interop_with_promises() {
    let e = Engine::new();
    e.eval(
        "var order = []; \
         function delay(ms){ return new Promise(function(res){ setTimeout(res, ms); }); } \
         order.push('a'); \
         delay(10).then(function(){ order.push('b'); }); \
         order.push('c');",
    )
    .unwrap();
    assert_eq!(str_val(&e, "order.join(',')"), "a,c,b");
}

#[test]
fn async_with_timer_gate() {
    let e = Engine::new();
    e.eval(
        "var out = ''; \
         async function f(){ await new Promise(function(res){ setTimeout(res, 20); }); return 'done'; } \
         f().then(function(v){ out = v; });",
    )
    .unwrap();
    assert_eq!(str_val(&e, "out"), "done");
}

#[test]
fn timer_throw_is_unhandled_not_fatal() {
    let e = Engine::new();
    e.eval("var after = false; setTimeout(function(){ throw 't-err'; }, 1); setTimeout(function(){ after = true; }, 2);")
        .unwrap();
    assert_eq!(e.eval("after").unwrap(), Value::Boolean(true));
    assert_eq!(e.take_unhandled().len(), 1);
}
