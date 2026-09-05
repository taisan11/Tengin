# Tengin

A small JavaScript engine written in Rust.

Tengin parses and evaluates JavaScript using the [Oxc](https://oxc.rs/) parser
(`oxc_parser` / `oxc_ast`), then runs a tree-walking interpreter over the
resulting AST.

## Note: `no_std` dropped

Tengin previously targeted `no_std + alloc` environments and shipped a
hand-written lexer and parser. That design has been retired: Oxc requires
`std`, so Tengin is now a `std` crate. The hand-written `lexer`/`parser` were
replaced by an Oxc-based front-end (`src/parser.rs`) that parses source with
`oxc_parser` and converts the `oxc_ast` into Tengin's interpreter AST.

## Quick start

```rust
use tengin::{Engine, Value};

let engine = Engine::new();
let result = engine.eval("1 + 2 * 3").unwrap();
assert_eq!(result, Value::Number(7.0));
```

## Architecture

- `parser` turns source text into an AST using Oxc, then converts it into the
  interpreter's AST (`ast`).
- `interpreter` evaluates the AST against an `Engine`.
- `value` defines the JavaScript `Value` representation.
- `builtins` installs the standard library (`Object`, `Array`, `Math`, …).

TypeScript and JSX syntax are accepted by the parser (type annotations are
ignored). Features outside the interpreter's supported subset (e.g. arrow
functions, `for`-`of`, class fields) produce a parse error, matching the
engine's current capability.
