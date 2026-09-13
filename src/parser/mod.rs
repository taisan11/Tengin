//! Oxc-based front-end: parses source with `oxc_parser` and lowers the
//! resulting `oxc_ast` into Tengin's interpreter AST ([`crate::ast`]).

mod expr;
mod patterns;
mod stmts;

use std::rc::Rc;
use std::string::{String, ToString};
use std::vec::Vec;

use oxc_allocator::Allocator;
use oxc_ast::ast;
use oxc_parser::Parser;
use oxc_span::SourceType;

use crate::ast::*;
use crate::error::Error;
pub fn parse(src: &str) -> Result<Program, Error> {
    let allocator = Allocator::new();
    // Accept JavaScript + TypeScript + JSX. Modules (`import`/`export`) are not
    // supported by the interpreter, so we keep a script-like context.
    let source_type = SourceType::cjs()
        .with_typescript(true)
        .with_jsx(true);
    let ret = Parser::new(&allocator, src, source_type).parse();
    if ret.panicked {
        return Err(Error::Parse("parser panicked".to_string()));
    }
    if !ret.diagnostics.is_empty() {
        let msg = ret
            .diagnostics
            .iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        return Err(Error::Parse(msg));
    }
    let mut c = Converter { strict: false };
    c.convert_program(&ret.program)
}

fn ident_name(id: &ast::IdentifierReference) -> Rc<str> {
    Rc::from(id.name.as_str())
}

fn quasi_str(q: &ast::TemplateElement) -> String {
    q.value
        .cooked
        .as_ref()
        .map(|c| c.as_str())
        .unwrap_or_else(|| q.value.raw.as_str())
        .to_string()
}

struct Converter {
    /// Whether the program (script) is strict mode code, detected from the
    /// directive prologue before conversion so statement-level early errors
    /// (Annex B restrictions) can be applied.
    strict: bool,
}

impl Converter {
    fn convert_program(&mut self, p: &ast::Program) -> Result<Program, Error> {
        // Oxc keeps the directive prologue in `Program.directives` (excluding it
        // from `body`), so scan it directly for `"use strict"`.
        let strict = p.directives.iter().any(|d| {
            d.expression.value.as_str() == "use strict"
        });
        self.strict = strict;
        Ok(Program {
            stmts: self.convert_stmts(&p.body)?,
            strict,
        })
    }

    /// `IsLabelledFunction(stmt)`: whether `stmt` is a labelled statement
    /// (possibly nested labels) whose innermost item is a function declaration.
    fn is_labelled_function(s: &ast::Statement) -> bool {
        match s {
            ast::Statement::LabeledStatement(l) => match &l.body {
                ast::Statement::FunctionDeclaration(_) => true,
                inner => Self::is_labelled_function(inner),
            },
            _ => false,
        }
    }
}
