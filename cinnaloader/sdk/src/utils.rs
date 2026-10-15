//! Miscellaneous utilities: strings, numbers, parsers, encodings, collections and ids.
//!
//! This section has **no client-owned surface**. Every helper here runs on values a mod
//! already holds — a string, a number, a file on disk — so nothing is refused and nothing
//! pretends to be. There is no "needs a hook" sentence in this module because there is
//! nothing here a hook could reach.
//!
//! * **Strings.** [`truncate_text`] cuts on a character boundary and appends `…`, never mid
//!   byte; [`pad_left`]/[`pad_right`], [`split_once`], [`join`], the four case conversions,
//!   [`hash_string`] (FNV-1a 64), [`pluralize`] against its rule, and a real
//!   [`render_template`] whose missing-value rule leaves the `{name}` in place.
//! * **Numbers.** [`clamp`], [`lerp`], [`round_to`], [`parse_number`] with real errors,
//!   [`humanize_bytes`] (1024 thresholds, `KiB`), [`format_duration`] compact and
//!   [`format_duration_long`] spelled out, and [`format_number`] with grouped thousands.
//! * **Parsing.** A namespaced id, [`parse_resource_id`], that validates both halves;
//!   [`parse_color`] for `#rgb`/`#rrggbb`/`#rrggbbaa` and named colours; [`parse_duration`]
//!   (`2h30m` → milliseconds) with a real error; and [`parse_version`]/[`compare_versions`].
//! * **Collections.** [`deduplicate`] keeping first-seen order, [`group_by`], [`chunk`],
//!   [`windows`], [`top_n_by`] with a stable tie-break, [`partition`], [`zip`], [`unzip`],
//!   [`flatten`].
//! * **Encodings.** base64, percent-encoding, CRC-32 and PackBits reuse the crate's
//!   [`crate::serialization`] codecs so there is one implementation, not two.
//! * **Misc.** [`uuid`]/[`random_uuid`] (v4-shaped, not cryptographic) with a deterministic
//!   [`uuid_from`], [`retry`]/[`retry_with_backoff`]/[`backoff_delays`], [`Throttle`] and
//!   [`Debounce`], [`memoize`], the validators, and the file and directory hashes.
//!
//! ```
//! use cinnaloader::utils;
//!
//! assert_eq!(utils::truncate_text("héllo wörld", 6), "héllo…");
//! assert_eq!(utils::humanize_bytes(1024), "1.0 KiB");
//! assert_eq!(utils::parse_duration("2h30m").unwrap(), 9_000_000);
//! let id = utils::parse_resource_id("mymod:textures/x.png").unwrap();
//! assert_eq!(utils::format_resource_id(&id), "mymod:textures/x.png");
//! ```

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::hash::Hash;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::math::Rgba8;
use crate::modinfo::ModManifest;
use crate::resources::ResourceId;
use crate::serialization::{self, Value};
use crate::version::Version;

/// The longest identifier or namespace this host accepts.
pub const MAX_ID_LEN: usize = 64;
/// The longest path string a validator will look at.
pub const MAX_PATH_LEN: usize = 1024;

/// What a utility refused, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Base64 that is not four-character blocks or holds a byte outside the alphabet.
    BadBase64(usize),
    /// A percent-escape that is truncated or not two hex digits.
    BadPercent(usize),
    /// A JSON document that did not parse.
    BadJson(String),
    /// A version string that is not `major.minor.patch`.
    BadVersion(String),
    /// A namespaced id that is missing a half or leaves the namespace.
    BadResourceId(String),
    /// A hex colour whose length or digits are wrong.
    BadColor(String),
    /// A duration string with no unit or a bad one.
    BadDuration(String),
    /// A number that is empty, unparseable or not finite.
    BadNumber(String),
    /// A path that is empty, absolute or escapes upward.
    BadPath(String),
    /// An identifier outside `[a-z0-9_.-]`.
    BadIdentifier(String),
    /// A namespace outside `[a-z0-9_.-]`.
    BadNamespace(String),
    /// A file or directory that could not be read.
    Io(String),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadBase64(pos) => write!(formatter, "bad base64 at byte {pos}"),
            Self::BadPercent(pos) => write!(formatter, "bad percent-escape at byte {pos}"),
            Self::BadJson(msg) => write!(formatter, "bad json: {msg}"),
            Self::BadVersion(text) => write!(formatter, "bad version {text:?}"),
            Self::BadResourceId(text) => write!(formatter, "bad resource id {text:?}"),
            Self::BadColor(text) => write!(formatter, "bad color {text:?}"),
            Self::BadDuration(text) => write!(formatter, "bad duration {text:?}"),
            Self::BadNumber(text) => write!(formatter, "bad number {text:?}"),
            Self::BadPath(text) => write!(formatter, "bad path {text:?}"),
            Self::BadIdentifier(text) => write!(formatter, "bad identifier {text:?}"),
            Self::BadNamespace(text) => write!(formatter, "bad namespace {text:?}"),
            Self::Io(msg) => write!(formatter, "io: {msg}"),
        }
    }
}

impl std::error::Error for Error {}

/// The text cut to `max` characters, ending in `…` when it had to cut.
#[must_use]
pub fn truncate_text(text: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max - 1).collect();
    out.push('…');
    out
}

/// The text padded on the left to `width` characters.
#[must_use]
pub fn pad_left(text: &str, width: usize, fill: char) -> String {
    let len = text.chars().count();
    if len >= width {
        return text.to_owned();
    }
    let mut out: String = std::iter::repeat_n(fill, width - len).collect();
    out.push_str(text);
    out
}

/// The text padded on the right to `width` characters.
#[must_use]
pub fn pad_right(text: &str, width: usize, fill: char) -> String {
    let len = text.chars().count();
    if len >= width {
        return text.to_owned();
    }
    let mut out = text.to_owned();
    out.extend(std::iter::repeat_n(fill, width - len));
    out
}

/// The text split at its first `sep`, both halves owned.
#[must_use]
pub fn split_once(text: &str, sep: char) -> Option<(String, String)> {
    text.split_once(sep)
        .map(|(head, tail)| (head.to_owned(), tail.to_owned()))
}

/// The parts joined by `sep`.
#[must_use]
pub fn join(parts: &[&str], sep: &str) -> String {
    parts.join(sep)
}

fn words(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    for (index, &ch) in chars.iter().enumerate() {
        if !ch.is_alphanumeric() {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            continue;
        }
        if !current.is_empty() {
            let prev = chars[index - 1];
            let lower_upper = prev.is_lowercase() && ch.is_uppercase();
            let acronym = prev.is_uppercase()
                && ch.is_uppercase()
                && chars.get(index + 1).is_some_and(|next| next.is_lowercase());
            if lower_upper || acronym {
                out.push(std::mem::take(&mut current));
            }
        }
        current.push(ch);
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn capitalize(word: &str) -> String {
    let mut out = String::new();
    for (index, ch) in word.chars().enumerate() {
        if index == 0 {
            out.extend(ch.to_uppercase());
        } else {
            out.extend(ch.to_lowercase());
        }
    }
    out
}

#[must_use]
pub fn to_snake_case(text: &str) -> String {
    words(text)
        .iter()
        .map(|word| word.to_lowercase())
        .collect::<Vec<_>>()
        .join("_")
}

#[must_use]
pub fn to_kebab_case(text: &str) -> String {
    words(text)
        .iter()
        .map(|word| word.to_lowercase())
        .collect::<Vec<_>>()
        .join("-")
}

#[must_use]
pub fn to_camel_case(text: &str) -> String {
    let mut out = String::new();
    for (index, word) in words(text).iter().enumerate() {
        if index == 0 {
            out.push_str(&word.to_lowercase());
        } else {
            out.push_str(&capitalize(word));
        }
    }
    out
}

#[must_use]
pub fn to_pascal_case(text: &str) -> String {
    words(text).iter().map(|word| capitalize(word)).collect()
}

/// FNV-1a 64 of the text, the same hash the rest of the crate uses.
#[must_use]
pub fn hash_string(text: &str) -> u64 {
    serialization::hash(text.as_bytes())
}

/// FNV-1a 64 of the bytes.
#[must_use]
pub fn hash_bytes(bytes: &[u8]) -> u64 {
    serialization::hash(bytes)
}

/// The count with its word: one takes the singular, everything else takes the plural.
#[must_use]
pub fn pluralize(count: i64, singular: &str, plural: &str) -> String {
    if count == 1 {
        format!("1 {singular}")
    } else {
        format!("{count} {plural}")
    }
}

/// The same, choosing the form the named language uses. A language with one form — the CJK
/// family and Vietnamese — never takes the singular; English and the rest take it at one.
#[must_use]
pub fn pluralize_localized(language: &str, count: i64, one: &str, many: &str) -> String {
    let base = language.split(['_', '-']).next().unwrap_or_default();
    let single_form = matches!(base, "ja" | "zh" | "ko" | "vi" | "th");
    if count == 1 && !single_form {
        one.to_owned()
    } else {
        many.to_owned()
    }
}

/// Replaces `{name}` with its value, `{{`/`}}` with a literal brace, and leaves a `{name}`
/// whose value was not supplied exactly as it was written.
#[must_use]
pub fn render_template(template: &str, values: &[(&str, &str)]) -> String {
    let chars: Vec<char> = template.chars().collect();
    let mut out = String::new();
    let mut index = 0;
    while index < chars.len() {
        match chars[index] {
            '{' if chars.get(index + 1) == Some(&'{') => {
                out.push('{');
                index += 2;
            }
            '}' if chars.get(index + 1) == Some(&'}') => {
                out.push('}');
                index += 2;
            }
            '{' => {
                let Some(end) = chars[index + 1..].iter().position(|&ch| ch == '}') else {
                    out.push('{');
                    index += 1;
                    continue;
                };
                let name: String = chars[index + 1..index + 1 + end].iter().collect();
                match values.iter().find(|(key, _)| *key == name) {
                    Some((_, value)) => out.push_str(value),
                    None => {
                        out.push('{');
                        out.push_str(&name);
                        out.push('}');
                    }
                }
                index = index + end + 2;
            }
            other => {
                out.push(other);
                index += 1;
            }
        }
    }
    out
}

/// The value limited to `[min, max]`.
#[must_use]
pub fn clamp(value: f64, min: f64, max: f64) -> f64 {
    value.clamp(min, max)
}

/// The point `t` of the way from `a` to `b`.
#[must_use]
pub fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// The value rounded to `places` decimal places, halves away from zero.
#[must_use]
pub fn round_to(value: f64, places: u32) -> f64 {
    let factor = 10f64.powi(places as i32);
    (value * factor).round() / factor
}

/// Parses a finite decimal, refusing an empty, unparseable or non-finite one.
pub fn parse_number(text: &str) -> Result<f64, Error> {
    let text = text.trim();
    if text.is_empty() {
        return Err(Error::BadNumber(String::new()));
    }
    let value: f64 = text.parse().map_err(|_| Error::BadNumber(text.to_owned()))?;
    if !value.is_finite() {
        return Err(Error::BadNumber(text.to_owned()));
    }
    Ok(value)
}

/// A byte count with 1024 thresholds and IEC units. `999` and `1000` stay in bytes because
/// they are below a kibibyte, which is why the boundary is worth naming.
#[must_use]
pub fn humanize_bytes(bytes: u64) -> String {
    const UNITS: [(u64, &str); 4] = [
        (1 << 40, "TiB"),
        (1 << 30, "GiB"),
        (1 << 20, "MiB"),
        (1 << 10, "KiB"),
    ];
    for (size, unit) in UNITS {
        if bytes >= size {
            return format!("{:.1} {unit}", bytes as f64 / size as f64);
        }
    }
    format!("{bytes} B")
}

/// A duration in milliseconds, compact and with at most two units: `2h30m`, `1m30s`, `1s`.
#[must_use]
pub fn format_duration(millis: u64) -> String {
    if millis < 1000 {
        return format!("{millis}ms");
    }
    let seconds = millis / 1000;
    let (hours, minutes, secs) = (seconds / 3600, (seconds % 3600) / 60, seconds % 60);
    if hours > 0 {
        if minutes > 0 {
            format!("{hours}h{minutes}m")
        } else {
            format!("{hours}h")
        }
    } else if minutes > 0 {
        if secs > 0 {
            format!("{minutes}m{secs}s")
        } else {
            format!("{minutes}m")
        }
    } else {
        format!("{secs}s")
    }
}

/// The same duration spelled out: `2 hours 30 minutes`, every non-zero unit.
#[must_use]
pub fn format_duration_long(millis: u64) -> String {
    let seconds = millis / 1000;
    let (days, hours, minutes, secs) = (
        seconds / 86_400,
        (seconds % 86_400) / 3600,
        (seconds % 3600) / 60,
        seconds % 60,
    );
    let mut parts = Vec::new();
    if days > 0 {
        parts.push(pluralize(days as i64, "day", "days"));
    }
    if hours > 0 {
        parts.push(pluralize(hours as i64, "hour", "hours"));
    }
    if minutes > 0 {
        parts.push(pluralize(minutes as i64, "minute", "minutes"));
    }
    if secs > 0 || parts.is_empty() {
        parts.push(pluralize(secs as i64, "second", "seconds"));
    }
    parts.join(" ")
}

/// A number with grouped thousands and the fraction as written.
#[must_use]
pub fn format_number(value: f64) -> String {
    if !value.is_finite() {
        return value.to_string();
    }
    let text = format!("{value}");
    if text.contains(['e', 'E']) {
        return text;
    }
    let (sign, digits) = match text.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", text.as_str()),
    };
    let (whole, fraction) = digits.split_once('.').unwrap_or((digits, ""));
    let mut grouped = String::new();
    for (index, ch) in whole.chars().enumerate() {
        if index > 0 && (whole.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    match fraction.is_empty() {
        true => format!("{sign}{grouped}"),
        false => format!("{sign}{grouped}.{fraction}"),
    }
}

/// Parses `namespace:path`, validating both halves through [`ResourceId`].
pub fn parse_resource_id(text: &str) -> Result<ResourceId, Error> {
    let (namespace, path) = text
        .split_once(':')
        .ok_or_else(|| Error::BadResourceId(text.to_owned()))?;
    ResourceId::new(namespace, path).ok_or_else(|| Error::BadResourceId(text.to_owned()))
}

/// Renders an id back to `namespace:path`.
#[must_use]
pub fn format_resource_id(id: &ResourceId) -> String {
    format!("{}:{}", id.namespace(), id.path())
}

const NAMED_COLORS: [(&str, [u8; 4]); 20] = [
    ("black", [0, 0, 0, 255]),
    ("white", [255, 255, 255, 255]),
    ("red", [255, 0, 0, 255]),
    ("green", [0, 128, 0, 255]),
    ("lime", [0, 255, 0, 255]),
    ("blue", [0, 0, 255, 255]),
    ("yellow", [255, 255, 0, 255]),
    ("cyan", [0, 255, 255, 255]),
    ("aqua", [0, 255, 255, 255]),
    ("magenta", [255, 0, 255, 255]),
    ("fuchsia", [255, 0, 255, 255]),
    ("gray", [128, 128, 128, 255]),
    ("grey", [128, 128, 128, 255]),
    ("silver", [192, 192, 192, 255]),
    ("maroon", [128, 0, 0, 255]),
    ("olive", [128, 128, 0, 255]),
    ("navy", [0, 0, 128, 255]),
    ("teal", [0, 128, 128, 255]),
    ("orange", [255, 165, 0, 255]),
    ("purple", [128, 0, 128, 255]),
];

/// A named colour, or `#rgb`, `#rrggbb` or `#rrggbbaa`, to eight-bit RGBA.
pub fn parse_color(text: &str) -> Result<Rgba8, Error> {
    let text = text.trim();
    if !text.starts_with('#') {
        let lowered = text.to_ascii_lowercase();
        return NAMED_COLORS
            .iter()
            .find(|(name, _)| *name == lowered)
            .map(|(_, channels)| Rgba8::new(channels[0], channels[1], channels[2], channels[3]))
            .ok_or_else(|| Error::BadColor(text.to_owned()));
    }
    let digits = &text[1..];
    let byte = |pair: &str| u8::from_str_radix(pair, 16).ok();
    match digits.len() {
        3 => {
            let mut channels = [0u8; 3];
            for (index, ch) in digits.chars().enumerate() {
                let nibble = ch.to_digit(16).ok_or_else(|| Error::BadColor(text.to_owned()))?;
                channels[index] = (nibble * 17) as u8;
            }
            Ok(Rgba8::new(channels[0], channels[1], channels[2], 255))
        }
        6 => Ok(Rgba8::new(
            byte(&digits[0..2]).ok_or_else(|| Error::BadColor(text.to_owned()))?,
            byte(&digits[2..4]).ok_or_else(|| Error::BadColor(text.to_owned()))?,
            byte(&digits[4..6]).ok_or_else(|| Error::BadColor(text.to_owned()))?,
            255,
        )),
        8 => Ok(Rgba8::new(
            byte(&digits[0..2]).ok_or_else(|| Error::BadColor(text.to_owned()))?,
            byte(&digits[2..4]).ok_or_else(|| Error::BadColor(text.to_owned()))?,
            byte(&digits[4..6]).ok_or_else(|| Error::BadColor(text.to_owned()))?,
            byte(&digits[6..8]).ok_or_else(|| Error::BadColor(text.to_owned()))?,
        )),
        _ => Err(Error::BadColor(text.to_owned())),
    }
}

/// Parses a duration like `2h30m`, `500ms`, `1d` to milliseconds, refusing what has no unit.
pub fn parse_duration(text: &str) -> Result<u64, Error> {
    let chars: Vec<char> = text.trim().chars().collect();
    if chars.is_empty() {
        return Err(Error::BadDuration(String::new()));
    }
    let mut total: u64 = 0;
    let mut tokens = 0;
    let mut index = 0;
    while index < chars.len() {
        if !chars[index].is_ascii_digit() {
            return Err(Error::BadDuration(text.to_owned()));
        }
        let start = index;
        while index < chars.len() && chars[index].is_ascii_digit() {
            index += 1;
        }
        let value: u64 = chars[start..index]
            .iter()
            .collect::<String>()
            .parse()
            .map_err(|_| Error::BadDuration(text.to_owned()))?;
        let unit = match chars.get(index) {
            Some('m') if chars.get(index + 1) == Some(&'s') => {
                index += 2;
                1
            }
            Some('s') => {
                index += 1;
                1000
            }
            Some('m') => {
                index += 1;
                60_000
            }
            Some('h') => {
                index += 1;
                3_600_000
            }
            Some('d') => {
                index += 1;
                86_400_000
            }
            _ => return Err(Error::BadDuration(text.to_owned())),
        };
        let amount = value
            .checked_mul(unit)
            .and_then(|part| total.checked_add(part))
            .ok_or_else(|| Error::BadDuration(text.to_owned()))?;
        total = amount;
        tokens += 1;
    }
    if tokens == 0 {
        return Err(Error::BadDuration(text.to_owned()));
    }
    Ok(total)
}

/// Parses a version, refusing text that is not `major[.minor[.patch]]`.
pub fn parse_version(text: &str) -> Result<Version, Error> {
    let version = Version::parse(text.trim());
    match version.is_numeric() {
        true => Ok(version),
        false => Err(Error::BadVersion(text.to_owned())),
    }
}

/// Orders two version strings by their numbers, not their text: `1.2.3` is below `1.10.0`.
#[must_use]
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    Version::parse(a).cmp(&Version::parse(b))
}

/// Parses a JSON document through the crate's own parser.
pub fn parse_json(text: &str) -> Result<Value, Error> {
    serialization::from_json(text).map_err(|error| Error::BadJson(error.to_string()))
}

/// Writes a value as indented JSON with two spaces per level.
#[must_use]
pub fn format_json(value: &Value) -> String {
    let mut out = String::new();
    write_json(value, 0, &mut out);
    out
}

fn write_json(value: &Value, depth: usize, out: &mut String) {
    match value {
        Value::List(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push_str("[\n");
            for (index, item) in items.iter().enumerate() {
                push_indent(out, depth + 1);
                write_json(item, depth + 1, out);
                if index + 1 < items.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            push_indent(out, depth);
            out.push(']');
        }
        Value::Map(map) => {
            if map.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push_str("{\n");
            let last = map.len() - 1;
            for (index, (key, item)) in map.iter().enumerate() {
                push_indent(out, depth + 1);
                out.push_str(&serialization::to_json(&Value::Str(key.to_owned())).unwrap_or_default());
                out.push_str(": ");
                write_json(item, depth + 1, out);
                if index != last {
                    out.push(',');
                }
                out.push('\n');
            }
            push_indent(out, depth);
            out.push('}');
        }
        _ => out.push_str(&serialization::to_json(value).unwrap_or_default()),
    }
}

fn push_indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}

/// Standard base64 with `=` padding.
#[must_use]
pub fn base64_encode(bytes: &[u8]) -> String {
    serialization::encode_base64(bytes)
}

/// Decodes standard base64, refusing a bad block.
pub fn base64_decode(text: &str) -> Result<Vec<u8>, Error> {
    serialization::decode_base64(text).map_err(|error| match error {
        serialization::Error::BadBase64(pos) => Error::BadBase64(pos),
        other => Error::BadBase64(other.to_string().len()),
    })
}

/// Percent-encodes everything outside the unreserved set; space becomes `%20`, not `+`.
#[must_use]
pub fn url_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        let unreserved = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~');
        if unreserved {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push_str(&format!("{byte:02X}"));
        }
    }
    out
}

/// Decodes percent-escapes to UTF-8; `+` is a literal plus.
pub fn url_decode(text: &str) -> Result<String, Error> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let pair = bytes
                .get(index + 1..index + 3)
                .ok_or(Error::BadPercent(index))?;
            let high = hex_digit(pair[0]).ok_or(Error::BadPercent(index))?;
            let low = hex_digit(pair[1]).ok_or(Error::BadPercent(index))?;
            out.push((high << 4) | low);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).map_err(|_| Error::BadPercent(index))
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// CRC-32 (IEEE), the checksum a file header usually carries.
#[must_use]
pub fn checksum(bytes: &[u8]) -> u32 {
    serialization::checksum(bytes)
}

/// PackBits compression; worst case it doubles the input.
#[must_use]
pub fn compress(bytes: &[u8]) -> Vec<u8> {
    serialization::compress(bytes)
}

/// Undoes [`compress`], refusing a stream that ends inside a run.
pub fn decompress(bytes: &[u8]) -> Result<Vec<u8>, Error> {
    serialization::decompress(bytes).map_err(|error| Error::Io(error.to_string()))
}

/// The items with duplicates removed, keeping the first time each one was seen.
#[must_use]
pub fn deduplicate<T: Clone + Eq + Hash>(items: &[T]) -> Vec<T> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for item in items {
        if seen.insert(item.clone()) {
            out.push(item.clone());
        }
    }
    out
}

/// The items split into runs of at most `size`; a zero size answers nothing.
#[must_use]
pub fn chunk<T: Clone>(items: &[T], size: usize) -> Vec<Vec<T>> {
    match size {
        0 => Vec::new(),
        _ => items.chunks(size).map(<[T]>::to_vec).collect(),
    }
}

/// Every overlapping run of `size` items.
#[must_use]
pub fn windows<T: Clone>(items: &[T], size: usize) -> Vec<Vec<T>> {
    match size == 0 || size > items.len() {
        true => Vec::new(),
        false => items.windows(size).map(<[T]>::to_vec).collect(),
    }
}

/// The items the predicate accepts, then the ones it rejects.
#[must_use]
pub fn partition<T: Clone>(items: &[T], predicate: impl Fn(&T) -> bool) -> (Vec<T>, Vec<T>) {
    let mut matched = Vec::new();
    let mut rest = Vec::new();
    for item in items {
        match predicate(item) {
            true => matched.push(item.clone()),
            false => rest.push(item.clone()),
        }
    }
    (matched, rest)
}

/// The items grouped by a key, the groups in key order.
#[must_use]
pub fn group_by<T: Clone, K: Ord>(items: &[T], key: impl Fn(&T) -> K) -> BTreeMap<K, Vec<T>> {
    let mut groups: BTreeMap<K, Vec<T>> = BTreeMap::new();
    for item in items {
        groups.entry(key(item)).or_default().push(item.clone());
    }
    groups
}

/// The `n` items with the largest keys, in descending order. Equal keys keep their original
/// order, because the sort is stable and the tie-break is first-seen.
#[must_use]
pub fn top_n_by<T: Clone, K: Ord>(items: &[T], n: usize, key: impl Fn(&T) -> K) -> Vec<T> {
    let mut ranked: Vec<&T> = items.iter().collect();
    ranked.sort_by_key(|item| std::cmp::Reverse(key(item)));
    ranked.truncate(n);
    ranked.into_iter().cloned().collect()
}

/// The two slices paired up to the shorter one.
#[must_use]
pub fn zip<A: Clone, B: Clone>(left: &[A], right: &[B]) -> Vec<(A, B)> {
    left.iter().cloned().zip(right.iter().cloned()).collect()
}

/// A pair list split back into two lists.
#[must_use]
pub fn unzip<A: Clone, B: Clone>(pairs: &[(A, B)]) -> (Vec<A>, Vec<B>) {
    let mut left = Vec::with_capacity(pairs.len());
    let mut right = Vec::with_capacity(pairs.len());
    for (a, b) in pairs {
        left.push(a.clone());
        right.push(b.clone());
    }
    (left, right)
}

/// One level of lists joined into one list.
#[must_use]
pub fn flatten<T: Clone>(items: &[Vec<T>]) -> Vec<T> {
    items.iter().flatten().cloned().collect()
}

/// A v4-shaped UUID from time and a process counter. Not cryptographic; use [`uuid_from`]
/// when a run needs the same id twice.
#[must_use]
pub fn uuid() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or(0);
    uuid_from(seed, counter)
}

/// The same as [`uuid`], under the name the wishlist also uses.
#[must_use]
pub fn random_uuid() -> String {
    uuid()
}

/// A UUID-shaped id that is the same for the same seed and counter.
#[must_use]
pub fn uuid_from(seed: u64, counter: u64) -> String {
    let mut state = seed ^ counter.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut bytes = [0u8; 16];
    for group in bytes.chunks_mut(8) {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        group.copy_from_slice(&z.to_le_bytes());
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let mut out = String::with_capacity(36);
    for (index, byte) in bytes.iter().enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) {
            out.push('-');
        }
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Calls `op` until it succeeds or `attempts` calls have been made, passing the attempt
/// number (from zero) and answering the last error.
pub fn retry<T, E>(attempts: u32, mut op: impl FnMut(u32) -> Result<T, E>) -> Result<T, E> {
    let attempts = attempts.max(1);
    let mut last = None;
    for attempt in 0..attempts {
        match op(attempt) {
            Ok(value) => return Ok(value),
            Err(error) => last = Some(error),
        }
    }
    Err(last.expect("at least one attempt runs"))
}

/// The delay before retry `i`: `base` doubled each time.
#[must_use]
pub fn backoff_delays(attempts: u32, base_millis: u64) -> Vec<u64> {
    (0..attempts).map(|index| base_millis << index.min(63)).collect()
}

/// [`retry`] with exponential backoff: the closure also receives the delay that precedes its
/// attempt, `base` doubling each retry. Nothing sleeps here — the host drives the schedule —
/// so the policy is passed to the caller rather than spent in this process.
pub fn retry_with_backoff<T, E>(
    attempts: u32,
    base_millis: u64,
    mut op: impl FnMut(u32, u64) -> Result<T, E>,
) -> Result<T, E> {
    let attempts = attempts.max(1);
    let mut last = None;
    let mut delay = base_millis;
    for attempt in 0..attempts {
        match op(attempt, delay) {
            Ok(value) => return Ok(value),
            Err(error) => {
                last = Some(error);
                delay = delay.saturating_mul(2);
            }
        }
    }
    Err(last.expect("at least one attempt runs"))
}

/// A limiter that lets at most one call through per interval.
#[derive(Debug, Clone)]
pub struct Throttle {
    interval: u64,
    next: Option<u64>,
}

impl Throttle {
    #[must_use]
    pub fn new(interval_millis: u64) -> Self {
        Self {
            interval: interval_millis,
            next: None,
        }
    }

    /// Whether a call at `now_millis` may proceed.
    pub fn allow(&mut self, now_millis: u64) -> bool {
        match self.next {
            Some(next) if now_millis < next => false,
            _ => {
                self.next = Some(now_millis.saturating_add(self.interval));
                true
            }
        }
    }
}

/// A limiter that fires once the input has been quiet for an interval.
#[derive(Debug, Clone)]
pub struct Debounce {
    interval: u64,
    deadline: u64,
    armed: bool,
}

impl Debounce {
    #[must_use]
    pub fn new(interval_millis: u64) -> Self {
        Self {
            interval: interval_millis,
            deadline: 0,
            armed: false,
        }
    }

    /// Records input at `now_millis`, pushing the deadline out.
    pub fn trigger(&mut self, now_millis: u64) {
        self.deadline = now_millis.saturating_add(self.interval);
        self.armed = true;
    }

    /// Whether the quiet period has elapsed; true at most once per burst.
    pub fn ready(&mut self, now_millis: u64) -> bool {
        if self.armed && now_millis >= self.deadline {
            self.armed = false;
            true
        } else {
            false
        }
    }
}

/// The cached value for `key`, computing and storing it only on a miss.
pub fn memoize<K, V>(cache: &mut HashMap<K, V>, key: K, compute: impl FnOnce() -> V) -> V
where
    K: Eq + Hash + Clone,
    V: Clone,
{
    if let Some(value) = cache.get(&key) {
        return value.clone();
    }
    let value = compute();
    cache.insert(key, value.clone());
    value
}

/// A relative path that stays inside its root.
pub fn validate_path(path: &str) -> Result<(), Error> {
    let path = path.trim();
    if path.is_empty() {
        return Err(Error::BadPath("a path cannot be empty".to_owned()));
    }
    if path.len() > MAX_PATH_LEN {
        return Err(Error::BadPath(path.to_owned()));
    }
    if path.starts_with('/') || path.starts_with('\\') || path.contains('\\') {
        return Err(Error::BadPath(path.to_owned()));
    }
    if path.split('/').any(|component| component == "..") {
        return Err(Error::BadPath(path.to_owned()));
    }
    if path.chars().any(char::is_control) {
        return Err(Error::BadPath(path.to_owned()));
    }
    Ok(())
}

fn valid_id_text(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= MAX_ID_LEN
        && text
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '_' | '.' | '-'))
}

/// An identifier inside `[a-z0-9_.-]`, at most [`MAX_ID_LEN`] bytes.
pub fn validate_identifier(text: &str) -> Result<(), Error> {
    match valid_id_text(text) {
        true => Ok(()),
        false => Err(Error::BadIdentifier(text.to_owned())),
    }
}

/// A namespace inside `[a-z0-9_.-]`, at most [`MAX_ID_LEN`] bytes.
pub fn validate_namespace(text: &str) -> Result<(), Error> {
    match valid_id_text(text) {
        true => Ok(()),
        false => Err(Error::BadNamespace(text.to_owned())),
    }
}

/// Every problem with a manifest, in the order they are checked; empty means valid.
#[must_use]
pub fn validate_manifest(manifest: &ModManifest) -> Vec<String> {
    let mut problems = Vec::new();
    if manifest.id.is_empty() {
        problems.push("the manifest has no id".to_owned());
    } else if let Err(error) = validate_identifier(&manifest.id) {
        problems.push(format!("id {error}"));
    }
    if manifest.name.trim().is_empty() {
        problems.push("the manifest has no name".to_owned());
    }
    if !manifest.version.is_numeric() {
        problems.push(format!("version {} is not numeric", manifest.version));
    }
    for dependency in &manifest.dependencies {
        if let Err(error) = validate_identifier(&dependency.id) {
            problems.push(format!("dependency {error}"));
        }
    }
    problems
}

/// The hash of a file at `path`, or `None` when it cannot be read.
#[must_use]
pub fn get_file_hash(path: &str) -> Option<u64> {
    std::fs::read(path).ok().map(|bytes| serialization::hash(&bytes))
}

/// The same as [`get_file_hash`], under the name the wishlist also uses.
#[must_use]
pub fn get_hash(path: &str) -> Option<u64> {
    get_file_hash(path)
}

/// A deterministic hash over a directory's tree: every file's relative path and contents,
/// in path order, so two trees with the same files answer the same value.
#[must_use]
pub fn get_directory_hash(root: &str) -> Option<u64> {
    let root = Path::new(root);
    let mut entries = Vec::new();
    walk(root, root, &mut entries).ok()?;
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    let mut buffer = Vec::new();
    for (path, bytes) in entries {
        buffer.extend_from_slice(path.as_bytes());
        buffer.push(0);
        buffer.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        buffer.extend_from_slice(&bytes);
    }
    Some(serialization::hash(&buffer))
}

fn walk(root: &Path, base: &Path, out: &mut Vec<(String, Vec<u8>)>) -> std::io::Result<()> {
    let mut children: Vec<PathBuf> = std::fs::read_dir(root)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<_, _>>()?;
    children.sort();
    for path in children {
        if path.is_dir() {
            walk(&path, base, out)?;
            continue;
        }
        let relative = path
            .strip_prefix(base)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        out.push((relative, std::fs::read(&path)?));
    }
    Ok(())
}

/// Whether two files hold the same bytes, or `None` when either cannot be read.
#[must_use]
pub fn compare_files(left: &str, right: &str) -> Option<bool> {
    let left = std::fs::read(left).ok()?;
    let right = std::fs::read(right).ok()?;
    Some(left == right)
}

/// Whether two directory trees hash the same, or `None` when either cannot be read.
#[must_use]
pub fn compare_directories(left: &str, right: &str) -> Option<bool> {
    Some(get_directory_hash(left)? == get_directory_hash(right)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serialization::{Map, Value};

    #[test]
    fn strings_truncate_pad_and_split() {
        assert_eq!(truncate_text("héllo wörld", 6), "héllo…");
        assert_eq!(truncate_text("abc", 5), "abc");
        assert_eq!(truncate_text("abcdef", 0), "");
        assert_eq!(pad_left("42", 5, '0'), "00042");
        assert_eq!(pad_right("42", 5, '.'), "42...");
        assert_eq!(pad_left("abc", 2, '0'), "abc");
        assert_eq!(
            split_once("key=value", '='),
            Some(("key".to_owned(), "value".to_owned()))
        );
        assert_eq!(split_once("nope", '='), None);
        assert_eq!(join(&["a", "b", "c"], ", "), "a, b, c");
    }

    #[test]
    fn case_conversions_agree() {
        assert_eq!(to_snake_case("HTTPServer"), "http_server");
        assert_eq!(to_snake_case("helloWorld"), "hello_world");
        assert_eq!(to_snake_case("hello world-2"), "hello_world_2");
        assert_eq!(to_kebab_case("HelloWorld"), "hello-world");
        assert_eq!(to_camel_case("hello_world"), "helloWorld");
        assert_eq!(to_pascal_case("hello-world"), "HelloWorld");
        assert_eq!(to_pascal_case("someID"), "SomeId");
    }

    #[test]
    fn a_pluraliser_and_template_follow_their_rules() {
        assert_eq!(pluralize(1, "apple", "apples"), "1 apple");
        assert_eq!(pluralize(0, "apple", "apples"), "0 apples");
        assert_eq!(pluralize(2, "apple", "apples"), "2 apples");
        assert_eq!(pluralize_localized("en_us", 1, "apple", "apples"), "apple");
        assert_eq!(pluralize_localized("ja_jp", 1, "apple", "apples"), "apples");
        assert_eq!(
            render_template("Hi {name}, {n} left", &[("name", "Ada"), ("n", "3")]),
            "Hi Ada, 3 left"
        );
        assert_eq!(render_template("{a} {missing}", &[("a", "x")]), "x {missing}");
        assert_eq!(render_template("{{literal}}", &[]), "{literal}");
    }

    #[test]
    fn numbers_round_parse_and_format() {
        assert_eq!(clamp(7.0, 0.0, 3.0), 3.0);
        assert_eq!(lerp(0.0, 10.0, 0.25), 2.5);
        assert_eq!(round_to(1.23456, 2), 1.23);
        assert_eq!(round_to(2.34567, 3), 2.346);
        assert_eq!(round_to(2.5, 0), 3.0);
        assert_eq!(round_to(-2.5, 0), -3.0);
        assert_eq!(parse_number(" 12.5 ").unwrap(), 12.5);
        assert_eq!(parse_number("1e3").unwrap(), 1000.0);
        assert!(parse_number("").is_err());
        assert!(parse_number("inf").is_err());
        assert_eq!(format_number(1234567.0), "1,234,567");
        assert_eq!(format_number(-9876.5), "-9,876.5");
        assert_eq!(format_number(1000.0), "1,000");
    }

    #[test]
    fn byte_and_duration_formatters_hit_their_boundaries() {
        assert_eq!(humanize_bytes(999), "999 B");
        assert_eq!(humanize_bytes(1000), "1000 B");
        assert_eq!(humanize_bytes(1024), "1.0 KiB");
        assert_eq!(humanize_bytes(1_000_000), "976.6 KiB");
        assert_eq!(humanize_bytes(1_048_576), "1.0 MiB");
        assert_eq!(format_duration(0), "0ms");
        assert_eq!(format_duration(1000), "1s");
        assert_eq!(format_duration(90_000), "1m30s");
        assert_eq!(format_duration(9_000_000), "2h30m");
        assert_eq!(format_duration_long(0), "0 seconds");
        assert_eq!(format_duration_long(1000), "1 second");
        assert_eq!(format_duration_long(90_000), "1 minute 30 seconds");
        assert_eq!(format_duration_long(9_000_000), "2 hours 30 minutes");
    }

    #[test]
    fn ids_colors_and_durations_parse_by_rule() {
        let id = parse_resource_id("mymod:textures/x.png").unwrap();
        assert_eq!(id.namespace(), "mymod");
        assert_eq!(id.path(), "textures/x.png");
        assert_eq!(format_resource_id(&id), "mymod:textures/x.png");
        assert!(parse_resource_id("textures").is_err());
        assert!(parse_resource_id("mod:../escape").is_err());
        assert!(parse_resource_id("mod:").is_err());
        assert_eq!(parse_color("#ff8800").unwrap().to_array(), [255, 136, 0, 255]);
        assert_eq!(parse_color("#f80").unwrap().to_array(), [255, 136, 0, 255]);
        assert_eq!(parse_color("#ff880080").unwrap().to_array(), [255, 136, 0, 128]);
        assert_eq!(parse_color("red").unwrap().to_array(), [255, 0, 0, 255]);
        assert!(parse_color("nope").is_err());
        assert_eq!(parse_duration("2h30m").unwrap(), 9_000_000);
        assert_eq!(parse_duration("500ms").unwrap(), 500);
        assert_eq!(parse_duration("1d").unwrap(), 86_400_000);
        assert!(parse_duration("2h30x").is_err());
        assert!(parse_duration("").is_err());
        assert_eq!(compare_versions("1.2.3", "1.10.0"), std::cmp::Ordering::Less);
        assert_eq!(compare_versions("1.0.0", "1.0.0"), std::cmp::Ordering::Equal);
        assert!(parse_version("2.0.0").is_ok());
        assert!(parse_version("beta").is_err());
    }

    #[test]
    fn encodings_round_trip_and_json_formats_pretty() {
        assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
        assert_eq!(base64_decode("aGVsbG8=").unwrap(), b"hello");
        assert!(base64_decode("!!!!").is_err());
        assert_eq!(url_encode("a b/c?d=e"), "a%20b%2Fc%3Fd%3De");
        assert_eq!(url_decode("a%20b%2Fc").unwrap(), "a b/c");
        assert!(url_decode("%zz").is_err());
        assert_eq!(checksum(b"123456789"), 0xCBF4_3926);
        assert_eq!(hash_string(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(hash_string("a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(hash_bytes(b"a"), hash_string("a"));
        assert_eq!(decompress(&compress(b"aaaaa")).unwrap(), b"aaaaa");
        assert_eq!(compress(b"aaaaa"), vec![252, 97]);
        let mut map = Map::new();
        map.insert("a", Value::Int(1));
        map.insert("b", Value::List(vec![Value::Int(2), Value::Int(3)]));
        let value = Value::Map(map);
        let pretty = format_json(&value);
        assert_eq!(pretty, "{\n  \"a\": 1,\n  \"b\": [\n    2,\n    3\n  ]\n}");
        assert_eq!(parse_json(&pretty).unwrap(), value);
        assert!(parse_json("{").is_err());
    }

    #[test]
    fn collections_keep_order() {
        let items = ["b", "a", "b", "c", "a"];
        assert_eq!(deduplicate(&items), ["b", "a", "c"]);
        assert_eq!(chunk(&[1, 2, 3, 4, 5], 2), vec![vec![1, 2], vec![3, 4], vec![5]]);
        assert_eq!(chunk(&[1, 2], 0), Vec::<Vec<i32>>::new());
        assert_eq!(windows(&[1, 2, 3], 2), vec![vec![1, 2], vec![2, 3]]);
        assert_eq!(flatten(&[vec![1, 2], vec![], vec![3]]), vec![1, 2, 3]);
        let (odd, even) = partition(&[1, 2, 3, 4], |n| *n % 2 == 1);
        assert_eq!(odd, vec![1, 3]);
        assert_eq!(even, vec![2, 4]);
        let grouped = group_by(&["ap", "an", "bo"], |word| word.chars().next().unwrap());
        assert_eq!(grouped[&'a'], vec!["ap", "an"]);
        assert_eq!(grouped[&'b'], vec!["bo"]);
        assert_eq!(zip(&[1, 2, 3], &["a", "b"]), vec![(1, "a"), (2, "b")]);
        let (numbers, letters) = unzip(&[(1, "a"), (2, "b")]);
        assert_eq!(numbers, vec![1, 2]);
        assert_eq!(letters, vec!["a", "b"]);
    }

    #[test]
    fn top_n_breaks_ties_by_input_order() {
        let scores = [("a", 5), ("b", 5), ("c", 3), ("d", 7)];
        assert_eq!(top_n_by(&scores, 2, |item| item.1), vec![("d", 7), ("a", 5)]);
        assert_eq!(top_n_by(&scores, 10, |item| item.1).len(), 4);
    }

    #[test]
    fn ids_are_stable_for_a_seed() {
        let first = uuid_from(42, 0);
        assert_eq!(first, uuid_from(42, 0));
        assert_ne!(first, uuid_from(42, 1));
        assert_eq!(first.len(), 36);
        assert_eq!(first.as_bytes()[14], b'4');
        assert!(matches!(first.as_bytes()[19], b'8' | b'9' | b'a' | b'b'));
        assert_eq!(uuid().len(), 36);
        assert_eq!(random_uuid().len(), 36);
    }

    #[test]
    fn retry_throttle_and_memoize_behave() {
        let mut calls = 0;
        let result: Result<i32, &str> = retry(5, |attempt| {
            calls += 1;
            if attempt < 2 {
                Err("boom")
            } else {
                Ok(attempt as i32)
            }
        });
        assert_eq!(result, Ok(2));
        assert_eq!(calls, 3);
        let mut always = 0;
        let failed: Result<i32, ()> = retry(2, |_| {
            always += 1;
            Err(())
        });
        assert!(failed.is_err());
        assert_eq!(always, 2);
        assert_eq!(backoff_delays(4, 100), vec![100, 200, 400, 800]);
        let mut delays = Vec::new();
        let backed: Result<(), ()> = retry_with_backoff(3, 50, |attempt, delay| {
            delays.push(delay);
            if attempt == 1 {
                Ok(())
            } else {
                Err(())
            }
        });
        assert_eq!(backed, Ok(()));
        assert_eq!(delays, vec![50, 100]);
        let mut clock = Throttle::new(10);
        assert!(clock.allow(0));
        assert!(!clock.allow(5));
        assert!(clock.allow(10));
        let mut debounce = Debounce::new(10);
        debounce.trigger(0);
        assert!(!debounce.ready(5));
        assert!(debounce.ready(10));
        assert!(!debounce.ready(20));
        let mut cache = HashMap::new();
        let mut computed = 0;
        for _ in 0..3 {
            let value = memoize(&mut cache, "k", || {
                computed += 1;
                7
            });
            assert_eq!(value, 7);
        }
        assert_eq!(computed, 1);
    }

    #[test]
    fn validators_and_file_hashes_work() {
        assert!(validate_path("assets/lang/en_us.txt").is_ok());
        assert!(validate_path("/etc/passwd").is_err());
        assert!(validate_path("a/../../b").is_err());
        assert!(validate_path("C:\\x").is_err());
        assert!(validate_path("").is_err());
        assert!(validate_identifier("sprint_v2").is_ok());
        assert!(validate_identifier("Sprint").is_err());
        assert!(validate_identifier("bad id").is_err());
        assert!(validate_namespace("mymod").is_ok());
        assert!(validate_namespace("MyMod").is_err());
        let good = ModManifest::new("hello", "Hello", "1.0.0").depends_on("other");
        assert!(validate_manifest(&good).is_empty());
        let bad = ModManifest::new("", "", "not-a-version").depends_on("Bad Id");
        assert_eq!(validate_manifest(&bad).len(), 4);

        let base = temp_dir("hashes");
        let left = base.join("left");
        let right = base.join("right");
        std::fs::create_dir_all(left.join("sub")).unwrap();
        std::fs::create_dir_all(right.join("sub")).unwrap();
        std::fs::write(left.join("one.txt"), b"one").unwrap();
        std::fs::write(left.join("sub/two.txt"), b"two").unwrap();
        std::fs::write(right.join("sub/two.txt"), b"two").unwrap();
        std::fs::write(right.join("one.txt"), b"one").unwrap();
        let one = left.join("one.txt");
        let one = one.to_str().unwrap();
        assert_eq!(get_file_hash(one), Some(hash_bytes(b"one")));
        assert_eq!(get_hash(one), get_file_hash(one));
        let left_root = left.to_str().unwrap();
        let right_root = right.to_str().unwrap();
        assert_eq!(get_directory_hash(left_root), get_directory_hash(right_root));
        assert_eq!(compare_directories(left_root, right_root), Some(true));
        assert_eq!(
            compare_files(one, right.join("one.txt").to_str().unwrap()),
            Some(true)
        );
        std::fs::write(right.join("one.txt"), b"one!").unwrap();
        assert_eq!(
            compare_files(one, right.join("one.txt").to_str().unwrap()),
            Some(false)
        );
        assert_eq!(compare_directories(left_root, right_root), Some(false));
        assert!(get_file_hash("/no/such/file").is_none());
        let _ = std::fs::remove_dir_all(&base);
    }

    fn temp_dir(name: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "cinnaloader-utils-{}-{}-{}",
            std::process::id(),
            name,
            id
        ));
        let _ = std::fs::remove_dir_all(&path);
        path
    }
}
