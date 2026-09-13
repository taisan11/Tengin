//! Derived-class constructors and `super()` integration tests.

use tengin::{Engine, Value};

fn str_val(e: &Engine, src: &str) -> String {
    e.eval(src).unwrap().to_string().to_string()
}

#[test]
fn explicit_super_forwards() {
    let e = Engine::new();
    e.eval(
        "class B extends Array { constructor(){ super(); this.push(9); } } \
         var b = new B();",
    )
    .unwrap();
    assert_eq!(e.eval("b instanceof B").unwrap(), Value::Boolean(true));
    assert_eq!(e.eval("b.length").unwrap(), Value::Number(1.0));
    assert_eq!(e.eval("b[0]").unwrap(), Value::Number(9.0));
}

#[test]
fn default_derived_ctor_forwards_args() {
    let e = Engine::new();
    e.eval(
        "class P extends Array {} \
         var a = new P(); \
         class Q extends Object {} \
         var q = new Q();",
    )
    .unwrap();
    assert_eq!(e.eval("a instanceof P").unwrap(), Value::Boolean(true));
    assert_eq!(e.eval("q instanceof Q").unwrap(), Value::Boolean(true));
}

#[test]
fn super_binds_this_for_later_use() {
    let e = Engine::new();
    e.eval(
        "class B { constructor(x){ this.x = x; } } \
         class D extends B { constructor(a){ super(a); this.y = this.x + 1; } } \
         var d = new D(41);",
    )
    .unwrap();
    assert_eq!(e.eval("d.x").unwrap(), Value::Number(41.0));
    assert_eq!(e.eval("d.y").unwrap(), Value::Number(42.0));
    assert_eq!(e.eval("d instanceof D").unwrap(), Value::Boolean(true));
    assert_eq!(e.eval("d instanceof B").unwrap(), Value::Boolean(true));
}

#[test]
fn explicit_object_return_overrides() {
    let e = Engine::new();
    e.eval(
        "class B { constructor(){ this.a = 1; } } \
         class D extends B { constructor(){ super(); return { z: 9 }; } } \
         var d = new D();",
    )
    .unwrap();
    assert_eq!(e.eval("d.z").unwrap(), Value::Number(9.0));
}

#[test]
fn super_call_value_is_instance() {
    let e = Engine::new();
    e.eval(
        "class B { constructor(x){ this.v = x * 2; } } \
         class D extends B { constructor(x){ var s = super(x); this.ok = s instanceof D; } } \
         var d = new D(21);",
    )
    .unwrap();
    assert_eq!(e.eval("d.v").unwrap(), Value::Number(42.0));
    assert_eq!(e.eval("d.ok").unwrap(), Value::Boolean(true));
}

#[test]
fn multi_level_derivation() {
    let e = Engine::new();
    e.eval(
        "class A { constructor(x){ this.a = x; } } \
         class B extends A { constructor(x){ super(x + 1); this.b = this.a + 1; } } \
         class C extends B { constructor(x){ super(x + 1); this.c = this.b + 1; } } \
         var c = new C(1);",
    )
    .unwrap();
    assert_eq!(str_val(&e, "[c.a, c.b, c.c].join(',')"), "3,4,5");
    assert_eq!(e.eval("c instanceof C").unwrap(), Value::Boolean(true));
    assert_eq!(e.eval("c instanceof A").unwrap(), Value::Boolean(true));
}

#[test]
fn base_class_unchanged() {
    let e = Engine::new();
    e.eval("class A { constructor(x){ this.x = x || 0; } } var a = new A(5);")
        .unwrap();
    assert_eq!(e.eval("a.x").unwrap(), Value::Number(5.0));
    assert_eq!(e.eval("a instanceof A").unwrap(), Value::Boolean(true));
}
