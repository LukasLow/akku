//! Compiling template source to nodes.
//!
//! Block structure (`if`, `for`, `block`) is parsed with an explicit stack of
//! open frames, so tag nesting never recurses. Expressions use recursive
//! descent, guarded by a depth counter, and every [`Expr`] records its own
//! depth, so neither the parser nor the evaluator can be driven past the
//! configured nesting limit.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::ast::{Expr, ExprKind, FilterCall, Node, Op};
use super::error::{ErrorKind, Fail, RawError};
use super::lexer::{next_segment, tokenize, SegmentKind, Tok, Token};
use super::value::Value;

type PResult<T> = Result<T, RawError>;

/// The precedence of comparisons; `not` binds just below it.
const PREC_CMP: u8 = 4;

/// The result of compiling one template.
pub(crate) struct Parsed {
    pub(crate) nodes: Vec<Node>,
    /// `{% extends "name" %}`: the name and the offset of its literal.
    pub(crate) parent: Option<(String, usize)>,
    /// Every block of the template by name (including nested ones).
    pub(crate) blocks: HashMap<String, Arc<Vec<Node>>>,
}

enum Frame {
    If {
        open: usize,
        branches: Vec<(Expr, Vec<Node>)>,
        cond: Option<Expr>,
        body: Vec<Node>,
        in_else: bool,
    },
    For {
        open: usize,
        key: Option<String>,
        value: String,
        iterable: Expr,
        body: Vec<Node>,
        otherwise: Option<Vec<Node>>,
    },
    Block {
        open: usize,
        name: String,
        body: Vec<Node>,
    },
}

impl Frame {
    fn open(&self) -> usize {
        match self {
            Frame::If { open, .. } | Frame::For { open, .. } | Frame::Block { open, .. } => *open,
        }
    }

    fn tag(&self) -> &'static str {
        match self {
            Frame::If { .. } => "if",
            Frame::For { .. } => "for",
            Frame::Block { .. } => "block",
        }
    }

    fn end_tag(&self) -> &'static str {
        match self {
            Frame::If { .. } => "endif",
            Frame::For { .. } => "endfor",
            Frame::Block { .. } => "endblock",
        }
    }

    fn body(&mut self) -> &mut Vec<Node> {
        match self {
            Frame::If { body, .. } | Frame::Block { body, .. } => body,
            Frame::For {
                body, otherwise, ..
            } => match otherwise {
                Some(otherwise) => otherwise,
                None => body,
            },
        }
    }
}

struct Compiler<'s> {
    source: &'s str,
    limit: usize,
    root: Vec<Node>,
    stack: Vec<Frame>,
    parent: Option<(String, usize)>,
    blocks: HashMap<String, Arc<Vec<Node>>>,
    declared: HashSet<String>,
    seen_content: bool,
}

/// Compile `source`, allowing tags and expressions nested up to `limit` deep.
pub(crate) fn compile(source: &str, limit: usize) -> PResult<Parsed> {
    let mut compiler = Compiler {
        source,
        limit,
        root: Vec::new(),
        stack: Vec::new(),
        parent: None,
        blocks: HashMap::new(),
        declared: HashSet::new(),
        seen_content: false,
    };
    compiler.run()?;
    if let Some(frame) = compiler.stack.last() {
        return Err(Fail::new(
            ErrorKind::UnclosedBlock { tag: frame.tag() },
            frame.open(),
        ));
    }
    Ok(Parsed {
        nodes: compiler.root,
        parent: compiler.parent,
        blocks: compiler.blocks,
    })
}

impl<'s> Compiler<'s> {
    fn current(&mut self) -> &mut Vec<Node> {
        match self.stack.last_mut() {
            Some(frame) => frame.body(),
            None => &mut self.root,
        }
    }

    /// Push literal text that starts at byte `offset` of the source.
    fn push_text(&mut self, text: &str, offset: usize, trim_start: bool) {
        let (text, offset) = if trim_start {
            let trimmed = text.trim_start();
            (trimmed, offset + (text.len() - trimmed.len()))
        } else {
            (text, offset)
        };
        if text.is_empty() {
            return;
        }
        if !text.trim().is_empty() {
            self.seen_content = true;
        }
        self.current().push(Node::Text(text.to_owned(), offset));
    }

    /// Trim trailing whitespace off the text node just before a `{%-` tag.
    fn trim_previous(&mut self) {
        let body = self.current();
        if let Some(Node::Text(text, _)) = body.last_mut() {
            let trimmed = text.trim_end().len();
            text.truncate(trimmed);
            if text.is_empty() {
                body.pop();
            }
        }
    }

    fn run(&mut self) -> PResult<()> {
        let source = self.source;
        let mut pos = 0usize;
        let mut trim_next = false;
        while pos < source.len() {
            let Some(segment) = next_segment(source, pos)? else {
                self.push_text(&source[pos..], pos, trim_next);
                break;
            };
            self.push_text(&source[pos..segment.open], pos, trim_next);
            if segment.trim_before {
                self.trim_previous();
            }
            pos = segment.end;
            trim_next = segment.trim_after;
            let closer_at = if segment.trim_after {
                segment.end - 3
            } else {
                segment.end - 2
            };
            match segment.kind {
                SegmentKind::Comment => {}
                SegmentKind::Var => {
                    self.seen_content = true;
                    let tokens = tokenize(
                        source,
                        segment.content_start,
                        segment.content_end,
                        closer_at,
                        "}}",
                    )?;
                    let mut parser = ExprParser::new(&tokens, self.limit);
                    let expr = parser.parse_expr()?;
                    parser.expect_end("`}}`")?;
                    self.current().push(Node::Output(expr));
                }
                SegmentKind::Tag => {
                    let tokens = tokenize(
                        source,
                        segment.content_start,
                        segment.content_end,
                        closer_at,
                        "%}",
                    )?;
                    if let Some(after) = self.tag(&tokens, segment.open, pos)? {
                        pos = after.0;
                        trim_next = after.1;
                    }
                }
            }
        }
        Ok(())
    }

    fn push_frame(&mut self, frame: Frame, open: usize) -> PResult<()> {
        if self.stack.len() >= self.limit {
            return Err(Fail::new(
                ErrorKind::NestingTooDeep { limit: self.limit },
                open,
            ));
        }
        self.stack.push(frame);
        Ok(())
    }

    /// Handle one `{% … %}` tag. Returns a new source position and trim flag
    /// when the tag consumed more source (`raw`).
    fn tag(
        &mut self,
        tokens: &[Token],
        open: usize,
        after: usize,
    ) -> PResult<Option<(usize, bool)>> {
        let Some(Token {
            tok: Tok::Ident(name),
            offset: name_offset,
        }) = tokens.first()
        else {
            let first = &tokens[0];
            return Err(Fail::new(
                ErrorKind::UnexpectedToken {
                    expected: "a tag name",
                    found: first.tok.describe(),
                },
                first.offset,
            ));
        };
        let name_offset = *name_offset;
        let rest = &tokens[1..];
        let limit = self.limit;
        match name.as_str() {
            "if" => {
                self.seen_content = true;
                let mut parser = ExprParser::new(rest, limit);
                let cond = parser.parse_expr()?;
                parser.expect_end("end of tag")?;
                self.push_frame(
                    Frame::If {
                        open,
                        branches: Vec::new(),
                        cond: Some(cond),
                        body: Vec::new(),
                        in_else: false,
                    },
                    open,
                )?;
            }
            "elif" => {
                let mut parser = ExprParser::new(rest, limit);
                let new_cond = parser.parse_expr()?;
                parser.expect_end("end of tag")?;
                match self.stack.last_mut() {
                    Some(Frame::If {
                        branches,
                        cond,
                        body,
                        in_else: false,
                        ..
                    }) => {
                        let previous = cond
                            .replace(new_cond)
                            .expect("an open branch has a condition");
                        branches.push((previous, std::mem::take(body)));
                    }
                    _ => return Err(unexpected_tag(name, open)),
                }
            }
            "else" => {
                expect_end_only(rest, "end of tag")?;
                match self.stack.last_mut() {
                    Some(Frame::If {
                        branches,
                        cond,
                        body,
                        in_else,
                        ..
                    }) if !*in_else => {
                        let previous = cond.take().expect("an open branch has a condition");
                        branches.push((previous, std::mem::take(body)));
                        *in_else = true;
                    }
                    Some(Frame::For { otherwise, .. }) if otherwise.is_none() => {
                        *otherwise = Some(Vec::new());
                    }
                    _ => return Err(unexpected_tag(name, open)),
                }
            }
            "endif" | "endfor" => {
                expect_end_only(rest, "end of tag")?;
                let Some(frame) = self.stack.last() else {
                    return Err(unexpected_tag(name, open));
                };
                if frame.end_tag() != name {
                    return Err(Fail::new(
                        ErrorKind::MismatchedEndTag {
                            expected: frame.end_tag(),
                            found: name.clone(),
                        },
                        open,
                    ));
                }
                let node = match self.stack.pop().expect("the frame exists") {
                    Frame::If {
                        mut branches,
                        cond,
                        body,
                        in_else,
                        ..
                    } => {
                        if in_else {
                            Node::If {
                                branches,
                                otherwise: body,
                            }
                        } else {
                            branches.push((cond.expect("an open branch has a condition"), body));
                            Node::If {
                                branches,
                                otherwise: Vec::new(),
                            }
                        }
                    }
                    Frame::For {
                        key,
                        value,
                        iterable,
                        body,
                        otherwise,
                        ..
                    } => Node::For {
                        key,
                        value,
                        iterable,
                        body,
                        otherwise: otherwise.unwrap_or_default(),
                    },
                    Frame::Block { .. } => unreachable!("the end tag matched `if` or `for`"),
                };
                self.current().push(node);
            }
            "for" => {
                self.seen_content = true;
                let (key, value, iterable) = parse_for(rest, limit)?;
                self.push_frame(
                    Frame::For {
                        open,
                        key,
                        value,
                        iterable,
                        body: Vec::new(),
                        otherwise: None,
                    },
                    open,
                )?;
            }
            "include" => {
                self.seen_content = true;
                let (included, offset) = parse_name_literal(rest, "a template name")?;
                self.current().push(Node::Include {
                    name: included,
                    offset,
                });
            }
            "extends" => {
                if self.seen_content || !self.stack.is_empty() || self.parent.is_some() {
                    return Err(Fail::new(ErrorKind::ExtendsNotFirst, open));
                }
                let (parent, offset) = parse_name_literal(rest, "a template name")?;
                self.parent = Some((parent, offset));
            }
            "block" => {
                self.seen_content = true;
                let (block_name, block_offset) = match rest {
                    [Token {
                        tok: Tok::Ident(block_name),
                        offset,
                    }, end @ ..] => {
                        expect_end_only(end, "end of tag")?;
                        (block_name.clone(), *offset)
                    }
                    [other, ..] => {
                        return Err(Fail::new(
                            ErrorKind::UnexpectedToken {
                                expected: "a block name",
                                found: other.tok.describe(),
                            },
                            other.offset,
                        ))
                    }
                    [] => unreachable!("the token list always ends with End"),
                };
                if !self.declared.insert(block_name.clone()) {
                    return Err(Fail::new(
                        ErrorKind::DuplicateBlock { name: block_name },
                        block_offset,
                    ));
                }
                self.push_frame(
                    Frame::Block {
                        open,
                        name: block_name,
                        body: Vec::new(),
                    },
                    open,
                )?;
            }
            "endblock" => {
                let optional_name = match rest {
                    [Token {
                        tok: Tok::Ident(given),
                        offset,
                    }, end @ ..] => {
                        expect_end_only(end, "end of tag")?;
                        Some((given.clone(), *offset))
                    }
                    other => {
                        expect_end_only(other, "end of tag")?;
                        None
                    }
                };
                let Some(frame) = self.stack.last() else {
                    return Err(unexpected_tag(name, open));
                };
                let Frame::Block {
                    name: open_name, ..
                } = frame
                else {
                    return Err(Fail::new(
                        ErrorKind::MismatchedEndTag {
                            expected: frame.end_tag(),
                            found: name.clone(),
                        },
                        open,
                    ));
                };
                if let Some((given, offset)) = optional_name {
                    if &given != open_name {
                        return Err(Fail::new(
                            ErrorKind::UnexpectedToken {
                                expected: "the name of the open block",
                                found: format!("`{given}`"),
                            },
                            offset,
                        ));
                    }
                }
                let Some(Frame::Block { name, body, .. }) = self.stack.pop() else {
                    unreachable!("the top frame is a block");
                };
                self.blocks.insert(name.clone(), Arc::new(body));
                self.current().push(Node::Block { name });
            }
            "raw" => {
                self.seen_content = true;
                expect_end_only(rest, "end of tag")?;
                return self.raw(open, after).map(Some);
            }
            _ => {
                return Err(Fail::new(
                    ErrorKind::UnknownTag { name: name.clone() },
                    name_offset,
                ))
            }
        }
        Ok(None)
    }

    /// Consume a `{% raw %}` body up to `{% endraw %}`. `after` is the offset
    /// just past the `raw` tag; returns where parsing resumes.
    fn raw(&mut self, open: usize, after: usize) -> PResult<(usize, bool)> {
        let source = self.source;
        let raw_trims_start = source
            .get(..after)
            .is_some_and(|head| head.ends_with("-%}"));
        let mut from = after;
        loop {
            let found = source
                .get(from..)
                .and_then(|rest| rest.find("{%"))
                .map(|at| from + at);
            let Some(start) = found else {
                return Err(Fail::new(ErrorKind::UnclosedBlock { tag: "raw" }, open));
            };
            // Search for the closing `%}` *after* the opening `{%`, so the
            // `%` of the opener can never be mistaken for the closer
            // (`{%}` is not a tag).
            let inner_start = start + 2;
            let Some(close) = source
                .get(inner_start..)
                .and_then(|rest| rest.find("%}"))
                .map(|at| inner_start + at)
            else {
                return Err(Fail::new(ErrorKind::UnclosedBlock { tag: "raw" }, open));
            };
            let mut inner = source.get(inner_start..close).unwrap_or("");
            let left_trim = inner.starts_with('-');
            if let Some(rest) = inner.strip_prefix('-') {
                inner = rest;
            }
            let right_trim = inner.ends_with('-');
            if let Some(rest) = inner.strip_suffix('-') {
                inner = rest;
            }
            if inner.trim() == "endraw" {
                let mut body = source.get(after..start).unwrap_or("");
                if raw_trims_start {
                    body = body.trim_start();
                }
                if left_trim {
                    body = body.trim_end();
                }
                self.push_text(body, after, false);
                return Ok((close + 2, right_trim));
            }
            from = start + 2;
        }
    }
}

fn unexpected_tag(name: &str, open: usize) -> RawError {
    Fail::new(
        ErrorKind::UnexpectedTag {
            name: name.to_owned(),
        },
        open,
    )
}

fn expect_end_only(tokens: &[Token], expected: &'static str) -> PResult<()> {
    match tokens {
        [Token {
            tok: Tok::End(_), ..
        }] => Ok(()),
        [other, ..] => Err(Fail::new(
            ErrorKind::UnexpectedToken {
                expected,
                found: other.tok.describe(),
            },
            other.offset,
        )),
        [] => Ok(()),
    }
}

fn parse_name_literal(tokens: &[Token], expected: &'static str) -> PResult<(String, usize)> {
    match tokens {
        [Token {
            tok: Tok::Str(name),
            offset,
        }, end @ ..] => {
            expect_end_only(end, "end of tag")?;
            Ok((name.clone(), *offset))
        }
        [other, ..] => Err(Fail::new(
            ErrorKind::UnexpectedToken {
                expected,
                found: other.tok.describe(),
            },
            other.offset,
        )),
        [] => unreachable!("the token list always ends with End"),
    }
}

/// `for x in expr` or `for key, value in expr`.
fn parse_for(tokens: &[Token], limit: usize) -> PResult<(Option<String>, String, Expr)> {
    let mut index = 0;
    let take_ident = |index: &mut usize| -> PResult<String> {
        match &tokens[*index] {
            Token {
                tok: Tok::Ident(name),
                ..
            } => {
                *index += 1;
                Ok(name.clone())
            }
            other => Err(Fail::new(
                ErrorKind::UnexpectedToken {
                    expected: "a loop variable name",
                    found: other.tok.describe(),
                },
                other.offset,
            )),
        }
    };
    let first = take_ident(&mut index)?;
    let (key, value) = if matches!(tokens[index].tok, Tok::Sym(",")) {
        index += 1;
        let second = take_ident(&mut index)?;
        (Some(first), second)
    } else {
        (None, first)
    };
    match &tokens[index] {
        Token {
            tok: Tok::Ident(word),
            ..
        } if word == "in" => index += 1,
        other => {
            return Err(Fail::new(
                ErrorKind::UnexpectedToken {
                    expected: "`in`",
                    found: other.tok.describe(),
                },
                other.offset,
            ))
        }
    }
    let mut parser = ExprParser::new(&tokens[index..], limit);
    let iterable = parser.parse_expr()?;
    parser.expect_end("end of tag")?;
    Ok((key, value, iterable))
}

/// A recursive-descent expression parser over one tag's tokens.
struct ExprParser<'t> {
    tokens: &'t [Token],
    index: usize,
    depth: usize,
    limit: usize,
}

impl<'t> ExprParser<'t> {
    fn new(tokens: &'t [Token], limit: usize) -> Self {
        Self {
            tokens,
            index: 0,
            depth: 0,
            limit,
        }
    }

    fn peek(&self) -> &'t Token {
        &self.tokens[self.index.min(self.tokens.len() - 1)]
    }

    fn peek_at(&self, ahead: usize) -> &'t Token {
        &self.tokens[(self.index + ahead).min(self.tokens.len() - 1)]
    }

    fn advance(&mut self) -> &'t Token {
        let token = self.peek();
        if self.index < self.tokens.len() - 1 {
            self.index += 1;
        }
        token
    }

    fn is_sym(&self, symbol: &str) -> bool {
        matches!(&self.peek().tok, Tok::Sym(found) if *found == symbol)
    }

    fn is_word(&self, word: &str) -> bool {
        matches!(&self.peek().tok, Tok::Ident(found) if found == word)
    }

    fn unexpected(&self, expected: &'static str) -> RawError {
        let token = self.peek();
        Fail::new(
            ErrorKind::UnexpectedToken {
                expected,
                found: token.tok.describe(),
            },
            token.offset,
        )
    }

    fn expect_end(&self, expected: &'static str) -> PResult<()> {
        if matches!(self.peek().tok, Tok::End(_)) {
            Ok(())
        } else {
            Err(self.unexpected(expected))
        }
    }

    fn expect_sym(&mut self, symbol: &'static str, expected: &'static str) -> PResult<()> {
        if self.is_sym(symbol) {
            self.advance();
            Ok(())
        } else {
            Err(self.unexpected(expected))
        }
    }

    fn enter(&mut self) -> PResult<()> {
        self.depth += 1;
        if self.depth > self.limit {
            return Err(Fail::new(
                ErrorKind::NestingTooDeep { limit: self.limit },
                self.peek().offset,
            ));
        }
        Ok(())
    }

    fn leave(&mut self) {
        self.depth -= 1;
    }

    fn make(&self, kind: ExprKind, offset: usize, children_depth: usize) -> PResult<Expr> {
        Expr::new(kind, offset, children_depth, self.limit)
    }

    fn binary(&self, op: Op, left: Expr, right: Expr) -> PResult<Expr> {
        let offset = left.offset;
        let depth = left.depth().max(right.depth());
        self.make(
            ExprKind::Binary(op, Box::new(left), Box::new(right)),
            offset,
            depth,
        )
    }

    fn parse_expr(&mut self) -> PResult<Expr> {
        self.enter()?;
        let result = self.parse_binary(0);
        self.leave();
        result
    }

    /// The binary operator at the cursor: `(operator, precedence, tokens)`.
    fn peek_binary(&self) -> Option<(Op, u8, usize)> {
        Some(match &self.peek().tok {
            Tok::Sym("==") => (Op::Eq, PREC_CMP, 1),
            Tok::Sym("!=") => (Op::Ne, PREC_CMP, 1),
            Tok::Sym("<") => (Op::Lt, PREC_CMP, 1),
            Tok::Sym("<=") => (Op::Le, PREC_CMP, 1),
            Tok::Sym(">") => (Op::Gt, PREC_CMP, 1),
            Tok::Sym(">=") => (Op::Ge, PREC_CMP, 1),
            Tok::Sym("~") => (Op::Concat, 5, 1),
            Tok::Sym("+") => (Op::Add, 6, 1),
            Tok::Sym("-") => (Op::Sub, 6, 1),
            Tok::Sym("*") => (Op::Mul, 7, 1),
            Tok::Sym("/") => (Op::Div, 7, 1),
            Tok::Sym("%") => (Op::Rem, 7, 1),
            Tok::Ident(word) => match word.as_str() {
                "or" => (Op::Or, 1, 1),
                "and" => (Op::And, 2, 1),
                "in" => (Op::In, PREC_CMP, 1),
                "not" if matches!(&self.peek_at(1).tok, Tok::Ident(next) if next == "in") => {
                    (Op::NotIn, PREC_CMP, 2)
                }
                _ => return None,
            },
            _ => return None,
        })
    }

    /// Precedence climbing: one loop for all binary operators, so a nested
    /// expression costs a handful of stack frames instead of one per level.
    fn parse_binary(&mut self, min_prec: u8) -> PResult<Expr> {
        let mut left = self.parse_prefix()?;
        while let Some((op, prec, width)) = self.peek_binary() {
            if prec < min_prec {
                break;
            }
            for _ in 0..width {
                self.advance();
            }
            let right = self.parse_binary(prec + 1)?;
            left = self.binary(op, left, right)?;
        }
        Ok(left)
    }

    fn parse_prefix(&mut self) -> PResult<Expr> {
        if self.is_word("not") {
            let offset = self.advance().offset;
            self.enter()?;
            let inner = self.parse_binary(PREC_CMP);
            self.leave();
            let inner = inner?;
            let depth = inner.depth();
            return self.make(ExprKind::Not(Box::new(inner)), offset, depth);
        }
        if self.is_sym("-") {
            let offset = self.advance().offset;
            self.enter()?;
            let inner = self.parse_prefix();
            self.leave();
            let inner = inner?;
            let depth = inner.depth();
            return self.make(ExprKind::Neg(Box::new(inner)), offset, depth);
        }
        self.parse_filtered()
    }

    fn parse_filtered(&mut self) -> PResult<Expr> {
        let mut expr = self.parse_postfix()?;
        while self.is_sym("|") {
            self.advance();
            let Token {
                tok: Tok::Ident(name),
                offset: name_offset,
            } = self.peek()
            else {
                return Err(self.unexpected("a filter name"));
            };
            let (name, name_offset) = (name.clone(), *name_offset);
            self.advance();
            let mut args = Vec::new();
            if self.is_sym("(") {
                self.advance();
                self.enter()?;
                if !self.is_sym(")") {
                    loop {
                        args.push(self.parse_expr()?);
                        if self.is_sym(",") {
                            self.advance();
                        } else {
                            break;
                        }
                    }
                }
                self.leave();
                self.expect_sym(")", "`)`")?;
            }
            let offset = expr.offset;
            let depth = args
                .iter()
                .map(Expr::depth)
                .chain(std::iter::once(expr.depth()))
                .max()
                .unwrap_or(0);
            expr = self.make(
                ExprKind::Filter(Box::new(FilterCall {
                    input: expr,
                    name,
                    name_offset,
                    args,
                })),
                offset,
                depth,
            )?;
        }
        Ok(expr)
    }

    fn parse_postfix(&mut self) -> PResult<Expr> {
        let mut expr = self.parse_primary()?;
        loop {
            if self.is_sym(".") {
                self.advance();
                let offset = expr.offset;
                let depth = expr.depth();
                match &self.peek().tok {
                    Tok::Ident(name) => {
                        let name = name.clone();
                        self.advance();
                        expr = self.make(ExprKind::Attr(Box::new(expr), name), offset, depth)?;
                    }
                    Tok::Int(index) => {
                        let literal_offset = self.peek().offset;
                        let literal =
                            self.make(ExprKind::Lit(Value::Int(*index)), literal_offset, 0)?;
                        self.advance();
                        let depth = depth.max(literal.depth());
                        expr = self.make(
                            ExprKind::Index(Box::new(expr), Box::new(literal)),
                            offset,
                            depth,
                        )?;
                    }
                    _ => return Err(self.unexpected("an attribute name")),
                }
            } else if self.is_sym("[") {
                self.advance();
                let offset = expr.offset;
                let index = self.parse_expr()?;
                self.expect_sym("]", "`]`")?;
                let depth = expr.depth().max(index.depth());
                expr = self.make(
                    ExprKind::Index(Box::new(expr), Box::new(index)),
                    offset,
                    depth,
                )?;
            } else {
                return Ok(expr);
            }
        }
    }

    fn parse_primary(&mut self) -> PResult<Expr> {
        let token = self.peek();
        let offset = token.offset;
        match &token.tok {
            Tok::Int(value) => {
                let value = Value::Int(*value);
                self.advance();
                self.make(ExprKind::Lit(value), offset, 0)
            }
            Tok::Float(value) => {
                let value = Value::Float(*value);
                self.advance();
                self.make(ExprKind::Lit(value), offset, 0)
            }
            Tok::Str(value) => {
                let value = Value::Str(value.clone());
                self.advance();
                self.make(ExprKind::Lit(value), offset, 0)
            }
            Tok::Ident(word) => match word.as_str() {
                "true" | "false" => {
                    let value = Value::Bool(word == "true");
                    self.advance();
                    self.make(ExprKind::Lit(value), offset, 0)
                }
                "none" => {
                    self.advance();
                    self.make(ExprKind::Lit(Value::Null), offset, 0)
                }
                "and" | "or" | "not" | "in" => Err(self.unexpected("an expression")),
                "super" if matches!(self.peek_at(1).tok, Tok::Sym("(")) => {
                    self.advance();
                    self.advance();
                    self.expect_sym(")", "`)`")?;
                    self.make(ExprKind::Super, offset, 0)
                }
                _ => {
                    let name = word.clone();
                    self.advance();
                    self.make(ExprKind::Var(name), offset, 0)
                }
            },
            Tok::Sym("(") => {
                self.advance();
                let inner = self.parse_expr()?;
                self.expect_sym(")", "`)`")?;
                Ok(inner)
            }
            Tok::Sym("[") => {
                self.advance();
                self.enter()?;
                let mut items = Vec::new();
                if !self.is_sym("]") {
                    loop {
                        items.push(self.parse_expr()?);
                        if self.is_sym(",") {
                            self.advance();
                            if self.is_sym("]") {
                                break;
                            }
                        } else {
                            break;
                        }
                    }
                }
                self.leave();
                self.expect_sym("]", "`]`")?;
                let depth = items.iter().map(Expr::depth).max().unwrap_or(0);
                self.make(ExprKind::List(items), offset, depth)
            }
            _ => Err(self.unexpected("an expression")),
        }
    }
}
