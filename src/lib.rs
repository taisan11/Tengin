#![no_std]
//! Tengin: a small JavaScript engine that runs in `no_std` + `alloc` environments.
//!
//! The crate is a tree-walking interpreter with a hand-written lexer and parser.
//! The only dependency is [`libm`](https://docs.rs/libm), so the core engine can
//! be built for targets without the standard library.
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
//! - [`lexer`] turns source text into tokens.
//! - [`parser`] produces the AST in [`ast`].
//! - [`interpreter`] evaluates the AST against an [`Engine`].
//! - [`value`] defines the JavaScript [`Value`] representation.
//! - [`builtins`] installs the standard library (`Object`, `Array`, `Math`, …).

extern crate alloc;

pub mod ast;
pub mod builtins;
pub mod environment;
pub mod error;
pub mod interpreter;
pub mod lexer;
pub mod parser;
pub mod value;

pub use error::Error;
pub use interpreter::Engine;
pub use value::Value;
