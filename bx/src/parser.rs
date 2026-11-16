use crate::ast::{BinOp, Expr, Stmt};
use crate::lexer::{lex, Tok, Tokens};

pub fn parse(src: &str) -> Result<Vec<Stmt>, String> {
    let src = strip_shebang(src);
    let toks = lex(&src)?;
    let mut p = Parser { toks, pos: 0 };
    p.parse_program()
}

/// Blank out a leading `#!` line so a script can be executed directly on Unix
/// (`#!/usr/bin/env bx`). The line keeps its bytes, so line numbers do not move.
fn strip_shebang(src: &str) -> std::borrow::Cow<'_, str> {
    if !src.starts_with("#!") {
        return std::borrow::Cow::Borrowed(src);
    }
    let first_end = src.find('\n').unwrap_or(src.len());
    let mut masked = String::with_capacity(src.len());
    masked.push_str(&" ".repeat(first_end));
    masked.push_str(&src[first_end..]);
    std::borrow::Cow::Owned(masked)
}

struct Parser {
    toks: Tokens,
    pos: usize,
}

const POSTFIX_BP: u8 = 15;
const UNARY_BP: u8 = 13;

impl Parser {
    fn new(toks: Tokens) -> Parser {
        Parser { toks, pos: 0 }
    }

    fn peek(&self) -> &Tok {
        &self.toks[self.pos].0
    }

    fn peek_at(&self, n: usize) -> &Tok {
        let i = (self.pos + n).min(self.toks.len() - 1);
        &self.toks[i].0
    }

    fn line(&self) -> u32 {
        self.toks[self.pos].1
    }

    fn advance(&mut self) -> Tok {
        let t = self.toks[self.pos].0.clone();
        if self.pos + 1 < self.toks.len() {
            self.pos += 1;
        }
        t
    }

    fn at_eof(&self) -> bool {
        matches!(self.peek(), Tok::Eof)
    }

    fn is_kw(&self, kw: &str) -> bool {
        matches!(self.peek(), Tok::Ident(s) if s == kw)
    }

    fn accept(&mut self, t: &Tok) -> bool {
        if self.peek() == t {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, t: &Tok, what: &str) -> Result<(), String> {
        if self.accept(t) {
            Ok(())
        } else {
            Err(format!(
                "line {}: expected {}, found {}",
                self.line(),
                what,
                describe(self.peek())
            ))
        }
    }

    fn skip_newlines(&mut self) {
        while matches!(self.peek(), Tok::Newline) {
            self.advance();
        }
    }

    fn skip_seps(&mut self) {
        while matches!(self.peek(), Tok::Newline | Tok::Semi) {
            self.advance();
        }
    }

    fn end_simple(&mut self) -> Result<(), String> {
        let mut saw = false;
        while matches!(self.peek(), Tok::Newline | Tok::Semi) {
            self.advance();
            saw = true;
        }
        match self.peek() {
            Tok::RBrace | Tok::Eof => Ok(()),
            _ if saw => Ok(()),
            _ => Err(format!(
                "line {}: expected newline or ';' before {}",
                self.line(),
                describe(self.peek())
            )),
        }
    }

    fn ident(&mut self) -> Result<String, String> {
        match self.peek().clone() {
            Tok::Ident(s) => {
                self.advance();
                Ok(s)
            }
            other => Err(format!(
                "line {}: expected identifier, found {}",
                self.line(),
                describe(&other)
            )),
        }
    }

    fn parse_program(&mut self) -> Result<Vec<Stmt>, String> {
        let mut stmts = Vec::new();
        loop {
            self.skip_seps();
            if self.at_eof() {
                break;
            }
            stmts.push(self.parse_stmt()?);
        }
        Ok(stmts)
    }

    fn parse_block(&mut self) -> Result<Vec<Stmt>, String> {
        self.expect(&Tok::LBrace, "'{'")?;
        let mut stmts = Vec::new();
        loop {
            self.skip_seps();
            if matches!(self.peek(), Tok::RBrace) {
                self.advance();
                break;
            }
            if self.at_eof() {
                return Err(format!(
                    "line {}: unexpected end of input, missing '}}'",
                    self.line()
                ));
            }
            stmts.push(self.parse_stmt()?);
        }
        Ok(stmts)
    }

    fn parse_stmt(&mut self) -> Result<Stmt, String> {
        let line = self.line();
        if self.is_kw("let") || self.is_kw("const") {
            self.advance();
            let name = self.ident()?;
            let init = if self.accept(&Tok::Assign) {
                self.skip_newlines();
                Some(self.parse_expr()?)
            } else {
                None
            };
            self.end_simple()?;
            return Ok(Stmt::Let(name, init, line));
        }
        if self.is_kw("fn") {
            if matches!(self.peek_at(1), Tok::LParen) {
                // anonymous fn as an expression statement
                let e = self.parse_expr()?;
                self.end_simple()?;
                return Ok(Stmt::Expr(e, line));
            }
            self.advance();
            let name = self.ident()?;
            let params = self.parse_params()?;
            let body = self.parse_block()?;
            return Ok(Stmt::Fn(name, params, body, line));
        }
        if self.is_kw("if") {
            self.advance();
            let cond = self.parse_cond()?;
            let then = self.parse_block()?;
            let save = self.pos;
            self.skip_newlines();
            let els = if self.is_kw("else") {
                self.advance();
                if self.is_kw("if") {
                    let nested = self.parse_stmt()?;
                    Some(vec![nested])
                } else {
                    Some(self.parse_block()?)
                }
            } else {
                self.pos = save;
                None
            };
            return Ok(Stmt::If(cond, then, els, line));
        }
        if self.is_kw("while") {
            self.advance();
            let cond = self.parse_cond()?;
            let body = self.parse_block()?;
            return Ok(Stmt::While(cond, body, line));
        }
        if self.is_kw("for") {
            return self.parse_for(line);
        }
        if self.is_kw("return") {
            self.advance();
            if matches!(
                self.peek(),
                Tok::Newline | Tok::Semi | Tok::RBrace | Tok::Eof
            ) {
                self.end_simple()?;
                return Ok(Stmt::Return(None, line));
            }
            let e = self.parse_expr()?;
            self.end_simple()?;
            return Ok(Stmt::Return(Some(e), line));
        }
        if self.is_kw("break") {
            self.advance();
            self.end_simple()?;
            return Ok(Stmt::Break(line));
        }
        if self.is_kw("continue") {
            self.advance();
            self.end_simple()?;
            return Ok(Stmt::Continue(line));
        }
        if self.is_kw("use") {
            self.advance();
            let path = match self.peek().clone() {
                Tok::Str(s) => {
                    self.advance();
                    s
                }
                _ => return Err(format!("line {}: 'use' expects a string path", line)),
            };
            self.end_simple()?;
            return Ok(Stmt::Use(path, line));
        }
        if matches!(self.peek(), Tok::LBrace) {
            let body = self.parse_block()?;
            return Ok(Stmt::Block(body));
        }
        let e = self.parse_expr()?;
        self.end_simple()?;
        Ok(Stmt::Expr(e, line))
    }

    fn parse_cond(&mut self) -> Result<Expr, String> {
        if matches!(self.peek(), Tok::LParen) {
            let save = self.pos;
            self.advance();
            let e = self.parse_expr()?;
            if self.accept(&Tok::RParen) {
                return Ok(e);
            }
            self.pos = save;
        }
        self.parse_expr()
    }

    fn parse_params(&mut self) -> Result<Vec<String>, String> {
        self.expect(&Tok::LParen, "'('")?;
        let mut params = Vec::new();
        self.skip_newlines();
        if self.accept(&Tok::RParen) {
            return Ok(params);
        }
        loop {
            self.skip_newlines();
            params.push(self.ident()?);
            self.skip_newlines();
            if self.accept(&Tok::Comma) {
                continue;
            }
            self.expect(&Tok::RParen, "')'")?;
            break;
        }
        Ok(params)
    }

    fn parse_for(&mut self, line: u32) -> Result<Stmt, String> {
        self.advance(); // for
        let paren = self.accept(&Tok::LParen);
        self.skip_newlines();

        // for x in xs  |  for k, v in xs  |  for let x in xs  |  ... with parens
        let let_kw = self.is_kw("let") || self.is_kw("const");
        let probe = self.pos + if let_kw { 1 } else { 0 };
        let at_ident = matches!(self.toks.get(probe).map(|t| &t.0), Some(Tok::Ident(_)));
        let at_comma = matches!(self.toks.get(probe + 1).map(|t| &t.0), Some(Tok::Comma));
        let at_value = matches!(self.toks.get(probe + 2).map(|t| &t.0), Some(Tok::Ident(_)));
        let in_at =
            |i: usize| matches!(self.toks.get(i).map(|t| &t.0), Some(Tok::Ident(s)) if s == "in");
        let two_names = at_ident && at_comma && at_value;
        let is_forin = if two_names {
            in_at(probe + 3)
        } else {
            at_ident && in_at(probe + 1)
        };
        if is_forin {
            if let_kw {
                self.advance();
            }
            let key = self.ident()?;
            let value = if self.accept(&Tok::Comma) {
                Some(self.ident()?)
            } else {
                None
            };
            self.advance(); // in
            self.skip_newlines();
            let iter = self.parse_expr()?;
            self.skip_newlines();
            if paren {
                self.expect(&Tok::RParen, "')'")?;
            }
            let body = self.parse_block()?;
            return Ok(Stmt::ForIn(key, value, iter, body, line));
        }

        if !paren {
            return Err(format!("line {}: expected 'in' or '(' in for header", line));
        }

        let init: Option<Box<Stmt>> = if self.is_kw("let") || self.is_kw("const") {
            let l = self.line();
            self.advance();
            let name = self.ident()?;
            let initv = if self.accept(&Tok::Assign) {
                self.skip_newlines();
                Some(self.parse_expr()?)
            } else {
                None
            };
            Some(Box::new(Stmt::Let(name, initv, l)))
        } else if matches!(self.peek(), Tok::Semi) {
            None
        } else {
            let l = self.line();
            Some(Box::new(Stmt::Expr(self.parse_expr()?, l)))
        };
        self.expect(&Tok::Semi, "';' in for header")?;
        self.skip_newlines();
        let cond = if matches!(self.peek(), Tok::Semi) {
            None
        } else {
            Some(self.parse_expr()?)
        };
        self.expect(&Tok::Semi, "';' in for header")?;
        self.skip_newlines();
        let step = if matches!(self.peek(), Tok::RParen) {
            None
        } else {
            Some(self.parse_expr()?)
        };
        self.skip_newlines();
        self.expect(&Tok::RParen, "')'")?;
        let body = self.parse_block()?;
        Ok(Stmt::ForC(init, cond, step, body, line))
    }
    fn parse_args(&mut self) -> Result<(Vec<Expr>, u32), String> {
        let line = self.line();
        self.expect(&Tok::LParen, "'('")?;
        let mut args = Vec::new();
        self.skip_newlines();
        if self.accept(&Tok::RParen) {
            return Ok((args, line));
        }
        loop {
            self.skip_newlines();
            args.push(self.parse_expr()?);
            self.skip_newlines();
            if self.accept(&Tok::Comma) {
                self.skip_newlines();
                if self.accept(&Tok::RParen) {
                    break;
                }
                continue;
            }
            self.expect(&Tok::RParen, "')'")?;
            break;
        }
        Ok((args, line))
    }

    fn parse_expr(&mut self) -> Result<Expr, String> {
        self.parse_bp(0)
    }

    fn parse_bp(&mut self, min_bp: u8) -> Result<Expr, String> {
        let mut lhs = self.parse_prefix()?;

        loop {
            let line = self.line();
            match self.peek().clone() {
                Tok::LParen if POSTFIX_BP >= min_bp => {
                    let (args, cline) = self.parse_args()?;
                    lhs = match lhs {
                        Expr::Member(obj, name, l) => {
                            Expr::MethodCall(obj, name, args, if l == 0 { cline } else { l })
                        }
                        other => Expr::Call(Box::new(other), args, cline),
                    };
                    continue;
                }
                Tok::LBracket if POSTFIX_BP >= min_bp => {
                    self.advance();
                    self.skip_newlines();
                    let idx = self.parse_expr()?;
                    self.skip_newlines();
                    self.expect(&Tok::RBracket, "']'")?;
                    lhs = Expr::Index(Box::new(lhs), Box::new(idx), line);
                    continue;
                }
                Tok::Dot if POSTFIX_BP >= min_bp => {
                    self.advance();
                    let name = self.ident()?;
                    lhs = Expr::Member(Box::new(lhs), name, line);
                    continue;
                }
                Tok::PlusPlus | Tok::MinusMinus if POSTFIX_BP >= min_bp => {
                    if !lhs.is_lvalue() {
                        return Err(format!("line {}: '++'/'--' needs a variable", line));
                    }
                    let op = if matches!(self.peek(), Tok::PlusPlus) {
                        BinOp::Add
                    } else {
                        BinOp::Sub
                    };
                    self.advance();
                    lhs = Expr::Compound(Box::new(lhs), op, Box::new(Expr::Int(1)), line);
                    continue;
                }
                _ => {}
            }

            if min_bp == 0 {
                match self.peek().clone() {
                    Tok::Assign => {
                        if !lhs.is_lvalue() {
                            return Err(format!("line {}: invalid assignment target", line));
                        }
                        self.advance();
                        self.skip_newlines();
                        let rhs = self.parse_bp(0)?;
                        lhs = Expr::Assign(Box::new(lhs), Box::new(rhs), line);
                        continue;
                    }
                    Tok::PlusEq | Tok::MinusEq | Tok::StarEq | Tok::SlashEq | Tok::PercentEq => {
                        let op = match self.peek() {
                            Tok::PlusEq => BinOp::Add,
                            Tok::MinusEq => BinOp::Sub,
                            Tok::StarEq => BinOp::Mul,
                            Tok::SlashEq => BinOp::Div,
                            _ => BinOp::Mod,
                        };
                        if !lhs.is_lvalue() {
                            return Err(format!("line {}: invalid assignment target", line));
                        }
                        self.advance();
                        self.skip_newlines();
                        let rhs = self.parse_bp(0)?;
                        lhs = Expr::Compound(Box::new(lhs), op, Box::new(rhs), line);
                        continue;
                    }
                    _ => {}
                }
            }

            // `cond ? then : else`, right-associative, looser than `||` and
            // tighter than assignment (which is handled above at min_bp == 0).
            if matches!(self.peek(), Tok::Question) && 2 >= min_bp {
                self.advance();
                self.skip_newlines();
                let then_branch = self.parse_bp(0)?;
                self.skip_newlines();
                self.expect(&Tok::Colon, "':' in a conditional expression")?;
                self.skip_newlines();
                let else_branch = self.parse_bp(1)?;
                lhs = Expr::Ternary(
                    Box::new(lhs),
                    Box::new(then_branch),
                    Box::new(else_branch),
                    line,
                );
                continue;
            }

            let (lbp, rbp) = match binding(self.peek()) {
                Some(v) => v,
                None => break,
            };
            if lbp < min_bp {
                break;
            }
            let tok = self.advance();
            self.skip_newlines();
            let rhs = self.parse_bp(rbp)?;
            lhs = match tok {
                Tok::Plus => Expr::Bin(BinOp::Add, Box::new(lhs), Box::new(rhs)),
                Tok::Minus => Expr::Bin(BinOp::Sub, Box::new(lhs), Box::new(rhs)),
                Tok::Star => Expr::Bin(BinOp::Mul, Box::new(lhs), Box::new(rhs)),
                Tok::Slash => Expr::Bin(BinOp::Div, Box::new(lhs), Box::new(rhs)),
                Tok::Percent => Expr::Bin(BinOp::Mod, Box::new(lhs), Box::new(rhs)),
                Tok::Eq => Expr::Bin(BinOp::Eq, Box::new(lhs), Box::new(rhs)),
                Tok::Ne => Expr::Bin(BinOp::Ne, Box::new(lhs), Box::new(rhs)),
                Tok::Lt => Expr::Bin(BinOp::Lt, Box::new(lhs), Box::new(rhs)),
                Tok::Le => Expr::Bin(BinOp::Le, Box::new(lhs), Box::new(rhs)),
                Tok::Gt => Expr::Bin(BinOp::Gt, Box::new(lhs), Box::new(rhs)),
                Tok::Ge => Expr::Bin(BinOp::Ge, Box::new(lhs), Box::new(rhs)),
                Tok::AndAnd => Expr::And(Box::new(lhs), Box::new(rhs)),
                Tok::OrOr => Expr::Or(Box::new(lhs), Box::new(rhs)),
                _ => unreachable!(),
            };
        }

        Ok(lhs)
    }

    fn parse_prefix(&mut self) -> Result<Expr, String> {
        self.skip_newlines();
        let line = self.line();
        let t = self.advance();
        let e = match t {
            Tok::Int(i) => Expr::Int(i),
            Tok::Float(f) => Expr::Float(f),
            Tok::Str(s) => interpolate(s, line)?,
            Tok::Minus => {
                let operand = self.parse_bp(UNARY_BP)?;
                // fold negative literals so `-3` is a constant
                match operand {
                    Expr::Int(i) => Expr::Int(-i),
                    Expr::Float(f) => Expr::Float(-f),
                    other => Expr::Neg(Box::new(other)),
                }
            }
            Tok::Bang => {
                let operand = self.parse_bp(UNARY_BP)?;
                Expr::Not(Box::new(operand))
            }
            Tok::LParen => {
                self.skip_newlines();
                let inner = self.parse_expr()?;
                self.skip_newlines();
                self.expect(&Tok::RParen, "')'")?;
                inner
            }
            Tok::LBracket => {
                let mut items = Vec::new();
                self.skip_newlines();
                if self.accept(&Tok::RBracket) {
                    Expr::Array(items)
                } else {
                    loop {
                        self.skip_newlines();
                        items.push(self.parse_expr()?);
                        self.skip_newlines();
                        if self.accept(&Tok::Comma) {
                            self.skip_newlines();
                            if self.accept(&Tok::RBracket) {
                                break;
                            }
                            continue;
                        }
                        self.expect(&Tok::RBracket, "']'")?;
                        break;
                    }
                    Expr::Array(items)
                }
            }
            Tok::LBrace => {
                let mut entries: Vec<(String, Expr)> = Vec::new();
                self.skip_newlines();
                if self.accept(&Tok::RBrace) {
                    Expr::Object(entries)
                } else {
                    loop {
                        self.skip_newlines();
                        let key = match self.peek().clone() {
                            Tok::Ident(s) => {
                                self.advance();
                                s
                            }
                            Tok::Str(s) => {
                                self.advance();
                                s
                            }
                            Tok::Int(i) => {
                                self.advance();
                                i.to_string()
                            }
                            other => {
                                return Err(format!(
                                    "line {}: expected object key, found {}",
                                    self.line(),
                                    describe(&other)
                                ))
                            }
                        };
                        self.expect(&Tok::Colon, "':' in object literal")?;
                        self.skip_newlines();
                        let val = self.parse_expr()?;
                        entries.push((key, val));
                        self.skip_newlines();
                        if self.accept(&Tok::Comma) {
                            self.skip_newlines();
                            if self.accept(&Tok::RBrace) {
                                break;
                            }
                            continue;
                        }
                        self.expect(&Tok::RBrace, "'}'")?;
                        break;
                    }
                    Expr::Object(entries)
                }
            }
            Tok::Ident(name) => match name.as_str() {
                "true" => Expr::Bool(true),
                "false" => Expr::Bool(false),
                "null" => Expr::Null,
                "fn" => {
                    let params = self.parse_params()?;
                    let body = self.parse_block()?;
                    Expr::Func(params, body, line)
                }
                _ => Expr::Ident(name),
            },
            other => return Err(format!("line {}: unexpected {}", line, describe(&other))),
        };
        Ok(e)
    }
}

fn binding(t: &Tok) -> Option<(u8, u8)> {
    match t {
        Tok::OrOr => Some((1, 2)),
        Tok::AndAnd => Some((3, 4)),
        Tok::Eq | Tok::Ne => Some((5, 6)),
        Tok::Lt | Tok::Le | Tok::Gt | Tok::Ge => Some((7, 8)),
        Tok::Plus | Tok::Minus => Some((9, 10)),
        Tok::Star | Tok::Slash | Tok::Percent => Some((11, 12)),
        _ => None,
    }
}

fn describe(t: &Tok) -> String {
    match t {
        Tok::Ident(s) => format!("'{}'", s),
        Tok::Int(i) => format!("number {}", i),
        Tok::Float(f) => format!("number {}", f),
        Tok::Str(s) => format!("string {:?}", s),
        Tok::Newline => "end of line".to_string(),
        Tok::Eof => "end of input".to_string(),
        other => format!("'{}'", token_text(other)),
    }
}

fn token_text(t: &Tok) -> &'static str {
    match t {
        Tok::Plus => "+",
        Tok::Minus => "-",
        Tok::Star => "*",
        Tok::Slash => "/",
        Tok::Percent => "%",
        Tok::Assign => "=",
        Tok::Eq => "==",
        Tok::Ne => "!=",
        Tok::Lt => "<",
        Tok::Le => "<=",
        Tok::Gt => ">",
        Tok::Ge => ">=",
        Tok::AndAnd => "&&",
        Tok::OrOr => "||",
        Tok::Bang => "!",
        Tok::PlusEq => "+=",
        Tok::MinusEq => "-=",
        Tok::StarEq => "*=",
        Tok::SlashEq => "/=",
        Tok::PercentEq => "%=",
        Tok::PlusPlus => "++",
        Tok::MinusMinus => "--",
        Tok::LParen => "(",
        Tok::RParen => ")",
        Tok::LBrace => "{",
        Tok::RBrace => "}",
        Tok::LBracket => "[",
        Tok::RBracket => "]",
        Tok::Comma => ",",
        Tok::Semi => ";",
        Tok::Colon => ":",
        Tok::Dot => ".",
        _ => "?",
    }
}

/// Expand `"a ${expr} b"` into a `+` chain. The inner source is parsed as a
/// full expression, so method calls and nested strings work; braces inside
/// quotes do not close the interpolation.
fn interpolate(text: String, line: u32) -> Result<Expr, String> {
    if !text.contains("${") {
        return Ok(Expr::Str(text.replace('\u{1}', "$")));
    }
    let chars: Vec<char> = text.chars().collect();
    let mut parts: Vec<Expr> = Vec::new();
    let mut literal = String::new();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '\u{1}' {
            literal.push('$');
            i += 1;
        } else if chars[i] == '$' && i + 1 < chars.len() && chars[i + 1] == '{' {
            if !literal.is_empty() {
                parts.push(Expr::Str(std::mem::take(&mut literal)));
            }
            let mut depth = 1usize;
            let mut j = i + 2;
            let mut quote: Option<char> = None;
            while j < chars.len() {
                let c = chars[j];
                if let Some(q) = quote {
                    if c == '\\' {
                        j += 2;
                        continue;
                    }
                    if c == q {
                        quote = None;
                    }
                } else if c == '"' || c == '\'' {
                    quote = Some(c);
                } else if c == '{' {
                    depth += 1;
                } else if c == '}' {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                j += 1;
            }
            if j >= chars.len() {
                return Err(format!("line {line}: unterminated string interpolation"));
            }
            let inner: String = chars[i + 2..j].iter().collect();
            if inner.trim().is_empty() {
                return Err(format!("line {line}: empty string interpolation"));
            }
            let inner_toks = lex(&inner).map_err(|e| format!("line {line}: {e}"))?;
            let mut sub = Parser::new(inner_toks);
            sub.skip_newlines();
            let expr = sub.parse_expr()?;
            sub.skip_newlines();
            if !sub.at_eof() {
                return Err(format!(
                    "line {line}: unexpected text after the interpolated expression"
                ));
            }
            parts.push(expr);
            i = j + 1;
        } else {
            literal.push(chars[i]);
            i += 1;
        }
    }
    if !literal.is_empty() {
        parts.push(Expr::Str(literal));
    }
    let mut iter = parts.into_iter();
    let mut acc = iter
        .next()
        .ok_or_else(|| format!("line {line}: empty string"))?;
    for part in iter {
        acc = Expr::Bin(BinOp::Add, Box::new(acc), Box::new(part));
    }
    Ok(acc)
}
