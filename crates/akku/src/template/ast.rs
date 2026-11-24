//! The compiled form of a template.
//!
//! Nesting is bounded by the compile-time nesting limit, both for nodes and
//! for expressions (each [`Expr`] records its own depth), so the derived
//! `Drop` and the recursive evaluator stay within a fixed stack budget.

use super::error::{ErrorKind, Fail, RawError};
use super::value::Value;

/// A template node.
#[derive(Debug)]
pub(crate) enum Node {
    /// Literal text and the offset where it starts.
    Text(String, usize),
    /// `{{ expr }}`.
    Output(Expr),
    /// `{% if %}` … `{% endif %}`.
    If {
        branches: Vec<(Expr, Vec<Node>)>,
        otherwise: Vec<Node>,
    },
    /// `{% for %}` … `{% endfor %}`.
    For {
        key: Option<String>,
        value: String,
        iterable: Expr,
        body: Vec<Node>,
        otherwise: Vec<Node>,
    },
    /// `{% include "name" %}`; `offset` is the string literal.
    Include { name: String, offset: usize },
    /// `{% block name %}` … `{% endblock %}`; the body lives in the
    /// template's block map so derived templates can override it.
    Block { name: String },
}

/// A binary operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Op {
    Or,
    And,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    In,
    NotIn,
    Concat,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

/// An expression with its source offset and nesting depth.
#[derive(Debug)]
pub(crate) struct Expr {
    pub(crate) kind: ExprKind,
    /// Byte offset where the expression starts.
    pub(crate) offset: usize,
    depth: usize,
}

#[derive(Debug)]
pub(crate) enum ExprKind {
    Lit(Value),
    Var(String),
    Attr(Box<Expr>, String),
    Index(Box<Expr>, Box<Expr>),
    Neg(Box<Expr>),
    Not(Box<Expr>),
    Binary(Op, Box<Expr>, Box<Expr>),
    Filter(Box<FilterCall>),
    List(Vec<Expr>),
    Super,
}

/// `input | name(args)`.
#[derive(Debug)]
pub(crate) struct FilterCall {
    pub(crate) input: Expr,
    pub(crate) name: String,
    pub(crate) name_offset: usize,
    pub(crate) args: Vec<Expr>,
}

impl Expr {
    /// Build an expression, rejecting trees deeper than `limit`.
    pub(crate) fn new(
        kind: ExprKind,
        offset: usize,
        children_depth: usize,
        limit: usize,
    ) -> Result<Expr, RawError> {
        let depth = children_depth + 1;
        if depth > limit {
            return Err(Fail::new(ErrorKind::NestingTooDeep { limit }, offset));
        }
        Ok(Expr {
            kind,
            offset,
            depth,
        })
    }

    pub(crate) fn depth(&self) -> usize {
        self.depth
    }
}
