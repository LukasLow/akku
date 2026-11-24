//! Rendering: the evaluator.
//!
//! Evaluation borrows values from the context, the loop scope and the
//! template instead of cloning them ([`Val`]). Every way a template could
//! run away is bounded: expression and tag nesting by the compile-time limit,
//! include/extends chains by the include depth, total includes and loop
//! iterations by counters, the output (plus every filter result) by the
//! output limit, and the values a template copies (list literals and filter
//! arguments that duplicate borrowed values) by a per-render copy budget, so
//! nested `[[a, a]]` literals cannot double memory at every level.

use std::cell::Cell;
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::html::escape;

use super::ast::{Expr, ExprKind, FilterCall, Node, Op};
use super::context::Context;
use super::engine::{AutoEscape, Engine, Template};
use super::error::{Error, ErrorKind, Result};
use super::filters;
use super::value::Value;

/// How deep structural equality and ordering follow nested values.
const MAX_COMPARE_DEPTH: usize = 64;
/// A hard ceiling on active block frames, whatever the configured limits.
const MAX_BLOCK_FRAMES: usize = 256;
/// The copy budget is this many times the output limit (in bytes).
const COPY_BUDGET_FACTOR: usize = 16;

/// The approximate heap footprint of `value` in bytes, or `None` once it
/// exceeds `max` (the walk stops early, so measuring is bounded too).
fn footprint(value: &Value, max: usize) -> Option<usize> {
    let node = std::mem::size_of::<Value>();
    let mut total = 0usize;
    let mut stack = vec![value];
    while let Some(value) = stack.pop() {
        total = total.checked_add(node)?;
        match value {
            Value::Str(text) | Value::Safe(text) => total = total.checked_add(text.len())?,
            Value::List(items) => stack.extend(items.iter()),
            Value::Map(map) => {
                for (key, item) in map {
                    total = total.checked_add(key.len().checked_add(node)?)?;
                    stack.push(item);
                }
            }
            Value::Null | Value::Bool(_) | Value::Int(_) | Value::Float(_) => {}
        }
        if total > max {
            return None;
        }
    }
    Some(total)
}

/// A value being evaluated: missing, borrowed or owned.
enum Val<'a> {
    Undef,
    Ref(&'a Value),
    Owned(Value),
}

impl<'a> Val<'a> {
    fn value(&self) -> Option<&Value> {
        match self {
            Val::Undef => None,
            Val::Ref(value) => Some(value),
            Val::Owned(value) => Some(value),
        }
    }

    fn into_owned(self) -> Option<Value> {
        match self {
            Val::Undef => None,
            Val::Ref(value) => Some(value.clone()),
            Val::Owned(value) => Some(value),
        }
    }

    fn is_truthy(&self) -> bool {
        self.value().is_some_and(Value::is_truthy)
    }
}

struct LoopState {
    index0: usize,
    length: usize,
}

impl LoopState {
    fn to_value(&self) -> Value {
        let index = self.index0 + 1;
        let mut map = BTreeMap::new();
        map.insert("index".to_owned(), Value::from(index));
        map.insert("index0".to_owned(), Value::from(self.index0));
        map.insert("first".to_owned(), Value::Bool(self.index0 == 0));
        map.insert("last".to_owned(), Value::Bool(index == self.length));
        map.insert("length".to_owned(), Value::from(self.length));
        map.insert(
            "revindex".to_owned(),
            Value::from(self.length - self.index0),
        );
        map.insert(
            "revindex0".to_owned(),
            Value::from(self.length - self.index0 - 1),
        );
        Value::Map(map)
    }
}

/// Loop variables and the state of the loops that are running.
#[derive(Default)]
struct Scope {
    vars: Vec<(String, Value)>,
    loops: Vec<LoopState>,
}

/// What an expression is evaluated against.
struct Cx<'a> {
    engine: &'a Engine,
    ctx: &'a Context,
    scope: &'a Scope,
    tpl: &'a Template,
    /// Bytes still allowed to be copied during this render.
    copy_budget: &'a Cell<usize>,
}

impl<'a> Cx<'a> {
    fn error(&self, kind: ErrorKind, offset: usize) -> Error {
        Error::at(
            kind,
            self.tpl.inner.name.as_deref(),
            &self.tpl.inner.source,
            offset,
        )
    }

    fn invalid(&self, message: impl Into<String>, offset: usize) -> Error {
        self.error(
            ErrorKind::InvalidOperation {
                message: message.into(),
            },
            offset,
        )
    }

    /// Take ownership of a strict value, charging a copy of a borrowed value
    /// against the per-render copy budget before cloning it.
    fn own(&self, value: Val<'_>, offset: usize) -> Result<Value> {
        match value {
            Val::Ref(value) => {
                let remaining = self.copy_budget.get();
                let Some(cost) = footprint(value, remaining) else {
                    return Err(self.copy_limit(offset));
                };
                self.copy_budget.set(remaining - cost);
                Ok(value.clone())
            }
            Val::Owned(value) => Ok(value),
            Val::Undef => unreachable!("a strict value is defined"),
        }
    }

    fn copy_limit(&self, offset: usize) -> Error {
        let limit = copy_budget(self.engine);
        self.error(ErrorKind::CopyLimitExceeded { limit }, offset)
    }

    fn lookup(&self, name: &str) -> Val<'a> {
        if let Some((_, value)) = self.scope.vars.iter().rev().find(|(var, _)| var == name) {
            return Val::Ref(value);
        }
        if name == "loop" {
            if let Some(state) = self.scope.loops.last() {
                return Val::Owned(state.to_value());
            }
        }
        match self.ctx.get(name) {
            Some(value) => Val::Ref(value),
            None => Val::Undef,
        }
    }
}

fn apply_attr<'a>(base: Val<'a>, name: &str) -> Val<'a> {
    match base {
        Val::Ref(Value::Map(map)) => map.get(name).map_or(Val::Undef, Val::Ref),
        Val::Owned(Value::Map(mut map)) => map.remove(name).map_or(Val::Undef, Val::Owned),
        _ => Val::Undef,
    }
}

fn apply_index<'a>(base: Val<'a>, index: &Value) -> Val<'a> {
    match (base, index) {
        (Val::Ref(Value::List(items)), Value::Int(at)) => usize::try_from(*at)
            .ok()
            .and_then(|at| items.get(at))
            .map_or(Val::Undef, Val::Ref),
        (Val::Owned(Value::List(items)), Value::Int(at)) => usize::try_from(*at)
            .ok()
            .and_then(|at| items.into_iter().nth(at))
            .map_or(Val::Undef, Val::Owned),
        (Val::Ref(Value::Map(map)), Value::Str(key) | Value::Safe(key)) => {
            map.get(key).map_or(Val::Undef, Val::Ref)
        }
        (Val::Owned(Value::Map(mut map)), Value::Str(key) | Value::Safe(key)) => {
            map.remove(key).map_or(Val::Undef, Val::Owned)
        }
        _ => Val::Undef,
    }
}

/// The dotted path up to the first missing part of `expr`, and the offset of
/// its first identifier, for `UndefinedVariable` errors.
fn describe_undefined(cx: &Cx<'_>, expr: &Expr) -> (String, usize) {
    let mut chain: Vec<&Expr> = Vec::new();
    let mut current = expr;
    loop {
        chain.push(current);
        match &current.kind {
            ExprKind::Attr(base, _) | ExprKind::Index(base, _) => current = base,
            _ => break,
        }
    }
    chain.reverse();
    let offset = chain.first().map_or(expr.offset, |root| root.offset);
    let mut path = String::new();
    let mut value: Val<'_> = Val::Undef;
    for part in chain {
        match &part.kind {
            ExprKind::Var(name) => {
                path.push_str(name);
                value = cx.lookup(name);
            }
            ExprKind::Attr(_, name) => {
                path.push('.');
                path.push_str(name);
                value = apply_attr(value, name);
            }
            ExprKind::Index(_, index) => {
                let index = eval(cx, index).ok().and_then(Val::into_owned);
                match &index {
                    Some(Value::Int(at)) => path.push_str(&format!(".{at}")),
                    Some(other) => path.push_str(&format!("[{other}]")),
                    None => path.push_str("[?]"),
                }
                value = match index {
                    Some(index) => apply_index(value, &index),
                    None => Val::Undef,
                };
            }
            _ => {
                if path.is_empty() {
                    path.push_str("<expression>");
                }
                break;
            }
        }
        if matches!(value, Val::Undef) {
            break;
        }
    }
    (path, offset)
}

fn undefined(cx: &Cx<'_>, expr: &Expr) -> Error {
    let (name, offset) = describe_undefined(cx, expr);
    cx.error(ErrorKind::UndefinedVariable { name }, offset)
}

/// Evaluate and require a value (missing values are an error).
fn eval_strict<'a>(cx: &Cx<'a>, expr: &'a Expr) -> Result<Val<'a>> {
    let value = eval(cx, expr)?;
    if matches!(value, Val::Undef) {
        return Err(undefined(cx, expr));
    }
    Ok(value)
}

fn values_equal(left: &Value, right: &Value, depth: usize) -> bool {
    if depth > MAX_COMPARE_DEPTH {
        return false;
    }
    match (left, right) {
        (Value::Int(a), Value::Int(b)) => a == b,
        (Value::Int(_) | Value::Float(_), Value::Int(_) | Value::Float(_)) => {
            left.as_f64() == right.as_f64()
        }
        (Value::Str(a) | Value::Safe(a), Value::Str(b) | Value::Safe(b)) => a == b,
        (Value::Null, Value::Null) => true,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::List(a), Value::List(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| values_equal(x, y, depth + 1))
        }
        (Value::Map(a), Value::Map(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, x)| b.get(key).is_some_and(|y| values_equal(x, y, depth + 1)))
        }
        _ => false,
    }
}

fn compare(left: &Value, right: &Value) -> Option<Ordering> {
    match (left, right) {
        (Value::Int(a), Value::Int(b)) => Some(a.cmp(b)),
        (Value::Int(_) | Value::Float(_), Value::Int(_) | Value::Float(_)) => {
            left.as_f64()?.partial_cmp(&right.as_f64()?)
        }
        (Value::Str(a) | Value::Safe(a), Value::Str(b) | Value::Safe(b)) => Some(a.cmp(b)),
        _ => None,
    }
}

fn eval<'a>(cx: &Cx<'a>, expr: &'a Expr) -> Result<Val<'a>> {
    match &expr.kind {
        ExprKind::Lit(value) => Ok(Val::Ref(value)),
        ExprKind::Var(name) => Ok(cx.lookup(name)),
        ExprKind::Attr(base, name) => Ok(apply_attr(eval(cx, base)?, name)),
        ExprKind::Index(base, index) => {
            let base = eval(cx, base)?;
            let index = eval_strict(cx, index)?;
            let index = index.value().expect("a strict value is defined");
            Ok(apply_index(base, index))
        }
        ExprKind::Neg(inner) => {
            let value = eval_strict(cx, inner)?;
            match value.value() {
                Some(Value::Int(n)) => match n.checked_neg() {
                    Some(negated) => Ok(Val::Owned(Value::Int(negated))),
                    None => Err(cx.invalid("integer overflow", expr.offset)),
                },
                Some(Value::Float(f)) => Ok(Val::Owned(Value::Float(-f))),
                Some(other) => Err(cx.invalid(
                    format!("cannot negate a {}", other.type_name()),
                    expr.offset,
                )),
                None => unreachable!("a strict value is defined"),
            }
        }
        ExprKind::Not(inner) => {
            let value = eval(cx, inner)?;
            Ok(Val::Owned(Value::Bool(!value.is_truthy())))
        }
        ExprKind::Binary(op, left, right) => eval_binary(cx, expr, *op, left, right),
        ExprKind::Filter(call) => eval_filter(cx, call),
        ExprKind::List(items) => {
            let mut values = Vec::with_capacity(items.len());
            for item in items {
                let value = eval_strict(cx, item)?;
                values.push(cx.own(value, item.offset)?);
            }
            Ok(Val::Owned(Value::List(values)))
        }
        ExprKind::Super => Err(cx.invalid(
            "`super()` can only be used as the whole expression `{{ super() }}` inside a block",
            expr.offset,
        )),
    }
}

fn eval_binary<'a>(
    cx: &Cx<'a>,
    expr: &Expr,
    op: Op,
    left: &'a Expr,
    right: &'a Expr,
) -> Result<Val<'a>> {
    let boolean = |value: bool| Ok(Val::Owned(Value::Bool(value)));
    match op {
        Op::Or => {
            let result = eval(cx, left)?.is_truthy() || eval(cx, right)?.is_truthy();
            return boolean(result);
        }
        Op::And => {
            let result = eval(cx, left)?.is_truthy() && eval(cx, right)?.is_truthy();
            return boolean(result);
        }
        Op::Eq | Op::Ne | Op::In | Op::NotIn => {
            // Lenient: a missing value behaves like `none`.
            let left_value = eval(cx, left)?;
            let right_value = eval(cx, right)?;
            let null = Value::Null;
            let a = left_value.value().unwrap_or(&null);
            let b = right_value.value().unwrap_or(&null);
            let result = match op {
                Op::Eq => values_equal(a, b, 0),
                Op::Ne => !values_equal(a, b, 0),
                _ => {
                    let contained = match b {
                        Value::Str(haystack) | Value::Safe(haystack) => match a {
                            Value::Str(needle) | Value::Safe(needle) => {
                                haystack.contains(needle.as_str())
                            }
                            other => {
                                return Err(cx.invalid(
                                    format!("cannot look for a {} in a string", other.type_name()),
                                    expr.offset,
                                ))
                            }
                        },
                        Value::List(items) => items.iter().any(|item| values_equal(a, item, 0)),
                        Value::Map(entries) => match a {
                            Value::Str(key) | Value::Safe(key) => entries.contains_key(key),
                            _ => false,
                        },
                        Value::Null => false,
                        other => {
                            return Err(cx.invalid(
                                format!("cannot use `in` with a {}", other.type_name()),
                                expr.offset,
                            ))
                        }
                    };
                    if op == Op::In {
                        contained
                    } else {
                        !contained
                    }
                }
            };
            return boolean(result);
        }
        _ => {}
    }
    let left_value = eval_strict(cx, left)?;
    let right_value = eval_strict(cx, right)?;
    let a = left_value.value().expect("a strict value is defined");
    let b = right_value.value().expect("a strict value is defined");
    match op {
        Op::Lt | Op::Le | Op::Gt | Op::Ge => {
            let ordering = compare(a, b).ok_or_else(|| {
                cx.invalid(
                    format!(
                        "cannot compare a {} with a {}",
                        a.type_name(),
                        b.type_name()
                    ),
                    expr.offset,
                )
            })?;
            boolean(match op {
                Op::Lt => ordering == Ordering::Less,
                Op::Le => ordering != Ordering::Greater,
                Op::Gt => ordering == Ordering::Greater,
                _ => ordering != Ordering::Less,
            })
        }
        Op::Concat => {
            let (Some(x), Some(y)) = (a.scalar_text(), b.scalar_text()) else {
                return Err(cx.invalid("`~` needs strings or numbers", expr.offset));
            };
            if x.len() + y.len() > cx.engine.limits.output_len {
                return Err(cx.error(
                    ErrorKind::OutputTooLarge {
                        limit: cx.engine.limits.output_len,
                    },
                    expr.offset,
                ));
            }
            Ok(Val::Owned(Value::Str(format!("{x}{y}"))))
        }
        _ => arithmetic(cx, expr, op, a, b).map(Val::Owned),
    }
}

fn arithmetic(cx: &Cx<'_>, expr: &Expr, op: Op, a: &Value, b: &Value) -> Result<Value> {
    let overflow = || cx.invalid("integer overflow", expr.offset);
    let zero = || cx.invalid("division by zero", expr.offset);
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => match op {
            Op::Add => x.checked_add(*y).map(Value::Int).ok_or_else(overflow),
            Op::Sub => x.checked_sub(*y).map(Value::Int).ok_or_else(overflow),
            Op::Mul => x.checked_mul(*y).map(Value::Int).ok_or_else(overflow),
            Op::Div => {
                if *y == 0 {
                    Err(zero())
                } else {
                    Ok(Value::Float(*x as f64 / *y as f64))
                }
            }
            _ => {
                if *y == 0 {
                    Err(zero())
                } else {
                    x.checked_rem(*y).map(Value::Int).ok_or_else(overflow)
                }
            }
        },
        (Value::Int(_) | Value::Float(_), Value::Int(_) | Value::Float(_)) => {
            let (x, y) = (a.as_f64().unwrap_or(0.0), b.as_f64().unwrap_or(0.0));
            match op {
                Op::Add => Ok(Value::Float(x + y)),
                Op::Sub => Ok(Value::Float(x - y)),
                Op::Mul => Ok(Value::Float(x * y)),
                Op::Div => {
                    if y == 0.0 {
                        Err(zero())
                    } else {
                        Ok(Value::Float(x / y))
                    }
                }
                _ => Err(cx.invalid("`%` needs two integers", expr.offset)),
            }
        }
        _ => Err(cx.invalid(
            format!(
                "cannot do arithmetic with a {} and a {}",
                a.type_name(),
                b.type_name()
            ),
            expr.offset,
        )),
    }
}

fn eval_filter<'a>(cx: &Cx<'a>, call: &'a FilterCall) -> Result<Val<'a>> {
    let FilterCall {
        input,
        name,
        name_offset,
        args,
    } = call;
    let (name, name_offset) = (name.as_str(), *name_offset);
    let custom = cx.engine.filters.get(name);
    let builtin = if custom.is_none() {
        filters::builtin(name)
    } else {
        None
    };
    let is_default = custom.is_none() && name == "default";
    if custom.is_none() && builtin.is_none() && !is_default {
        return Err(cx.error(
            ErrorKind::UnknownFilter {
                name: name.to_owned(),
            },
            name_offset,
        ));
    }
    let input_value = eval(cx, input)?;
    let mut arg_values: Vec<Value> = Vec::with_capacity(args.len());
    for arg in args {
        let value = eval_strict(cx, arg)?;
        arg_values.push(cx.own(value, arg.offset)?);
    }
    let failed = |message: String| {
        cx.error(
            ErrorKind::FilterFailed {
                name: name.to_owned(),
                message,
            },
            name_offset,
        )
    };
    if is_default {
        if arg_values.is_empty() || arg_values.len() > 2 {
            return Err(failed(format!(
                "expected 1 to 2 argument(s), found {}",
                arg_values.len()
            )));
        }
        let falsy_too = arg_values.get(1).is_some_and(Value::is_truthy);
        let replace = match input_value.value() {
            None | Some(Value::Null) => true,
            Some(value) => falsy_too && !value.is_truthy(),
        };
        return Ok(if replace {
            Val::Owned(arg_values.swap_remove(0))
        } else {
            input_value
        });
    }
    let Some(input_ref) = input_value.value() else {
        return Err(undefined(cx, input));
    };
    let limit = cx.engine.limits.output_len;
    let result = match (custom, builtin) {
        (Some(filter), _) => filter(input_ref, &arg_values).map_err(failed)?,
        (None, Some(filter)) => match filter(input_ref, &arg_values, limit) {
            Ok(value) => value,
            Err(filters::FilterError::Message(message)) => return Err(failed(message)),
            Err(filters::FilterError::TooLarge) => {
                return Err(cx.error(ErrorKind::OutputTooLarge { limit }, name_offset))
            }
        },
        (None, None) => unreachable!("the filter exists"),
    };
    if let Value::Str(text) | Value::Safe(text) = &result {
        if text.len() > cx.engine.limits.output_len {
            return Err(cx.error(
                ErrorKind::OutputTooLarge {
                    limit: cx.engine.limits.output_len,
                },
                name_offset,
            ));
        }
    }
    Ok(Val::Owned(result))
}

// ---------------------------------------------------------------------------
// Rendering.
// ---------------------------------------------------------------------------

/// The active definitions of a block, most-derived first, for `super()`.
struct BlockFrame {
    chain: Vec<Template>,
    depth: usize,
    candidates: Vec<(Template, Arc<Vec<Node>>)>,
    index: usize,
}

struct Run<'e> {
    engine: &'e Engine,
    ctx: &'e Context,
    out: String,
    scope: Scope,
    iterations: usize,
    includes: usize,
    /// Bytes still allowed to be copied (see [`Cx::own`]).
    copy_budget: Cell<usize>,
    /// `None` marks the boundary of an include, hiding the caller's blocks.
    blocks: Vec<Option<BlockFrame>>,
}

/// The per-render copy budget in bytes.
fn copy_budget(engine: &Engine) -> usize {
    engine.limits.output_len.saturating_mul(COPY_BUDGET_FACTOR)
}

/// Render `template` against `context` with the engine's filters and limits.
pub(crate) fn render(engine: &Engine, template: &Template, context: &Context) -> Result<String> {
    let mut run = Run {
        engine,
        ctx: context,
        out: String::new(),
        scope: Scope::default(),
        iterations: 0,
        includes: 0,
        copy_budget: Cell::new(copy_budget(engine)),
        blocks: Vec::new(),
    };
    run.render_template(template, 0)?;
    Ok(run.out)
}

impl<'e> Run<'e> {
    fn error(&self, tpl: &Template, kind: ErrorKind, offset: usize) -> Error {
        Error::at(kind, tpl.inner.name.as_deref(), &tpl.inner.source, offset)
    }

    fn emit(&mut self, text: &str, tpl: &Template, offset: usize) -> Result<()> {
        let limit = self.engine.limits.output_len;
        if self.out.len() + text.len() > limit {
            return Err(self.error(tpl, ErrorKind::OutputTooLarge { limit }, offset));
        }
        self.out.push_str(text);
        Ok(())
    }

    /// Render `template` with its `extends` chain resolved.
    #[inline(never)]
    fn render_template(&mut self, template: &Template, depth: usize) -> Result<()> {
        let limit = self.engine.limits.include_depth;
        let mut chain: Vec<Template> = vec![template.clone()];
        loop {
            let last = chain.last().expect("the chain is not empty").clone();
            let Some((parent_name, offset)) = last.inner.parent.clone() else {
                break;
            };
            if depth + chain.len() > limit {
                return Err(self.error(&last, ErrorKind::IncludeDepthExceeded { limit }, offset));
            }
            let Some(parent) = self.engine.templates.get(&parent_name) else {
                return Err(self.error(
                    &last,
                    ErrorKind::TemplateNotFound { name: parent_name },
                    offset,
                ));
            };
            chain.push(parent.clone());
        }
        let root = chain.last().expect("the chain is not empty").clone();
        self.render_nodes(&root, &chain, &root.inner.nodes, depth)
    }

    fn render_nodes(
        &mut self,
        tpl: &Template,
        chain: &[Template],
        nodes: &[Node],
        depth: usize,
    ) -> Result<()> {
        for node in nodes {
            self.render_node(tpl, chain, node, depth)?;
        }
        Ok(())
    }

    /// One node. Kept small and non-inlined: nested tags recurse through
    /// here, so its stack frame sets how deep a template can safely nest.
    #[inline(never)]
    fn render_node(
        &mut self,
        tpl: &Template,
        chain: &[Template],
        node: &Node,
        depth: usize,
    ) -> Result<()> {
        match node {
            Node::Text(text, offset) => self.emit(text, tpl, *offset),
            Node::Output(expr) => self.output(tpl, expr),
            Node::If {
                branches,
                otherwise,
            } => self.render_if(tpl, chain, depth, branches, otherwise),
            Node::For {
                key,
                value,
                iterable,
                body,
                otherwise,
            } => self.render_for(
                tpl,
                chain,
                depth,
                key.as_deref(),
                value,
                iterable,
                body,
                otherwise,
            ),
            Node::Include { name, offset } => self.render_include(tpl, depth, name, *offset),
            Node::Block { name } => self.render_block(chain, depth, name),
        }
    }

    #[inline(never)]
    fn render_if(
        &mut self,
        tpl: &Template,
        chain: &[Template],
        depth: usize,
        branches: &[(Expr, Vec<Node>)],
        otherwise: &[Node],
    ) -> Result<()> {
        for (condition, body) in branches {
            let truthy =
                self.eval_with(tpl, |cx| eval(cx, condition).map(|value| value.is_truthy()))?;
            if truthy {
                return self.render_nodes(tpl, chain, body, depth);
            }
        }
        self.render_nodes(tpl, chain, otherwise, depth)
    }

    /// Run `f` with an evaluation context over the current scope.
    fn eval_with<'s, T>(
        &'s self,
        tpl: &'s Template,
        f: impl FnOnce(&Cx<'s>) -> Result<T>,
    ) -> Result<T> {
        let cx = Cx {
            engine: self.engine,
            ctx: self.ctx,
            scope: &self.scope,
            tpl,
            copy_budget: &self.copy_budget,
        };
        f(&cx)
    }

    #[inline(never)]
    fn output(&mut self, tpl: &Template, expr: &Expr) -> Result<()> {
        if matches!(expr.kind, ExprKind::Super) {
            return self.render_super(tpl, expr.offset);
        }
        let cx = Cx {
            engine: self.engine,
            ctx: self.ctx,
            scope: &self.scope,
            tpl,
            copy_budget: &self.copy_budget,
        };
        let value = eval(&cx, expr)?;
        let Some(value) = value.value() else {
            return Err(undefined(&cx, expr));
        };
        // `cx` borrows `self.scope`; printing only touches `self.out`.
        let limit = self.engine.limits.output_len;
        let engine_escape = self.engine.auto_escape;
        let rendered: std::borrow::Cow<'_, str> = match value {
            Value::Str(text) => match engine_escape {
                AutoEscape::Html => std::borrow::Cow::Owned(escape(text)),
                AutoEscape::None => std::borrow::Cow::Borrowed(text.as_str()),
            },
            Value::Safe(text) => std::borrow::Cow::Borrowed(text.as_str()),
            Value::List(_) | Value::Map(_) => {
                return Err(cx.error(
                    ErrorKind::NotPrintable {
                        type_name: value.type_name(),
                    },
                    expr.offset,
                ))
            }
            scalar => scalar.scalar_text().unwrap_or_default(),
        };
        if self.out.len() + rendered.len() > limit {
            return Err(cx.error(ErrorKind::OutputTooLarge { limit }, expr.offset));
        }
        self.out.push_str(&rendered);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    #[inline(never)]
    fn render_for(
        &mut self,
        tpl: &Template,
        chain: &[Template],
        depth: usize,
        key: Option<&str>,
        value: &str,
        iterable: &Expr,
        body: &[Node],
        otherwise: &[Node],
    ) -> Result<()> {
        // Collect first: the scope is mutated while the loop runs.
        let (items, is_map): (Vec<(Value, Value)>, bool) = self.eval_with(tpl, |cx| {
            let iterable_value = eval_strict(cx, iterable)?;
            // An owned iterable (a literal or filter result) is moved, not
            // copied; a borrowed one is copied item by item, which is linear
            // in data that already exists.
            if let Val::Owned(owned) = iterable_value {
                return match owned {
                    Value::List(list) => Ok((
                        list.into_iter()
                            .enumerate()
                            .map(|(index, item)| (Value::from(index), item))
                            .collect(),
                        false,
                    )),
                    Value::Map(map) => Ok((
                        map.into_iter()
                            .map(|(name, item)| (Value::Str(name), item))
                            .collect(),
                        true,
                    )),
                    other => Err(cx.error(
                        ErrorKind::NotIterable {
                            type_name: other.type_name(),
                        },
                        iterable.offset,
                    )),
                };
            }
            match iterable_value.value().expect("a strict value is defined") {
                Value::List(list) => Ok((
                    list.iter()
                        .enumerate()
                        .map(|(index, item)| (Value::from(index), item.clone()))
                        .collect(),
                    false,
                )),
                Value::Map(map) => Ok((
                    map.iter()
                        .map(|(name, item)| (Value::Str(name.clone()), item.clone()))
                        .collect(),
                    true,
                )),
                other => Err(cx.error(
                    ErrorKind::NotIterable {
                        type_name: other.type_name(),
                    },
                    iterable.offset,
                )),
            }
        })?;
        if items.is_empty() {
            return self.render_nodes(tpl, chain, otherwise, depth);
        }
        let length = items.len();
        let base = self.scope.vars.len();
        for (index0, (item_key, item)) in items.into_iter().enumerate() {
            self.iterations += 1;
            let limit = self.engine.limits.loop_iterations;
            if self.iterations > limit {
                return Err(self.error(
                    tpl,
                    ErrorKind::LoopLimitExceeded { limit },
                    iterable.offset,
                ));
            }
            match key {
                Some(key_name) => {
                    self.scope.vars.push((key_name.to_owned(), item_key));
                    self.scope.vars.push((value.to_owned(), item));
                }
                // One variable: a list yields its items, a map its keys.
                None => self
                    .scope
                    .vars
                    .push((value.to_owned(), if is_map { item_key } else { item })),
            }
            self.scope.loops.push(LoopState { index0, length });
            let result = self.render_nodes(tpl, chain, body, depth);
            self.scope.loops.pop();
            self.scope.vars.truncate(base);
            result?;
        }
        Ok(())
    }

    #[inline(never)]
    fn render_include(
        &mut self,
        tpl: &Template,
        depth: usize,
        name: &str,
        offset: usize,
    ) -> Result<()> {
        let limits = self.engine.limits;
        if depth + 1 > limits.include_depth {
            return Err(self.error(
                tpl,
                ErrorKind::IncludeDepthExceeded {
                    limit: limits.include_depth,
                },
                offset,
            ));
        }
        self.includes += 1;
        if self.includes > limits.include_calls {
            return Err(self.error(
                tpl,
                ErrorKind::IncludeLimitExceeded {
                    limit: limits.include_calls,
                },
                offset,
            ));
        }
        let Some(included) = self.engine.templates.get(name) else {
            return Err(self.error(
                tpl,
                ErrorKind::TemplateNotFound {
                    name: name.to_owned(),
                },
                offset,
            ));
        };
        self.blocks.push(None);
        let result = self.render_template(included, depth + 1);
        self.blocks.pop();
        result
    }

    #[inline(never)]
    fn render_block(&mut self, chain: &[Template], depth: usize, name: &str) -> Result<()> {
        let candidates: Vec<(Template, Arc<Vec<Node>>)> = chain
            .iter()
            .filter_map(|template| {
                template
                    .inner
                    .blocks
                    .get(name)
                    .map(|body| (template.clone(), Arc::clone(body)))
            })
            .collect();
        if candidates.is_empty() {
            return Ok(());
        }
        self.run_candidate(chain.to_vec(), depth, candidates, 0)
    }

    fn run_candidate(
        &mut self,
        chain: Vec<Template>,
        depth: usize,
        candidates: Vec<(Template, Arc<Vec<Node>>)>,
        index: usize,
    ) -> Result<()> {
        let (owner, body) = candidates[index].clone();
        // `super()` can render a less-derived block whose body names a block
        // that leads back here (`a` -> super -> `b` -> `a` -> ...), so the
        // number of active block frames is bounded to keep the stack safe.
        let limit = self.max_block_frames();
        if self.blocks.len() >= limit {
            return Err(self.error(&owner, ErrorKind::NestingTooDeep { limit }, 0));
        }
        self.blocks.push(Some(BlockFrame {
            chain: chain.clone(),
            depth,
            candidates,
            index,
        }));
        let result = self.render_nodes(&owner, &chain, &body, depth);
        self.blocks.pop();
        result
    }

    /// The most block (and include-boundary) frames that may be active at
    /// once: every template in an include/extends chain may nest blocks up to
    /// the nesting depth.
    fn max_block_frames(&self) -> usize {
        let limits = self.engine.limits;
        limits
            .nesting_depth
            .saturating_add(1)
            .saturating_mul(limits.include_depth.saturating_add(1))
            .min(MAX_BLOCK_FRAMES)
    }

    fn render_super(&mut self, tpl: &Template, offset: usize) -> Result<()> {
        let Some(Some(frame)) = self.blocks.last() else {
            return Err(self.error(
                tpl,
                ErrorKind::InvalidOperation {
                    message: "`super()` can only be used inside a block".to_owned(),
                },
                offset,
            ));
        };
        let next = frame.index + 1;
        if next >= frame.candidates.len() {
            return Ok(());
        }
        let (chain, depth, candidates) =
            (frame.chain.clone(), frame.depth, frame.candidates.clone());
        self.run_candidate(chain, depth, candidates, next)
    }
}
