//! Lowering of Oxc statement nodes into Tengin statements.

use std::rc::Rc;
use std::vec::Vec;

use oxc_ast::ast;

use crate::ast::*;
use crate::error::Error;

use super::Converter;

impl Converter {
    pub(crate) fn convert_stmts(&mut self, stmts: &[ast::Statement]) -> Result<Vec<Stmt>, Error> {
        stmts.iter().map(|s| self.convert_stmt(s)).collect()
    }

    pub(crate) fn stmt_to_vec(&mut self, s: &ast::Statement) -> Result<Vec<Stmt>, Error> {
        match s {
            ast::Statement::BlockStatement(b) => self.convert_stmts(&b.body),
            other => Ok(vec![self.convert_stmt(other)?]),
        }
    }

    pub(crate) fn convert_stmt(&mut self, s: &ast::Statement) -> Result<Stmt, Error> {
        match s {
            ast::Statement::BlockStatement(b) => {
                Ok(Stmt::Block(self.convert_stmts(&b.body)?))
            }
            ast::Statement::BreakStatement(b) => {
                Ok(Stmt::Break(b.label.as_ref().map(|l| Rc::from(l.name.as_str()))))
            }
            ast::Statement::ContinueStatement(c) => Ok(Stmt::Continue(
                c.label.as_ref().map(|l| Rc::from(l.name.as_str())),
            )),
            ast::Statement::DebuggerStatement(_) => Ok(Stmt::Empty),
            ast::Statement::EmptyStatement(_) => Ok(Stmt::Empty),
            ast::Statement::ExpressionStatement(e) => {
                Ok(Stmt::Expr(self.convert_expr(&e.expression)?))
            }
            ast::Statement::ForInStatement(f) => {
                let (name, kind) = self.convert_for_left(&f.left)?;
                let expr = self.convert_expr(&f.right)?;
                let body = self.stmt_to_vec(&f.body)?;
                Ok(Stmt::ForIn {
                    kind,
                    name,
                    expr,
                    body,
                })
            }
            ast::Statement::ForOfStatement(f) => {
                let (name, kind) = self.convert_for_left(&f.left)?;
                let expr = self.convert_expr(&f.right)?;
                let body = self.stmt_to_vec(&f.body)?;
                Ok(Stmt::ForOf {
                    kind,
                    name,
                    expr,
                    body,
                })
            }
            ast::Statement::ForStatement(f) => {
                let init = match &f.init {
                    Some(i) => Some(Box::new(self.convert_for_init(i)?)),
                    None => None,
                };
                let cond = match &f.test {
                    Some(t) => Some(self.convert_expr(t)?),
                    None => None,
                };
                let update = match &f.update {
                    Some(u) => Some(self.convert_expr(u)?),
                    None => None,
                };
                let body = self.stmt_to_vec(&f.body)?;
                Ok(Stmt::For {
                    init,
                    cond,
                    update,
                    body,
                })
            }
            ast::Statement::IfStatement(i) => {
                let cond = self.convert_expr(&i.test)?;
                let then = self.stmt_to_vec(&i.consequent)?;
                let else_ = match &i.alternate {
                    Some(a) => self.stmt_to_vec(a)?,
                    None => Vec::new(),
                };
                Ok(Stmt::If { cond, then, else_ })
            }
            ast::Statement::ReturnStatement(r) => Ok(Stmt::Return(match &r.argument {
                Some(a) => Some(self.convert_expr(a)?),
                None => None,
            })),
            ast::Statement::SwitchStatement(s) => {
                let discriminant = self.convert_expr(&s.discriminant)?;
                let mut cases = Vec::new();
                for c in &s.cases {
                    let test = match &c.test {
                        Some(t) => Some(self.convert_expr(t)?),
                        None => None,
                    };
                    cases.push((test, self.convert_stmts(&c.consequent)?));
                }
                Ok(Stmt::Switch { discriminant, cases })
            }
            ast::Statement::ThrowStatement(t) => {
                Ok(Stmt::Throw(self.convert_expr(&t.argument)?))
            }
            ast::Statement::TryStatement(t) => {
                let try_block = self.convert_stmts(&t.block.body)?;
                let catch = match &t.handler {
                    Some(h) => {
                        let name = match &h.param {
                            Some(cp) => match &cp.pattern {
                                ast::BindingPattern::BindingIdentifier(id) => {
                                    Rc::from(id.name.as_str())
                                }
                                _ => Rc::from(""),
                            },
                            None => Rc::from(""),
                        };
                        Some((name, self.convert_stmts(&h.body.body)?))
                    }
                    None => None,
                };
                let finally = match &t.finalizer {
                    Some(b) => Some(self.convert_stmts(&b.body)?),
                    None => None,
                };
                Ok(Stmt::Try {
                    try_block,
                    catch,
                    finally,
                })
            }
            ast::Statement::DoWhileStatement(d) => {
                let cond = self.convert_expr(&d.test)?;
                let body = self.stmt_to_vec(&d.body)?;
                Ok(Stmt::DoWhile { cond, body })
            }
            ast::Statement::WhileStatement(w) => {
                let cond = self.convert_expr(&w.test)?;
                let body = self.stmt_to_vec(&w.body)?;
                Ok(Stmt::While { cond, body })
            }
            ast::Statement::WithStatement(w) => {
                // `with` is not supported semantically; execute its body in the
                // current scope so the program still parses and runs.
                let _obj = self.convert_expr(&w.object)?;
                let body = self.stmt_to_vec(&w.body)?;
                Ok(Stmt::Block(body))
            }
            ast::Statement::LabeledStatement(l) => {
                let label = Rc::from(l.label.name.as_str());
                let body = Box::new(self.convert_stmt(&l.body)?);
                Ok(Stmt::Labeled { label, body })
            }
            ast::Statement::VariableDeclaration(v) => {
                let (d, k) = self.convert_var_decl(v)?;
                Ok(Stmt::Var(d, k))
            }
            ast::Statement::FunctionDeclaration(f) => {
                let name: Rc<str> = match &f.id {
                    Some(id) => Rc::from(id.name.as_str()),
                    None => return Err(Error::Parse("anonymous function declaration".to_string())),
                };
                let (params, body) = self.convert_function(f)?;
                Ok(Stmt::FunctionDecl { name, params, body })
            }
            ast::Statement::ClassDeclaration(c) => {
                let name: Rc<str> = match &c.id {
                    Some(id) => Rc::from(id.name.as_str()),
                    None => return Err(Error::Parse("anonymous class declaration".to_string())),
                };
                let class = self.convert_class(c, Some(name.clone()))?;
                Ok(Stmt::Var(
                    vec![(Pattern::Ident(name), Some(Expr::Class(Box::new(class))))],
                    VarKind::Let,
                ))
            }
            // TypeScript-only declarations: no runtime effect, ignore.
            ast::Statement::TSTypeAliasDeclaration(_)
            | ast::Statement::TSInterfaceDeclaration(_)
            | ast::Statement::TSEnumDeclaration(_)
            | ast::Statement::TSExternalModuleDeclaration(_)
            | ast::Statement::TSNamespaceDeclaration(_)
            | ast::Statement::TSGlobalDeclaration(_)
            | ast::Statement::TSImportEqualsDeclaration(_) => Ok(Stmt::Empty),
            // Modules (import/export): accept and degrade gracefully. The
            // interpreter has no module linker, so imports introduce no bindings
            // and exports keep their inner declaration's runtime effect.
            ast::Statement::ImportDeclaration(_)
            | ast::Statement::ExportAllDeclaration(_)
            | ast::Statement::ExportNamedDeclaration(_)
            | ast::Statement::ExportFromDeclaration(_)
            | ast::Statement::TSExportAssignment(_)
            | ast::Statement::TSNamespaceExportDeclaration(_) => Ok(Stmt::Empty),
            ast::Statement::ExportDeclaration(d) => self.convert_export_decl(d),
            ast::Statement::ExportDefaultDeclaration(d) => self.convert_export_default(d),
        }
    }

    pub(crate) fn convert_export_decl(&mut self, d: &ast::ExportDeclaration) -> Result<Stmt, Error> {
        match &d.declaration {
            ast::Declaration::VariableDeclaration(v) => {
                let (d, k) = self.convert_var_decl(v)?;
                Ok(Stmt::Var(d, k))
            }
            ast::Declaration::FunctionDeclaration(f) => {
                let name: Rc<str> = match &f.id {
                    Some(id) => Rc::from(id.name.as_str()),
                    None => return Err(Error::Parse("anonymous function declaration".to_string())),
                };
                let (params, body) = self.convert_function(f)?;
                Ok(Stmt::FunctionDecl { name, params, body })
            }
            ast::Declaration::ClassDeclaration(c) => {
                let name: Rc<str> = match &c.id {
                    Some(id) => Rc::from(id.name.as_str()),
                    None => return Err(Error::Parse("anonymous class declaration".to_string())),
                };
                let class = self.convert_class(c, Some(name.clone()))?;
                Ok(Stmt::Var(
                    vec![(Pattern::Ident(name), Some(Expr::Class(Box::new(class))))],
                    VarKind::Let,
                ))
            }
            _ => Ok(Stmt::Empty),
        }
    }

    pub(crate) fn convert_export_default(
        &mut self,
        d: &ast::ExportDefaultDeclaration,
    ) -> Result<Stmt, Error> {
        match &d.declaration {
            ast::ExportDefaultDeclarationKind::FunctionDeclaration(f) => match &f.id {
                Some(id) => {
                    let name = Rc::from(id.name.as_str());
                    let (params, body) = self.convert_function(f)?;
                    Ok(Stmt::FunctionDecl { name, params, body })
                }
                None => {
                    let (params, body) = self.convert_function(f)?;
                    Ok(Stmt::Expr(Expr::Function { params, body }))
                }
            },
            ast::ExportDefaultDeclarationKind::ClassDeclaration(c) => {
                let class = self.convert_class(c, None)?;
                Ok(Stmt::Expr(Expr::Class(Box::new(class))))
            }
            ast::ExportDefaultDeclarationKind::TSInterfaceDeclaration(_) => Ok(Stmt::Empty),
            other => match other.as_expression() {
                Some(e) => Ok(Stmt::Expr(self.convert_expr(e)?)),
                None => Ok(Stmt::Empty),
            },
        }
    }

    pub(crate) fn convert_var_decl(
        &mut self,
        v: &ast::VariableDeclaration,
    ) -> Result<(Vec<(Pattern, Option<Expr>)>, VarKind), Error> {
        let kind = match v.kind {
            ast::VariableDeclarationKind::Var => VarKind::Var,
            ast::VariableDeclarationKind::Const => VarKind::Const,
            ast::VariableDeclarationKind::Let => VarKind::Let,
            ast::VariableDeclarationKind::Using | ast::VariableDeclarationKind::AwaitUsing => VarKind::Let,
        };
        let mut out = Vec::new();
        for d in &v.declarations {
            let name = self.convert_binding_pattern(&d.id)?;
            let init = match &d.init {
                Some(e) => Some(self.convert_expr(e)?),
                None => None,
            };
            out.push((name, init));
        }
        Ok((out, kind))
    }

    pub(crate) fn convert_for_init(&mut self, init: &ast::ForStatementInit) -> Result<Stmt, Error> {
        match init {
            ast::ForStatementInit::VariableDeclaration(v) => {
                let (d, k) = self.convert_var_decl(v)?;
                Ok(Stmt::Var(d, k))
            }
            other => match other.as_expression() {
                Some(e) => Ok(Stmt::Expr(self.convert_expr(e)?)),
                None => Err(Error::Parse("unsupported for-loop initializer".to_string())),
            },
        }
    }

    pub(crate) fn convert_for_left(&mut self, left: &ast::ForStatementLeft) -> Result<(Pattern, VarKind), Error> {
        let var = VarKind::Var;
        match left {
            ast::ForStatementLeft::VariableDeclaration(v) => {
                let kind = match v.kind {
                    ast::VariableDeclarationKind::Var => VarKind::Var,
                    ast::VariableDeclarationKind::Let => VarKind::Let,
                    ast::VariableDeclarationKind::Const => VarKind::Const,
                    ast::VariableDeclarationKind::Using | ast::VariableDeclarationKind::AwaitUsing => VarKind::Let,
                };
                match v.declarations.first() {
                    Some(d) => Ok((self.convert_binding_pattern(&d.id)?, kind)),
                    None => Err(Error::Parse("empty for-in initializer".to_string())),
                }
            }
            ast::ForStatementLeft::AssignmentTargetIdentifier(id) => {
                Ok((Pattern::Ident(Rc::from(id.name.as_str())), var))
            }
            ast::ForStatementLeft::ArrayAssignmentTarget(a) => {
                Ok((self.convert_array_pattern_target(a)?, var))
            }
            ast::ForStatementLeft::ObjectAssignmentTarget(o) => {
                Ok((self.convert_object_pattern_target(o)?, var))
            }
            ast::ForStatementLeft::StaticMemberExpression(m) => Ok((
                Pattern::Member(Box::new(self.convert_static_member(m)?)),
                var,
            )),
            ast::ForStatementLeft::ComputedMemberExpression(m) => Ok((
                Pattern::Member(Box::new(self.convert_computed_member(m)?)),
                var,
            )),
            ast::ForStatementLeft::PrivateFieldExpression(p) => Ok((
                Pattern::Member(Box::new(self.convert_private_member(&p.object, &p.field)?)),
                var,
            )),
            _ => Err(Error::Parse("unsupported for-in/of left side".to_string())),
        }
    }
}
