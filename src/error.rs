use alloc::rc::Rc;
use alloc::string::String;
use core::fmt;

use crate::value::Value;

/// Settlement of an awaited promise, delivered to a suspended continuation.
#[derive(Debug, Clone)]
pub enum Settlement {
    Fulfilled(Value),
    Rejected(Value),
}

/// A suspended `await` continuation: given the settlement of the awaited
/// promise, resume execution and produce the completion value of the suspended
/// fragment (`None` = empty completion, as in statement lists). The returned
/// `Err(Error::Suspend{..})` means the fragment hit another (nested) `await`
/// and suspended again.
pub type SuspendCont = Rc<
    dyn Fn(&crate::interpreter::Engine, Settlement) -> Result<Option<Value>>,
>;

/// Errors produced by the Tengin engine.
pub enum Error {
    /// A parse/lex failure.
    Parse(String),
    /// A feature that parses but is not implemented yet (distinct from both
    /// `Parse` and a JS `SyntaxError`, so callers can tell "valid syntax,
    /// engine limitation" apart from genuine syntax errors).
    Unimplemented(String),
    /// A thrown JavaScript exception (carries the thrown value).
    Runtime(Value),
    /// Internal control-flow signal for `return`. Never escapes a function call.
    Return(Value),
    /// Internal control-flow signal for `break`. Never escapes a loop/switch
    /// (the innermost `BreakableStatement` consumes it). The optional label
    /// names the targeted labelled statement; the value is the statement-list
    /// completion value accumulated up to the break, per spec `UpdateEmpty`:
    /// `None` means the completion value is still empty.
    #[allow(dead_code)]
    Break(Option<alloc::rc::Rc<str>>, Option<Value>),
    /// Internal control-flow signal for `continue`. Never escapes a loop.
    /// The optional label names the targeted labelled statement; the value
    /// follows the same empty/non-empty rule as `Break`.
    #[allow(dead_code)]
    Continue(Option<alloc::rc::Rc<str>>, Option<Value>),
    /// Internal tail-call signal: a `return f(…)` whose callee is a regular
    /// user function. The interpreter unwinds the current call frame and
    /// re-invokes `func` without growing the stack (proper tail calls).
    TailCall {
        func: Value,
        this: Value,
        args: Vec<Value>,
    },
    /// Internal suspension signal for `await` on a still-pending promise.
    /// `awaited` is the promise being waited on; `cont` resumes the suspended
    /// fragment once it settles. Always caught at the nearest enclosing
    /// `async` function boundary (which turns it into a reaction job) — it
    /// never escapes to user code.
    Suspend { awaited: Value, cont: SuspendCont },
}

impl Clone for Error {
    fn clone(&self) -> Self {
        match self {
            Error::Parse(s) => Error::Parse(s.clone()),
            Error::Unimplemented(s) => Error::Unimplemented(s.clone()),
            Error::Runtime(v) => Error::Runtime(v.clone()),
            Error::Return(v) => Error::Return(v.clone()),
            Error::Break(l, v) => Error::Break(l.clone(), v.clone()),
            Error::Continue(l, v) => Error::Continue(l.clone(), v.clone()),
            Error::TailCall { func, this, args } => Error::TailCall {
                func: func.clone(),
                this: this.clone(),
                args: args.clone(),
            },
            Error::Suspend { awaited, cont } => Error::Suspend {
                awaited: awaited.clone(),
                cont: cont.clone(),
            },
        }
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Parse(s) => f.debug_tuple("Parse").field(s).finish(),
            Error::Unimplemented(s) => f.debug_tuple("Unimplemented").field(s).finish(),
            Error::Runtime(v) => f.debug_tuple("Runtime").field(v).finish(),
            Error::Return(v) => f.debug_tuple("Return").field(v).finish(),
            Error::Break(l, v) => f.debug_tuple("Break").field(l).field(v).finish(),
            Error::Continue(l, v) => f.debug_tuple("Continue").field(l).field(v).finish(),
            Error::TailCall { func, this, args } => f
                .debug_struct("TailCall")
                .field("func", func)
                .field("this", this)
                .field("args", args)
                .finish(),
            Error::Suspend { awaited, .. } => {
                f.debug_struct("Suspend").field("awaited", awaited).finish()
            }
        }
    }
}

pub type Result<T> = core::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Parse(s) => write!(f, "parse error: {s}"),
            Error::Unimplemented(s) => write!(f, "unimplemented: {s}"),
            Error::Runtime(v) => write!(f, "runtime error: {}", v.error_message()),
            Error::Return(_) => write!(f, "unexpected return"),
            Error::Break(..) => write!(f, "unexpected break"),
            Error::Continue(..) => write!(f, "unexpected continue"),
            Error::TailCall { .. } => write!(f, "unexpected tail call"),
            Error::Suspend { .. } => write!(f, "suspended await"),
        }
    }
}
