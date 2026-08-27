use alloc::string::String;
use core::fmt;

use crate::value::Value;

/// Errors produced by the Tengin engine.
#[derive(Debug, Clone)]
pub enum Error {
    /// A parse/lex failure.
    Parse(String),
    /// A thrown JavaScript exception (carries the thrown value).
    Runtime(Value),
    /// Internal control-flow signal for `return`. Never escapes a function call.
    Return(Value),
    /// Internal control-flow signal for `break`. Never escapes a loop/switch.
    #[allow(dead_code)]
    Break,
    /// Internal control-flow signal for `continue`. Never escapes a loop.
    #[allow(dead_code)]
    Continue,
}

pub type Result<T> = core::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Parse(s) => write!(f, "parse error: {s}"),
            Error::Runtime(v) => write!(f, "runtime error: {}", v.to_string()),
            Error::Return(_) => write!(f, "unexpected return"),
            Error::Break => write!(f, "unexpected break"),
            Error::Continue => write!(f, "unexpected continue"),
        }
    }
}
