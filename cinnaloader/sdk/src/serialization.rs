//! Serialization: a typed value tree with real JSON, binary and text codecs, and the
//! client's own formats refused.
//!
//! A [`Value`] is one of null, bool, int, float, string, list or map. Mapped keys keep the
//! order they were inserted in, which is the order [`to_json`] and [`to_text`] write and the
//! order a caller iterates. A duplicate key on parse keeps the first position and the last
//! value.
//!
//! Three codecs round trip the tree exactly:
//!
//! * **JSON** — a writer and a recursive-descent parser. Strings take `\" \\ \/ \b \f \n \r
//!   \t \uXXXX` (surrogate pairs included) and pass multi-byte UTF-8 through unchanged. A
//!   malformed document comes back as [`Error::Syntax`] with a byte position.
//! * **Binary** — a tag byte then a payload: null, false and true are bare tags, an int is a
//!   zigzag LEB128 varint, a float is eight little-endian bytes, and a string, list or map is
//!   length-prefixed with a varint. Reading past the end is [`Error::Truncated`]; a byte left
//!   over is [`Error::Trailing`].
//! * **Text** — the tree's own display form: `(` list `)`, `{ "key" value }`, the same string
//!   escapes, and `nan` / `inf` / `-inf` for the floats JSON cannot hold.
//!
//! The client's NBT dialect and level format are refused, each with its own sentence in
//! [`why_unknown`] and listed by [`refusals`], never faked.
//!
//! ```
//! use cinnaloader::serialization::{self, Map, Value};
//!
//! let mut map = Map::new();
//! map.insert("name", Value::Str("stone".into()));
//! map.insert("count", Value::Int(3));
//! let value = Value::Map(map);
//!
//! let json = serialization::to_json(&value).expect("a finite tree");
//! assert_eq!(json, r#"{"name":"stone","count":3}"#);
//! assert_eq!(serialization::from_json(&json).expect("well formed"), value);
//!
//! let bytes = serialization::to_binary(&value);
//! assert_eq!(bytes.len(), 22);
//! assert_eq!(serialization::from_binary(&bytes).expect("a whole value"), value);
//! assert!(serialization::from_binary(&bytes[..bytes.len() - 1]).is_err());
//!
//! assert_eq!(
//!     serialization::decode_base64(&serialization::encode_base64(&bytes)).expect("valid"),
//!     bytes
//! );
//! ```

use std::fmt;
use std::sync::{Mutex, OnceLock};

/// The largest run of bytes a string or a decompressed block may hold.
pub const MAX_BYTES: usize = 16 * 1024 * 1024;
/// The most elements a list or map may declare.
pub const MAX_ITEMS: usize = 1_000_000;
/// The deepest a value may nest before parsing refuses rather than overflow the stack.
pub const MAX_DEPTH: usize = 128;

/// What went wrong, and where.
#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// A malformed document, with the byte position it gave up at.
    Syntax { pos: usize, msg: String },
    /// A complete value, then bytes the format has no room for.
    Trailing { pos: usize },
    /// The document ended inside a value, a length or a run.
    Truncated { pos: usize },
    /// A float JSON has no spelling for.
    NonFinite,
    /// No serializer is registered under that name.
    UnknownFormat(String),
    /// No schema is registered under that name.
    NoSchema(String),
    /// A version step no migration covers.
    NoMigration { from: u64, to: u64 },
    /// Base64 that is not four-character blocks or holds a byte outside the alphabet.
    BadBase64(usize),
    /// Hex that is odd-length or holds a byte outside the alphabet.
    BadHex(usize),
    /// A document larger than this host will build.
    TooLarge { what: &'static str, limit: usize },
    /// The client's own format, with [`why_unknown`] as the reason.
    Refused(String),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax { pos, msg } => write!(formatter, "at byte {pos}: {msg}"),
            Self::Trailing { pos } => write!(formatter, "trailing bytes from byte {pos}"),
            Self::Truncated { pos } => write!(formatter, "truncated at byte {pos}"),
            Self::NonFinite => formatter.write_str("a non-finite float has no JSON spelling"),
            Self::UnknownFormat(name) => write!(formatter, "no serializer named {name}"),
            Self::NoSchema(name) => write!(formatter, "no schema named {name}"),
            Self::NoMigration { from, to } => {
                write!(formatter, "no migration from version {from} to {to}")
            }
            Self::BadBase64(pos) => write!(formatter, "bad base64 at byte {pos}"),
            Self::BadHex(pos) => write!(formatter, "bad hex at byte {pos}"),
            Self::TooLarge { what, limit } => {
                write!(formatter, "{what} over the {limit}-byte limit")
            }
            Self::Refused(why) => formatter.write_str(why),
        }
    }
}

impl std::error::Error for Error {}

/// One typed value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<Value>),
    Map(Map),
}

impl Value {
    /// A string, the constructor a caller writes most.
    #[must_use]
    pub fn str(text: impl Into<String>) -> Self {
        Self::Str(text.into())
    }

    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(flag) => Some(*flag),
            _ => None,
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
            Self::Float(number) => Some(*number),
            Self::Int(number) => Some(*number as f64),
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

    #[must_use]
    pub fn as_list(&self) -> Option<&[Value]> {
        match self {
            Self::List(items) => Some(items),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_map(&self) -> Option<&Map> {
        match self {
            Self::Map(map) => Some(map),
            _ => None,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut text = String::new();
        write_text(&mut text, self);
        formatter.write_str(&text)
    }
}

/// A map that keeps insertion order, so a document round trips byte for byte.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Map {
    entries: Vec<(String, Value)>,
}

impl Map {
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Inserts a key, replacing a stored value **in its original position**, and answers
    /// the value that was there.
    pub fn insert(&mut self, key: impl Into<String>, value: Value) -> Option<Value> {
        let key = key.into();
        if let Some((_, held)) = self.entries.iter_mut().find(|(name, _)| *name == key) {
            return Some(std::mem::replace(held, value));
        }
        self.entries.push((key, value));
        None
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.entries
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        self.entries
            .iter_mut()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
    }

    pub fn remove(&mut self, key: &str) -> Option<Value> {
        let at = self.entries.iter().position(|(name, _)| name == key)?;
        Some(self.entries.remove(at).1)
    }

    #[must_use]
    pub fn contains_key(&self, key: &str) -> bool {
        self.entries.iter().any(|(name, _)| name == key)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.entries
            .iter()
            .map(|(name, value)| (name.as_str(), value))
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(name, _)| name.as_str())
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl FromIterator<(String, Value)> for Map {
    fn from_iter<I: IntoIterator<Item = (String, Value)>>(iter: I) -> Self {
        let mut map = Self::new();
        for (key, value) in iter {
            map.insert(key, value);
        }
        map
    }
}

fn zigzag(number: i64) -> u64 {
    ((number << 1) ^ (number >> 63)) as u64
}

fn unzigzag(number: u64) -> i64 {
    ((number >> 1) as i64) ^ -((number & 1) as i64)
}

fn put_varint(out: &mut Vec<u8>, mut number: u64) {
    loop {
        let byte = (number & 0x7f) as u8;
        number >>= 7;
        if number == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn get_varint(bytes: &[u8], pos: &mut usize) -> Result<u64, Error> {
    let mut value = 0u64;
    let mut shift = 0u32;
    loop {
        let byte = *bytes.get(*pos).ok_or(Error::Truncated { pos: *pos })?;
        *pos += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
        if shift >= 64 {
            return Err(Error::Syntax {
                pos: *pos,
                msg: "a varint longer than 64 bits".to_owned(),
            });
        }
    }
}

/// Writes a value as the tagged binary form this module defines.
#[must_use]
pub fn to_binary(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    write_binary(&mut out, value);
    out
}

fn write_binary(out: &mut Vec<u8>, value: &Value) {
    match value {
        Value::Null => out.push(0x00),
        Value::Bool(false) => out.push(0x01),
        Value::Bool(true) => out.push(0x02),
        Value::Int(number) => {
            out.push(0x03);
            put_varint(out, zigzag(*number));
        }
        Value::Float(number) => {
            out.push(0x04);
            out.extend_from_slice(&number.to_le_bytes());
        }
        Value::Str(text) => {
            out.push(0x05);
            put_varint(out, text.len() as u64);
            out.extend_from_slice(text.as_bytes());
        }
        Value::List(items) => {
            out.push(0x06);
            put_varint(out, items.len() as u64);
            for item in items {
                write_binary(out, item);
            }
        }
        Value::Map(map) => {
            out.push(0x07);
            put_varint(out, map.len() as u64);
            for (key, held) in &map.entries {
                put_varint(out, key.len() as u64);
                out.extend_from_slice(key.as_bytes());
                write_binary(out, held);
            }
        }
    }
}

/// Reads the tagged binary form, refusing truncation and any byte left over.
pub fn from_binary(bytes: &[u8]) -> Result<Value, Error> {
    let mut pos = 0;
    let value = read_binary(bytes, &mut pos, 0)?;
    if pos != bytes.len() {
        return Err(Error::Trailing { pos });
    }
    Ok(value)
}

fn read_binary(bytes: &[u8], pos: &mut usize, depth: usize) -> Result<Value, Error> {
    if depth > MAX_DEPTH {
        return Err(Error::Syntax {
            pos: *pos,
            msg: "nested deeper than this host allows".to_owned(),
        });
    }
    let tag = *bytes.get(*pos).ok_or(Error::Truncated { pos: *pos })?;
    *pos += 1;
    match tag {
        0x00 => Ok(Value::Null),
        0x01 => Ok(Value::Bool(false)),
        0x02 => Ok(Value::Bool(true)),
        0x03 => Ok(Value::Int(unzigzag(get_varint(bytes, pos)?))),
        0x04 => {
            let end = pos.checked_add(8).ok_or(Error::Truncated { pos: *pos })?;
            let slice = bytes.get(*pos..end).ok_or(Error::Truncated { pos: *pos })?;
            *pos = end;
            let mut eight = [0u8; 8];
            eight.copy_from_slice(slice);
            Ok(Value::Float(f64::from_le_bytes(eight)))
        }
        0x05 => {
            let len = get_varint(bytes, pos)? as usize;
            if len > MAX_BYTES {
                return Err(Error::TooLarge {
                    what: "a string",
                    limit: MAX_BYTES,
                });
            }
            let end = pos.checked_add(len).ok_or(Error::Truncated { pos: *pos })?;
            let slice = bytes.get(*pos..end).ok_or(Error::Truncated { pos: *pos })?;
            *pos = end;
            let text = std::str::from_utf8(slice).map_err(|_| Error::Syntax {
                pos: *pos,
                msg: "a string that is not UTF-8".to_owned(),
            })?;
            Ok(Value::Str(text.to_owned()))
        }
        0x06 => {
            let count = get_varint(bytes, pos)? as usize;
            if count > MAX_ITEMS {
                return Err(Error::TooLarge {
                    what: "a list",
                    limit: MAX_ITEMS,
                });
            }
            let mut items = Vec::with_capacity(count.min(1024));
            for _ in 0..count {
                items.push(read_binary(bytes, pos, depth + 1)?);
            }
            Ok(Value::List(items))
        }
        0x07 => {
            let count = get_varint(bytes, pos)? as usize;
            if count > MAX_ITEMS {
                return Err(Error::TooLarge {
                    what: "a map",
                    limit: MAX_ITEMS,
                });
            }
            let mut map = Map::new();
            for _ in 0..count {
                let len = get_varint(bytes, pos)? as usize;
                if len > MAX_BYTES {
                    return Err(Error::TooLarge {
                        what: "a key",
                        limit: MAX_BYTES,
                    });
                }
                let end = pos.checked_add(len).ok_or(Error::Truncated { pos: *pos })?;
                let slice = bytes.get(*pos..end).ok_or(Error::Truncated { pos: *pos })?;
                *pos = end;
                let key = std::str::from_utf8(slice)
                    .map_err(|_| Error::Syntax {
                        pos: *pos,
                        msg: "a key that is not UTF-8".to_owned(),
                    })?
                    .to_owned();
                let held = read_binary(bytes, pos, depth + 1)?;
                map.insert(key, held);
            }
            Ok(Value::Map(map))
        }
        other => Err(Error::Syntax {
            pos: *pos - 1,
            msg: format!("unknown tag 0x{other:02x}"),
        }),
    }
}

/// Writes a value as JSON, in map insertion order. A non-finite float is refused, since
/// JSON has no spelling for one.
pub fn to_json(value: &Value) -> Result<String, Error> {
    let mut out = String::new();
    write_json(&mut out, value)?;
    Ok(out)
}

fn write_json(out: &mut String, value: &Value) -> Result<(), Error> {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
        Value::Int(number) => out.push_str(&number.to_string()),
        Value::Float(number) => {
            if !number.is_finite() {
                return Err(Error::NonFinite);
            }
            out.push_str(&float_text(*number));
        }
        Value::Str(text) => write_quoted(out, text),
        Value::List(items) => {
            out.push('[');
            for (at, item) in items.iter().enumerate() {
                if at > 0 {
                    out.push(',');
                }
                write_json(out, item)?;
            }
            out.push(']');
        }
        Value::Map(map) => {
            out.push('{');
            for (at, (key, held)) in map.entries.iter().enumerate() {
                if at > 0 {
                    out.push(',');
                }
                write_quoted(out, key);
                out.push(':');
                write_json(out, held)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

/// Reads JSON into the value tree. A duplicate key keeps the first position and the last
/// value.
pub fn from_json(text: &str) -> Result<Value, Error> {
    let mut cursor = Cursor::new(text);
    let value = json_value(&mut cursor, 0)?;
    cursor.skip_ws();
    if cursor.pos != text.len() {
        return Err(Error::Trailing { pos: cursor.pos });
    }
    Ok(value)
}

fn json_value(cursor: &mut Cursor, depth: usize) -> Result<Value, Error> {
    if depth > MAX_DEPTH {
        return Err(cursor.err("nested deeper than this host allows"));
    }
    cursor.skip_ws();
    let Some(ch) = cursor.peek() else {
        return Err(cursor.err("an empty document"));
    };
    match ch {
        '{' => {
            cursor.next();
            json_object(cursor, depth)
        }
        '[' => {
            cursor.next();
            json_array(cursor, depth)
        }
        '"' => Ok(Value::Str(cursor.quoted()?)),
        't' => cursor.keyword("true").map(|()| Value::Bool(true)),
        'f' => cursor.keyword("false").map(|()| Value::Bool(false)),
        'n' => cursor.keyword("null").map(|()| Value::Null),
        '-' | '0'..='9' => json_number(cursor),
        _ => Err(cursor.err("a character no value starts with")),
    }
}

fn json_object(cursor: &mut Cursor, depth: usize) -> Result<Value, Error> {
    let mut map = Map::new();
    cursor.skip_ws();
    if cursor.peek() == Some('}') {
        cursor.next();
        return Ok(Value::Map(map));
    }
    loop {
        cursor.skip_ws();
        let key = cursor.quoted()?;
        cursor.skip_ws();
        if cursor.next() != Some(':') {
            return Err(cursor.err("an object member without a ':'"));
        }
        let held = json_value(cursor, depth + 1)?;
        map.insert(key, held);
        cursor.skip_ws();
        match cursor.next() {
            Some(',') => continue,
            Some('}') => return Ok(Value::Map(map)),
            _ => return Err(cursor.err("an object member without a ',' or '}'")),
        }
    }
}

fn json_array(cursor: &mut Cursor, depth: usize) -> Result<Value, Error> {
    let mut items = Vec::new();
    cursor.skip_ws();
    if cursor.peek() == Some(']') {
        cursor.next();
        return Ok(Value::List(items));
    }
    loop {
        items.push(json_value(cursor, depth + 1)?);
        cursor.skip_ws();
        match cursor.next() {
            Some(',') => continue,
            Some(']') => return Ok(Value::List(items)),
            _ => return Err(cursor.err("a list item without a ',' or ']'")),
        }
    }
}

fn json_number(cursor: &mut Cursor) -> Result<Value, Error> {
    let start = cursor.pos;
    while let Some(ch) = cursor.peek() {
        if ch.is_ascii_digit() || matches!(ch, '-' | '+' | '.' | 'e' | 'E') {
            cursor.next();
        } else {
            break;
        }
    }
    let text = &cursor.text[start..cursor.pos];
    if text.contains(['.', 'e', 'E']) {
        text.parse::<f64>()
            .map(Value::Float)
            .map_err(|_| Error::Syntax {
                pos: start,
                msg: format!("the number {text} does not parse"),
            })
    } else {
        match text.parse::<i64>() {
            Ok(number) => Ok(Value::Int(number)),
            Err(_) => text.parse::<f64>().map(Value::Float).map_err(|_| Error::Syntax {
                pos: start,
                msg: format!("the number {text} does not parse"),
            }),
        }
    }
}

/// Writes a value as the module's own text form, which unlike JSON can spell `nan` and
/// `inf` and so round trips every float.
#[must_use]
pub fn to_text(value: &Value) -> String {
    let mut out = String::new();
    write_text(&mut out, value);
    out
}

fn write_text(out: &mut String, value: &Value) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
        Value::Int(number) => out.push_str(&number.to_string()),
        Value::Float(number) => out.push_str(&float_text(*number)),
        Value::Str(text) => write_quoted(out, text),
        Value::List(items) => {
            out.push('(');
            for (at, item) in items.iter().enumerate() {
                if at > 0 {
                    out.push(' ');
                }
                write_text(out, item);
            }
            out.push(')');
        }
        Value::Map(map) => {
            out.push('{');
            for (at, (key, held)) in map.entries.iter().enumerate() {
                if at > 0 {
                    out.push(' ');
                }
                write_quoted(out, key);
                out.push(' ');
                write_text(out, held);
            }
            out.push('}');
        }
    }
}

/// Reads the module's own text form, [`to_text`]'s inverse.
pub fn from_text(text: &str) -> Result<Value, Error> {
    let mut cursor = Cursor::new(text);
    let value = text_value(&mut cursor, 0)?;
    cursor.skip_ws();
    if cursor.pos != text.len() {
        return Err(Error::Trailing { pos: cursor.pos });
    }
    Ok(value)
}

fn text_value(cursor: &mut Cursor, depth: usize) -> Result<Value, Error> {
    if depth > MAX_DEPTH {
        return Err(cursor.err("nested deeper than this host allows"));
    }
    cursor.skip_ws();
    let Some(ch) = cursor.peek() else {
        return Err(cursor.err("an empty document"));
    };
    match ch {
        '(' => {
            cursor.next();
            text_list(cursor, depth)
        }
        '{' => {
            cursor.next();
            text_map(cursor, depth)
        }
        '"' => Ok(Value::Str(cursor.quoted()?)),
        _ => text_atom(cursor),
    }
}

fn text_list(cursor: &mut Cursor, depth: usize) -> Result<Value, Error> {
    let mut items = Vec::new();
    loop {
        cursor.skip_ws();
        match cursor.peek() {
            Some(')') => {
                cursor.next();
                return Ok(Value::List(items));
            }
            None => return Err(cursor.err("a list without its ')'")),
            Some(_) => items.push(text_value(cursor, depth + 1)?),
        }
    }
}

fn text_map(cursor: &mut Cursor, depth: usize) -> Result<Value, Error> {
    let mut map = Map::new();
    loop {
        cursor.skip_ws();
        match cursor.peek() {
            Some('}') => {
                cursor.next();
                return Ok(Value::Map(map));
            }
            None => return Err(cursor.err("a map without its '}'")),
            Some(_) => {}
        }
        let key = cursor.quoted()?;
        let held = text_value(cursor, depth + 1)?;
        map.insert(key, held);
    }
}

fn text_atom(cursor: &mut Cursor) -> Result<Value, Error> {
    let start = cursor.pos;
    while let Some(ch) = cursor.peek() {
        if ch.is_whitespace() || "(){}".contains(ch) {
            break;
        }
        cursor.next();
    }
    let token = &cursor.text[start..cursor.pos];
    let malformed = || Error::Syntax {
        pos: start,
        msg: format!("the token {token} is not a value"),
    };
    match token {
        "null" => Ok(Value::Null),
        "true" => Ok(Value::Bool(true)),
        "false" => Ok(Value::Bool(false)),
        "nan" => Ok(Value::Float(f64::NAN)),
        "inf" | "+inf" => Ok(Value::Float(f64::INFINITY)),
        "-inf" => Ok(Value::Float(f64::NEG_INFINITY)),
        _ => {
            if token.is_empty() {
                return Err(cursor.err("a value that is not there"));
            }
            if token.contains(['.', 'e', 'E']) {
                token.parse::<f64>().map(Value::Float).map_err(|_| malformed())
            } else {
                match token.parse::<i64>() {
                    Ok(number) => Ok(Value::Int(number)),
                    Err(_) => token.parse::<f64>().map(Value::Float).map_err(|_| malformed()),
                }
            }
        }
    }
}

/// The shortest decimal that reads back as the same float, with a `.0` so an integral
/// float stays a float.
fn float_text(number: f64) -> String {
    if number.is_nan() {
        return "nan".to_owned();
    }
    if number.is_infinite() {
        return if number.is_sign_negative() {
            "-inf".to_owned()
        } else {
            "inf".to_owned()
        };
    }
    let mut text = number.to_string();
    if !text.contains(['.', 'e', 'E']) {
        text.push_str(".0");
    }
    text
}

fn write_quoted(out: &mut String, text: &str) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if ch < ' ' => out.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out.push('"');
}

struct Cursor<'a> {
    text: &'a str,
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(text: &'a str) -> Self {
        Self { text, pos: 0 }
    }

    fn peek(&self) -> Option<char> {
        self.text[self.pos..].chars().next()
    }

    fn next(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.pos += ch.len_utf8();
        Some(ch)
    }

    fn err(&self, msg: &str) -> Error {
        Error::Syntax {
            pos: self.pos,
            msg: msg.to_owned(),
        }
    }

    fn skip_ws(&mut self) {
        while let Some(ch) = self.peek() {
            if ch.is_whitespace() {
                self.next();
            } else {
                break;
            }
        }
    }

    fn keyword(&mut self, word: &str) -> Result<(), Error> {
        for want in word.chars() {
            if self.next() != Some(want) {
                return Err(self.err("a keyword that does not finish"));
            }
        }
        Ok(())
    }

    fn hex4(&mut self) -> Result<u32, Error> {
        let mut value = 0u32;
        for _ in 0..4 {
            let ch = self.next().ok_or_else(|| self.err("a short \\u escape"))?;
            let digit = ch
                .to_digit(16)
                .ok_or_else(|| self.err("a \\u escape that is not hex"))?;
            value = value * 16 + digit;
        }
        Ok(value)
    }

    fn quoted(&mut self) -> Result<String, Error> {
        if self.next() != Some('"') {
            return Err(self.err("a value that is not a quoted string"));
        }
        let mut out = String::new();
        loop {
            let ch = self.next().ok_or_else(|| self.err("a string without its '\"'"))?;
            match ch {
                '"' => return Ok(out),
                '\\' => {
                    let esc = self.next().ok_or_else(|| self.err("a string without its '\"'"))?;
                    match esc {
                        '"' => out.push('"'),
                        '\\' => out.push('\\'),
                        '/' => out.push('/'),
                        'b' => out.push('\u{8}'),
                        'f' => out.push('\u{c}'),
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        't' => out.push('\t'),
                        'u' => out.push(self.hex_escape()?),
                        _ => return Err(self.err("an escape this format does not know")),
                    }
                }
                ch if ch < ' ' => return Err(self.err("a raw control character in a string")),
                ch => out.push(ch),
            }
        }
    }

    fn hex_escape(&mut self) -> Result<char, Error> {
        let high = self.hex4()?;
        if (0xDC00..=0xDFFF).contains(&high) {
            return Err(self.err("a lone low surrogate"));
        }
        if (0xD800..=0xDBFF).contains(&high) {
            if self.next() != Some('\\') || self.next() != Some('u') {
                return Err(self.err("a high surrogate without its pair"));
            }
            let low = self.hex4()?;
            if !(0xDC00..=0xDFFF).contains(&low) {
                return Err(self.err("a high surrogate paired with the wrong half"));
            }
            let code = 0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00);
            return char::from_u32(code).ok_or_else(|| self.err("a code point that is not a character"));
        }
        char::from_u32(high).ok_or_else(|| self.err("a code point that is not a character"))
    }
}

/// A named codec, so a mod can register its own beside the built-in three.
#[derive(Debug, Clone)]
pub struct Serializer {
    pub name: String,
    encode: fn(&Value) -> Result<Vec<u8>, Error>,
    decode: fn(&[u8]) -> Result<Value, Error>,
}

impl Serializer {
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        encode: fn(&Value) -> Result<Vec<u8>, Error>,
        decode: fn(&[u8]) -> Result<Value, Error>,
    ) -> Self {
        Self {
            name: name.into(),
            encode,
            decode,
        }
    }

    #[must_use]
    pub fn json() -> Self {
        Self::new("json", json_encode, json_decode)
    }

    #[must_use]
    pub fn binary() -> Self {
        Self::new("binary", binary_encode, binary_decode)
    }

    #[must_use]
    pub fn text() -> Self {
        Self::new("text", text_encode, text_decode)
    }
}

fn json_encode(value: &Value) -> Result<Vec<u8>, Error> {
    to_json(value).map(String::into_bytes)
}

fn json_decode(bytes: &[u8]) -> Result<Value, Error> {
    let text = std::str::from_utf8(bytes).map_err(|_| Error::Syntax {
        pos: 0,
        msg: "JSON that is not UTF-8".to_owned(),
    })?;
    from_json(text)
}

fn binary_encode(value: &Value) -> Result<Vec<u8>, Error> {
    Ok(to_binary(value))
}

fn binary_decode(bytes: &[u8]) -> Result<Value, Error> {
    from_binary(bytes)
}

fn text_encode(value: &Value) -> Result<Vec<u8>, Error> {
    Ok(to_text(value).into_bytes())
}

fn text_decode(bytes: &[u8]) -> Result<Value, Error> {
    let text = std::str::from_utf8(bytes).map_err(|_| Error::Syntax {
        pos: 0,
        msg: "text that is not UTF-8".to_owned(),
    })?;
    from_text(text)
}

/// What kind of value a schema field holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Null,
    Bool,
    Int,
    Float,
    Str,
    List,
    Map,
    Any,
}

impl Kind {
    #[must_use]
    pub fn of(value: &Value) -> Self {
        match value {
            Value::Null => Self::Null,
            Value::Bool(_) => Self::Bool,
            Value::Int(_) => Self::Int,
            Value::Float(_) => Self::Float,
            Value::Str(_) => Self::Str,
            Value::List(_) => Self::List,
            Value::Map(_) => Self::Map,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Bool => "bool",
            Self::Int => "int",
            Self::Float => "float",
            Self::Str => "string",
            Self::List => "list",
            Self::Map => "map",
            Self::Any => "any",
        }
    }

    /// Whether a value of this kind fits here; `Float` also takes an `Int`, and `Any`
    /// takes everything.
    #[must_use]
    pub fn accepts(self, value: &Value) -> bool {
        match self {
            Self::Any => true,
            Self::Float => matches!(value, Value::Float(_) | Value::Int(_)),
            other => other == Self::of(value),
        }
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One named field of a schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub kind: Kind,
    pub required: bool,
}

/// How one version's document becomes the next version's. Compared by the versions it
/// covers, since two closures are never the same function.
#[derive(Clone)]
pub struct Migration {
    pub from: u64,
    pub to: u64,
    pub description: String,
    apply: fn(Value) -> Result<Value, Error>,
}

impl fmt::Debug for Migration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Migration")
            .field("from", &self.from)
            .field("to", &self.to)
            .field("description", &self.description)
            .finish()
    }
}

impl PartialEq for Migration {
    fn eq(&self, other: &Self) -> bool {
        self.from == other.from && self.to == other.to
    }
}

impl Migration {
    #[must_use]
    pub fn new(
        from: u64,
        to: u64,
        description: &str,
        apply: fn(Value) -> Result<Value, Error>,
    ) -> Self {
        Self {
            from,
            to,
            description: description.to_owned(),
            apply,
        }
    }

    /// Runs it over a value.
    pub fn run(&self, value: Value) -> Result<Value, Error> {
        (self.apply)(value)
    }
}

/// A named shape a tree is checked against, with the migrations that carry it forward.
#[derive(Debug, Clone, PartialEq)]
pub struct Schema {
    pub name: String,
    pub version: u64,
    pub fields: Vec<Field>,
    pub migrations: Vec<Migration>,
}

impl Schema {
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            version: 0,
            fields: Vec::new(),
            migrations: Vec::new(),
        }
    }

    #[must_use]
    pub fn version(mut self, version: u64) -> Self {
        self.version = version;
        self
    }

    #[must_use]
    pub fn field(mut self, name: &str, kind: Kind) -> Self {
        self.fields.push(Field {
            name: name.to_owned(),
            kind,
            required: false,
        });
        self
    }

    #[must_use]
    pub fn required(mut self, name: &str, kind: Kind) -> Self {
        self.fields.push(Field {
            name: name.to_owned(),
            kind,
            required: true,
        });
        self
    }

    #[must_use]
    pub fn migration(mut self, migration: Migration) -> Self {
        self.migrations.push(migration);
        self
    }

    #[must_use]
    pub fn field_for(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|field| field.name == name)
    }

    /// What is wrong with a tree against this schema: unknown keys, wrong kinds, required
    /// fields that are missing.
    #[must_use]
    pub fn check(&self, value: &Value) -> Vec<String> {
        let Value::Map(map) = value else {
            return vec![format!("{} expects a map, not {}", self.name, Kind::of(value))];
        };
        let mut problems = Vec::new();
        for (key, held) in map.iter() {
            match self.field_for(key) {
                Some(field) if !field.kind.accepts(held) => problems.push(format!(
                    "{key} holds {} where {} is declared",
                    Kind::of(held),
                    field.kind
                )),
                Some(_) => {}
                None => problems.push(format!("{key} is not part of {}", self.name)),
            }
        }
        for field in &self.fields {
            if field.required && !map.contains_key(&field.name) {
                problems.push(format!("{} is required and missing", field.name));
            }
        }
        problems
    }
}

#[derive(Default)]
struct Registry {
    serializers: Vec<Serializer>,
    schemas: Vec<Schema>,
}

fn held() -> &'static Mutex<Registry> {
    static HELD: OnceLock<Mutex<Registry>> = OnceLock::new();
    HELD.get_or_init(|| {
        Mutex::new(Registry {
            serializers: vec![Serializer::json(), Serializer::binary(), Serializer::text()],
            schemas: Vec::new(),
        })
    })
}

fn with<T>(body: impl FnOnce(&mut Registry) -> T) -> T {
    let mut guard = held().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Registers a serializer, refusing a name already taken so a built-in cannot be shadowed.
pub fn register_serializer(serializer: Serializer) -> bool {
    with(|registry| {
        if registry
            .serializers
            .iter()
            .any(|held| held.name == serializer.name)
        {
            crate::warn!("a serializer named {} is already registered", serializer.name);
            return false;
        }
        registry.serializers.push(serializer);
        true
    })
}

/// Removes a serializer by name, and answers whether it was there.
pub fn unregister_serializer(name: &str) -> bool {
    with(|registry| {
        let before = registry.serializers.len();
        registry.serializers.retain(|serializer| serializer.name != name);
        registry.serializers.len() != before
    })
}

/// The serializer registered under a name.
#[must_use]
pub fn get_serializer(name: &str) -> Option<Serializer> {
    with(|registry| {
        registry
            .serializers
            .iter()
            .find(|serializer| serializer.name == name)
            .cloned()
    })
}

/// Encodes a value with the serializer registered under `format`.
pub fn serialize(format: &str, value: &Value) -> Result<Vec<u8>, Error> {
    let serializer =
        get_serializer(format).ok_or_else(|| Error::UnknownFormat(format.to_owned()))?;
    (serializer.encode)(value)
}

/// Decodes bytes with the serializer registered under `format`.
pub fn deserialize(format: &str, bytes: &[u8]) -> Result<Value, Error> {
    let serializer =
        get_serializer(format).ok_or_else(|| Error::UnknownFormat(format.to_owned()))?;
    (serializer.decode)(bytes)
}

/// Registers a schema, replacing one of the same name.
pub fn register_schema(schema: Schema) -> bool {
    with(|registry| {
        registry.schemas.retain(|held| held.name != schema.name);
        registry.schemas.push(schema);
        true
    })
}

/// The schema registered under a name.
#[must_use]
pub fn get_schema(name: &str) -> Option<Schema> {
    with(|registry| {
        registry
            .schemas
            .iter()
            .find(|schema| schema.name == name)
            .cloned()
    })
}

/// Checks a tree against a registered schema, listing what is wrong.
#[must_use]
pub fn validate(schema_name: &str, value: &Value) -> Vec<String> {
    match get_schema(schema_name) {
        Some(schema) => schema.check(value),
        None => vec![format!("no schema named {schema_name} is registered")],
    }
}

/// Moves a value from one version to another through the schema's registered migrations,
/// refusing a step no migration covers.
pub fn migrate(schema_name: &str, from: u64, to: u64, value: Value) -> Result<Value, Error> {
    let schema =
        get_schema(schema_name).ok_or_else(|| Error::NoSchema(schema_name.to_owned()))?;
    let mut current = value;
    let mut step = from;
    while step < to {
        let next = step + 1;
        let migration = schema
            .migrations
            .iter()
            .find(|migration| migration.from == step && migration.to == next)
            .ok_or(Error::NoMigration { from: step, to: next })?;
        current = migration.run(current)?;
        step = next;
    }
    Ok(current)
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encodes bytes as standard base64 with `=` padding.
#[must_use]
pub fn encode_base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = u32::from(chunk[0]);
        let second = u32::from(chunk.get(1).copied().unwrap_or(0));
        let third = u32::from(chunk.get(2).copied().unwrap_or(0));
        let group = (first << 16) | (second << 8) | third;
        out.push(ALPHABET[(group >> 18 & 0x3f) as usize] as char);
        out.push(ALPHABET[(group >> 12 & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[(group >> 6 & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[(group & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// Decodes standard base64, refusing a block that is not four characters or that holds a
/// byte outside the alphabet.
pub fn decode_base64(text: &str) -> Result<Vec<u8>, Error> {
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return Err(Error::BadBase64(bytes.len()));
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    let mut at = 0;
    while at < bytes.len() {
        let block = &bytes[at..at + 4];
        let pad = block.iter().filter(|&&byte| byte == b'=').count();
        if pad > 2 || (pad > 0 && at + 4 != bytes.len()) {
            return Err(Error::BadBase64(at));
        }
        if pad > 0 {
            let first_pad = block.iter().position(|&byte| byte == b'=').unwrap_or(0);
            if block[first_pad..].iter().any(|&byte| byte != b'=') {
                return Err(Error::BadBase64(at + first_pad));
            }
        }
        let mut group = 0u32;
        for (index, &byte) in block.iter().enumerate() {
            let value = if byte == b'=' {
                0
            } else {
                ALPHABET
                    .iter()
                    .position(|&known| known == byte)
                    .ok_or(Error::BadBase64(at + index))? as u32
            };
            group = (group << 6) | value;
        }
        out.push((group >> 16) as u8);
        if pad < 2 {
            out.push((group >> 8) as u8);
        }
        if pad < 1 {
            out.push(group as u8);
        }
        at += 4;
    }
    Ok(out)
}

/// Encodes bytes as lowercase hex.
#[must_use]
pub fn encode_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(hex_digit(byte >> 4));
        out.push(hex_digit(byte & 0x0f));
    }
    out
}

fn hex_digit(nibble: u8) -> char {
    if nibble < 10 {
        (b'0' + nibble) as char
    } else {
        (b'a' + nibble - 10) as char
    }
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Decodes hex, upper or lower case, refusing an odd length or a byte outside the alphabet.
pub fn decode_hex(text: &str) -> Result<Vec<u8>, Error> {
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return Err(Error::BadHex(bytes.len()));
    }
    let mut out = Vec::with_capacity(bytes.len() / 2);
    let mut at = 0;
    while at < bytes.len() {
        let high = hex_value(bytes[at]).ok_or(Error::BadHex(at))?;
        let low = hex_value(bytes[at + 1]).ok_or(Error::BadHex(at + 1))?;
        out.push((high << 4) | low);
        at += 2;
    }
    Ok(out)
}

/// Compresses bytes with PackBits: a control byte `n` copies `n + 1` literal bytes, and a
/// control byte above 128 repeats the next byte `257 - n` times. Worst case it doubles.
#[must_use]
pub fn compress(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let mut run = 1;
        while at + run < bytes.len() && bytes[at + run] == bytes[at] && run < 128 {
            run += 1;
        }
        if run >= 2 {
            out.push((257 - run) as u8);
            out.push(bytes[at]);
            at += run;
            continue;
        }
        let start = at;
        while at < bytes.len() && at - start < 128 {
            if at + 1 < bytes.len() && bytes[at] == bytes[at + 1] {
                break;
            }
            at += 1;
        }
        let literals = &bytes[start..at];
        out.push((literals.len() - 1) as u8);
        out.extend_from_slice(literals);
    }
    out
}

/// Undoes [`compress`], refusing a stream that ends inside a run.
pub fn decompress(bytes: &[u8]) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let control = bytes[at];
        at += 1;
        match control {
            0..=127 => {
                let count = usize::from(control) + 1;
                let end = at.checked_add(count).ok_or(Error::Truncated { pos: at })?;
                let slice = bytes.get(at..end).ok_or(Error::Truncated { pos: at })?;
                out.extend_from_slice(slice);
                at = end;
            }
            129..=255 => {
                let count = 257 - usize::from(control);
                let byte = *bytes.get(at).ok_or(Error::Truncated { pos: at })?;
                at += 1;
                out.extend(std::iter::repeat_n(byte, count));
            }
            _ => {
                return Err(Error::Syntax {
                    pos: at - 1,
                    msg: "a PackBits control byte of 128".to_owned(),
                })
            }
        }
        if out.len() > MAX_BYTES {
            return Err(Error::TooLarge {
                what: "a decompressed block",
                limit: MAX_BYTES,
            });
        }
    }
    Ok(out)
}

const CRC32_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut index = 0;
    while index < 256 {
        let mut value = index as u32;
        let mut bit = 0;
        while bit < 8 {
            value = if value & 1 == 1 {
                (value >> 1) ^ 0xEDB8_8320
            } else {
                value >> 1
            };
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
};

/// CRC-32 (IEEE), the checksum a file header usually carries.
#[must_use]
pub fn checksum(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in bytes {
        crc = (crc >> 8) ^ CRC32_TABLE[((crc ^ u32::from(byte)) & 0xff) as usize];
    }
    !crc
}

/// FNV-1a 64, a fast hash for a key or a bucket.
#[must_use]
pub fn hash(bytes: &[u8]) -> u64 {
    let mut value = 0xcbf2_9ce4_8422_2325u64;
    for &byte in bytes {
        value ^= u64::from(byte);
        value = value.wrapping_mul(0x0000_0100_0000_01b3);
    }
    value
}

/// Why a question about the client's own formats has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "to_nbt" => {
            "writing the client's own NBT dialect would hand a mod a byte layout only the \
             client defines, and this host cannot see it"
                .to_owned()
        }
        "from_nbt" => {
            "reading the client's own NBT dialect means decoding a byte layout only the \
             client defines, which this host cannot do"
                .to_owned()
        }
        "to_game_data" => {
            "writing the client's level format would put bytes into a save this host never \
             owns"
                .to_owned()
        }
        "from_game_data" => {
            "reading the client's level format means parsing a save only the client writes, \
             which this host cannot do"
                .to_owned()
        }
        _ => format!("{what} is not a format this host knows"),
    }
}

/// What this host cannot reach about the client's own formats.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["to_nbt", "from_nbt", "to_game_data", "from_game_data"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

fn refused(what: &str) -> Error {
    let why = why_unknown(what);
    crate::warn!(
        "a mod asked this host for the client's data format: {why}"
    );
    Error::Refused(why)
}

/// Writing the client's NBT dialect. [`Error::Refused`], with [`why_unknown`].
pub fn to_nbt(_value: &Value) -> Result<Vec<u8>, Error> {
    Err(refused("to_nbt"))
}

/// Reading the client's NBT dialect. [`Error::Refused`], with [`why_unknown`].
pub fn from_nbt(_bytes: &[u8]) -> Result<Value, Error> {
    Err(refused("from_nbt"))
}

/// Writing the client's level format. [`Error::Refused`], with [`why_unknown`].
pub fn to_game_data(_value: &Value) -> Result<Vec<u8>, Error> {
    Err(refused("to_game_data"))
}

/// Reading the client's level format. [`Error::Refused`], with [`why_unknown`].
pub fn from_game_data(_bytes: &[u8]) -> Result<Value, Error> {
    Err(refused("from_game_data"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn sample() -> Value {
        let mut inner = Map::new();
        inner.insert("ok", Value::Bool(true));
        inner.insert("none", Value::Null);
        let mut map = Map::new();
        map.insert("name", Value::Str("héllo \"world\"\n".to_owned()));
        map.insert("count", Value::Int(-3));
        map.insert("ratio", Value::Float(1.5));
        map.insert("items", Value::List(vec![Value::Int(1), Value::Float(2.0)]));
        map.insert("meta", Value::Map(inner));
        Value::Map(map)
    }

    #[test]
    fn a_value_tree_round_trips_through_json_and_text() {
        let value = sample();
        let json = to_json(&value).expect("finite floats");
        assert_eq!(json.len(), 99, "the exact JSON the writer produces");
        assert!(
            json.contains("héllo \\\"world\\\"\\n"),
            "utf-8 passes through and escapes are written"
        );
        assert_eq!(from_json(&json).expect("well formed"), value);

        let text = to_text(&value);
        assert_eq!(format!("{value}"), text, "display is the text form");
        assert_eq!(from_text(&text).expect("well formed"), value);
        assert_eq!(text, r#"{"name" "héllo \"world\"\n" "count" -3 "ratio" 1.5 "items" (1 2.0) "meta" {"ok" true "none" null}}"#);

        assert_eq!(
            to_json(&Value::Map({
                let mut map = Map::new();
                map.insert("b", Value::Int(1));
                map.insert("a", Value::Int(2));
                map
            }))
            .expect("finite"),
            r#"{"b":1,"a":2}"#,
            "insertion order, not sorted"
        );
    }

    #[test]
    fn json_refuses_a_malformed_document_with_a_position() {
        assert_eq!(
            from_json(r#"{"a":}"#),
            Err(Error::Syntax {
                pos: 5,
                msg: "a character no value starts with".to_owned()
            })
        );
        assert_eq!(from_json("{"), Err(Error::Syntax { pos: 1, msg: "a value that is not a quoted string".to_owned() }));
        assert_eq!(
            from_json("1 2"),
            Err(Error::Trailing { pos: 2 })
        );
        assert_eq!(
            from_json(r#""\uD800""#),
            Err(Error::Syntax {
                pos: 8,
                msg: "a high surrogate without its pair".to_owned()
            })
        );
        assert_eq!(
            from_json("{}").expect("empty").as_map().expect("a map").len(),
            0
        );
        // A duplicate key keeps the first position and the last value.
        let dup = from_json(r#"{"a":1,"a":2}"#).expect("well formed");
        let map = dup.as_map().expect("a map");
        assert_eq!(map.len(), 1);
        assert_eq!(map.get("a"), Some(&Value::Int(2)));
    }

    #[test]
    fn the_binary_form_round_trips_and_has_exact_bytes() {
        let mut map = Map::new();
        map.insert("name", Value::Str("stone".to_owned()));
        map.insert("count", Value::Int(3));
        let value = Value::Map(map);
        let bytes = to_binary(&value);
        assert_eq!(
            bytes,
            vec![
                0x07, 0x02, 0x04, b'n', b'a', b'm', b'e', 0x05, 0x05, b's', b't', b'o', b'n',
                b'e', 0x05, b'c', b'o', b'u', b'n', b't', 0x03, 0x06,
            ]
        );
        assert_eq!(bytes.len(), 22);
        assert_eq!(from_binary(&bytes).expect("a whole value"), value);
        assert_eq!(to_binary(&Value::Int(-1))[1], 0x01, "zigzag of -1 is 1");
        assert_eq!(to_binary(&Value::Null), vec![0x00]);
        assert_eq!(to_binary(&Value::List(vec![Value::Bool(true)])), vec![0x06, 0x01, 0x02]);
    }

    #[test]
    fn the_binary_form_refuses_truncation_and_over_read() {
        let bytes = to_binary(&sample());
        assert_eq!(
            from_binary(&bytes[..bytes.len() - 1]),
            Err(Error::Truncated { pos: bytes.len() - 1 })
        );
        let mut extra = bytes.clone();
        extra.push(0x00);
        assert_eq!(from_binary(&extra), Err(Error::Trailing { pos: bytes.len() }));
        assert_eq!(
            from_binary(&[0x7f]),
            Err(Error::Syntax {
                pos: 0,
                msg: "unknown tag 0x7f".to_owned()
            })
        );
        // A length prefix longer than the bytes that follow is refused, never guessed.
        assert!(matches!(
            from_binary(&[0x05, 0x40, b'a']),
            Err(Error::Truncated { .. })
        ));
    }

    #[test]
    fn base64_and_hex_round_trip_and_refuse_bad_input() {
        assert_eq!(encode_base64(b"hi"), "aGk=");
        assert_eq!(decode_base64("aGk=").expect("valid"), b"hi");
        assert_eq!(encode_base64(b"h"), "aA==");
        assert_eq!(encode_base64(b"abc"), "YWJj");
        assert_eq!(decode_base64("a").unwrap_err(), Error::BadBase64(1));
        assert_eq!(decode_base64("aGk*").unwrap_err(), Error::BadBase64(3));
        assert!(decode_base64("====").is_err(), "too much padding");

        assert_eq!(encode_hex(b"Hi\n"), "48690a");
        assert_eq!(decode_hex("48690A").expect("upper too"), b"Hi\n");
        assert_eq!(decode_hex("abc").unwrap_err(), Error::BadHex(3));
        assert_eq!(decode_hex("zz").unwrap_err(), Error::BadHex(0));
        assert_eq!(decode_hex(&encode_hex(&sample_bytes())).expect("round trip"), sample_bytes());
    }

    fn sample_bytes() -> Vec<u8> {
        to_binary(&sample())
    }

    #[test]
    fn packbits_round_trips_and_refuses_a_truncated_stream() {
        let data = b"aaaaabbbbbbbbbbbbcd";
        let packed = compress(data);
        assert!(packed.len() < data.len(), "the runs really shrank");
        assert_eq!(decompress(&packed).expect("whole"), data);
        assert_eq!(compress(b"abc"), vec![2, b'a', b'b', b'c'], "three literals");
        assert_eq!(decompress(&compress(b"abc")).expect("whole"), b"abc");
        assert!(matches!(
            decompress(&packed[..packed.len() - 1]),
            Err(Error::Truncated { .. })
        ));
        assert_eq!(compress(&[]), Vec::<u8>::new());
        assert_eq!(decompress(&[]).expect("empty"), Vec::<u8>::new());
    }

    #[test]
    fn a_document_migrates_and_a_missing_step_is_refused() {
        let _serial = serial();
        fn add_flag(value: Value) -> Result<Value, Error> {
            let mut map = match value {
                Value::Map(map) => map,
                other => {
                    let mut map = Map::new();
                    map.insert("value", other);
                    map
                }
            };
            map.insert("flag", Value::Bool(true));
            Ok(Value::Map(map))
        }
        register_schema(
            Schema::new("doc").version(2).migration(Migration::new(
                1,
                2,
                "adds a flag",
                add_flag,
            )),
        );
        let start = Value::Map({
            let mut map = Map::new();
            map.insert("name", Value::Str("probe".to_owned()));
            map
        });
        let migrated = migrate("doc", 1, 2, start.clone()).expect("the step exists");
        assert_eq!(
            migrated.as_map().and_then(|map| map.get("flag")),
            Some(&Value::Bool(true))
        );
        assert_eq!(
            migrate("doc", 1, 3, start.clone()),
            Err(Error::NoMigration { from: 2, to: 3 })
        );
        assert_eq!(migrate("doc", 2, 2, start.clone()).expect("identity"), start);
        assert_eq!(migrate("missing", 1, 2, start), Err(Error::NoSchema("missing".to_owned())));
    }

    #[test]
    fn a_schema_validates_a_tree_and_serializers_are_registered() {
        let _serial = serial();
        register_schema(
            Schema::new("item")
                .field("name", Kind::Str)
                .required("count", Kind::Int)
                .field("weight", Kind::Float),
        );
        let good = Value::Map({
            let mut map = Map::new();
            map.insert("name", Value::Str("stone".to_owned()));
            map.insert("count", Value::Int(3));
            map.insert("weight", Value::Int(2));
            map
        });
        assert!(validate("item", &good).is_empty());
        let mut bad = good.clone();
        if let Value::Map(map) = &mut bad {
            map.insert("name", Value::Int(9));
            map.insert("extra", Value::Null);
        }
        let problems = validate("item", &bad);
        assert!(problems.iter().any(|problem| problem.contains("name holds int")));
        assert!(problems.iter().any(|problem| problem.contains("extra is not part")));
        assert!(validate("item", &Value::Null)[0].contains("expects a map"));
        assert_eq!(validate("nope", &good)[0], "no schema named nope is registered");

        assert!(get_serializer("json").is_some());
        assert!(get_serializer("binary").is_some());
        assert!(get_serializer("text").is_some());
        assert!(register_serializer(Serializer::new(
            "probe",
            text_encode,
            text_decode
        )));
        assert!(
            !register_serializer(Serializer::new("probe", text_encode, text_decode)),
            "a taken name is refused"
        );
        assert_eq!(
            serialize("probe", &good).expect("registered"),
            serialize("text", &good).expect("built in")
        );
        assert_eq!(deserialize("probe", &serialize("probe", &good).unwrap()).unwrap(), good);
        assert_eq!(
            serialize("nope", &good).unwrap_err(),
            Error::UnknownFormat("nope".to_owned())
        );
        assert!(unregister_serializer("probe"));
        assert!(!unregister_serializer("probe"));
    }

    #[test]
    fn checksums_and_hashes_match_their_known_vectors() {
        assert_eq!(checksum(b"123456789"), 0xCBF4_3926);
        assert_eq!(checksum(b""), 0);
        assert_eq!(hash(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(hash(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(hash(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn the_client_own_formats_are_refused() {
        assert_eq!(refusals().len(), 4);
        assert!(why_unknown("to_nbt").contains("NBT"));
        assert!(why_unknown("from_game_data").contains("level format"));
        assert!(matches!(to_nbt(&Value::Null), Err(Error::Refused(_))));
        assert!(matches!(from_nbt(&[]), Err(Error::Refused(_))));
        assert!(matches!(to_game_data(&Value::Null), Err(Error::Refused(_))));
        assert!(matches!(from_game_data(&[]), Err(Error::Refused(_))));
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has a sentence");
            assert!(why_unknown(&what) == why);
        }
        assert!(why_unknown("something_else").contains("something_else"));
    }
}
