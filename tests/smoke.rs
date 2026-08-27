use tengin::Engine;

#[test]
fn var_and_assert() {
    let engine = Engine::new();
    let result = engine.eval(
        "var x = 0; \
         assert.sameValue(x, 0, 'x is 0'); \
         assert.sameValue(isNaN(undefined), true); \
         var y; this.y++; assert.sameValue(isNaN(y), true);",
    );
    assert!(result.is_ok(), "expected test to pass, got {result:?}");
}

#[test]
fn thrown_exception_is_reported() {
    let engine = Engine::new();
    let result = engine.eval("assert.sameValue(1, 2);");
    assert!(result.is_err(), "expected a thrown assertion to be an error");
}
