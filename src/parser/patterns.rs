//! Lowering of Oxc destructuring and assignment patterns into Tengin patterns.

use std::rc::Rc;
use std::vec::Vec;

use oxc_ast::ast;

use crate::ast::*;
use crate::error::Error;

use super::Converter;

impl Converter {
    pub(crate) fn convert_array_pattern_target(
        &mut self,
        a: &ast::ArrayAssignmentTarget,
    ) -> Result<Pattern, Error> {
        let mut elems = Vec::new();
        for el in &a.elements {
            match el {
                None => elems.push(None),
                Some(e) => elems.push(Some(self.convert_assignment_target_element(e)?)),
            }
        }
        let rest = match &a.rest {
            Some(r) => Some(Box::new(self.convert_rest_pattern(&r.target)?)),
            None => None,
        };
        Ok(Pattern::Array { elems, rest })
    }

    pub(crate) fn convert_assignment_target_element(
        &mut self,
        e: &ast::AssignmentTargetMaybeDefault,
    ) -> Result<Pattern, Error> {
        match e {
            ast::AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(d) => {
                self.convert_rest_pattern(&d.binding)
            }
            ast::AssignmentTargetMaybeDefault::AssignmentTargetIdentifier(id) => {
                Ok(Pattern::Ident(Rc::from(id.name.as_str())))
            }
            ast::AssignmentTargetMaybeDefault::ArrayAssignmentTarget(a) => {
                self.convert_array_pattern_target(a)
            }
            ast::AssignmentTargetMaybeDefault::ObjectAssignmentTarget(o) => {
                self.convert_object_pattern_target(o)
            }
            ast::AssignmentTargetMaybeDefault::StaticMemberExpression(m) => {
                Ok(Pattern::Member(Box::new(self.convert_static_member(m)?)))
            }
            ast::AssignmentTargetMaybeDefault::ComputedMemberExpression(m) => {
                Ok(Pattern::Member(Box::new(self.convert_computed_member(m)?)))
            }
            ast::AssignmentTargetMaybeDefault::PrivateFieldExpression(p) => {
                Ok(Pattern::Member(Box::new(
                    self.convert_private_member(&p.object, &p.field)?,
                )))
            }
            ast::AssignmentTargetMaybeDefault::TSAsExpression(e) => {
                self.convert_expr_as_pattern(&e.expression)
            }
            ast::AssignmentTargetMaybeDefault::TSSatisfiesExpression(e) => {
                self.convert_expr_as_pattern(&e.expression)
            }
            ast::AssignmentTargetMaybeDefault::TSNonNullExpression(e) => {
                self.convert_expr_as_pattern(&e.expression)
            }
            ast::AssignmentTargetMaybeDefault::TSTypeAssertion(e) => {
                self.convert_expr_as_pattern(&e.expression)
            }
        }
    }

    /// Convert a plain `Expression` (member/identifier, possibly TS-wrapped) in
    /// destructuring-target position into a `Pattern`.
    pub(crate) fn convert_expr_as_pattern(&mut self, e: &ast::Expression) -> Result<Pattern, Error> {
        match e {
            ast::Expression::Identifier(id) => Ok(Pattern::Ident(Rc::from(id.name.as_str()))),
            ast::Expression::ComputedMemberExpression(m) => {
                Ok(Pattern::Member(Box::new(self.convert_computed_member(m)?)))
            }
            ast::Expression::StaticMemberExpression(m) => {
                Ok(Pattern::Member(Box::new(self.convert_static_member(m)?)))
            }
            ast::Expression::TSAsExpression(e) => self.convert_expr_as_pattern(&e.expression),
            ast::Expression::TSSatisfiesExpression(e) => self.convert_expr_as_pattern(&e.expression),
            ast::Expression::TSNonNullExpression(e) => self.convert_expr_as_pattern(&e.expression),
            ast::Expression::TSTypeAssertion(e) => self.convert_expr_as_pattern(&e.expression),
            _ => Err(Error::Parse("unsupported assignment target (elem)".to_string())),
        }
    }

    /// Convert an `AssignmentTarget` (used for rest/default bindings) into a
    /// destructuring `Pattern`.
    pub(crate) fn convert_rest_pattern(&mut self, t: &ast::AssignmentTarget) -> Result<Pattern, Error> {
        match t {
            ast::AssignmentTarget::AssignmentTargetIdentifier(id) => {
                Ok(Pattern::Ident(Rc::from(id.name.as_str())))
            }
            ast::AssignmentTarget::ArrayAssignmentTarget(a) => {
                self.convert_array_pattern_target(a)
            }
            ast::AssignmentTarget::ObjectAssignmentTarget(o) => {
                self.convert_object_pattern_target(o)
            }
            ast::AssignmentTarget::StaticMemberExpression(m) => {
                Ok(Pattern::Member(Box::new(self.convert_static_member(m)?)))
            }
            ast::AssignmentTarget::ComputedMemberExpression(m) => {
                Ok(Pattern::Member(Box::new(self.convert_computed_member(m)?)))
            }
            ast::AssignmentTarget::PrivateFieldExpression(p) => Ok(Pattern::Member(Box::new(
                self.convert_private_member(&p.object, &p.field)?,
            ))),
            _ => Err(Error::Parse("unsupported assignment target (elem)".to_string())),
        }
    }

    pub(crate) fn convert_object_pattern_target(
        &mut self,
        o: &ast::ObjectAssignmentTarget,
    ) -> Result<Pattern, Error> {
        let mut props = Vec::new();
        for p in &o.properties {
            match p {
                ast::AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(id) => {
                    props.push((
                        PropKey::Ident(Rc::from(id.binding.name.as_str())),
                        Pattern::Ident(Rc::from(id.binding.name.as_str())),
                    ));
                }
                ast::AssignmentTargetProperty::AssignmentTargetPropertyProperty(pr) => {
                    let key = self.convert_property_key(&pr.name)?;
                    let pat = self.convert_assignment_target_element(&pr.binding)?;
                    props.push((key, pat));
                }
            }
        }
        let rest = match &o.rest {
            Some(r) => Some(Box::new(self.convert_rest_pattern(&r.target)?)),
            None => None,
        };
        Ok(Pattern::Object { props, rest })
    }

    // ---- expressions ----
}
