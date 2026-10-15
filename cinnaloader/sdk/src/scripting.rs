//! Scripting: a small language a mod ships, lexed, parsed and interpreted here for real, a
//! host function registry, an instruction budget, and the client's own scripting layer
//! refused.
//!
//! The language is deliberately small and complete:
//!
//! * **Lexer.** Numbers (`12`, `-3`, `1.5`), single- and double-quoted strings with
//!   `\n \t \r \\ \" \' \0`, identifiers, the operators `+ - * / %`, `== != < <= > >=`, the
//!   words `and or not`, the punctuation `( ) { } , ;`, and the keywords `if else while let
//!   return true false`. A character it does not know is refused with its position.
//! * **Parser.** Precedence from loosest to tightest is `or`, `and`, `not`, `== !=`,
//!   `< <= > >=`, `+ -`, `* / %`, unary `-`, then a call or a primary. Every binary operator
//!   is left-associative and every unary one right-associative, so `1 + 2 * 3` is `7` and
//!   `(1 + 2) * 3` is `9`. A malformed program is refused with the position it gave up at.
//! * **Evaluator.** Bindings live in scopes that nest and shadow; a block gets its own scope.
//!   Integers and floats mix by promotion: `int op int` stays an integer, and if either side
//!   is a float the result is a float. `/` between two integers is integer division, and `%`
//!   is the remainder. `+` concatenates when either side is a string. `and` and `or`
//!   short-circuit, so the right hand side of `false and x` is never evaluated. Comparing
//!   numbers compares numerically across the two types; comparing strings compares
//!   lexicographically. Dividing by zero is a real error, never an infinity.
//! * **Host functions.** [`register_function`] names a Rust closure and the arity it takes,
//!   and a script calls it by name. An unknown name is refused by name, and a wrong argument
//!   count by name and count.
//! * **A budget.** Every step is counted against [`set_instruction_limit`], because a script
//!   is data and data can loop forever: without the limit a `while true {}` would spin the
//!   loader's thread until the process died. The run stops with [`ScriptError::Budget`],
//!   naming the step it stopped at. [`set_memory_limit`] caps the source and values a
//!   runtime accepts, [`set_execution_timeout`] stops a run that outlasts its wall clock, and
//!   [`interrupt`] stops the next run before it starts.
//!
//! [`eval`] answers the script's value or a positioned [`ScriptError`]; [`eval_with`] seeds
//! bindings first. [`create_runtime`] and [`load_script`] keep named, already-parsed scripts
//! for a caller that ships several.
//!
//! What is *not* here is the client. Running a script inside the client's own process, and
//! reading or extending the client's own scripting layer, are refused, each with its own
//! sentence in [`why_unknown`] and listed by [`refusals`].
//!
//! ```
//! use cinnaloader::scripting::{self, Value};
//!
//! assert_eq!(scripting::eval("1 + 2 * 3").expect("a value"), Value::Int(7));
//! assert_eq!(scripting::eval("(1 + 2) * 3").expect("a value"), Value::Int(9));
//! assert_eq!(
//!     scripting::eval_with(
//!         "greeting + name",
//!         &[("greeting", Value::str("hi ")), ("name", Value::str("there"))],
//!     )
//!     .expect("a value"),
//!     Value::str("hi there"),
//! );
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// The step budget a runtime starts with, so a `while true {}` cannot hang the process.
pub const DEFAULT_INSTRUCTION_LIMIT: u64 = 1_000_000;

/// One script value. Integers and floats are distinct, and a comparison sees across them.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
}

impl Value {
    #[must_use]
    pub fn str(text: impl Into<String>) -> Self {
        Self::Str(text.into())
    }

    #[must_use]
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Bool(_) => "boolean",
            Self::Int(_) => "integer",
            Self::Float(_) => "float",
            Self::Str(_) => "string",
        }
    }

    #[must_use]
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(number) => Some(*number),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_float(&self) -> Option<f64> {
        match self {
            Self::Int(number) => Some(*number as f64),
            Self::Float(number) => Some(*number),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(flag) => Some(*flag),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(text) => Some(text),
            _ => None,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => formatter.write_str("null"),
            Self::Bool(flag) => write!(formatter, "{flag}"),
            Self::Int(number) => write!(formatter, "{number}"),
            Self::Float(number) => write!(formatter, "{number}"),
            Self::Str(text) => formatter.write_str(text),
        }
    }
}

/// Anything the lexer, parser or evaluator refuses, with the position it gave up at where
/// one applies.
#[derive(Debug, Clone, PartialEq)]
pub enum ScriptError {
    Syntax { pos: usize, msg: String },
    UndefinedName { name: String, pos: usize },
    Type { pos: usize, msg: String },
    Arithmetic { pos: usize, msg: String },
    DivideByZero { pos: usize },
    UnknownFunction { name: String, pos: usize },
    Arity { name: String, expected: usize, got: usize, pos: usize },
    Budget { steps: u64, limit: u64 },
    Timeout { millis: u64 },
    Memory { limit: usize },
    Interrupted,
    UnknownRuntime(u64),
    UnknownScript { runtime: u64, name: String },
    Host { name: String, msg: String },
    Refused(String),
}

impl ScriptError {
    /// The byte offset the error is about, when it has one.
    #[must_use]
    pub fn pos(&self) -> Option<usize> {
        match self {
            Self::Syntax { pos, .. }
            | Self::UndefinedName { pos, .. }
            | Self::Type { pos, .. }
            | Self::Arithmetic { pos, .. }
            | Self::DivideByZero { pos }
            | Self::UnknownFunction { pos, .. }
            | Self::Arity { pos, .. } => Some(*pos),
            _ => None,
        }
    }
}

impl fmt::Display for ScriptError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax { pos, msg } => write!(formatter, "at {pos}: {msg}"),
            Self::UndefinedName { name, pos } => write!(formatter, "at {pos}: {name} is not bound"),
            Self::Type { pos, msg } => write!(formatter, "at {pos}: {msg}"),
            Self::Arithmetic { pos, msg } => write!(formatter, "at {pos}: {msg}"),
            Self::DivideByZero { pos } => write!(formatter, "at {pos}: division by zero"),
            Self::UnknownFunction { name, pos } => {
                write!(formatter, "at {pos}: function {name} is not registered")
            }
            Self::Arity {
                name,
                expected,
                got,
                pos,
            } => write!(
                formatter,
                "at {pos}: function {name} takes {expected} arguments, got {got}"
            ),
            Self::Budget { steps, limit } => {
                write!(formatter, "the run hit the {limit}-step budget after {steps} steps")
            }
            Self::Timeout { millis } => {
                write!(formatter, "the run passed its {millis}ms execution timeout")
            }
            Self::Memory { limit } => {
                write!(formatter, "the source or value is over the {limit}-byte memory limit")
            }
            Self::Interrupted => formatter.write_str("the run was interrupted"),
            Self::UnknownRuntime(id) => write!(formatter, "runtime {id} does not exist"),
            Self::UnknownScript { runtime, name } => {
                write!(formatter, "runtime {runtime} has no script named {name}")
            }
            Self::Host { name, msg } => {
                write!(formatter, "the host function {name} failed: {msg}")
            }
            Self::Refused(why) => formatter.write_str(why),
        }
    }
}

impl std::error::Error for ScriptError {}

/// What the runtimes and the registry hold, for a caller that wants a readout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RuntimeStats {
    pub runtimes: usize,
    pub scripts: usize,
    pub functions: usize,
    pub executions: u64,
    pub steps: u64,
    pub errors: usize,
    pub interruptions: u64,
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(Value),
    Str(String),
    Ident(String),
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    EqEq,
    NotEq,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    Not,
    LParen,
    RParen,
    LBrace,
    RBrace,
    Comma,
    Semi,
    Eq,
    If,
    Else,
    While,
    Let,
    Return,
    True,
    False,
    Eof,
}

impl Tok {
    fn describe(&self) -> String {
        match self {
            Self::Num(value) => format!("the number {value}"),
            Self::Str(_) => "a string".to_owned(),
            Self::Ident(name) => format!("the name {name}"),
            Self::Plus => "+".to_owned(),
            Self::Minus => "-".to_owned(),
            Self::Star => "*".to_owned(),
            Self::Slash => "/".to_owned(),
            Self::Percent => "%".to_owned(),
            Self::EqEq => "==".to_owned(),
            Self::NotEq => "!=".to_owned(),
            Self::Lt => "<".to_owned(),
            Self::Le => "<=".to_owned(),
            Self::Gt => ">".to_owned(),
            Self::Ge => ">=".to_owned(),
            Self::And => "and".to_owned(),
            Self::Or => "or".to_owned(),
            Self::Not => "not".to_owned(),
            Self::LParen => "(".to_owned(),
            Self::RParen => ")".to_owned(),
            Self::LBrace => "{".to_owned(),
            Self::RBrace => "}".to_owned(),
            Self::Comma => ",".to_owned(),
            Self::Semi => ";".to_owned(),
            Self::Eq => "=".to_owned(),
            Self::If => "if".to_owned(),
            Self::Else => "else".to_owned(),
            Self::While => "while".to_owned(),
            Self::Let => "let".to_owned(),
            Self::Return => "return".to_owned(),
            Self::True => "true".to_owned(),
            Self::False => "false".to_owned(),
            Self::Eof => "end of input".to_owned(),
        }
    }
}

fn is_value_token(tok: &Tok) -> bool {
    matches!(
        tok,
        Tok::Num(_) | Tok::Str(_) | Tok::Ident(_) | Tok::True | Tok::False | Tok::RParen | Tok::RBrace
    )
}

fn lex(source: &str) -> Result<Vec<(Tok, usize)>, ScriptError> {
    let bytes = source.as_bytes();
    let len = bytes.len();
    let mut tokens: Vec<(Tok, usize)> = Vec::new();
    let mut i = 0;
    while i < len {
        let byte = bytes[i];
        match byte {
            b' ' | b'\t' | b'\r' | b'\n' => i += 1,
            b'0'..=b'9' => {
                let (value, next) = lex_number(source, i, false)?;
                tokens.push((Tok::Num(value), i));
                i = next;
            }
            b'-' => {
                let after_value = tokens.last().is_some_and(|(tok, _)| is_value_token(tok));
                let next = bytes.get(i + 1).copied().unwrap_or(0);
                if !after_value && next.is_ascii_digit() {
                    let (value, next) = lex_number(source, i + 1, true)?;
                    tokens.push((Tok::Num(value), i));
                    i = next;
                } else {
                    tokens.push((Tok::Minus, i));
                    i += 1;
                }
            }
            b'"' | b'\'' => {
                let (text, next) = lex_string(source, i, byte)?;
                tokens.push((Tok::Str(text), i));
                i = next;
            }
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                let (word, next) = lex_ident(source, i);
                tokens.push((keyword(&word), i));
                i = next;
            }
            b'(' => {
                tokens.push((Tok::LParen, i));
                i += 1;
            }
            b')' => {
                tokens.push((Tok::RParen, i));
                i += 1;
            }
            b'{' => {
                tokens.push((Tok::LBrace, i));
                i += 1;
            }
            b'}' => {
                tokens.push((Tok::RBrace, i));
                i += 1;
            }
            b',' => {
                tokens.push((Tok::Comma, i));
                i += 1;
            }
            b';' => {
                tokens.push((Tok::Semi, i));
                i += 1;
            }
            b'+' => {
                tokens.push((Tok::Plus, i));
                i += 1;
            }
            b'*' => {
                tokens.push((Tok::Star, i));
                i += 1;
            }
            b'/' => {
                tokens.push((Tok::Slash, i));
                i += 1;
            }
            b'%' => {
                tokens.push((Tok::Percent, i));
                i += 1;
            }
            b'=' => {
                if bytes.get(i + 1) == Some(&b'=') {
                    tokens.push((Tok::EqEq, i));
                    i += 2;
                } else {
                    tokens.push((Tok::Eq, i));
                    i += 1;
                }
            }
            b'!' => {
                if bytes.get(i + 1) == Some(&b'=') {
                    tokens.push((Tok::NotEq, i));
                    i += 2;
                } else {
                    return Err(unexpected(source, i));
                }
            }
            b'<' => {
                if bytes.get(i + 1) == Some(&b'=') {
                    tokens.push((Tok::Le, i));
                    i += 2;
                } else {
                    tokens.push((Tok::Lt, i));
                    i += 1;
                }
            }
            b'>' => {
                if bytes.get(i + 1) == Some(&b'=') {
                    tokens.push((Tok::Ge, i));
                    i += 2;
                } else {
                    tokens.push((Tok::Gt, i));
                    i += 1;
                }
            }
            _ => return Err(unexpected(source, i)),
        }
    }
    tokens.push((Tok::Eof, len));
    Ok(tokens)
}

fn unexpected(source: &str, at: usize) -> ScriptError {
    let ch = source[at..].chars().next().unwrap_or('?');
    ScriptError::Syntax {
        pos: at,
        msg: format!("unexpected character {ch:?}"),
    }
}

fn lex_number(source: &str, start: usize, negative: bool) -> Result<(Value, usize), ScriptError> {
    let bytes = source.as_bytes();
    let mut i = start;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    let mut float = false;
    if i < bytes.len() && bytes[i] == b'.' {
        float = true;
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
    }
    let from = if negative { start - 1 } else { start };
    let text = &source[from..i];
    if float {
        let number = text.parse::<f64>().map_err(|_| ScriptError::Syntax {
            pos: from,
            msg: format!("{text} is not a number"),
        })?;
        Ok((Value::Float(number), i))
    } else {
        let number = text.parse::<i64>().map_err(|_| ScriptError::Syntax {
            pos: from,
            msg: format!("{text} is not an integer"),
        })?;
        Ok((Value::Int(number), i))
    }
}

fn lex_string(source: &str, start: usize, quote: u8) -> Result<(String, usize), ScriptError> {
    let bytes = source.as_bytes();
    let mut out = String::new();
    let mut i = start + 1;
    while i < bytes.len() {
        let byte = bytes[i];
        if byte == quote {
            return Ok((out, i + 1));
        }
        if byte == b'\\' {
            i += 1;
            if i >= bytes.len() {
                return Err(ScriptError::Syntax {
                    pos: i,
                    msg: "the source ends inside an escape".to_owned(),
                });
            }
            let escaped = bytes[i];
            out.push(match escaped {
                b'n' => '\n',
                b't' => '\t',
                b'r' => '\r',
                b'\\' => '\\',
                b'"' => '"',
                b'\'' => '\'',
                b'0' => '\0',
                other => {
                    return Err(ScriptError::Syntax {
                        pos: i,
                        msg: format!("unknown escape \\{}", other as char),
                    })
                }
            });
            i += 1;
        } else if byte < 0x80 {
            out.push(byte as char);
            i += 1;
        } else {
            let ch = source[i..].chars().next().unwrap_or('?');
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    Err(ScriptError::Syntax {
        pos: start,
        msg: "a string is never closed".to_owned(),
    })
}

fn lex_ident(source: &str, start: usize) -> (String, usize) {
    let bytes = source.as_bytes();
    let mut i = start;
    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
        i += 1;
    }
    (source[start..i].to_owned(), i)
}

fn keyword(word: &str) -> Tok {
    match word {
        "and" => Tok::And,
        "or" => Tok::Or,
        "not" => Tok::Not,
        "if" => Tok::If,
        "else" => Tok::Else,
        "while" => Tok::While,
        "let" => Tok::Let,
        "return" => Tok::Return,
        "true" => Tok::True,
        "false" => Tok::False,
        _ => Tok::Ident(word.to_owned()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

#[derive(Debug, Clone)]
enum ExprKind {
    Lit(Value),
    Ident(String),
    Neg(Box<Expr>),
    Not(Box<Expr>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>, usize),
}

#[derive(Debug, Clone)]
struct Expr {
    kind: ExprKind,
    pos: usize,
}

#[derive(Debug, Clone)]
enum Stmt {
    Let(String, Expr),
    Assign(String, Expr),
    Expr(Expr),
    Return(Option<Expr>),
    If(Expr, Vec<Stmt>, Option<Vec<Stmt>>),
    While(Expr, Vec<Stmt>),
    Block(Vec<Stmt>),
}

type Program = Vec<Stmt>;

struct Parser {
    tokens: Vec<(Tok, usize)>,
    index: usize,
    eof: Tok,
}

impl Parser {
    fn new(tokens: Vec<(Tok, usize)>) -> Self {
        Self {
            tokens,
            index: 0,
            eof: Tok::Eof,
        }
    }

    fn peek(&self) -> &Tok {
        self.tokens
            .get(self.index)
            .map_or(&self.eof, |(tok, _)| tok)
    }

    fn peek_at(&self, ahead: usize) -> &Tok {
        self.tokens
            .get(self.index + ahead)
            .map_or(&self.eof, |(tok, _)| tok)
    }

    fn pos(&self) -> usize {
        self.tokens.get(self.index).map_or(0, |(_, pos)| *pos)
    }

    fn bump(&mut self) -> usize {
        let pos = self.pos();
        if self.index < self.tokens.len() {
            self.index += 1;
        }
        pos
    }

    fn expect(&mut self, wanted: Tok) -> Result<(), ScriptError> {
        if self.peek() == &wanted {
            self.bump();
            Ok(())
        } else {
            let found = self.peek().describe();
            Err(ScriptError::Syntax {
                pos: self.pos(),
                msg: format!("expected {}, found {found}", wanted.describe()),
            })
        }
    }

    fn end_statement(&mut self) -> Result<(), ScriptError> {
        if self.peek() == &Tok::Semi {
            self.bump();
            Ok(())
        } else if matches!(self.peek(), Tok::Eof | Tok::RBrace) {
            Ok(())
        } else {
            let found = self.peek().describe();
            Err(ScriptError::Syntax {
                pos: self.pos(),
                msg: format!("expected ;, found {found}"),
            })
        }
    }

    fn expect_ident(&mut self) -> Result<String, ScriptError> {
        match self.peek().clone() {
            Tok::Ident(name) => {
                self.bump();
                Ok(name)
            }
            other => Err(ScriptError::Syntax {
                pos: self.pos(),
                msg: format!("expected a name, found {}", other.describe()),
            }),
        }
    }

    fn parse_program(&mut self) -> Result<Program, ScriptError> {
        let mut program = Vec::new();
        while self.peek() != &Tok::Eof {
            program.push(self.parse_stmt()?);
        }
        Ok(program)
    }

    fn parse_stmt(&mut self) -> Result<Stmt, ScriptError> {
        match self.peek().clone() {
            Tok::Let => {
                self.bump();
                let name = self.expect_ident()?;
                self.expect(Tok::Eq)?;
                let value = self.parse_expr()?;
                self.end_statement()?;
                Ok(Stmt::Let(name, value))
            }
            Tok::Return => {
                self.bump();
                if self.peek() == &Tok::Semi {
                    self.bump();
                    Ok(Stmt::Return(None))
                } else {
                    let value = self.parse_expr()?;
                    self.end_statement()?;
                    Ok(Stmt::Return(Some(value)))
                }
            }
            Tok::If => self.parse_if(),
            Tok::While => {
                self.bump();
                let cond = self.parse_expr()?;
                let body = self.parse_block()?;
                Ok(Stmt::While(cond, body))
            }
            Tok::LBrace => Ok(Stmt::Block(self.parse_block()?)),
            Tok::Ident(name) if self.peek_at(1) == &Tok::Eq => {
                self.bump();
                self.bump();
                let value = self.parse_expr()?;
                self.end_statement()?;
                Ok(Stmt::Assign(name, value))
            }
            _ => {
                let value = self.parse_expr()?;
                self.end_statement()?;
                Ok(Stmt::Expr(value))
            }
        }
    }

    fn parse_if(&mut self) -> Result<Stmt, ScriptError> {
        self.bump();
        let cond = self.parse_expr()?;
        let then = self.parse_block()?;
        let els = if self.peek() == &Tok::Else {
            self.bump();
            if self.peek() == &Tok::If {
                Some(vec![self.parse_if()?])
            } else {
                Some(self.parse_block()?)
            }
        } else {
            None
        };
        Ok(Stmt::If(cond, then, els))
    }

    fn parse_block(&mut self) -> Result<Vec<Stmt>, ScriptError> {
        self.expect(Tok::LBrace)?;
        let mut stmts = Vec::new();
        while self.peek() != &Tok::RBrace {
            if self.peek() == &Tok::Eof {
                return Err(ScriptError::Syntax {
                    pos: self.pos(),
                    msg: "expected a }, found end of input".to_owned(),
                });
            }
            stmts.push(self.parse_stmt()?);
        }
        self.bump();
        Ok(stmts)
    }

    fn parse_expr(&mut self) -> Result<Expr, ScriptError> {
        self.parse_or()
    }

    fn parse_or(&mut self) -> Result<Expr, ScriptError> {
        let mut left = self.parse_and()?;
        while self.peek() == &Tok::Or {
            let pos = self.bump();
            let right = self.parse_and()?;
            left = binary(BinOp::Or, left, right, pos);
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, ScriptError> {
        let mut left = self.parse_not()?;
        while self.peek() == &Tok::And {
            let pos = self.bump();
            let right = self.parse_not()?;
            left = binary(BinOp::And, left, right, pos);
        }
        Ok(left)
    }

    fn parse_not(&mut self) -> Result<Expr, ScriptError> {
        if self.peek() == &Tok::Not {
            let pos = self.bump();
            let operand = self.parse_not()?;
            Ok(Expr {
                kind: ExprKind::Not(Box::new(operand)),
                pos,
            })
        } else {
            self.parse_equality()
        }
    }

    fn parse_equality(&mut self) -> Result<Expr, ScriptError> {
        let mut left = self.parse_comparison()?;
        loop {
            let op = match self.peek() {
                Tok::EqEq => BinOp::Eq,
                Tok::NotEq => BinOp::Ne,
                _ => break,
            };
            let pos = self.bump();
            let right = self.parse_comparison()?;
            left = binary(op, left, right, pos);
        }
        Ok(left)
    }

    fn parse_comparison(&mut self) -> Result<Expr, ScriptError> {
        let mut left = self.parse_additive()?;
        loop {
            let op = match self.peek() {
                Tok::Lt => BinOp::Lt,
                Tok::Le => BinOp::Le,
                Tok::Gt => BinOp::Gt,
                Tok::Ge => BinOp::Ge,
                _ => break,
            };
            let pos = self.bump();
            let right = self.parse_additive()?;
            left = binary(op, left, right, pos);
        }
        Ok(left)
    }

    fn parse_additive(&mut self) -> Result<Expr, ScriptError> {
        let mut left = self.parse_multiplicative()?;
        loop {
            let op = match self.peek() {
                Tok::Plus => BinOp::Add,
                Tok::Minus => BinOp::Sub,
                _ => break,
            };
            let pos = self.bump();
            let right = self.parse_multiplicative()?;
            left = binary(op, left, right, pos);
        }
        Ok(left)
    }

    fn parse_multiplicative(&mut self) -> Result<Expr, ScriptError> {
        let mut left = self.parse_unary()?;
        loop {
            let op = match self.peek() {
                Tok::Star => BinOp::Mul,
                Tok::Slash => BinOp::Div,
                Tok::Percent => BinOp::Rem,
                _ => break,
            };
            let pos = self.bump();
            let right = self.parse_unary()?;
            left = binary(op, left, right, pos);
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, ScriptError> {
        if self.peek() == &Tok::Minus {
            let pos = self.bump();
            let operand = self.parse_unary()?;
            return Ok(Expr {
                kind: ExprKind::Neg(Box::new(operand)),
                pos,
            });
        }
        self.parse_call()
    }

    fn parse_call(&mut self) -> Result<Expr, ScriptError> {
        if let Tok::Ident(name) = self.peek().clone() {
            if self.peek_at(1) == &Tok::LParen {
                let pos = self.bump();
                self.bump();
                let mut args = Vec::new();
                if self.peek() != &Tok::RParen {
                    loop {
                        args.push(self.parse_expr()?);
                        if self.peek() == &Tok::Comma {
                            self.bump();
                        } else {
                            break;
                        }
                    }
                }
                self.expect(Tok::RParen)?;
                return Ok(Expr {
                    kind: ExprKind::Call(name, args, pos),
                    pos,
                });
            }
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<Expr, ScriptError> {
        let token = self.peek().clone();
        let pos = self.pos();
        match token {
            Tok::Num(value) => {
                self.bump();
                Ok(Expr {
                    kind: ExprKind::Lit(value),
                    pos,
                })
            }
            Tok::Str(text) => {
                self.bump();
                Ok(Expr {
                    kind: ExprKind::Lit(Value::Str(text)),
                    pos,
                })
            }
            Tok::True => {
                self.bump();
                Ok(Expr {
                    kind: ExprKind::Lit(Value::Bool(true)),
                    pos,
                })
            }
            Tok::False => {
                self.bump();
                Ok(Expr {
                    kind: ExprKind::Lit(Value::Bool(false)),
                    pos,
                })
            }
            Tok::Ident(name) => {
                self.bump();
                Ok(Expr {
                    kind: ExprKind::Ident(name),
                    pos,
                })
            }
            Tok::LParen => {
                self.bump();
                let inner = self.parse_expr()?;
                self.expect(Tok::RParen)?;
                Ok(inner)
            }
            other => Err(ScriptError::Syntax {
                pos,
                msg: format!("expected an expression, found {}", other.describe()),
            }),
        }
    }

    fn parse_literal(&mut self) -> Result<Value, ScriptError> {
        let token = self.peek().clone();
        let pos = self.pos();
        match token {
            Tok::Num(value) => {
                self.bump();
                Ok(value)
            }
            Tok::Str(text) => {
                self.bump();
                Ok(Value::Str(text))
            }
            Tok::True => {
                self.bump();
                Ok(Value::Bool(true))
            }
            Tok::False => {
                self.bump();
                Ok(Value::Bool(false))
            }
            other => Err(ScriptError::Syntax {
                pos,
                msg: format!("expected a value, found {}", other.describe()),
            }),
        }
    }
}

fn binary(op: BinOp, left: Expr, right: Expr, pos: usize) -> Expr {
    Expr {
        kind: ExprKind::Bin(op, Box::new(left), Box::new(right)),
        pos,
    }
}

fn compile(source: &str) -> Result<Program, ScriptError> {
    Parser::new(lex(source)?).parse_program()
}

fn arithmetic(pos: usize, msg: &str) -> ScriptError {
    ScriptError::Arithmetic {
        pos,
        msg: msg.to_owned(),
    }
}

fn divide_by_zero(pos: usize) -> ScriptError {
    ScriptError::DivideByZero { pos }
}

fn type_error(pos: usize, expected: &str, found: &Value) -> ScriptError {
    ScriptError::Type {
        pos,
        msg: format!("expected {expected}, found {}", found.type_name()),
    }
}

fn numbers(a: &Value, b: &Value, pos: usize) -> Result<(f64, f64), ScriptError> {
    match (a.as_float(), b.as_float()) {
        (Some(x), Some(y)) => Ok((x, y)),
        _ => Err(type_error(pos, "numbers", a)),
    }
}

fn equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Float(x), Value::Float(y)) => x == y,
        (Value::Int(_) | Value::Float(_), Value::Int(_) | Value::Float(_)) => {
            a.as_float() == b.as_float()
        }
        _ => false,
    }
}

fn binary_add(a: &Value, b: &Value, pos: usize) -> Result<Value, ScriptError> {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => x
            .checked_add(*y)
            .map(Value::Int)
            .ok_or_else(|| arithmetic(pos, "integer overflow")),
        (Value::Str(_), _) | (_, Value::Str(_)) => Ok(Value::Str(format!("{a}{b}"))),
        _ => {
            let (x, y) = numbers(a, b, pos)?;
            Ok(Value::Float(x + y))
        }
    }
}

fn binary_sub(a: &Value, b: &Value, pos: usize) -> Result<Value, ScriptError> {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => x
            .checked_sub(*y)
            .map(Value::Int)
            .ok_or_else(|| arithmetic(pos, "integer overflow")),
        _ => {
            let (x, y) = numbers(a, b, pos)?;
            Ok(Value::Float(x - y))
        }
    }
}

fn binary_mul(a: &Value, b: &Value, pos: usize) -> Result<Value, ScriptError> {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => x
            .checked_mul(*y)
            .map(Value::Int)
            .ok_or_else(|| arithmetic(pos, "integer overflow")),
        _ => {
            let (x, y) = numbers(a, b, pos)?;
            Ok(Value::Float(x * y))
        }
    }
}

fn binary_div(a: &Value, b: &Value, pos: usize) -> Result<Value, ScriptError> {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => {
            if *y == 0 {
                return Err(divide_by_zero(pos));
            }
            x.checked_div(*y)
                .map(Value::Int)
                .ok_or_else(|| arithmetic(pos, "integer overflow"))
        }
        _ => {
            let (x, y) = numbers(a, b, pos)?;
            if y == 0.0 {
                return Err(divide_by_zero(pos));
            }
            Ok(Value::Float(x / y))
        }
    }
}

fn binary_rem(a: &Value, b: &Value, pos: usize) -> Result<Value, ScriptError> {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => {
            if *y == 0 {
                return Err(divide_by_zero(pos));
            }
            x.checked_rem(*y)
                .map(Value::Int)
                .ok_or_else(|| arithmetic(pos, "integer overflow"))
        }
        _ => {
            let (x, y) = numbers(a, b, pos)?;
            if y == 0.0 {
                return Err(divide_by_zero(pos));
            }
            Ok(Value::Float(x % y))
        }
    }
}

fn binary_order(op: BinOp, a: &Value, b: &Value, pos: usize) -> Result<Value, ScriptError> {
    let ordering = match (a, b) {
        (Value::Str(left), Value::Str(right)) => left.partial_cmp(right),
        _ => {
            let (x, y) = numbers(a, b, pos)?;
            x.partial_cmp(&y)
        }
    };
    let flag = match op {
        BinOp::Lt => ordering == Some(std::cmp::Ordering::Less),
        BinOp::Le => matches!(
            ordering,
            Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
        ),
        BinOp::Gt => ordering == Some(std::cmp::Ordering::Greater),
        BinOp::Ge => matches!(
            ordering,
            Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
        ),
        _ => false,
    };
    Ok(Value::Bool(flag))
}

#[derive(Default)]
struct Env {
    scopes: Vec<BTreeMap<String, Value>>,
}

impl Env {
    fn new() -> Self {
        Self {
            scopes: vec![BTreeMap::new()],
        }
    }

    fn get(&self, name: &str) -> Option<&Value> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name))
    }

    fn declare(&mut self, name: &str, value: Value) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_owned(), value);
        }
    }

    fn assign(&mut self, name: &str, value: Value) -> bool {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(slot) = scope.get_mut(name) {
                *slot = value;
                return true;
            }
        }
        false
    }

    fn push(&mut self) {
        self.scopes.push(BTreeMap::new());
    }

    fn pop(&mut self) {
        self.scopes.pop();
    }
}

enum Flow {
    Normal,
    Return(Value),
}

struct Interp<'a> {
    held: &'a Held,
    env: Env,
    steps: u64,
    limit: u64,
    stack: Vec<String>,
    start: Instant,
    timeout: u64,
}

impl<'a> Interp<'a> {
    fn new(held: &'a Held, bindings: &[(&str, Value)], limit: u64, timeout: u64) -> Self {
        let mut env = Env::new();
        for (key, value) in &held.globals {
            env.declare(key, value.clone());
        }
        for (key, value) in bindings {
            env.declare(key, value.clone());
        }
        Self {
            held,
            env,
            steps: 0,
            limit,
            stack: Vec::new(),
            start: Instant::now(),
            timeout,
        }
    }

    fn tick(&mut self) -> Result<(), ScriptError> {
        self.steps += 1;
        if self.limit != 0 && self.steps > self.limit {
            return Err(ScriptError::Budget {
                steps: self.steps,
                limit: self.limit,
            });
        }
        if self.timeout != 0 && self.steps.is_multiple_of(256) {
            let elapsed = self.start.elapsed().as_millis() as u64;
            if elapsed > self.timeout {
                return Err(ScriptError::Timeout {
                    millis: self.timeout,
                });
            }
        }
        Ok(())
    }

    fn run(&mut self, program: &[Stmt]) -> Result<Value, ScriptError> {
        let last = program.len().saturating_sub(1);
        let mut answer = Value::Null;
        for (index, stmt) in program.iter().enumerate() {
            if index == last {
                if let Stmt::Expr(expr) = stmt {
                    answer = self.eval(expr)?;
                    continue;
                }
            }
            if let Flow::Return(value) = self.exec_stmt(stmt)? {
                return Ok(value);
            }
        }
        Ok(answer)
    }

    fn exec_block(&mut self, stmts: &[Stmt]) -> Result<Flow, ScriptError> {
        for stmt in stmts {
            if let Flow::Return(value) = self.exec_stmt(stmt)? {
                return Ok(Flow::Return(value));
            }
        }
        Ok(Flow::Normal)
    }

    fn exec_scoped(&mut self, stmts: &[Stmt]) -> Result<Flow, ScriptError> {
        self.env.push();
        let flow = self.exec_block(stmts)?;
        self.env.pop();
        Ok(flow)
    }

    fn exec_stmt(&mut self, stmt: &Stmt) -> Result<Flow, ScriptError> {
        self.tick()?;
        match stmt {
            Stmt::Let(name, value) => {
                let value = self.eval(value)?;
                self.env.declare(name, value);
                Ok(Flow::Normal)
            }
            Stmt::Assign(name, value) => {
                let value = self.eval(value)?;
                if self.env.assign(name, value) {
                    Ok(Flow::Normal)
                } else {
                    Err(ScriptError::UndefinedName {
                        name: name.clone(),
                        pos: 0,
                    })
                }
            }
            Stmt::Expr(expr) => {
                self.eval(expr)?;
                Ok(Flow::Normal)
            }
            Stmt::Return(value) => {
                let value = match value {
                    Some(expr) => self.eval(expr)?,
                    None => Value::Null,
                };
                Ok(Flow::Return(value))
            }
            Stmt::If(cond, then, els) => {
                let value = self.eval(cond)?;
                let flag = self.as_bool(value, cond.pos)?;
                if flag {
                    self.exec_scoped(then)
                } else if let Some(els) = els {
                    self.exec_scoped(els)
                } else {
                    Ok(Flow::Normal)
                }
            }
            Stmt::While(cond, body) => {
                loop {
                    let value = self.eval(cond)?;
                    let flag = self.as_bool(value, cond.pos)?;
                    if !flag {
                        break;
                    }
                    if let Flow::Return(value) = self.exec_scoped(body)? {
                        return Ok(Flow::Return(value));
                    }
                    self.tick()?;
                }
                Ok(Flow::Normal)
            }
            Stmt::Block(stmts) => self.exec_scoped(stmts),
        }
    }

    fn as_bool(&self, value: Value, pos: usize) -> Result<bool, ScriptError> {
        match value {
            Value::Bool(flag) => Ok(flag),
            other => Err(type_error(pos, "a boolean", &other)),
        }
    }

    fn eval(&mut self, expr: &Expr) -> Result<Value, ScriptError> {
        self.tick()?;
        match &expr.kind {
            ExprKind::Lit(value) => Ok(value.clone()),
            ExprKind::Ident(name) => {
                self.env.get(name).cloned().ok_or_else(|| ScriptError::UndefinedName {
                    name: name.clone(),
                    pos: expr.pos,
                })
            }
            ExprKind::Neg(operand) => {
                let value = self.eval(operand)?;
                match value {
                    Value::Int(number) => number
                        .checked_neg()
                        .map(Value::Int)
                        .ok_or_else(|| arithmetic(expr.pos, "integer overflow")),
                    Value::Float(number) => Ok(Value::Float(-number)),
                    other => Err(type_error(expr.pos, "a number", &other)),
                }
            }
            ExprKind::Not(operand) => {
                let value = self.eval(operand)?;
                let flag = self.as_bool(value, expr.pos)?;
                Ok(Value::Bool(!flag))
            }
            ExprKind::Bin(op, left, right) => self.eval_binary(*op, left, right, expr.pos),
            ExprKind::Call(name, args, pos) => self.eval_call(name, args, *pos),
        }
    }

    fn eval_binary(
        &mut self,
        op: BinOp,
        left: &Expr,
        right: &Expr,
        pos: usize,
    ) -> Result<Value, ScriptError> {
        if op == BinOp::And {
            let value = self.eval(left)?;
            let flag = self.as_bool(value, pos)?;
            if !flag {
                return Ok(Value::Bool(false));
            }
            let value = self.eval(right)?;
            let flag = self.as_bool(value, pos)?;
            return Ok(Value::Bool(flag));
        }
        if op == BinOp::Or {
            let value = self.eval(left)?;
            let flag = self.as_bool(value, pos)?;
            if flag {
                return Ok(Value::Bool(true));
            }
            let value = self.eval(right)?;
            let flag = self.as_bool(value, pos)?;
            return Ok(Value::Bool(flag));
        }
        let a = self.eval(left)?;
        let b = self.eval(right)?;
        match op {
            BinOp::Add => binary_add(&a, &b, pos),
            BinOp::Sub => binary_sub(&a, &b, pos),
            BinOp::Mul => binary_mul(&a, &b, pos),
            BinOp::Div => binary_div(&a, &b, pos),
            BinOp::Rem => binary_rem(&a, &b, pos),
            BinOp::Eq => Ok(Value::Bool(equal(&a, &b))),
            BinOp::Ne => Ok(Value::Bool(!equal(&a, &b))),
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => binary_order(op, &a, &b, pos),
            BinOp::And | BinOp::Or => Ok(Value::Null),
        }
    }

    fn eval_call(&mut self, name: &str, args: &[Expr], pos: usize) -> Result<Value, ScriptError> {
        let mut values = Vec::with_capacity(args.len());
        for arg in args {
            values.push(self.eval(arg)?);
        }
        self.stack.push(name.to_owned());
        let result = self.call_host(name, &values, pos);
        self.stack.pop();
        result
    }

    fn call_host(&self, name: &str, values: &[Value], pos: usize) -> Result<Value, ScriptError> {
        let Some((arity, body)) = self.held.functions.get(name) else {
            return Err(ScriptError::UnknownFunction {
                name: name.to_owned(),
                pos,
            });
        };
        if *arity != values.len() {
            return Err(ScriptError::Arity {
                name: name.to_owned(),
                expected: *arity,
                got: values.len(),
                pos,
            });
        }
        body(values).map_err(|msg| ScriptError::Host {
            name: name.to_owned(),
            msg,
        })
    }
}

type HostFn = (usize, Box<dyn Fn(&[Value]) -> Result<Value, String> + Send + 'static>);

#[derive(Default)]
struct Runtime {
    scripts: BTreeMap<String, Program>,
    contexts: Vec<u64>,
    executions: u64,
    steps: u64,
    errors: Vec<String>,
}

#[derive(Default)]
struct Held {
    functions: BTreeMap<String, HostFn>,
    modules: Vec<String>,
    types: Vec<String>,
    event_names: Vec<String>,
    globals: BTreeMap<String, Value>,
    permissions: BTreeMap<String, u32>,
    instruction_limit: u64,
    memory_limit: usize,
    execution_timeout: u64,
    interrupted: bool,
    errors: Vec<String>,
    stack: Vec<String>,
    runtimes: BTreeMap<u64, Runtime>,
    next_runtime: u64,
    next_context: u64,
    executions: u64,
    steps: u64,
    interruptions: u64,
}

fn held() -> &'static Mutex<Held> {
    static HELD: OnceLock<Mutex<Held>> = OnceLock::new();
    HELD.get_or_init(|| {
        let held = Held {
            instruction_limit: DEFAULT_INSTRUCTION_LIMIT,
            ..Held::default()
        };
        Mutex::new(held)
    })
}

fn with<T>(body: impl FnOnce(&mut Held) -> T) -> T {
    let mut guard = held().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn execute_program(
    program: &[Stmt],
    bindings: &[(&str, Value)],
    runtime: Option<u64>,
) -> Result<Value, ScriptError> {
    if let Some(id) = runtime {
        if !with(|held| held.runtimes.contains_key(&id)) {
            return Err(ScriptError::UnknownRuntime(id));
        }
    }
    let (result, steps, stack) = with(|held| {
        if held.interrupted {
            held.interrupted = false;
            held.interruptions += 1;
            return (Err(ScriptError::Interrupted), 0, Vec::new());
        }
        let limit = held.instruction_limit;
        let timeout = held.execution_timeout;
        let mut interp = Interp::new(held, bindings, limit, timeout);
        let result = interp.run(program);
        let steps = interp.steps;
        let stack = std::mem::take(&mut interp.stack);
        (result, steps, stack)
    });
    with(|held| {
        held.steps += steps;
        held.stack = stack;
        if result.is_ok() {
            held.executions += 1;
        } else if let Err(error) = &result {
            held.errors.push(error.to_string());
        }
        if let Some(id) = runtime {
            if let Some(rt) = held.runtimes.get_mut(&id) {
                rt.steps += steps;
                if result.is_ok() {
                    rt.executions += 1;
                } else if let Err(error) = &result {
                    rt.errors.push(error.to_string());
                }
            }
        }
    });
    result
}

/// Runs `source` and answers its value, or a positioned [`ScriptError`].
pub fn eval(source: &str) -> Result<Value, ScriptError> {
    execute_program(&compile(source)?, &[], None)
}

/// Runs `source` with `bindings` in scope first, so a caller can seed names.
pub fn eval_with(source: &str, bindings: &[(&str, Value)]) -> Result<Value, ScriptError> {
    execute_program(&compile(source)?, bindings, None)
}

/// Makes a runtime that keeps named, already-parsed scripts. Answers its id.
#[must_use]
pub fn create_runtime() -> u64 {
    with(|held| {
        held.next_runtime += 1;
        let id = held.next_runtime;
        held.runtimes.insert(id, Runtime::default());
        id
    })
}

/// Forgets a runtime, its scripts and its contexts.
pub fn destroy_runtime(runtime: u64) -> bool {
    with(|held| held.runtimes.remove(&runtime).is_some())
}

/// Adds a context to a runtime. `None` when the runtime does not exist.
#[must_use]
pub fn create_context(runtime: u64) -> Option<u64> {
    with(|held| {
        held.next_context += 1;
        let id = held.next_context;
        held.runtimes.get_mut(&runtime)?.contexts.push(id);
        Some(id)
    })
}

/// Forgets a context, and answers whether it was there.
pub fn destroy_context(runtime: u64, context: u64) -> bool {
    with(|held| match held.runtimes.get_mut(&runtime) {
        Some(rt) => {
            let before = rt.contexts.len();
            rt.contexts.retain(|held| *held != context);
            rt.contexts.len() != before
        }
        None => false,
    })
}

/// Lexes and parses a script into a runtime under `name`. A malformed program is refused
/// with its position.
pub fn load_script(runtime: u64, name: &str, source: &str) -> Result<(), ScriptError> {
    let program = compile(source)?;
    with(|held| {
        if held.memory_limit != 0 && source.len() > held.memory_limit {
            return Err(ScriptError::Memory {
                limit: held.memory_limit,
            });
        }
        match held.runtimes.get_mut(&runtime) {
            Some(rt) => {
                rt.scripts.insert(name.to_owned(), program);
                Ok(())
            }
            None => Err(ScriptError::UnknownRuntime(runtime)),
        }
    })
}

/// Forgets a loaded script, and answers whether it was there.
pub fn unload_script(runtime: u64, name: &str) -> bool {
    with(|held| match held.runtimes.get_mut(&runtime) {
        Some(rt) => rt.scripts.remove(name).is_some(),
        None => false,
    })
}

/// Replaces a loaded script, keeping the old one when the new one does not compile.
pub fn reload_script(runtime: u64, name: &str, source: &str) -> Result<(), ScriptError> {
    let program = compile(source)?;
    with(|held| match held.runtimes.get_mut(&runtime) {
        Some(rt) => {
            rt.scripts.insert(name.to_owned(), program);
            Ok(())
        }
        None => Err(ScriptError::UnknownRuntime(runtime)),
    })
}

/// Runs a script that [`load_script`] stored, answering its value.
pub fn execute(runtime: u64, name: &str) -> Result<Value, ScriptError> {
    let program = with(|held| {
        held.runtimes
            .get(&runtime)
            .and_then(|rt| rt.scripts.get(name).cloned())
    });
    let Some(program) = program else {
        return Err(ScriptError::UnknownScript {
            runtime,
            name: name.to_owned(),
        });
    };
    execute_program(&program, &[], Some(runtime))
}

/// Parses and runs `source` against a runtime, for a script never stored.
pub fn evaluate(runtime: u64, source: &str) -> Result<Value, ScriptError> {
    execute_program(&compile(source)?, &[], Some(runtime))
}

fn declare(list: &mut Vec<String>, name: &str) -> bool {
    if name.is_empty() || list.iter().any(|held| held == name) {
        return false;
    }
    list.push(name.to_owned());
    true
}

fn forget(list: &mut Vec<String>, name: &str) -> bool {
    let before = list.len();
    list.retain(|held| held != name);
    list.len() != before
}

/// Names a module the scripts may import. A duplicate or empty name is refused.
pub fn register_module(name: &str) -> bool {
    with(|held| declare(&mut held.modules, name))
}

/// Forgets a module, and answers whether it was there.
pub fn unregister_module(name: &str) -> bool {
    with(|held| forget(&mut held.modules, name))
}

/// Names a type the scripts may build. A duplicate or empty name is refused.
pub fn register_type(name: &str) -> bool {
    with(|held| declare(&mut held.types, name))
}

/// Names an event a script may raise. A duplicate or empty name is refused.
pub fn register_event(name: &str) -> bool {
    with(|held| declare(&mut held.event_names, name))
}

/// Registers a host function a script calls by name, with the argument count it takes. A
/// duplicate or empty name is refused.
pub fn register_function<F>(name: &str, arity: usize, body: F) -> bool
where
    F: Fn(&[Value]) -> Result<Value, String> + Send + 'static,
{
    if name.is_empty() {
        return false;
    }
    with(|held| held.functions.insert(name.to_owned(), (arity, Box::new(body))).is_none())
}

/// Forgets a host function, and answers whether it was there.
pub fn unregister_function(name: &str) -> bool {
    with(|held| held.functions.remove(name).is_some())
}

/// Binds a global name every script sees.
pub fn set_global(name: &str, value: Value) -> bool {
    if name.is_empty() {
        return false;
    }
    with(|held| {
        if held.memory_limit != 0 && serialize_value(&value).len() > held.memory_limit {
            return false;
        }
        held.globals.insert(name.to_owned(), value);
        true
    })
}

/// A global by name.
#[must_use]
pub fn get_global(name: &str) -> Option<Value> {
    with(|held| held.globals.get(name).cloned())
}

/// Sets the level a named permission is held at.
pub fn set_permission(name: &str, level: u32) -> bool {
    if name.is_empty() {
        return false;
    }
    with(|held| {
        held.permissions.insert(name.to_owned(), level);
        true
    })
}

/// Every permission and its level.
#[must_use]
pub fn get_permissions() -> BTreeMap<String, u32> {
    with(|held| held.permissions.clone())
}

/// Caps the source and serialized values a runtime accepts, in bytes. `0` is no cap.
pub fn set_memory_limit(bytes: usize) -> bool {
    with(|held| held.memory_limit = bytes);
    true
}

/// Caps how many steps one run may take. `0` is no cap; the default is
/// [`DEFAULT_INSTRUCTION_LIMIT`].
pub fn set_instruction_limit(steps: u64) -> bool {
    with(|held| held.instruction_limit = steps);
    true
}

/// Caps how long one run may take, in milliseconds. `0` is no cap.
pub fn set_execution_timeout(millis: u64) -> bool {
    with(|held| held.execution_timeout = millis);
    true
}

/// Stops the next run before it starts, as a one-shot. Answers whether it was set.
pub fn interrupt() -> bool {
    with(|held| {
        held.interrupted = true;
        true
    })
}

/// The call stack the last run left when it stopped.
#[must_use]
pub fn get_stack_trace() -> Vec<String> {
    with(|held| held.stack.clone())
}

/// Every refusal the evaluator recorded, newest last.
#[must_use]
pub fn get_errors() -> Vec<String> {
    with(|held| held.errors.clone())
}

/// A readout of the runtimes, the registry and the counters.
#[must_use]
pub fn get_runtime_stats() -> RuntimeStats {
    with(|held| RuntimeStats {
        runtimes: held.runtimes.len(),
        scripts: held.runtimes.values().map(|rt| rt.scripts.len()).sum(),
        functions: held.functions.len(),
        executions: held.executions,
        steps: held.steps,
        errors: held.errors.len(),
        interruptions: held.interruptions,
    })
}

/// Writes a value as its own text form: `null`, `true`, an integer, a float with a decimal
/// point, or a quoted and escaped string.
#[must_use]
pub fn serialize_value(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(flag) => flag.to_string(),
        Value::Int(number) => number.to_string(),
        Value::Float(number) => {
            if number.is_finite() && number.fract() == 0.0 {
                format!("{number:.1}")
            } else {
                number.to_string()
            }
        }
        Value::Str(text) => quote(text),
    }
}

/// Reads back what [`serialize_value`] wrote, refusing anything with a position.
pub fn deserialize_value(text: &str) -> Result<Value, ScriptError> {
    let mut parser = Parser::new(lex(text)?);
    let value = parser.parse_literal()?;
    parser.expect(Tok::Eof)?;
    Ok(value)
}

fn quote(text: &str) -> String {
    let mut out = String::from("\"");
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

fn refused(what: &str) -> ScriptError {
    let why = why_unknown(what);
    crate::warn!("a mod asked this host to run a script the client owns: {why}");
    ScriptError::Refused(why)
}

/// Runs a script inside the client's own process. [`ScriptError::Refused`], with
/// [`why_unknown`].
pub fn execute_in_client(_name: &str) -> Result<Value, ScriptError> {
    Err(refused("client"))
}

/// Reads or extends the client's own scripting layer. `None`, with [`why_unknown`].
#[must_use]
pub fn client_script(_name: &str) -> Option<String> {
    crate::warn!(
        "a mod asked this host for the client's scripting layer: {}",
        why_unknown("client_scripts")
    );
    None
}

/// Why a scripting question about the client has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "client" => {
            "running a script inside the client would execute it in the client's own process, \
             which this host cannot start, own or sandbox"
                .to_owned()
        }
        "client_scripts" => {
            "the client's own scripting layer is built into the client and this host has no \
             handle on it, so a mod can neither read it nor add to it"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach about the client's scripting.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["client", "client_scripts"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with(|held| {
            held.functions.clear();
            held.modules.clear();
            held.types.clear();
            held.event_names.clear();
            held.globals.clear();
            held.permissions.clear();
            held.instruction_limit = DEFAULT_INSTRUCTION_LIMIT;
            held.memory_limit = 0;
            held.execution_timeout = 0;
            held.interrupted = false;
            held.errors.clear();
            held.stack.clear();
            held.runtimes.clear();
            held.next_runtime = 0;
            held.next_context = 0;
            held.executions = 0;
            held.steps = 0;
            held.interruptions = 0;
        });
    }

    #[test]
    fn precedence_binds_multiplication_before_addition() {
        let _serial = serial();
        fresh();
        assert_eq!(eval("1 + 2 * 3").expect("a value"), Value::Int(7));
        assert_eq!(eval("(1 + 2) * 3").expect("a value"), Value::Int(9));
        assert_eq!(eval("2 + 3 - 4").expect("a value"), Value::Int(1));
        assert_eq!(eval("20 / 5 % 3").expect("a value"), Value::Int(1));
        assert_eq!(eval("-2 + 5").expect("a value"), Value::Int(3));
        assert_eq!(eval("1.5 + 2").expect("a value"), Value::Float(3.5));
        assert_eq!(eval("7 / 2").expect("a value"), Value::Int(3));
        assert_eq!(eval("7.0 / 2").expect("a value"), Value::Float(3.5));
        assert_eq!(eval("\"a\" + \"b\"").expect("a value"), Value::str("ab"));
    }

    #[test]
    fn scopes_nest_and_shadow_and_bindings_seed_them() {
        let _serial = serial();
        fresh();
        assert_eq!(
            eval("let x = 1; { let x = 2; return x; }").expect("an inner value"),
            Value::Int(2)
        );
        assert_eq!(
            eval("let x = 1; { let x = 2; } return x;").expect("an outer value"),
            Value::Int(1)
        );
        assert_eq!(
            eval("let x = 1; x = x + 4; return x;").expect("an assignment"),
            Value::Int(5)
        );
        assert_eq!(
            eval_with("x * 2", &[("x", Value::Int(4))]).expect("a binding"),
            Value::Int(8)
        );
        assert_eq!(
            eval("let x = 0; while x < 3 { x = x + 1; } return x;").expect("a loop"),
            Value::Int(3)
        );
    }

    #[test]
    fn and_or_short_circuit_so_the_right_side_is_never_evaluated() {
        let _serial = serial();
        fresh();
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&calls);
        assert!(register_function("boom", 0, move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
            Ok(Value::Bool(true))
        }));
        assert_eq!(eval("false and boom()").expect("a value"), Value::Bool(false));
        assert_eq!(calls.load(Ordering::SeqCst), 0, "the right side never ran");
        assert_eq!(eval("true or boom()").expect("a value"), Value::Bool(true));
        assert_eq!(calls.load(Ordering::SeqCst), 0, "the right side never ran");
        assert_eq!(eval("true and boom()").expect("a value"), Value::Bool(true));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(eval("not false").expect("a value"), Value::Bool(true));
    }

    #[test]
    fn a_host_function_is_called_by_name_and_answers_a_value() {
        let _serial = serial();
        fresh();
        assert!(register_function("double", 1, |args| {
            Ok(Value::Int(args[0].as_int().unwrap_or(0) * 2))
        }));
        assert_eq!(eval("double(21)").expect("a value"), Value::Int(42));
        assert!(register_function("join", 2, |args| {
            Ok(Value::Str(format!("{}{}", args[0], args[1])))
        }));
        assert_eq!(eval("join(\"a\", \"b\")").expect("a value"), Value::str("ab"));
        assert!(unregister_function("double"));
        assert!(!unregister_function("double"));
    }

    #[test]
    fn an_unknown_function_and_a_wrong_arity_are_refused_by_name() {
        let _serial = serial();
        fresh();
        assert_eq!(
            eval("missing()").unwrap_err(),
            ScriptError::UnknownFunction {
                name: "missing".to_owned(),
                pos: 0,
            }
        );
        assert!(register_function("add", 2, |args| {
            Ok(Value::Int(
                args[0].as_int().unwrap_or(0) + args[1].as_int().unwrap_or(0),
            ))
        }));
        assert_eq!(
            eval("add(1)").unwrap_err(),
            ScriptError::Arity {
                name: "add".to_owned(),
                expected: 2,
                got: 1,
                pos: 0,
            }
        );
        assert_eq!(
            eval("add(1, 2, 3)").unwrap_err(),
            ScriptError::Arity {
                name: "add".to_owned(),
                expected: 2,
                got: 3,
                pos: 0,
            }
        );
        assert_eq!(eval("add(1, 2)").expect("the right arity"), Value::Int(3));
    }

    #[test]
    fn integer_and_float_division_by_zero_are_real_errors() {
        let _serial = serial();
        fresh();
        assert!(matches!(eval("1 / 0"), Err(ScriptError::DivideByZero { pos: 2 })));
        assert!(matches!(eval("1.0 / 0"), Err(ScriptError::DivideByZero { pos: 4 })));
        assert!(matches!(eval("7 % 0"), Err(ScriptError::DivideByZero { pos: 2 })));
        assert_eq!(eval("1 / 0").unwrap_err().pos(), Some(2));
    }

    #[test]
    fn a_runaway_while_true_is_stopped_by_the_instruction_budget() {
        let _serial = serial();
        fresh();
        assert!(set_instruction_limit(500));
        match eval("while true {}") {
            Err(ScriptError::Budget { steps, limit }) => {
                assert_eq!(limit, 500);
                assert!(steps > 500, "stopped at step {steps}");
            }
            other => panic!("expected a budget stop, got {other:?}"),
        }
        assert_eq!(eval("1 + 1").expect("a value after the stop"), Value::Int(2));
    }

    #[test]
    fn a_malformed_program_is_refused_with_a_position() {
        let _serial = serial();
        fresh();
        assert_eq!(
            eval("1 +").unwrap_err(),
            ScriptError::Syntax {
                pos: 3,
                msg: "expected an expression, found end of input".to_owned(),
            }
        );
        assert_eq!(
            eval("1 $ 2").unwrap_err(),
            ScriptError::Syntax {
                pos: 2,
                msg: "unexpected character '$'".to_owned(),
            }
        );
        assert_eq!(
            eval("let = 3").unwrap_err(),
            ScriptError::Syntax {
                pos: 4,
                msg: "expected a name, found =".to_owned(),
            }
        );
        assert!(matches!(eval("1 2"), Err(ScriptError::Syntax { pos: 2, .. })));
    }

    #[test]
    fn runtimes_globals_and_the_serialized_form_round_trip() {
        let _serial = serial();
        fresh();
        let runtime = create_runtime();
        assert!(load_script(runtime, "hello", "return greeting + \"!\";").is_ok());
        assert!(set_global("greeting", Value::str("hi")));
        assert_eq!(
            execute(runtime, "hello").expect("a value"),
            Value::str("hi!")
        );
        assert_eq!(evaluate(runtime, "6 * 7").expect("a value"), Value::Int(42));
        let context = create_context(runtime).expect("a context");
        assert!(destroy_context(runtime, context));
        assert!(unload_script(runtime, "hello"));
        assert!(destroy_runtime(runtime));

        let value = Value::str("a\"b\n");
        let text = serialize_value(&value);
        assert_eq!(text, "\"a\\\"b\\n\"");
        assert_eq!(deserialize_value(&text).expect("a value"), value);
        assert_eq!(deserialize_value("1.5").expect("a value"), Value::Float(1.5));

        assert_eq!(get_runtime_stats().functions, 0);
        assert!(set_instruction_limit(DEFAULT_INSTRUCTION_LIMIT));
        assert!(set_permission("scripting:run", 2));
        assert_eq!(get_permissions().get("scripting:run"), Some(&2));
    }

    #[test]
    fn the_client_side_is_refused_with_a_reason() {
        let _serial = serial();
        fresh();
        let keys = refusals()
            .into_iter()
            .map(|(what, _)| what)
            .collect::<Vec<_>>();
        assert_eq!(keys, vec!["client".to_owned(), "client_scripts".to_owned()]);
        assert!(execute_in_client("boot").is_err());
        assert!(why_unknown("client").contains("client's own process"));
        assert_eq!(client_script("boot"), None);
    }
}
