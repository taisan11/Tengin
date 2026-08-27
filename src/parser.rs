use alloc::format;
use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::rc::Rc;
use alloc::vec;
use alloc::vec::Vec;

use crate::ast::*;
use crate::error::Error;
use crate::lexer::{Kw, Lexer, Token, TokenKind};

pub fn parse(src: &str) -> Result<Program, Error> {
    let mut lexer = Lexer::new(src);
    let tokens = lexer.tokenize()?;
    let mut p = Parser { tokens, pos: 0 };
    p.parse_program()
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> &Token {
        self.tokens.get(self.pos).expect("token stream exhausted")
    }

    fn peek_kind(&self) -> &TokenKind {
        &self.peek().kind
    }

    fn peek_is_punct(&self, s: &str) -> bool {
        if let TokenKind::Punct = &self.peek().kind {
            self.peek().text.as_ref() == s
        } else {
            false
        }
    }

    fn peek_is_kw(&self, kw: Kw) -> bool {
        matches!(&self.peek().kind, TokenKind::Kw(k) if *k == kw)
    }

    fn bump(&mut self) -> Token {
        let t = self.tokens.get(self.pos).cloned().expect("token stream exhausted");
        self.pos += 1;
        t
    }

    fn at_end(&self) -> bool {
        self.peek().kind == TokenKind::Eof
    }

    fn expect_punct(&mut self, s: &str) -> Result<(), Error> {
        if self.peek_is_punct(s) {
            self.pos += 1;
            Ok(())
        } else {
            Err(Error::Parse(format!(
                "expected '{}' (found '{}' at line {})",
                s,
                self.peek().text,
                self.peek().line
            )))
        }
    }

    fn expect_ident(&mut self) -> Result<Rc<str>, Error> {
        match self.peek().kind.clone() {
            TokenKind::Ident => Ok(self.bump().text),
            _ => Err(Error::Parse("expected identifier".into())),
        }
    }

    fn parse_program(&mut self) -> Result<Program, Error> {
        let mut stmts = Vec::new();
        while !self.at_end() {
            stmts.push(self.parse_stmt()?);
        }
        Ok(Program { stmts })
    }

    fn parse_block_stmts(&mut self) -> Result<Vec<Stmt>, Error> {
        self.expect_punct("{")?;
        let mut stmts = Vec::new();
        while !self.peek_is_punct("}") && !self.at_end() {
            stmts.push(self.parse_stmt()?);
        }
        self.expect_punct("}")?;
        Ok(stmts)
    }

    fn parse_stmt_or_block(&mut self) -> Result<Vec<Stmt>, Error> {
        if self.peek_is_punct("{") {
            self.parse_block_stmts()
        } else {
            Ok(vec![self.parse_stmt()?])
        }
    }

    fn parse_stmt(&mut self) -> Result<Stmt, Error> {
        if let TokenKind::Kw(kw) = self.peek().kind.clone() {
            match kw {
                Kw::Var => {
                    let v = self.parse_var_decl()?;
                    self.expect_punct(";")?;
                    return Ok(v);
                }
                Kw::Function => return self.parse_function_decl(),
                Kw::Return => {
                    self.bump();
                    let expr = if self.peek_is_punct(";") || self.peek_is_punct("}") {
                        None
                    } else {
                        Some(self.parse_expr()?)
                    };
                    self.expect_punct(";")?;
                    return Ok(Stmt::Return(expr));
                }
                Kw::Break => {
                    self.bump();
                    self.expect_punct(";")?;
                    return Ok(Stmt::Break);
                }
                Kw::Continue => {
                    self.bump();
                    self.expect_punct(";")?;
                    return Ok(Stmt::Continue);
                }
                Kw::If => return self.parse_if(),
                Kw::For => return self.parse_for(),
                Kw::Switch => return self.parse_switch(),
                Kw::Try => return self.parse_try(),
                Kw::Throw => {
                    self.bump();
                    let e = self.parse_expr()?;
                    self.expect_punct(";")?;
                    return Ok(Stmt::Throw(e));
                }
                Kw::While => return self.parse_while(),
                Kw::Do => return self.parse_do(),
                Kw::Class => return self.parse_class_decl(),
                Kw::Delete => {
                    self.bump();
                    let operand = self.parse_unary()?;
                    return Ok(Stmt::Expr(Expr::Unary {
                        op: UnaryOp::Delete,
                        operand: Box::new(operand),
                    }));
                }
                _ => {}
            }
        }
        if self.peek_is_punct("{") {
            return Ok(Stmt::Block(self.parse_block_stmts()?));
        }
        let e = self.parse_expr()?;
        self.expect_punct(";")?;
        Ok(Stmt::Expr(e))
    }

    fn parse_var_decl(&mut self) -> Result<Stmt, Error> {
        self.bump(); // 'var'
        let mut list = Vec::new();
        loop {
            let name = self.expect_ident()?;
            let init = if self.peek_is_punct("=") {
                self.bump();
                Some(self.parse_expr()?)
            } else {
                None
            };
            list.push((name, init));
            if self.peek_is_punct(",") {
                self.bump();
                continue;
            }
            break;
        }
        Ok(Stmt::Var(list))
    }

    fn parse_params(&mut self) -> Result<Vec<Rc<str>>, Error> {
        self.expect_punct("(")?;
        let mut params = Vec::new();
        if !self.peek_is_punct(")") {
            loop {
                params.push(self.expect_ident()?);
                if self.peek_is_punct(",") {
                    self.bump();
                    continue;
                }
                break;
            }
        }
        self.expect_punct(")")?;
        Ok(params)
    }

    fn parse_function_decl(&mut self) -> Result<Stmt, Error> {
        self.bump(); // 'function'
        let name = self.expect_ident()?;
        let params = self.parse_params()?;
        let body = self.parse_block_stmts()?;
        Ok(Stmt::FunctionDecl { name, params, body })
    }

    fn parse_if(&mut self) -> Result<Stmt, Error> {
        self.bump(); // 'if'
        self.expect_punct("(")?;
        let cond = self.parse_expr()?;
        self.expect_punct(")")?;
        let then = self.parse_stmt_or_block()?;
        let mut else_ = Vec::new();
        if self.peek_is_kw(Kw::Else) {
            self.bump();
            else_ = self.parse_stmt_or_block()?;
        }
        Ok(Stmt::If { cond, then, else_ })
    }

    fn parse_while(&mut self) -> Result<Stmt, Error> {
        self.bump(); // 'while'
        self.expect_punct("(")?;
        let cond = self.parse_expr()?;
        self.expect_punct(")")?;
        let body = self.parse_stmt_or_block()?;
        Ok(Stmt::For {
            init: None,
            cond: Some(cond),
            update: None,
            body,
        })
    }

    fn parse_do(&mut self) -> Result<Stmt, Error> {
        self.bump(); // 'do'
        let body = self.parse_stmt_or_block()?;
        self.expect_punct("while")?;
        self.expect_punct("(")?;
        let cond = self.parse_expr()?;
        self.expect_punct(")")?;
        self.expect_punct(";")?;
        Ok(Stmt::For {
            init: None,
            cond: Some(cond),
            update: None,
            body,
        })
    }

    fn parse_for(&mut self) -> Result<Stmt, Error> {
        self.bump(); // 'for'
        self.expect_punct("(")?;
        if self.peek_is_kw(Kw::Var) {
            self.bump(); // 'var'
            let name = self.expect_ident()?;
            if self.peek_is_kw(Kw::In) {
                self.bump(); // 'in'
                let expr = self.parse_expr()?;
                self.expect_punct(")")?;
                let body = self.parse_stmt_or_block()?;
                return Ok(Stmt::ForIn {
                    var: true,
                    name,
                    expr,
                    body,
                });
            }
            let init = Some(Box::new(Stmt::Var(vec![(name, None)])));
            self.expect_punct(";")?;
            return Ok(Stmt::For {
                init,
                cond: self.parse_for_cond()?,
                update: self.parse_for_update()?,
                body: self.parse_stmt_or_block()?,
            });
        }
        let lhs = self.parse_expr()?;
        if self.peek_is_kw(Kw::In) {
            self.bump(); // 'in'
            let expr = self.parse_expr()?;
            self.expect_punct(")")?;
            let body = self.parse_stmt_or_block()?;
            if let Expr::Ident(name) = lhs {
                return Ok(Stmt::ForIn {
                    var: false,
                    name,
                    expr,
                    body,
                });
            }
            return Err(Error::Parse("for-in target must be an identifier".into()));
        }
        self.expect_punct(";")?;
        let init = Some(Box::new(Stmt::Expr(lhs)));
        Ok(Stmt::For {
            init,
            cond: self.parse_for_cond()?,
            update: self.parse_for_update()?,
            body: self.parse_stmt_or_block()?,
        })
    }

    fn parse_for_cond(&mut self) -> Result<Option<Expr>, Error> {
        if self.peek_is_punct(";") {
            self.bump();
            Ok(None)
        } else {
            let e = self.parse_expr()?;
            self.expect_punct(";")?;
            Ok(Some(e))
        }
    }

    fn parse_for_update(&mut self) -> Result<Option<Expr>, Error> {
        if self.peek_is_punct(")") {
            self.bump();
            Ok(None)
        } else {
            let e = self.parse_expr()?;
            self.expect_punct(")")?;
            Ok(Some(e))
        }
    }

    fn parse_switch(&mut self) -> Result<Stmt, Error> {
        self.bump(); // 'switch'
        self.expect_punct("(")?;
        let disc = self.parse_expr()?;
        self.expect_punct(")")?;
        self.expect_punct("{")?;
        let mut cases = Vec::new();
        while !self.peek_is_punct("}") && !self.at_end() {
            if self.peek_is_kw(Kw::Case) {
                self.bump();
                let e = self.parse_expr()?;
                self.expect_punct(":")?;
                let body = self.parse_case_body()?;
                cases.push((Some(e), body));
            } else if self.peek_is_kw(Kw::Default) {
                self.bump();
                self.expect_punct(":")?;
                let body = self.parse_case_body()?;
                cases.push((None, body));
            } else {
                return Err(Error::Parse("unexpected token in switch".into()));
            }
        }
        self.expect_punct("}")?;
        Ok(Stmt::Switch { discriminant: disc, cases })
    }

    fn parse_case_body(&mut self) -> Result<Vec<Stmt>, Error> {
        let mut stmts = Vec::new();
        while !self.at_end() {
            if self.peek_is_punct("}") {
                break;
            }
            if self.peek_is_kw(Kw::Case) || self.peek_is_kw(Kw::Default) {
                break;
            }
            stmts.push(self.parse_stmt()?);
        }
        Ok(stmts)
    }

    fn parse_try(&mut self) -> Result<Stmt, Error> {
        self.bump(); // 'try'
        let try_block = self.parse_block_stmts()?;
        let mut catch = None;
        let mut finally = None;
        if self.peek_is_kw(Kw::Catch) {
            self.bump();
            self.expect_punct("(")?;
            let name = self.expect_ident()?;
            self.expect_punct(")")?;
            let cb = self.parse_block_stmts()?;
            catch = Some((name, cb));
        }
        if self.peek_is_kw(Kw::Finally) {
            self.bump();
            let fb = self.parse_block_stmts()?;
            finally = Some(fb);
        }
        Ok(Stmt::Try { try_block, catch, finally })
    }

    // ---- Expressions ----

    fn parse_expr(&mut self) -> Result<Expr, Error> {
        self.parse_assignment()
    }

    fn parse_assignment(&mut self) -> Result<Expr, Error> {
        let lhs = self.parse_ternary()?;
        if self.peek_is_punct("=") {
            self.bump();
            let v = self.parse_assignment()?;
            return Ok(Expr::Assignment {
                target: Box::new(lhs),
                value: Box::new(v),
            });
        }
        if self.peek_is_punct("+=") {
            self.bump();
            let v = self.parse_assignment()?;
            return Ok(Expr::Assignment {
                target: Box::new(lhs.clone()),
                value: Box::new(Expr::Binary {
                    op: BinaryOp::Add,
                    left: Box::new(lhs),
                    right: Box::new(v),
                }),
            });
        }
        if self.peek_is_punct("-=") {
            self.bump();
            let v = self.parse_assignment()?;
            return Ok(Expr::Assignment {
                target: Box::new(lhs.clone()),
                value: Box::new(Expr::Binary {
                    op: BinaryOp::Sub,
                    left: Box::new(lhs),
                    right: Box::new(v),
                }),
            });
        }
        if self.peek_is_punct("*=") {
            self.bump();
            let v = self.parse_assignment()?;
            return Ok(Expr::Assignment {
                target: Box::new(lhs.clone()),
                value: Box::new(Expr::Binary {
                    op: BinaryOp::Mul,
                    left: Box::new(lhs),
                    right: Box::new(v),
                }),
            });
        }
        if self.peek_is_punct("/=") {
            self.bump();
            let v = self.parse_assignment()?;
            return Ok(Expr::Assignment {
                target: Box::new(lhs.clone()),
                value: Box::new(Expr::Binary {
                    op: BinaryOp::Div,
                    left: Box::new(lhs),
                    right: Box::new(v),
                }),
            });
        }
        if self.peek_is_punct("%=") {
            self.bump();
            let v = self.parse_assignment()?;
            return Ok(Expr::Assignment {
                target: Box::new(lhs.clone()),
                value: Box::new(Expr::Binary {
                    op: BinaryOp::Rem,
                    left: Box::new(lhs),
                    right: Box::new(v),
                }),
            });
        }
        Ok(lhs)
    }

    fn parse_ternary(&mut self) -> Result<Expr, Error> {
        let cond = self.parse_logical()?;
        if self.peek_is_punct("?") {
            self.bump();
            let then = self.parse_assignment()?;
            self.expect_punct(":")?;
            let else_ = self.parse_assignment()?;
            return Ok(Expr::Ternary {
                cond: Box::new(cond),
                then: Box::new(then),
                else_: Box::new(else_),
            });
        }
        Ok(cond)
    }

    fn parse_logical(&mut self) -> Result<Expr, Error> {
        let mut left = self.parse_equality()?;
        loop {
            if self.peek_is_punct("&&") {
                self.bump();
                let right = self.parse_equality()?;
                left = Expr::Logical {
                    op: LogicalOp::And,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            } else if self.peek_is_punct("||") {
                self.bump();
                let right = self.parse_equality()?;
                left = Expr::Logical {
                    op: LogicalOp::Or,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_equality(&mut self) -> Result<Expr, Error> {
        let mut left = self.parse_relational()?;
        loop {
            let op = match self.peek().text.as_ref() {
                "===" => BinaryOp::Seq,
                "!==" => BinaryOp::Sne,
                "==" => BinaryOp::Eq,
                "!=" => BinaryOp::Ne,
                _ => break,
            };
            self.bump();
            let right = self.parse_relational()?;
            left = Expr::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_relational(&mut self) -> Result<Expr, Error> {
        let mut left = self.parse_additive()?;
        loop {
            if self.peek_is_kw(Kw::Instanceof) {
                self.bump();
                let right = self.parse_additive()?;
                left = Expr::InstanceOf {
                    left: Box::new(left),
                    right: Box::new(right),
                };
                continue;
            }
            let op = match self.peek().text.as_ref() {
                "<" => BinaryOp::Lt,
                ">" => BinaryOp::Gt,
                "<=" => BinaryOp::Le,
                ">=" => BinaryOp::Ge,
                _ => break,
            };
            self.bump();
            let right = self.parse_additive()?;
            left = Expr::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_additive(&mut self) -> Result<Expr, Error> {
        let mut left = self.parse_multiplicative()?;
        loop {
            if self.peek_is_punct("+") {
                self.bump();
                let right = self.parse_multiplicative()?;
                left = Expr::Binary {
                    op: BinaryOp::Add,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            } else if self.peek_is_punct("-") {
                self.bump();
                let right = self.parse_multiplicative()?;
                left = Expr::Binary {
                    op: BinaryOp::Sub,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_multiplicative(&mut self) -> Result<Expr, Error> {
        let mut left = self.parse_unary()?;
        loop {
            if self.peek_is_punct("*") {
                self.bump();
                let right = self.parse_unary()?;
                left = Expr::Binary {
                    op: BinaryOp::Mul,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            } else if self.peek_is_punct("/") {
                self.bump();
                let right = self.parse_unary()?;
                left = Expr::Binary {
                    op: BinaryOp::Div,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            } else if self.peek_is_punct("%") {
                self.bump();
                let right = self.parse_unary()?;
                left = Expr::Binary {
                    op: BinaryOp::Rem,
                    left: Box::new(left),
                    right: Box::new(right),
                };
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, Error> {
        if let TokenKind::Kw(kw) = self.peek().kind.clone() {
            match kw {
                Kw::Typeof => {
                    self.bump();
                    let operand = self.parse_unary()?;
                    return Ok(Expr::Unary {
                        op: UnaryOp::Typeof,
                        operand: Box::new(operand),
                    });
                }
                Kw::Void => {
                    self.bump();
                    let operand = self.parse_unary()?;
                    return Ok(Expr::Unary {
                        op: UnaryOp::Void,
                        operand: Box::new(operand),
                    });
                }
                _ => {}
            }
        }
        if self.peek_is_punct("!") {
            self.bump();
            let operand = self.parse_unary()?;
            return Ok(Expr::Unary {
                op: UnaryOp::Not,
                operand: Box::new(operand),
            });
        }
        if self.peek_is_punct("-") {
            self.bump();
            let operand = self.parse_unary()?;
            return Ok(Expr::Unary {
                op: UnaryOp::Neg,
                operand: Box::new(operand),
            });
        }
        if self.peek_is_punct("+") {
            self.bump();
            let operand = self.parse_unary()?;
            return Ok(Expr::Unary {
                op: UnaryOp::Plus,
                operand: Box::new(operand),
            });
        }
        if self.peek_is_kw(Kw::Delete) {
            self.bump();
            let operand = self.parse_unary()?;
            return Ok(Expr::Unary {
                op: UnaryOp::Delete,
                operand: Box::new(operand),
            });
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> Result<Expr, Error> {
        let mut node = self.parse_primary()?;
        loop {
            if self.peek_is_punct(".") {
                self.bump();
                let prop = self.expect_ident()?;
                node = Expr::Member {
                    obj: Box::new(node),
                    prop,
                    computed: None,
                };
            } else if self.peek_is_punct("[") {
                self.bump();
                let idx = self.parse_expr()?;
                self.expect_punct("]")?;
                node = Expr::Member {
                    obj: Box::new(node),
                    prop: Rc::from(""),
                    computed: Some(Box::new(idx)),
                };
            } else if self.peek_is_punct("(") {
                self.bump();
                let mut args = Vec::new();
                if !self.peek_is_punct(")") {
                    loop {
                        args.push(self.parse_expr()?);
                        if self.peek_is_punct(",") {
                            self.bump();
                            continue;
                        }
                        break;
                    }
                }
                self.expect_punct(")")?;
                node = Expr::Call {
                    callee: Box::new(node),
                    args,
                };
            } else if self.peek_is_punct("++") {
                self.bump();
                node = Expr::Unary {
                    op: UnaryOp::PostInc,
                    operand: Box::new(node),
                };
            } else if self.peek_is_punct("--") {
                self.bump();
                node = Expr::Unary {
                    op: UnaryOp::PostDec,
                    operand: Box::new(node),
                };
            } else {
                break;
            }
        }
        Ok(node)
    }

    fn parse_primary(&mut self) -> Result<Expr, Error> {
        let t = self.peek().clone();
        match t.kind {
            TokenKind::Number => {
                self.bump();
                let n: f64 = t.text.parse().unwrap_or(f64::NAN);
                Ok(Expr::Lit(Lit::Number(n)))
            }
            TokenKind::Str => {
                self.bump();
                Ok(Expr::Lit(Lit::String(t.text)))
            }
            TokenKind::Kw(Kw::True) => {
                self.bump();
                Ok(Expr::Lit(Lit::Bool(true)))
            }
            TokenKind::Kw(Kw::False) => {
                self.bump();
                Ok(Expr::Lit(Lit::Bool(false)))
            }
            TokenKind::Kw(Kw::Undefined) => {
                self.bump();
                Ok(Expr::Lit(Lit::Undefined))
            }
            TokenKind::Kw(Kw::Null) => {
                self.bump();
                Ok(Expr::Lit(Lit::Null))
            }
            TokenKind::Kw(Kw::This) => {
                self.bump();
                Ok(Expr::This)
            }
            TokenKind::Kw(Kw::Function) => {
                self.bump();
                self.parse_function_expr()
            }
            TokenKind::Kw(Kw::New) => {
                self.bump();
                self.parse_new_rest()
            }
            TokenKind::Ident => {
                self.bump();
                Ok(Expr::Ident(t.text))
            }
            TokenKind::Punct if t.text.as_ref() == "(" => {
                self.bump();
                let e = self.parse_expr()?;
                self.expect_punct(")")?;
                Ok(e)
            }
            TokenKind::Punct if t.text.as_ref() == "[" => self.parse_array(),
            TokenKind::Punct if t.text.as_ref() == "{" => self.parse_object(),
            _ => {
                let lo = self.pos.saturating_sub(3);
                let hi = (self.pos + 1).min(self.tokens.len());
                let ctx: Vec<String> = self.tokens[lo..hi].iter().map(|x| x.text.to_string()).collect();
                Err(Error::Parse(format!(
                    "unexpected token '{}' at line {} (ctx: {:?})",
                    t.text, t.line, ctx
                )))
            }
        }
    }

    fn parse_function_expr(&mut self) -> Result<Expr, Error> {
        // Optional name (ignored for now).
        if let TokenKind::Ident = self.peek().kind {
            self.bump();
        }
        let params = self.parse_params()?;
        let body = self.parse_block_stmts()?;
        Ok(Expr::Function { params, body })
    }

    fn parse_new_rest(&mut self) -> Result<Expr, Error> {
        let target = self.parse_primary()?;
        let mut args = Vec::new();
        if self.peek_is_punct("(") {
            self.bump();
            if !self.peek_is_punct(")") {
                loop {
                    args.push(self.parse_expr()?);
                    if self.peek_is_punct(",") {
                        self.bump();
                        continue;
                    }
                    break;
                }
            }
            self.expect_punct(")")?;
        }
        Ok(Expr::New {
            callee: Box::new(target),
            args,
        })
    }

    fn parse_array(&mut self) -> Result<Expr, Error> {
        self.expect_punct("[")?;
        let mut elems = Vec::new();
        if !self.peek_is_punct("]") {
            loop {
                elems.push(self.parse_expr()?);
                if self.peek_is_punct(",") {
                    self.bump();
                    continue;
                }
                break;
            }
        }
        self.expect_punct("]")?;
        Ok(Expr::Array(elems))
    }

    fn parse_object(&mut self) -> Result<Expr, Error> {
        self.expect_punct("{")?;
        let mut props = Vec::new();
        while !self.peek_is_punct("}") && !self.at_end() {
            // Accessor / method shorthand: `get k(){}`, `set k(v){}`, `k(){}`, `k: v`.
            if let TokenKind::Ident = self.peek().kind {
                let txt = self.peek().text.as_ref().to_string();
                if txt == "get" || txt == "set" {
                    let is_get = txt == "get";
                    self.bump();
                    let key = self.parse_prop_key()?;
                    self.expect_punct("(")?;
                    let params = if is_get {
                        Vec::new()
                    } else {
                        vec![self.expect_ident()?]
                    };
                    self.expect_punct(")")?;
                    let body = self.parse_block_stmts()?;
                    props.push(Prop::Method { key, params, body });
                    continue;
                }
            }
            let key = self.parse_prop_key()?;
            if self.peek_is_punct("(") {
                // Method shorthand.
                self.expect_punct("(")?;
                let mut params = Vec::new();
                if !self.peek_is_punct(")") {
                    loop {
                        params.push(self.expect_ident()?);
                        if self.peek_is_punct(",") {
                            self.bump();
                            continue;
                        }
                        break;
                    }
                }
                self.expect_punct(")")?;
                let body = self.parse_block_stmts()?;
                props.push(Prop::Method { key, params, body });
                continue;
            }
            self.expect_punct(":")?;
            let value = self.parse_expr()?;
            props.push(Prop::Init { key, value });
        }
        self.expect_punct("}")?;
        Ok(Expr::Object(props))
    }

    fn parse_prop_key(&mut self) -> Result<PropKey, Error> {
        let t = self.peek().clone();
        match t.kind {
            TokenKind::Ident => {
                self.bump();
                Ok(PropKey::Ident(t.text))
            }
            TokenKind::Str => {
                self.bump();
                Ok(PropKey::Str(t.text))
            }
            TokenKind::Number => {
                self.bump();
                Ok(PropKey::Str(t.text))
            }
            TokenKind::Kw(_) => {
                // Allow keywords as keys (e.g. `default`, `delete`).
                self.bump();
                Ok(PropKey::Ident(t.text))
            }
            TokenKind::Punct if t.text.as_ref() == "[" => {
                self.bump();
                let e = self.parse_expr()?;
                self.expect_punct("]")?;
                Ok(PropKey::Computed(e))
            }
            _ => Err(Error::Parse(format!(
                "expected object property key (found '{}')",
                t.text
            ))),
        }
    }
}
