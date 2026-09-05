//! Tengin: a small JavaScript engine.
//!
//! Tengin parses and evaluates JavaScript using the [Oxc](https://oxc.rs/)
//! parser (`oxc_parser` / `oxc_ast`), then runs a tree-walking interpreter over
//! the resulting AST. Note: Tengin requires `std` (Oxc is not `no_std`
//! compatible), so the earlier `no_std` + `alloc` design was retired.
//!
//! # Quick start
//!
//! ```
//! use tengin::{Engine, Value};
//!
//! let engine = Engine::new();
//! let result = engine.eval("1 + 2 * 3").unwrap();
//! assert_eq!(result, Value::Number(7.0));
//! ```
//!
//! # Architecture
//!
//! - [`parser`] turns source text into an AST using Oxc, then converts it into
//!   the interpreter's AST in [`ast`].
//! - [`interpreter`] evaluates the AST against an [`Engine`].
//! - [`value`] defines the JavaScript [`Value`] representation.
//! - [`builtins`] installs the standard library (`Object`, `Array`, `Math`, …).

extern crate alloc;

pub mod ast;
pub mod builtins;
pub mod environment;
pub mod error;
pub mod interpreter;
pub mod parser;
pub mod value;

pub use error::Error;
pub use interpreter::Engine;
pub use value::Value;
