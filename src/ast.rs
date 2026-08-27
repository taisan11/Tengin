use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::vec::Vec;

#[derive(Debug, Clone)]
pub struct Program {
    pub stmts: Vec<Stmt>,
}

#[derive(Debug, Clone)]
pub enum Stmt {
    /// `var` (possibly multiple declarators).
    Var(Vec<(Rc<str>, Option<Expr>)>),
    Expr(Expr),
    FunctionDecl {
        name: Rc<str>,
        params: Vec<Rc<str>>,
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
        var: bool,
        name: Rc<str>,
        expr: Expr,
        body: Vec<Stmt>,
    },
    Break,
    Continue,
    Throw(Expr),
    /// Expression used as a statement (e.g. assignment).
    Empty,
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
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    Function {
        params: Vec<Rc<str>>,
        body: Vec<Stmt>,
    },
    New {
        callee: Box<Expr>,
        args: Vec<Expr>,
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
        target: Box<Expr>,
        value: Box<Expr>,
    },
    /// `super` (only meaningful inside class methods/constructors).
    Super,
    Array(Vec<Expr>),
    /// Object literal `{ key: value, "key": value, [computed]: value, method(){} }`.
    Object(Vec<Prop>),
    /// `class` expression.
    Class(Box<Class>),
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
    Method { key: PropKey, params: Vec<Rc<str>>, body: Vec<Stmt> },
    /// Accessor `get key() { ... }` / `set key(v) { ... }`.
    Accessor {
        get: bool,
        key: PropKey,
        params: Vec<Rc<str>>,
        body: Vec<Stmt>,
    },
}

/// A member of a `class` body.
#[derive(Debug, Clone)]
pub enum ClassElem {
    Constructor { params: Vec<Rc<str>>, body: Vec<Stmt> },
    Method {
        is_static: bool,
        get: bool,
        key: PropKey,
        params: Vec<Rc<str>>,
        body: Vec<Stmt>,
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
}

#[derive(Debug, Clone, Copy)]
pub enum LogicalOp {
    And,
    Or,
}

#[derive(Debug, Clone, Copy)]
pub enum UnaryOp {
    Neg,
    Plus,
    Not,
    Typeof,
    Void,
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
}
