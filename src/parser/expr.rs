//! Lowering of Oxc expression nodes into Tengin expressions.

use std::rc::Rc;
use std::string::ToString;
use std::vec::Vec;

use oxc_ast::ast;

use crate::ast::*;
use crate::error::Error;

use super::{ident_name, quasi_str, Converter};

impl Converter {
    pub(crate) fn convert_expr(&mut self, e: &ast::Expression) -> Result<Expr, Error> {
        match e {
            ast::Expression::BooleanLiteral(b) => Ok(Expr::Lit(Lit::Bool(b.value))),
            ast::Expression::NullLiteral(_) => Ok(Expr::Lit(Lit::Null)),
            ast::Expression::NumericLiteral(n) => Ok(Expr::Lit(Lit::Number(n.value))),
            ast::Expression::BigIntLiteral(b) => {
                // oxc's `value` is the canonical base-10 digit string.
                let s = b.value.as_str();
                Ok(Expr::Lit(Lit::BigInt(Rc::from(s))))
            }
            ast::Expression::RegExpLiteral(r) => {
                Ok(Expr::Lit(Lit::Regex {
                    pattern: Rc::from(r.regex.pattern.text.as_str()),
                    flags: Rc::from(r.regex.flags.to_string().as_str()),
                }))
            }
            ast::Expression::StringLiteral(s) => {
                Ok(Expr::Lit(Lit::String(Rc::from(s.value.as_str()))))
            }
            ast::Expression::TemplateLiteral(t) => self.convert_template(t),
            ast::Expression::Identifier(id) => Ok(Expr::Ident(ident_name(id))),
            ast::Expression::Super(_) => Ok(Expr::Super),
            ast::Expression::ThisExpression(_) => Ok(Expr::This),
            ast::Expression::ArrayExpression(a) => {
                let mut elems = Vec::new();
                for el in &a.elements {
                    match el {
                        ast::ArrayExpressionElement::SpreadElement(sp) => {
                            elems.push(ArrayElem::Spread(self.convert_expr(&sp.argument)?));
                        }
                        ast::ArrayExpressionElement::Elision(_) => {
                            elems.push(ArrayElem::Elision);
                        }
                        other => match other.as_expression() {
                            Some(e) => elems.push(ArrayElem::Expr(self.convert_expr(e)?)),
                            None => {
                                return Err(Error::Parse("unsupported array element".to_string()))
                            }
                        },
                    }
                }
                Ok(Expr::Array(elems))
            }
            ast::Expression::ObjectExpression(o) => {
                let mut props = Vec::new();
                for p in &o.properties {
                    match p {
                        ast::ObjectPropertyKind::ObjectProperty(op) => {
                            props.push(self.convert_object_prop(op)?);
                        }
                        ast::ObjectPropertyKind::SpreadProperty(sp) => {
                            props.push(Prop::Spread(self.convert_expr(&sp.argument)?));
                        }
                    }
                }
                Ok(Expr::Object(props))
            }
            ast::Expression::FunctionExpression(f) => {
                let name = f
                    .id
                    .as_ref()
                    .map(|id| Rc::from(id.name.as_str()));
                let (params, body) = self.convert_function(f)?;
                Ok(Expr::Function { name, params, body })
            }
            ast::Expression::ClassExpression(c) => {
                let class = self.convert_class(c, None)?;
                Ok(Expr::Class(Box::new(class)))
            }
            ast::Expression::ArrowFunctionExpression(a) => {
                let mut params = Vec::new();
                for item in &a.params.items {
                    params.push(self.convert_binding_pattern(&item.pattern)?);
                }
                let body = match &a.body {
                    ast::ArrowFunctionBody::FunctionBody(b) => self.convert_stmts(&b.statements)?,
                    other => match other.as_expression() {
                        Some(e) => vec![Stmt::Return(Some(self.convert_expr(e)?))],
                        None => Vec::new(),
                    },
                };
                let body = if a.r#async {
                    vec![Stmt::AsyncBody(body)]
                } else {
                    body
                };
                Ok(Expr::Arrow { params, body })
            }
            ast::Expression::AssignmentExpression(a) => {
                let target = self.convert_target_expr(&a.left)?;
                let right = self.convert_expr(&a.right)?;
                let value = if a.operator.is_assign() {
                    right
                } else {
                    self.convert_compound_assignment(a.operator, target.clone(), right)?
                };
                Ok(Expr::Assignment {
                    target,
                    value: Box::new(value),
                })
            }
            ast::Expression::AwaitExpression(a) => {
                Ok(Expr::Await(Box::new(self.convert_expr(&a.argument)?)))
            }
            ast::Expression::YieldExpression(y) => {
                let expr = match &y.argument {
                    Some(e) => Some(Box::new(self.convert_expr(e)?)),
                    None => None,
                };
                Ok(Expr::Yield {
                    expr,
                    delegated: y.delegate,
                })
            }
            ast::Expression::BinaryExpression(b) => {
                if b.operator == ast::BinaryOperator::Instanceof {
                    let left = self.convert_expr(&b.left)?;
                    let right = self.convert_expr(&b.right)?;
                    return Ok(Expr::InstanceOf {
                        left: Box::new(left),
                        right: Box::new(right),
                    });
                }
                let op = self.convert_binary_op(b.operator)?;
                let left = self.convert_expr(&b.left)?;
                let right = self.convert_expr(&b.right)?;
                Ok(Expr::Binary {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                })
            }
            ast::Expression::LogicalExpression(l) => {
                let op = match l.operator {
                    ast::LogicalOperator::And => LogicalOp::And,
                    ast::LogicalOperator::Or => LogicalOp::Or,
                    ast::LogicalOperator::Coalesce => LogicalOp::Coalesce,
                };
                let left = self.convert_expr(&l.left)?;
                let right = self.convert_expr(&l.right)?;
                Ok(Expr::Logical {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                })
            }
            ast::Expression::UnaryExpression(u) => {
                let op = self.convert_unary_op(u.operator)?;
                let operand = self.convert_expr(&u.argument)?;
                Ok(Expr::Unary {
                    op,
                    operand: Box::new(operand),
                })
            }
            ast::Expression::UpdateExpression(u) => {
                let op = match (u.operator, u.prefix) {
                    (ast::UpdateOperator::Increment, true) => UnaryOp::PreInc,
                    (ast::UpdateOperator::Increment, false) => UnaryOp::PostInc,
                    (ast::UpdateOperator::Decrement, true) => UnaryOp::PreDec,
                    (ast::UpdateOperator::Decrement, false) => UnaryOp::PostDec,
                };
                let operand = self.convert_simple_target(&u.argument)?;
                Ok(Expr::Unary {
                    op,
                    operand: Box::new(operand),
                })
            }
            ast::Expression::ConditionalExpression(c) => {
                let cond = self.convert_expr(&c.test)?;
                let then = self.convert_expr(&c.consequent)?;
                let else_ = self.convert_expr(&c.alternate)?;
                Ok(Expr::Ternary {
                    cond: Box::new(cond),
                    then: Box::new(then),
                    else_: Box::new(else_),
                })
            }
            ast::Expression::CallExpression(c) => {
                let callee = self.convert_expr(&c.callee)?;
                let mut args = Vec::new();
                for a in &c.arguments {
                    match a {
                        ast::Argument::SpreadElement(sp) => {
                            args.push(Arg::Spread(self.convert_expr(&sp.argument)?));
                        }
                        other => match other.as_expression() {
                            Some(e) => args.push(Arg::Expr(self.convert_expr(e)?)),
                            None => return Err(Error::Parse("unsupported call argument".to_string())),
                        },
                    }
                }
                let optional = c.optional;
                Ok(Expr::Call {
                    callee: Box::new(callee),
                    args,
                    optional,
                })
            }
            ast::Expression::NewExpression(n) => {
                let callee = self.convert_expr(&n.callee)?;
                let mut args = Vec::new();
                for a in &n.arguments {
                    match a {
                        ast::Argument::SpreadElement(sp) => {
                            args.push(Arg::Spread(self.convert_expr(&sp.argument)?));
                        }
                        other => match other.as_expression() {
                            Some(e) => args.push(Arg::Expr(self.convert_expr(e)?)),
                            None => return Err(Error::Parse("unsupported new argument".to_string())),
                        },
                    }
                }
                Ok(Expr::New {
                    callee: Box::new(callee),
                    args,
                })
            }
            ast::Expression::ChainExpression(ch) => self.convert_chain(&ch.expression),
            ast::Expression::ComputedMemberExpression(m) => self.convert_computed_member(m),
            ast::Expression::StaticMemberExpression(m) => self.convert_static_member(m),
            ast::Expression::PrivateFieldExpression(p) => {
                self.convert_private_member(&p.object, &p.field)
            }
            ast::Expression::ParenthesizedExpression(p) => self.convert_expr(&p.expression),
            ast::Expression::SequenceExpression(s) => {
                let mut exprs: Vec<Expr> = Vec::with_capacity(s.expressions.len());
                for e in &s.expressions {
                    exprs.push(self.convert_expr(e)?);
                }
                Ok(Expr::Sequence(exprs))
            }
            ast::Expression::TaggedTemplateExpression(t) => {
                let tag = self.convert_expr(&t.tag)?;
                self.convert_tagged(tag, &t.quasi)
            }
            ast::Expression::ImportExpression(_) => Ok(Expr::Lit(Lit::Undefined)),
            ast::Expression::PrivateInExpression(p) => {
                let obj = self.convert_expr(&p.right)?;
                let key = Rc::from(format!("#{}", p.left.name).as_str());
                Ok(Expr::Binary {
                    op: BinaryOp::In,
                    left: Box::new(Expr::Lit(Lit::String(key))),
                    right: Box::new(obj),
                })
            }
            ast::Expression::ImportMeta(_) | ast::Expression::NewTarget(_) => {
                Ok(Expr::Lit(Lit::Undefined))
            }
            ast::Expression::JSXElement(_) | ast::Expression::JSXFragment(_) => {
                Err(Error::Parse("JSX runtime is not supported".to_string()))
            }
            // TypeScript type-expression wrappers: ignore the type, keep the value.
            ast::Expression::TSAsExpression(e) => self.convert_expr(&e.expression),
            ast::Expression::TSSatisfiesExpression(e) => self.convert_expr(&e.expression),
            ast::Expression::TSTypeAssertion(e) => self.convert_expr(&e.expression),
            ast::Expression::TSNonNullExpression(e) => self.convert_expr(&e.expression),
            ast::Expression::TSInstantiationExpression(e) => self.convert_expr(&e.expression),
            ast::Expression::V8IntrinsicExpression(_) => {
                Err(Error::Parse("V8 intrinsics are not supported".to_string()))
            }
        }
    }

    pub(crate) fn convert_tagged(&mut self, tag: Expr, quasi: &ast::TemplateLiteral) -> Result<Expr, Error> {
        let mut cooked = Vec::new();
        let mut raws = Vec::new();
        let mut subs = Vec::new();
        let first = quasi.quasis.first().map(quasi_str).unwrap_or_default();
        cooked.push(Expr::Lit(Lit::String(Rc::from(first.as_str()))));
        raws.push(Expr::Lit(Lit::String(Rc::from(
            quasi
                .quasis
                .first()
                .map(|q| q.value.raw.as_str())
                .unwrap_or(""),
        ))));
        for (i, e) in quasi.expressions.iter().enumerate() {
            subs.push(self.convert_expr(e)?);
            let q = quasi.quasis.get(i + 1).map(quasi_str).unwrap_or_default();
            cooked.push(Expr::Lit(Lit::String(Rc::from(q.as_str()))));
            let raw = quasi
                .quasis
                .get(i + 1)
                .map(|q| q.value.raw.as_str())
                .unwrap_or("");
            raws.push(Expr::Lit(Lit::String(Rc::from(raw))));
        }
        Ok(Expr::Tagged {
            tag: Box::new(tag),
            cooked,
            raws,
            subs,
        })
    }

    pub(crate) fn convert_template(&mut self, t: &ast::TemplateLiteral) -> Result<Expr, Error> {
        let mut pieces: Vec<Expr> = Vec::new();
        let first = t.quasis.first().map(quasi_str).unwrap_or_default();
        pieces.push(Expr::Lit(Lit::String(Rc::from(first.as_str()))));
        for (i, e) in t.expressions.iter().enumerate() {
            // Substitutions convert via ToString(ToPrimitive(_, string)),
            // which differs from the `+` operator's algorithm.
            pieces.push(Expr::ToString(Box::new(self.convert_expr(e)?)));
            let q = t.quasis.get(i + 1).map(quasi_str).unwrap_or_default();
            pieces.push(Expr::Lit(Lit::String(Rc::from(q.as_str()))));
        }
        let mut result = pieces.remove(0);
        for p in pieces {
            result = Expr::Binary {
                op: BinaryOp::Add,
                left: Box::new(result),
                right: Box::new(p),
            };
        }
        Ok(result)
    }

    pub(crate) fn convert_chain(&mut self, e: &ast::ChainElement) -> Result<Expr, Error> {
        match e {
            ast::ChainElement::CallExpression(c) => {
                let callee = self.convert_expr(&c.callee)?;
                let mut args = Vec::new();
                for a in &c.arguments {
                    match a {
                        ast::Argument::SpreadElement(sp) => {
                            args.push(Arg::Spread(self.convert_expr(&sp.argument)?));
                        }
                        other => match other.as_expression() {
                            Some(e) => args.push(Arg::Expr(self.convert_expr(e)?)),
                            None => return Err(Error::Parse("unsupported call argument".to_string())),
                        },
                    }
                }
                Ok(Expr::Call {
                    callee: Box::new(callee),
                    args,
                    optional: c.optional,
                })
            }
            ast::ChainElement::TSNonNullExpression(e) => self.convert_expr(&e.expression),
            ast::ChainElement::ComputedMemberExpression(m) => self.convert_computed_member(m),
            ast::ChainElement::StaticMemberExpression(m) => self.convert_static_member(m),
            ast::ChainElement::PrivateFieldExpression(p) => {
                self.convert_private_member(&p.object, &p.field)
            }
        }
    }

    pub(crate) fn convert_static_member(&mut self, m: &ast::StaticMemberExpression) -> Result<Expr, Error> {
        let obj = self.convert_expr(&m.object)?;
        let prop = Rc::from(m.property.name.as_str());
        Ok(Expr::Member {
            obj: Box::new(obj),
            prop,
            computed: None,
            optional: m.optional,
        })
    }

    pub(crate) fn convert_private_member(
        &mut self,
        obj: &ast::Expression,
        field: &ast::PrivateIdentifier,
    ) -> Result<Expr, Error> {
        let obj = self.convert_expr(obj)?;
        let prop = Rc::from(format!("#{}", field.name).as_str());
        Ok(Expr::Member {
            obj: Box::new(obj),
            prop,
            computed: None,
            optional: false,
        })
    }

    pub(crate) fn convert_computed_member(
        &mut self,
        m: &ast::ComputedMemberExpression,
    ) -> Result<Expr, Error> {
        let obj = self.convert_expr(&m.object)?;
        let expr = self.convert_expr(&m.expression)?;
        Ok(Expr::Member {
            obj: Box::new(obj),
            prop: Rc::from(""),
            computed: Some(Box::new(expr)),
            optional: m.optional,
        })
    }

    pub(crate) fn convert_simple_target(&mut self, s: &ast::SimpleAssignmentTarget) -> Result<Expr, Error> {
        match s {
            ast::SimpleAssignmentTarget::AssignmentTargetIdentifier(id) => {
                Ok(Expr::Ident(Rc::from(id.name.as_str())))
            }
            ast::SimpleAssignmentTarget::ComputedMemberExpression(m) => self.convert_computed_member(m),
            ast::SimpleAssignmentTarget::StaticMemberExpression(m) => self.convert_static_member(m),
            ast::SimpleAssignmentTarget::PrivateFieldExpression(p) => {
                self.convert_private_member(&p.object, &p.field)
            }
            ast::SimpleAssignmentTarget::TSAsExpression(e) => self.convert_expr(&e.expression),
            ast::SimpleAssignmentTarget::TSSatisfiesExpression(e) => self.convert_expr(&e.expression),
            ast::SimpleAssignmentTarget::TSNonNullExpression(e) => self.convert_expr(&e.expression),
            ast::SimpleAssignmentTarget::TSTypeAssertion(e) => self.convert_expr(&e.expression),
        }
    }

    pub(crate) fn convert_target_expr(&mut self, t: &ast::AssignmentTarget) -> Result<AssignTarget, Error> {
        match t {
            ast::AssignmentTarget::AssignmentTargetIdentifier(id) => {
                Ok(AssignTarget::Expr(Box::new(Expr::Ident(Rc::from(
                    id.name.as_str(),
                )))))
            }
            ast::AssignmentTarget::ComputedMemberExpression(m) => {
                Ok(AssignTarget::Expr(Box::new(self.convert_computed_member(m)?)))
            }
            ast::AssignmentTarget::StaticMemberExpression(m) => {
                Ok(AssignTarget::Expr(Box::new(self.convert_static_member(m)?)))
            }
            ast::AssignmentTarget::PrivateFieldExpression(p) => {
                Ok(AssignTarget::Expr(Box::new(
                    self.convert_private_member(&p.object, &p.field)?,
                )))
            }
            ast::AssignmentTarget::TSAsExpression(e) => self.convert_expr_as_target(&e.expression),
            ast::AssignmentTarget::TSSatisfiesExpression(e) => self.convert_expr_as_target(&e.expression),
            ast::AssignmentTarget::TSNonNullExpression(e) => self.convert_expr_as_target(&e.expression),
            ast::AssignmentTarget::TSTypeAssertion(e) => self.convert_expr_as_target(&e.expression),
            ast::AssignmentTarget::ArrayAssignmentTarget(a) => {
                Ok(AssignTarget::Pattern(self.convert_array_pattern_target(a)?))
            }
            ast::AssignmentTarget::ObjectAssignmentTarget(o) => {
                Ok(AssignTarget::Pattern(self.convert_object_pattern_target(o)?))
            }
        }
    }

    /// Convert a plain `Expression` (identifier/member, possibly TS-wrapped) that
    /// appears in assignment-target position into an `AssignTarget`.
    pub(crate) fn convert_expr_as_target(&mut self, e: &ast::Expression) -> Result<AssignTarget, Error> {
        match e {
            ast::Expression::Identifier(id) => Ok(AssignTarget::Expr(Box::new(Expr::Ident(Rc::from(
                id.name.as_str(),
            ))))),
            ast::Expression::ComputedMemberExpression(m) => {
                Ok(AssignTarget::Expr(Box::new(self.convert_computed_member(m)?)))
            }
            ast::Expression::StaticMemberExpression(m) => {
                Ok(AssignTarget::Expr(Box::new(self.convert_static_member(m)?)))
            }
            ast::Expression::TSAsExpression(e) => self.convert_expr_as_target(&e.expression),
            ast::Expression::TSSatisfiesExpression(e) => self.convert_expr_as_target(&e.expression),
            ast::Expression::TSNonNullExpression(e) => self.convert_expr_as_target(&e.expression),
            ast::Expression::TSTypeAssertion(e) => self.convert_expr_as_target(&e.expression),
            _ => Err(Error::Parse("unsupported assignment target (elem)".to_string())),
        }
    }

    pub(crate) fn convert_binding_pattern(&mut self, p: &ast::BindingPattern) -> Result<Pattern, Error> {
        match p {
            ast::BindingPattern::BindingIdentifier(id) => {
                Ok(Pattern::Ident(Rc::from(id.name.as_str())))
            }
            ast::BindingPattern::ObjectPattern(o) => {
                let mut props = Vec::new();
                for bp in &o.properties {
                    let key = self.convert_property_key(&bp.key)?;
                    let pat = self.convert_binding_pattern(&bp.value)?;
                    props.push((key, pat));
                }
                let rest = match &o.rest {
                    Some(r) => Some(Box::new(self.convert_binding_pattern(&r.argument)?)),
                    None => None,
                };
                Ok(Pattern::Object { props, rest })
            }
            ast::BindingPattern::ArrayPattern(a) => {
                let mut elems = Vec::new();
                for el in &a.elements {
                    match el {
                        None => elems.push(None),
                        Some(e) => elems.push(Some(self.convert_binding_pattern(e)?)),
                    }
                }
                let rest = match &a.rest {
                    Some(r) => Some(Box::new(self.convert_binding_pattern(&r.argument)?)),
                    None => None,
                };
                Ok(Pattern::Array { elems, rest })
            }
            ast::BindingPattern::AssignmentPattern(ap) => {
                Ok(Pattern::Default {
                    inner: Box::new(self.convert_binding_pattern(&ap.left)?),
                    default: Box::new(self.convert_expr(&ap.right)?),
                })
            }
        }
    }

    pub(crate) fn convert_function(&mut self, f: &ast::Function) -> Result<(Vec<Pattern>, Vec<Stmt>), Error> {
        let mut params = Vec::new();
        for item in &f.params.items {
            params.push(self.convert_binding_pattern(&item.pattern)?);
        }
        let body = match &f.body {
            Some(b) => self.convert_stmts(&b.statements)?,
            None => Vec::new(),
        };
        // Generator/async functions are marked with wrapper statements; the
        // function factory unwraps them and installs the behaviour.
        // `async function*` (async generators) are parsed but explicitly
        // unimplemented.
        if f.generator && f.r#async {
            return Err(Error::Unimplemented(
                "async generators are not implemented".to_string(),
            ));
        }
        let body = if f.generator {
            vec![Stmt::GeneratorBody(body)]
        } else if f.r#async {
            vec![Stmt::AsyncBody(body)]
        } else {
            body
        };
        Ok((params, body))
    }

    pub(crate) fn convert_function_from_expr(
        &mut self,
        e: &ast::Expression,
    ) -> Result<(Vec<Pattern>, Vec<Stmt>), Error> {
        match e {
            ast::Expression::FunctionExpression(f) => self.convert_function(f),
            _ => Err(Error::Parse("expected function in method shorthand".to_string())),
        }
    }

    pub(crate) fn convert_class(&mut self, c: &ast::Class, name: Option<Rc<str>>) -> Result<Class, Error> {
        let name = name.or_else(|| c.id.as_ref().map(|id| Rc::from(id.name.as_str())));
        let extends = match &c.heritage {
            Some(h) => Some(self.convert_expr(&h.expression)?),
            None => None,
        };
        let mut elements = Vec::new();
        for el in &c.body.body {
            match el {
                ast::ClassElement::MethodDefinition(m) => {
                    let key = self.convert_property_key(&m.key)?;
                    let (params, body) = self.convert_function(&m.value)?;
                    match m.kind {
                        ast::MethodDefinitionKind::Constructor => {
                            elements.push(ClassElem::Constructor { params, body });
                        }
                        ast::MethodDefinitionKind::Method => {
                            elements.push(ClassElem::Method {
                                is_static: m.r#static,
                                get: false,
                                key,
                                params,
                                body,
                            });
                        }
                        ast::MethodDefinitionKind::Get => {
                            elements.push(ClassElem::Method {
                                is_static: m.r#static,
                                get: true,
                                key,
                                params,
                                body,
                            });
                        }
                        ast::MethodDefinitionKind::Set => {
                            elements.push(ClassElem::Method {
                                is_static: m.r#static,
                                get: false,
                                key,
                                params,
                                body,
                            });
                        }
                    }
                }
                ast::ClassElement::PropertyDefinition(p) => {
                    let key = self.convert_property_key(&p.key)?;
                    let init = match &p.value {
                        Some(v) => Some(self.convert_expr(v)?),
                        None => None,
                    };
                    elements.push(ClassElem::Field {
                        is_static: p.r#static,
                        key,
                        init,
                    });
                }
                ast::ClassElement::AccessorProperty(a) => {
                    let key = self.convert_property_key(&a.key)?;
                    elements.push(ClassElem::AccessorProp {
                        is_static: a.r#static,
                        key,
                    });
                }
                ast::ClassElement::StaticBlock(_) | ast::ClassElement::TSIndexSignature(_) => {}
            }
        }
        Ok(Class { name, extends, elements })
    }

    pub(crate) fn convert_property_key(&mut self, key: &ast::PropertyKey) -> Result<PropKey, Error> {
        match key {
            ast::PropertyKey::StaticIdentifier(i) => Ok(PropKey::Ident(Rc::from(i.name.as_str()))),
            ast::PropertyKey::PrivateIdentifier(p) => {
                Ok(PropKey::Ident(Rc::from(format!("#{}", p.name).as_str())))
            }
            other => match other.as_expression() {
                Some(e) => match e {
                    ast::Expression::StringLiteral(s) => {
                        Ok(PropKey::Str(Rc::from(s.value.as_str())))
                    }
                    ast::Expression::NumericLiteral(n) => {
                        Ok(PropKey::Str(Rc::from(n.value.to_string().as_str())))
                    }
                    _ => Ok(PropKey::Computed(self.convert_expr(e)?)),
                },
                None => Err(Error::Parse("unsupported property key".to_string())),
            },
        }
    }

    pub(crate) fn convert_object_prop(&mut self, p: &ast::ObjectProperty) -> Result<Prop, Error> {
        let key = self.convert_property_key(&p.key)?;
        if p.method {
            let (params, body) = self.convert_function_from_expr(&p.value)?;
            return Ok(Prop::Method { key, params, body });
        }
        match p.kind {
            ast::PropertyKind::Init => {
                let value = self.convert_expr(&p.value)?;
                Ok(Prop::Init { key, value })
            }
            ast::PropertyKind::Get => {
                let (params, body) = self.convert_function_from_expr(&p.value)?;
                Ok(Prop::Accessor {
                    get: true,
                    key,
                    params,
                    body,
                })
            }
            ast::PropertyKind::Set => {
                let (params, body) = self.convert_function_from_expr(&p.value)?;
                Ok(Prop::Accessor {
                    get: false,
                    key,
                    params,
                    body,
                })
            }
        }
    }

    pub(crate) fn convert_compound_assignment(
        &mut self,
        op: ast::AssignmentOperator,
        target: AssignTarget,
        right: Expr,
    ) -> Result<Expr, Error> {
        use ast::AssignmentOperator::*;
        let bin_op = match op {
            Addition => BinaryOp::Add,
            Subtraction => BinaryOp::Sub,
            Multiplication => BinaryOp::Mul,
            Division => BinaryOp::Div,
            Remainder => BinaryOp::Rem,
            Exponential => BinaryOp::Exp,
            BitwiseAnd => BinaryOp::BitAnd,
            BitwiseOR => BinaryOp::BitOr,
            BitwiseXOR => BinaryOp::BitXor,
            ShiftLeft => BinaryOp::Shl,
            ShiftRight => BinaryOp::Shr,
            ShiftRightZeroFill => BinaryOp::Ushr,
            LogicalOr => {
                return Ok(Expr::Logical {
                    op: LogicalOp::Or,
                    left: Box::new(self.target_as_expr(target)?),
                    right: Box::new(right),
                })
            }
            LogicalAnd => {
                return Ok(Expr::Logical {
                    op: LogicalOp::And,
                    left: Box::new(self.target_as_expr(target)?),
                    right: Box::new(right),
                })
            }
            LogicalNullish => {
                return Ok(Expr::Logical {
                    op: LogicalOp::Coalesce,
                    left: Box::new(self.target_as_expr(target)?),
                    right: Box::new(right),
                })
            }
            _ => return Err(Error::Parse("compound assignment operator not supported".to_string())),
        };
        Ok(Expr::Binary {
            op: bin_op,
            left: Box::new(self.target_as_expr(target)?),
            right: Box::new(right),
        })
    }

    pub(crate) fn target_as_expr(&mut self, target: AssignTarget) -> Result<Expr, Error> {
        match target {
            AssignTarget::Expr(e) => Ok(*e),
            AssignTarget::Pattern(_) => Err(Error::Parse(
                "compound assignment to a pattern is not supported".to_string(),
            )),
        }
    }

    pub(crate) fn convert_binary_op(&mut self, op: ast::BinaryOperator) -> Result<BinaryOp, Error> {
        use ast::BinaryOperator::*;
        Ok(match op {
            Equality => BinaryOp::Eq,
            Inequality => BinaryOp::Ne,
            StrictEquality => BinaryOp::Seq,
            StrictInequality => BinaryOp::Sne,
            LessThan => BinaryOp::Lt,
            GreaterThan => BinaryOp::Gt,
            LessEqualThan => BinaryOp::Le,
            GreaterEqualThan => BinaryOp::Ge,
            Addition => BinaryOp::Add,
            Subtraction => BinaryOp::Sub,
            Multiplication => BinaryOp::Mul,
            Division => BinaryOp::Div,
            Remainder => BinaryOp::Rem,
            In => BinaryOp::In,
            BitwiseAnd => BinaryOp::BitAnd,
            BitwiseOR => BinaryOp::BitOr,
            BitwiseXOR => BinaryOp::BitXor,
            ShiftLeft => BinaryOp::Shl,
            ShiftRight => BinaryOp::Shr,
            ShiftRightZeroFill => BinaryOp::Ushr,
            Exponential => BinaryOp::Exp,
            _ => return Err(Error::Parse("binary operator not supported".to_string())),
        })
    }

    pub(crate) fn convert_unary_op(&mut self, op: ast::UnaryOperator) -> Result<UnaryOp, Error> {
        use ast::UnaryOperator::*;
        Ok(match op {
            UnaryNegation => UnaryOp::Neg,
            UnaryPlus => UnaryOp::Plus,
            LogicalNot => UnaryOp::Not,
            Typeof => UnaryOp::Typeof,
            Void => UnaryOp::Void,
            Delete => UnaryOp::Delete,
            BitwiseNot => UnaryOp::BitNot,
        })
    }
}
