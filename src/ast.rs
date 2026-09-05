use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::vec::Vec;

#[derive(Debug, Clone)]
pub struct Program {
    pub stmts: Vec<Stmt>,
    /// Whether the program begins with a `"use strict"` directive.
    pub strict: bool,
}

/// The binding kind of a variable declaration, driving scoping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarKind {
    /// Function/global scoped (`var`).
    Var,
    /// Block scoped (`let`).
    Let,
    /// Block scoped and immutable (`const`).
    Const,
}

#[derive(Debug, Clone)]
pub enum Stmt {
    /// `var` / `let` / `const` (possibly multiple declarators).
    Var(Vec<(Pattern, Option<Expr>)>, VarKind),
    Expr(Expr),    FunctionDecl {
        name: Rc<str>,
        params: Vec<Pattern>,
        body: Vec<Stmt>,
    },
    Return(Option<Expr>),
    Block(Vec<Stmt>),
    If {
        cond: Expr,
        then: Vec<Stmt>,
        else_: Vec<Stmt>,
    },
    Try {
        try_block: Vec<Stmt>,
        catch: Option<(Rc<str>, Vec<Stmt>)>,
        finally: Option<Vec<Stmt>>,
    },
    Switch {
        discriminant: Expr,
        cases: Vec<(Option<Expr>, Vec<Stmt>)>,
    },
    For {
        init: Option<Box<Stmt>>,
        cond: Option<Expr>,
        update: Option<Expr>,
        body: Vec<Stmt>,
    },
    ForIn {
        kind: VarKind,
        name: Pattern,
        expr: Expr,
        body: Vec<Stmt>,
    },
    ForOf {
        kind: VarKind,
        name: Pattern,
        expr: Expr,
        body: Vec<Stmt>,
    },
    While {
        cond: Expr,
        body: Vec<Stmt>,
    },
    DoWhile {
        cond: Expr,
        body: Vec<Stmt>,
    },
    Labeled {
        label: Rc<str>,
        body: Box<Stmt>,
    },
    Break(Option<Rc<str>>),
    Continue(Option<Rc<str>>),
    Throw(Expr),
    /// Expression used as a statement (e.g. assignment).
    Empty,
}

/// A binding or assignment target pattern (`var [a, b] = ...`, `[a, b] = ...`).
#[derive(Debug, Clone)]
pub enum Pattern {
    Ident(Rc<str>),
    Array {
        elems: Vec<Option<Pattern>>,
        rest: Option<Box<Pattern>>,
    },
    Object {
        props: Vec<(PropKey, Pattern)>,
        rest: Option<Box<Pattern>>,
    },
    /// An assignment to a member/identifier expression used as a loop lvalue.
    Member(Box<Expr>),
    /// A pattern with a default value (`[a = 1]` / `{ a = 1 }`), used in
    /// declarations, parameters, and `for`/`for-of` loop heads.
    Default {
        inner: Box<Pattern>,
        default: Box<Expr>,
    },
}

#[derive(Debug, Clone)]
pub enum Expr {
    Lit(Lit),
    Ident(Rc<str>),
    This,
    /// `obj.prop` (computed == None) or `obj[key]` (computed == Some(expr)).
    Member {
        obj: Box<Expr>,
        prop: Rc<str>,
        computed: Option<Box<Expr>>,
        /// `?.` short-circuiting applies to this member access.
        optional: bool,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Arg>,
        /// `?.()` short-circuiting applies to this call.
        optional: bool,
    },
    Function {
        params: Vec<Pattern>,
        body: Vec<Stmt>,
    },
    /// Arrow function `(params) => body`.
    Arrow {
        params: Vec<Pattern>,
        body: Vec<Stmt>,
    },
    New {
        callee: Box<Expr>,
        args: Vec<Arg>,
    },
    InstanceOf {
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Logical {
        op: LogicalOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Unary {
        op: UnaryOp,
        operand: Box<Expr>,
    },
    Ternary {
        cond: Box<Expr>,
        then: Box<Expr>,
        else_: Box<Expr>,
    },
    Assignment {
        target: AssignTarget,
        value: Box<Expr>,
    },
    /// `super` (only meaningful inside class methods/constructors).
    Super,
    Array(Vec<ArrayElem>),
    /// Object literal `{ key: value, "key": value, [computed]: value, method(){} }`.
    Object(Vec<Prop>),
    /// `class` expression.
    Class(Box<Class>),
    /// Tagged template `tag\`...\``.
    Tagged {
        tag: Box<Expr>,
        cooked: Vec<Expr>,
        raws: Vec<Expr>,
        subs: Vec<Expr>,
    },
    /// `await expr` (parsed so async code does not fail to parse).
    Await(Box<Expr>),
    /// `yield expr` / `yield*` (parsed so generators do not fail to parse).
    Yield {
        expr: Option<Box<Expr>>,
        delegated: bool,
    },
}

impl Program {
    /// Whether the program references a private-field member expression
    /// (`obj.#name`). The engine does not implement class private names, so an
    /// eval/script body that references one is a `SyntaxError` (the private name
    /// is never in scope). This is used by `eval` to reject such bodies.
    pub fn has_private_member(&self) -> bool {
        self.stmts.iter().any(stmt_has_private)
    }
}

fn stmt_has_private(s: &Stmt) -> bool {
    match s {
        Stmt::Var(items, _) => items
            .iter()
            .any(|(p, e)| pattern_has_private(p) || e.as_ref().is_some_and(expr_has_private)),
        Stmt::Expr(e) => expr_has_private(e),
        Stmt::FunctionDecl { body, .. } => body.iter().any(stmt_has_private),
        Stmt::Return(e) => e.as_ref().is_some_and(expr_has_private),
        Stmt::Block(b) => b.iter().any(stmt_has_private),
        Stmt::If { cond, then, else_ } => {
            expr_has_private(cond)
                || then.iter().any(stmt_has_private)
                || else_.iter().any(stmt_has_private)
        }
        Stmt::Try { try_block, catch, finally } => {
            try_block.iter().any(stmt_has_private)
                || catch.as_ref().is_some_and(|(_, b)| b.iter().any(stmt_has_private))
                || finally.as_ref().is_some_and(|b| b.iter().any(stmt_has_private))
        }
        Stmt::Switch { discriminant, cases } => {
            expr_has_private(discriminant)
                || cases.iter().any(|(e, b)| {
                    e.as_ref().is_some_and(expr_has_private) || b.iter().any(stmt_has_private)
                })
        }
        Stmt::For { init, cond, update, body } => {
            init.as_deref().is_some_and(stmt_has_private)
                || cond.as_ref().is_some_and(expr_has_private)
                || update.as_ref().is_some_and(expr_has_private)
                || body.iter().any(stmt_has_private)
        }
        Stmt::ForIn { name, expr, body, .. } => {
            pattern_has_private(name) || expr_has_private(expr) || body.iter().any(stmt_has_private)
        }
        Stmt::ForOf { name, expr, body, .. } => {
            pattern_has_private(name) || expr_has_private(expr) || body.iter().any(stmt_has_private)
        }
        Stmt::While { cond, body } => expr_has_private(cond) || body.iter().any(stmt_has_private),
        Stmt::DoWhile { cond, body } => expr_has_private(cond) || body.iter().any(stmt_has_private),
        Stmt::Labeled { body, .. } => stmt_has_private(body),
        Stmt::Throw(e) => expr_has_private(e),
        Stmt::Break(_) | Stmt::Continue(_) | Stmt::Empty => false,
    }
}

fn expr_has_private(e: &Expr) -> bool {
    match e {
        Expr::Member { obj, prop, computed, .. } => {
            prop.starts_with('#')
                || expr_has_private(obj)
                || computed.as_ref().is_some_and(|c| expr_has_private(c))
        }
        Expr::Call { callee, args, .. } => {
            expr_has_private(callee) || args.iter().any(arg_has_private)
        }
        Expr::Function { params, body } => {
            params.iter().any(pattern_has_private) || body.iter().any(stmt_has_private)
        }
        Expr::Arrow { params, body } => {
            params.iter().any(pattern_has_private) || body.iter().any(stmt_has_private)
        }
        Expr::New { callee, args } => expr_has_private(callee) || args.iter().any(arg_has_private),
        Expr::InstanceOf { left, right } => expr_has_private(left) || expr_has_private(right),
        Expr::Binary { left, right, .. } => expr_has_private(left) || expr_has_private(right),
        Expr::Logical { left, right, .. } => expr_has_private(left) || expr_has_private(right),
        Expr::Unary { operand, .. } => expr_has_private(operand),
        Expr::Ternary { cond, then, else_ } => {
            expr_has_private(cond) || expr_has_private(then) || expr_has_private(else_)
        }
        Expr::Assignment { target, value } => {
            let target_private = match target {
                AssignTarget::Expr(x) => expr_has_private(x),
                AssignTarget::Pattern(p) => pattern_has_private(p),
            };
            target_private || expr_has_private(value)
        }
        Expr::Array(elems) => elems.iter().any(|el| match el {
            ArrayElem::Expr(x) | ArrayElem::Spread(x) => expr_has_private(x),
            ArrayElem::Elision => false,
        }),
        Expr::Object(props) => props.iter().any(prop_has_private),
        Expr::Class(c) => class_has_private(c),
        Expr::Tagged { tag, subs, .. } => expr_has_private(tag) || subs.iter().any(expr_has_private),
        Expr::Await(x) => expr_has_private(x),
        Expr::Yield { expr, .. } => expr.as_ref().is_some_and(|x| expr_has_private(x)),
        Expr::Lit(_) | Expr::Ident(_) | Expr::This | Expr::Super => false,
    }
}

fn arg_has_private(a: &Arg) -> bool {
    match a {
        Arg::Expr(x) | Arg::Spread(x) => expr_has_private(x),
    }
}

fn prop_has_private(p: &Prop) -> bool {
    match p {
        Prop::Init { key, value } => key_has_private(key) || expr_has_private(value),
        Prop::Method { key, params, body } => {
            key_has_private(key) || params.iter().any(pattern_has_private) || body.iter().any(stmt_has_private)
        }
        Prop::Accessor { key, params, body, .. } => {
            key_has_private(key) || params.iter().any(pattern_has_private) || body.iter().any(stmt_has_private)
        }
        Prop::Spread(x) => expr_has_private(x),
    }
}

fn key_has_private(k: &PropKey) -> bool {
    match k {
        PropKey::Computed(x) => expr_has_private(x),
        PropKey::Ident(_) | PropKey::Str(_) => false,
    }
}

fn class_has_private(c: &Class) -> bool {
    c.extends.as_ref().is_some_and(expr_has_private)
        || c.elements
            .iter()
            .any(|el| class_elem_has_private(el))
}

fn class_elem_has_private(e: &ClassElem) -> bool {
    match e {
        ClassElem::Constructor { params, body } => {
            params.iter().any(pattern_has_private) || body.iter().any(stmt_has_private)
        }
        ClassElem::Method { key, params, body, .. } => {
            key_has_private(key) || params.iter().any(pattern_has_private) || body.iter().any(stmt_has_private)
        }
        ClassElem::Field { key, init, .. } => {
            key_has_private(key) || init.as_ref().is_some_and(expr_has_private)
        }
        ClassElem::AccessorProp { key, .. } => key_has_private(key),
    }
}

fn pattern_has_private(p: &Pattern) -> bool {
    match p {
        Pattern::Member(x) => expr_has_private(x),
        Pattern::Array { elems, rest } => {
            elems.iter().flatten().any(pattern_has_private)
                || rest.as_deref().is_some_and(pattern_has_private)
        }
        Pattern::Object { props, rest } => {
            props.iter().any(|(k, p)| key_has_private(k) || pattern_has_private(p))
                || rest.as_deref().is_some_and(pattern_has_private)
        }
        Pattern::Default { inner, default } => {
            pattern_has_private(inner) || expr_has_private(default)
        }
        Pattern::Ident(_) => false,
    }
}

/// An element of an array literal: either a value or a `...spread`.
#[derive(Debug, Clone)]
pub enum ArrayElem {
    Expr(Expr),
    Spread(Expr),
    /// `,,` hole.
    Elision,
}

/// A call/new argument: either a value or a `...spread`.
#[derive(Debug, Clone)]
pub enum Arg {
    Expr(Expr),
    Spread(Expr),
}

/// An assignment target: a plain expression (member/identifier) or a pattern.
#[derive(Debug, Clone)]
pub enum AssignTarget {
    Expr(Box<Expr>),
    Pattern(Pattern),
}

/// A key in an object literal.
#[derive(Debug, Clone)]
pub enum PropKey {
    /// Bare identifier key (`{ foo: 1 }`).
    Ident(Rc<str>),
    /// String literal key (`{ "foo": 1 }`).
    Str(Rc<str>),
    /// Computed key (`{ [expr]: 1 }`).
    Computed(Expr),
}

/// A single property inside an object literal.
#[derive(Debug, Clone)]
pub enum Prop {
    /// `key: value`.
    Init { key: PropKey, value: Expr },
    /// Method shorthand `key() { ... }`.
    Method { key: PropKey, params: Vec<Pattern>, body: Vec<Stmt> },
    /// Accessor `get key() { ... }` / `set key(v) { ... }`.
    Accessor {
        get: bool,
        key: PropKey,
        params: Vec<Pattern>,
        body: Vec<Stmt>,
    },
    /// `...spread`.
    Spread(Expr),
}

/// A member of a `class` body.
#[derive(Debug, Clone)]
pub enum ClassElem {
    Constructor { params: Vec<Pattern>, body: Vec<Stmt> },
    Method {
        is_static: bool,
        get: bool,
        key: PropKey,
        params: Vec<Pattern>,
        body: Vec<Stmt>,
    },
    /// Class field `name = init` / `name`.
    Field {
        is_static: bool,
        key: PropKey,
        init: Option<Expr>,
    },
    /// Class accessor property `accessor name`.
    AccessorProp {
        is_static: bool,
        key: PropKey,
    },
}

/// A class expression or declaration.
#[derive(Debug, Clone)]
pub struct Class {
    pub name: Option<Rc<str>>,
    pub extends: Option<Expr>,
    pub elements: Vec<ClassElem>,
}

#[derive(Debug, Clone, Copy)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Seq,
    Sne,
    Lt,
    Gt,
    Le,
    Ge,
    /// `in` operator.
    In,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    Ushr,
    Exp,
}

#[derive(Debug, Clone, Copy)]
pub enum LogicalOp {
    And,
    Or,
    /// `??` nullish coalescing.
    Coalesce,
}

#[derive(Debug, Clone, Copy)]
pub enum UnaryOp {
    Neg,
    Plus,
    Not,
    Typeof,
    Void,
    BitNot,
    PreInc,
    PreDec,
    PostInc,
    PostDec,
    Delete,
}

#[derive(Debug, Clone)]
pub enum Lit {
    Number(f64),
    String(Rc<str>),
    Bool(bool),
    Undefined,
    Null,
    /// BigInt literal `123n`.
    BigInt(Rc<str>),
    /// Regular expression literal `/re/flags`.
    Regex {
        pattern: Rc<str>,
        flags: Rc<str>,
    },
}
