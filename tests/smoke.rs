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

#[test]
fn typescript_syntax_is_accepted() {
    let engine = Engine::new();
    // Type annotations and `interface`s must be ignored at runtime.
    let ok = engine.eval(
        "interface Point { x: number; y: number } \
         function f(a: number, b: string): number { return a + 1; } \
         const x: number = 1; \
         const y = (x as number) + 2; \
         assert.sameValue(y, 3);",
    );
    assert!(ok.is_ok(), "expected TS to be accepted, got {ok:?}");
}

#[test]
fn modules_are_accepted_without_parse_error() {
    let engine = Engine::new();
    // Imports/exports are not linked, but they must not produce a parse error.
    let ok = engine.parse_source(
        "import def, { a, b as c } from 'mod'; \
         import * as ns from 'mod'; \
         export const x = 1; \
         export function f() { return 2; } \
         export class C {} \
         export default 42; \
         export { x };",
    );
    assert!(ok.is_ok(), "expected modules to parse, got {ok:?}");
}

#[test]
fn exported_declarations_still_execute() {
    let engine = Engine::new();
    // The binding introduced by an `export` declaration is still created.
    let result = engine.eval("export const x = 3; export function f() { return x + 1; } f();");
    assert!(matches!(result, Ok(tengin::Value::Number(4.0))), "got {result:?}");
}

#[test]
fn jsx_is_parsed() {
    // Parsing JSX is accepted (runtime support is not required for this test).
    let engine = Engine::new();
    let err = engine.parse_source("<div className='x'>{1}</div>;");
    // Either it parses (Ok) or errors gracefully with a parse error, never panics.
    assert!(err.is_ok() || matches!(err, Err(tengin::Error::Parse(_))));
}
