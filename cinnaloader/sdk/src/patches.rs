//! Patching and transformation: real byte patches over a buffer a mod owns, and the client's
//! own code refused.
//!
//! A patch here is bytes, and bytes are something this host can really handle, so what a mod
//! builds on is genuinely real:
//!
//! * **Pattern search.** A [`Pattern`] mixes hex bytes with `?` wildcards; [`find_all`]
//!   answers every match offset in the buffer, in order, overlapping matches included. A
//!   pattern longer than the buffer, or one that is not there, answers nothing.
//! * **Patching.** [`apply`] refuses a replacement whose length differs from the pattern
//!   ([`PatchError::LengthMismatch`]), refuses an out-of-range offset, and refuses bytes that
//!   are not the pattern rather than writing over them blindly ([`PatchError::PatternMismatch`]
//!   carries the offset, the expected bytes and the ones found).
//! * **A ledger.** Every applied patch records its target, its offset, the bytes it replaced
//!   and the bytes it wrote; [`applied`] reads it back, [`verify`] asks whether the buffer
//!   still holds the replacement, [`revert`] and [`revert_all`] put the original bytes back
//!   for real, and [`original_at`] answers them.
//! * **A chain.** [`apply_all`] applies a list in order, skips a patch whose match overlaps
//!   one already applied, and reports each patch as applied or skipped with its reason.
//! * **Source transformation.** [`Transformer`] is a real find/replace over source-like text,
//!   and [`get_transformation_report`] and [`export_report`] name what was renamed.
//!
//! What is *not* real is the client's own code: its binary, its functions, its loaded classes
//! and its live memory. Patching any of those needs a hook, or an address or class loader this
//! host does not have, so [`register_injection_point`], [`register_redirect`],
//! [`register_overwrite`] and [`register_mixin`] are refused with a sentence in
//! [`why_unknown`], listed by [`refusals`], and never a fake success.
//!
//! ```
//! use cinnaloader::patches::{self, Patch, Pattern, Transformer};
//!
//! let image = [0x48, 0x8b, 0x01, 0xc3, 0x48, 0x8b, 0x02, 0xc3];
//! let wild = Pattern::parse("48 8b ? c3").expect("a pattern");
//! assert_eq!(patches::find_all(&image, &wild), vec![0, 4], "every match, in order");
//!
//! let mut buffer = image;
//! let patch = Patch::new(
//!     "hello:prologue",
//!     Pattern::new(&[0x48, 0x8b, 0x01, 0xc3]),
//!     vec![0x90, 0x90, 0x90, 0x90],
//! )
//! .target("hello:image");
//! let handle = patches::apply(&mut buffer, &patch, 0).expect("the bytes are there");
//! assert!(patches::verify(&buffer, handle), "the replacement is in place");
//! assert_eq!(patches::original_at(handle).as_deref(), Some(&[0x48, 0x8b, 0x01, 0xc3][..]));
//! assert!(patches::revert(&mut buffer, handle));
//! assert_eq!(buffer, image, "reverting restores the bytes");
//!
//! patches::register_transformer(Transformer::new("hello:rename", "old_name", "new_name"));
//! let report = patches::apply_transformations("let old_name = 1;");
//! assert_eq!(report.renames[0].to, "new_name");
//! assert!(patches::unregister_transformer("hello:rename"));
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Mutex, MutexGuard, OnceLock};

/// What went wrong finding or applying a patch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchError {
    /// A pattern with no bytes in it.
    EmptyPattern,
    /// A token that is neither a hex byte nor `?`.
    BadPattern(String),
    /// The replacement is not the pattern's length.
    LengthMismatch { pattern: usize, replacement: usize },
    /// The patch does not fit at that offset.
    OutOfRange { at: usize, pattern: usize, buffer: usize },
    /// The bytes at the offset are not the pattern.
    PatternMismatch { at: usize, expected: Vec<u8>, found: Vec<u8> },
    /// The pattern is not in the buffer.
    NotFound,
    /// No patch is registered under that id.
    UnknownPatch(String),
    /// A patch is already registered under that id.
    Duplicate(String),
    /// No applied patch has that handle.
    UnknownHandle(u64),
    /// A later patch's match overlaps one already applied.
    Overlap { at: usize, with: usize },
}

impl fmt::Display for PatchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyPattern => write!(formatter, "the pattern is empty"),
            Self::BadPattern(token) => write!(formatter, "{token} is not a hex byte or a wildcard"),
            Self::LengthMismatch { pattern, replacement } => write!(
                formatter,
                "the replacement is {replacement} bytes for a {pattern}-byte pattern"
            ),
            Self::OutOfRange { at, pattern, buffer } => write!(
                formatter,
                "a {pattern}-byte patch at offset {at} does not fit a {buffer}-byte buffer"
            ),
            Self::PatternMismatch { at, .. } => {
                write!(formatter, "the bytes at offset {at} are not the pattern")
            }
            Self::NotFound => write!(formatter, "no match for the pattern in the buffer"),
            Self::UnknownPatch(id) => write!(formatter, "no patch registered as {id}"),
            Self::Duplicate(id) => write!(formatter, "a patch named {id} is already registered"),
            Self::UnknownHandle(handle) => write!(formatter, "no applied patch has handle {handle}"),
            Self::Overlap { at, with } => write!(
                formatter,
                "the patch at offset {at} overlaps the patch at offset {with}"
            ),
        }
    }
}

impl std::error::Error for PatchError {}

/// A byte pattern: each byte is either an exact value or a `?` wildcard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    bytes: Vec<u8>,
    wild: Vec<bool>,
}

impl Pattern {
    /// A pattern of exact bytes.
    #[must_use]
    pub fn new(bytes: &[u8]) -> Self {
        Self {
            bytes: bytes.to_vec(),
            wild: vec![false; bytes.len()],
        }
    }

    /// Parses `"48 8b ? c3"`, where `?` or `??` is a wildcard.
    pub fn parse(spec: &str) -> Result<Self, PatchError> {
        let mut bytes = Vec::new();
        let mut wild = Vec::new();
        for token in spec.split_whitespace() {
            if token == "?" || token == "??" {
                bytes.push(0);
                wild.push(true);
            } else {
                let byte = u8::from_str_radix(token, 16)
                    .map_err(|_| PatchError::BadPattern(token.to_owned()))?;
                bytes.push(byte);
                wild.push(false);
            }
        }
        if bytes.is_empty() {
            return Err(PatchError::EmptyPattern);
        }
        Ok(Self { bytes, wild })
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Whether the wildcards are set, one per byte.
    #[must_use]
    pub fn wildcards(&self) -> &[bool] {
        &self.wild
    }

    /// A copy of the bytes, wildcard positions as whatever they carry.
    #[must_use]
    pub fn literal(&self) -> Vec<u8> {
        self.bytes.clone()
    }

    /// Whether the buffer holds this pattern starting at `at`.
    #[must_use]
    pub fn matches_at(&self, buf: &[u8], at: usize) -> bool {
        let Some(end) = at.checked_add(self.bytes.len()) else {
            return false;
        };
        if end > buf.len() {
            return false;
        }
        buf[at..end]
            .iter()
            .zip(&self.wild)
            .enumerate()
            .all(|(i, (byte, wild))| *wild || *byte == self.bytes[i])
    }
}

impl fmt::Display for Pattern {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let parts: Vec<String> = self
            .bytes
            .iter()
            .zip(&self.wild)
            .map(|(byte, wild)| {
                if *wild {
                    "??".to_owned()
                } else {
                    format!("{byte:02x}")
                }
            })
            .collect();
        write!(formatter, "{}", parts.join(" "))
    }
}

/// Every offset at which the pattern matches, in order, overlapping matches included.
#[must_use]
pub fn find_all(buf: &[u8], pattern: &Pattern) -> Vec<usize> {
    if pattern.is_empty() || pattern.len() > buf.len() {
        return Vec::new();
    }
    (0..=buf.len() - pattern.len())
        .filter(|at| pattern.matches_at(buf, *at))
        .collect()
}

/// The first offset at which the pattern matches, where there is one.
#[must_use]
pub fn find_first(buf: &[u8], pattern: &Pattern) -> Option<usize> {
    (0..=buf.len().saturating_sub(pattern.len()))
        .find(|at| pattern.matches_at(buf, *at))
}

/// A patch a mod declares: a pattern to match, a replacement to write, and a target name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Patch {
    pub id: String,
    pub target: String,
    pub pattern: Pattern,
    pub replacement: Vec<u8>,
}

impl Patch {
    #[must_use]
    pub fn new(id: &str, pattern: Pattern, replacement: Vec<u8>) -> Self {
        Self {
            id: id.to_owned(),
            target: String::new(),
            pattern,
            replacement,
        }
    }

    #[must_use]
    pub fn target(mut self, target: &str) -> Self {
        self.target = target.to_owned();
        self
    }

    /// Checks the pattern is non-empty and the replacement is its length.
    pub fn validate(&self) -> Result<(), PatchError> {
        if self.pattern.is_empty() {
            return Err(PatchError::EmptyPattern);
        }
        if self.pattern.len() != self.replacement.len() {
            return Err(PatchError::LengthMismatch {
                pattern: self.pattern.len(),
                replacement: self.replacement.len(),
            });
        }
        Ok(())
    }
}

/// One patch the ledger still holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedPatch {
    pub handle: u64,
    pub id: String,
    pub target: String,
    pub offset: usize,
    pub original: Vec<u8>,
    pub replacement: Vec<u8>,
}

/// What happened to one patch in a chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainStep {
    pub id: String,
    pub applied: bool,
    pub at: Option<usize>,
    pub reason: Option<String>,
}

/// Two registered patches that would match the same bytes of the same target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub target: String,
    pub first: String,
    pub second: String,
}

/// How a registered patch stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchStatus {
    pub id: String,
    pub registered: bool,
    pub order: Option<usize>,
    pub applied: usize,
    pub problem: Option<String>,
}

/// Where in a client method an injection point would sit, if this host could install one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InjectionPoint {
    BeforeMethod,
    AfterMethod,
    AtMethodHead,
    AtMethodReturn,
    BeforeCall,
    AfterCall,
    BeforeFieldRead,
    AfterFieldRead,
    BeforeFieldWrite,
    AfterFieldWrite,
    BeforeInstruction,
    AfterInstruction,
    AtConstructor,
    AtExceptionHandler,
    AtTickStart,
    AtTickEnd,
    AtRenderStage,
}

impl InjectionPoint {
    pub const ALL: [Self; 17] = [
        Self::BeforeMethod,
        Self::AfterMethod,
        Self::AtMethodHead,
        Self::AtMethodReturn,
        Self::BeforeCall,
        Self::AfterCall,
        Self::BeforeFieldRead,
        Self::AfterFieldRead,
        Self::BeforeFieldWrite,
        Self::AfterFieldWrite,
        Self::BeforeInstruction,
        Self::AfterInstruction,
        Self::AtConstructor,
        Self::AtExceptionHandler,
        Self::AtTickStart,
        Self::AtTickEnd,
        Self::AtRenderStage,
    ];

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::BeforeMethod => "before_method",
            Self::AfterMethod => "after_method",
            Self::AtMethodHead => "at_method_head",
            Self::AtMethodReturn => "at_method_return",
            Self::BeforeCall => "before_call",
            Self::AfterCall => "after_call",
            Self::BeforeFieldRead => "before_field_read",
            Self::AfterFieldRead => "after_field_read",
            Self::BeforeFieldWrite => "before_field_write",
            Self::AfterFieldWrite => "after_field_write",
            Self::BeforeInstruction => "before_instruction",
            Self::AfterInstruction => "after_instruction",
            Self::AtConstructor => "at_constructor",
            Self::AtExceptionHandler => "at_exception_handler",
            Self::AtTickStart => "at_tick_start",
            Self::AtTickEnd => "at_tick_end",
            Self::AtRenderStage => "at_render_stage",
        }
    }
}

impl fmt::Display for InjectionPoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.name())
    }
}

/// A real find/replace over source-like text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transformer {
    pub id: String,
    pub find: String,
    pub replace: String,
}

impl Transformer {
    #[must_use]
    pub fn new(id: &str, find: &str, replace: &str) -> Self {
        Self {
            id: id.to_owned(),
            find: find.to_owned(),
            replace: replace.to_owned(),
        }
    }

    /// Replaces every occurrence in `source`, answering the text and the rename where it did.
    #[must_use]
    pub fn transform(&self, source: &str) -> (String, Option<Rename>) {
        let count = source.matches(&self.find).count();
        let renamed = (count > 0).then(|| Rename {
            from: self.find.clone(),
            to: self.replace.clone(),
            count,
        });
        (source.replace(&self.find, &self.replace), renamed)
    }
}

/// One identifier the transformers renamed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rename {
    pub from: String,
    pub to: String,
    pub count: usize,
}

/// What the last transformation did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TransformationReport {
    pub applied: Vec<String>,
    pub renames: Vec<Rename>,
    pub input_bytes: usize,
    pub output_bytes: usize,
    pub count: usize,
}

#[derive(Default)]
struct State {
    definitions: BTreeMap<String, Patch>,
    order: Vec<String>,
    ledger: Vec<AppliedPatch>,
    next_handle: u64,
    transformers: BTreeMap<String, Transformer>,
    report: TransformationReport,
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(State::default()))
}

fn with_state<T>(body: impl FnOnce(&mut State) -> T) -> T {
    let mut guard: MutexGuard<'_, State> = state().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Registers a patch definition under its id. `false` when the id was already taken.
pub fn register(patch: Patch) -> bool {
    with_state(|state| {
        let id = patch.id.clone();
        let fresh = !state.definitions.contains_key(&id);
        state.definitions.insert(id.clone(), patch);
        if fresh {
            state.order.push(id);
        }
        fresh
    })
}

/// Forgets a registered patch.
pub fn unregister(id: &str) -> bool {
    with_state(|state| {
        if state.definitions.remove(id).is_none() {
            return false;
        }
        state.order.retain(|name| name != id);
        true
    })
}

/// A registered patch.
#[must_use]
pub fn get(id: &str) -> Option<Patch> {
    with_state(|state| state.definitions.get(id).cloned())
}

/// Every registered patch, in apply order.
#[must_use]
pub fn get_all() -> Vec<Patch> {
    with_state(|state| {
        state
            .order
            .iter()
            .filter_map(|id| state.definitions.get(id).cloned())
            .collect()
    })
}

/// The order registered patches apply in.
#[must_use]
pub fn get_order() -> Vec<String> {
    with_state(|state| state.order.clone())
}

/// Sets the apply order, which must name every registered patch exactly once.
pub fn set_order(ids: &[String]) -> bool {
    with_state(|state| {
        let known: BTreeSet<&String> = state.definitions.keys().collect();
        let given: BTreeSet<&String> = ids.iter().collect();
        if ids.len() != state.definitions.len() || known != given {
            return false;
        }
        state.order = ids.to_vec();
        true
    })
}

/// How a patch stands, or `None` when it is neither registered nor applied.
#[must_use]
pub fn get_status(id: &str) -> Option<PatchStatus> {
    with_state(|state| {
        let registered = state.definitions.contains_key(id);
        let applied = state.ledger.iter().filter(|entry| entry.id == id).count();
        if !registered && applied == 0 {
            return None;
        }
        let problem = state
            .definitions
            .get(id)
            .and_then(|patch| patch.validate().err())
            .map(|error| error.to_string());
        Some(PatchStatus {
            id: id.to_owned(),
            registered,
            order: state.order.iter().position(|name| name == id),
            applied,
            problem,
        })
    })
}

/// Registered patches on the same target with the same pattern, which would match the same
/// bytes.
#[must_use]
pub fn get_conflicts() -> Vec<Conflict> {
    with_state(|state| {
        let patches: Vec<&Patch> = state
            .order
            .iter()
            .filter_map(|id| state.definitions.get(id))
            .collect();
        let mut conflicts = Vec::new();
        for (index, first) in patches.iter().enumerate() {
            for second in patches.iter().skip(index + 1) {
                if first.target == second.target && first.pattern == second.pattern {
                    conflicts.push(Conflict {
                        target: first.target.clone(),
                        first: first.id.clone(),
                        second: second.id.clone(),
                    });
                }
            }
        }
        conflicts
    })
}

/// Checks every registered patch, answering the ones that do not hold.
pub fn validate() -> Result<(), Vec<PatchError>> {
    with_state(|state| {
        let errors: Vec<PatchError> = state
            .order
            .iter()
            .filter_map(|id| state.definitions.get(id))
            .filter_map(|patch| patch.validate().err())
            .collect();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    })
}

/// The original bytes a registered patch expects, from its pattern.
#[must_use]
pub fn get_original(id: &str) -> Option<Vec<u8>> {
    with_state(|state| state.definitions.get(id).map(|patch| patch.pattern.literal()))
}

/// The bytes a registered patch writes.
#[must_use]
pub fn get_replacement(id: &str) -> Option<Vec<u8>> {
    with_state(|state| {
        state
            .definitions
            .get(id)
            .map(|patch| patch.replacement.clone())
    })
}

/// Replaces the pattern's bytes at `at` with the replacement, recording them in the ledger.
pub fn apply(buf: &mut [u8], patch: &Patch, at: usize) -> Result<u64, PatchError> {
    patch.validate()?;
    let len = patch.pattern.len();
    let Some(end) = at.checked_add(len) else {
        return Err(PatchError::OutOfRange {
            at,
            pattern: len,
            buffer: buf.len(),
        });
    };
    if end > buf.len() {
        return Err(PatchError::OutOfRange {
            at,
            pattern: len,
            buffer: buf.len(),
        });
    }
    if !patch.pattern.matches_at(buf, at) {
        return Err(PatchError::PatternMismatch {
            at,
            expected: patch.pattern.literal(),
            found: buf[at..end].to_vec(),
        });
    }
    let original = buf[at..end].to_vec();
    buf[at..end].copy_from_slice(&patch.replacement);
    Ok(with_state(|state| {
        let handle = state.next_handle;
        state.next_handle += 1;
        state.ledger.push(AppliedPatch {
            handle,
            id: patch.id.clone(),
            target: patch.target.clone(),
            offset: at,
            original,
            replacement: patch.replacement.clone(),
        });
        handle
    }))
}

/// Applies a list in order, at each patch's first match. A patch whose match overlaps one
/// already applied is skipped, and every patch is reported with its reason.
pub fn apply_all(buf: &mut [u8], patches: &[Patch]) -> Vec<ChainStep> {
    let mut steps = Vec::new();
    let mut covered: Vec<(usize, usize)> = Vec::new();
    for patch in patches {
        let Some(at) = find_first(buf, &patch.pattern) else {
            steps.push(ChainStep {
                id: patch.id.clone(),
                applied: false,
                at: None,
                reason: Some(PatchError::NotFound.to_string()),
            });
            continue;
        };
        let end = at + patch.pattern.len();
        let overlap = covered
            .iter()
            .find(|(start, stop)| at < *stop && *start < end)
            .copied();
        if let Some((with, _)) = overlap {
            steps.push(ChainStep {
                id: patch.id.clone(),
                applied: false,
                at: Some(at),
                reason: Some(PatchError::Overlap { at, with }.to_string()),
            });
            continue;
        }
        match apply(buf, patch, at) {
            Ok(_handle) => {
                covered.push((at, end));
                steps.push(ChainStep {
                    id: patch.id.clone(),
                    applied: true,
                    at: Some(at),
                    reason: None,
                });
            }
            Err(error) => steps.push(ChainStep {
                id: patch.id.clone(),
                applied: false,
                at: Some(at),
                reason: Some(error.to_string()),
            }),
        }
    }
    steps
}

/// Whether the buffer still holds the replacement this handle wrote.
#[must_use]
pub fn verify(buf: &[u8], handle: u64) -> bool {
    with_state(|state| {
        let Some(entry) = state.ledger.iter().find(|entry| entry.handle == handle) else {
            return false;
        };
        let Some(end) = entry.offset.checked_add(entry.replacement.len()) else {
            return false;
        };
        buf.get(entry.offset..end) == Some(entry.replacement.as_slice())
    })
}

/// Puts a handle's original bytes back, answering whether the handle was held.
pub fn revert(buf: &mut [u8], handle: u64) -> bool {
    with_state(|state| {
        let Some(index) = state.ledger.iter().position(|entry| entry.handle == handle) else {
            return false;
        };
        let offset = state.ledger[index].offset;
        let original = state.ledger[index].original.clone();
        let Some(end) = offset.checked_add(original.len()) else {
            return false;
        };
        if end > buf.len() {
            return false;
        }
        buf[offset..end].copy_from_slice(&original);
        state.ledger.remove(index);
        true
    })
}

/// The same as [`revert`], under the wishlist's name.
pub fn remove(buf: &mut [u8], handle: u64) -> bool {
    revert(buf, handle)
}

/// Reverts every applied patch, most recent first, answering how many were undone.
pub fn revert_all(buf: &mut [u8]) -> usize {
    let handles: Vec<u64> = with_state(|state| {
        state.ledger.iter().rev().map(|entry| entry.handle).collect()
    });
    handles.into_iter().filter(|handle| revert(buf, *handle)).count()
}

/// The same as [`revert_all`], under the wishlist's name.
pub fn remove_all(buf: &mut [u8]) -> usize {
    revert_all(buf)
}

/// The same as [`revert_all`], under the wishlist's name.
pub fn rollback(buf: &mut [u8]) -> usize {
    revert_all(buf)
}

/// Every applied patch the ledger still holds, oldest first.
#[must_use]
pub fn applied() -> Vec<AppliedPatch> {
    with_state(|state| state.ledger.clone())
}

/// The original bytes a handle recorded.
#[must_use]
pub fn original_at(handle: u64) -> Option<Vec<u8>> {
    with_state(|state| {
        state
            .ledger
            .iter()
            .find(|entry| entry.handle == handle)
            .map(|entry| entry.original.clone())
    })
}

/// Registers a transformer under its id. `false` when the id was already taken.
pub fn register_transformer(transformer: Transformer) -> bool {
    with_state(|state| {
        let fresh = !state.transformers.contains_key(&transformer.id);
        state.transformers.insert(transformer.id.clone(), transformer);
        fresh
    })
}

/// Forgets a transformer.
pub fn unregister_transformer(id: &str) -> bool {
    with_state(|state| state.transformers.remove(id).is_some())
}

/// A registered transformer.
#[must_use]
pub fn get_transformer(id: &str) -> Option<Transformer> {
    with_state(|state| state.transformers.get(id).cloned())
}

/// Runs every registered transformer over `source`, in id order, and stores the report.
pub fn apply_transformations(source: &str) -> TransformationReport {
    with_state(|state| {
        let mut text = source.to_owned();
        let mut applied = Vec::new();
        let mut renames = Vec::new();
        let mut count = 0;
        for (id, transformer) in &state.transformers {
            let (next, rename) = transformer.transform(&text);
            if let Some(rename) = rename {
                count += rename.count;
                applied.push(id.clone());
                renames.push(rename);
            }
            text = next;
        }
        state.report = TransformationReport {
            applied,
            renames,
            input_bytes: source.len(),
            output_bytes: text.len(),
            count,
        };
        state.report.clone()
    })
}

/// What the last [`apply_transformations`] produced.
#[must_use]
pub fn get_transformation_report() -> TransformationReport {
    with_state(|state| state.report.clone())
}

/// The last report as text, one rename per line.
#[must_use]
pub fn export_report() -> String {
    with_state(|state| {
        let report = &state.report;
        let mut lines = vec![format!(
            "{} transformer(s), {} replacement(s), {} -> {} bytes",
            report.applied.len(),
            report.count,
            report.input_bytes,
            report.output_bytes
        )];
        for rename in &report.renames {
            lines.push(format!("{} -> {} ({})", rename.from, rename.to, rename.count));
        }
        lines.join("\n")
    })
}

fn refused(what: &str) -> bool {
    crate::warn!(
        "a mod asked this host to patch client code: {}",
        why_unknown(what)
    );
    false
}

/// Why patching the client's own code has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "binary" => {
            "the client's binary is mapped from its image, and patching it needs a hook \
             installed before its threads start, which this host does not have"
                .to_owned()
        }
        "function" => {
            "redirecting a client function needs a native detour at a known address, and this \
             host has no address table for the client's functions"
                .to_owned()
        }
        "class" => {
            "overwriting or mixing into a loaded class needs the client's own class loader, \
             which a mod host never sees"
                .to_owned()
        }
        "memory" => {
            "writing the client's live memory needs its heap and object layout, which this \
             host can neither map nor validate"
                .to_owned()
        }
        _ => format!("{what} is not something this host can patch"),
    }
}

/// What this host cannot patch in this section.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["binary", "function", "class", "memory"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

/// Patching the client's binary image. `false`.
pub fn patch_client_binary(_path: &str) -> bool {
    refused("binary")
}

/// Redirecting a client function. `false`.
pub fn patch_client_function(_name: &str) -> bool {
    refused("function")
}

/// Overwriting a loaded client class. `false`.
pub fn patch_client_class(_name: &str) -> bool {
    refused("class")
}

/// Writing the client's live memory. `false`.
pub fn patch_client_memory(_name: &str) -> bool {
    refused("memory")
}

/// An injection point inside a client method. `false`: it would need a hook.
pub fn register_injection_point(_point: InjectionPoint, _target: &str) -> bool {
    refused("function")
}

/// Redirecting a call inside a client method. `false`: it would need a hook.
pub fn register_redirect(_target: &str, _from: &str, _to: &str) -> bool {
    refused("function")
}

/// Overwriting a method body. `false`: it would need a hook.
pub fn register_overwrite(_target: &str, _replacement: &str) -> bool {
    refused("class")
}

/// Mixing into a client class. `false`: it would need a hook.
pub fn register_mixin(_target: &str) -> bool {
    refused("class")
}

#[cfg(test)]
fn reset() {
    with_state(|state| *state = State::default());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    #[test]
    fn a_wildcard_pattern_answers_every_match_in_order() {
        let image = [0x48u8, 0x8b, 0x01, 0xc3, 0x48, 0x8b, 0x02, 0xc3];
        let wild = Pattern::parse("48 8b ? c3").unwrap();
        assert_eq!(wild.len(), 4);
        assert_eq!(wild.to_string(), "48 8b ?? c3");
        assert_eq!(find_all(&image, &wild), vec![0, 4]);
        assert_eq!(find_all(&image, &Pattern::new(&[0x48, 0x8b, 0x01, 0xc3])), vec![0]);

        let runs = [0x90u8, 0x90, 0x90, 0x90, 0x90];
        assert_eq!(
            find_all(&runs, &Pattern::new(&[0x90, 0x90, 0x90, 0x90])),
            vec![0, 1],
            "overlapping matches are found"
        );
        assert!(find_all(&image, &Pattern::new(&[0x48, 0x8b, 0x01, 0xc3, 0x90])).is_empty());
        assert!(find_all(&image, &Pattern::new(&[0xde, 0xad])).is_empty());
        assert_eq!(find_first(&image, &wild), Some(0));
        assert!(matches!(Pattern::parse("zz"), Err(PatchError::BadPattern(_))));
        assert_eq!(Pattern::parse("").unwrap_err(), PatchError::EmptyPattern);
    }

    #[test]
    fn a_patch_replaces_bytes_and_the_ledger_records_them() {
        let _serial = serial();
        reset();
        let original = [0x48u8, 0x8b, 0x01, 0xc3, 0x90];
        let mut buffer = original;
        let patch = Patch::new(
            "hello:prologue",
            Pattern::new(&[0x48, 0x8b, 0x01, 0xc3]),
            vec![0x90, 0x90, 0x90, 0x90],
        )
        .target("hello:image");
        assert!(register(patch.clone()));
        assert_eq!(get("hello:prologue").unwrap().target, "hello:image");
        assert_eq!(
            get_original("hello:prologue").as_deref(),
            Some(&[0x48, 0x8b, 0x01, 0xc3][..])
        );
        assert_eq!(
            get_replacement("hello:prologue").as_deref(),
            Some(&[0x90, 0x90, 0x90, 0x90][..])
        );

        let handle = apply(&mut buffer, &patch, 0).unwrap();
        assert_eq!(&buffer[0..4], &[0x90, 0x90, 0x90, 0x90]);
        assert_eq!(buffer[4], 0x90, "the byte past the patch is untouched");
        assert!(verify(&buffer, handle));
        let log = applied();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].handle, handle);
        assert_eq!(log[0].offset, 0);
        assert_eq!(log[0].original, vec![0x48, 0x8b, 0x01, 0xc3]);
        assert_eq!(log[0].replacement, vec![0x90, 0x90, 0x90, 0x90]);
        assert_eq!(
            original_at(handle).as_deref(),
            Some(&[0x48, 0x8b, 0x01, 0xc3][..])
        );
    }

    #[test]
    fn a_patch_at_a_bad_offset_or_on_a_mismatch_is_refused() {
        let _serial = serial();
        reset();
        let mut buffer = [0x48u8, 0x8b, 0x01, 0xc3];
        let patch = Patch::new(
            "hello:p",
            Pattern::new(&[0x48, 0x8b, 0x01, 0xc3]),
            vec![0x90, 0x90, 0x90, 0x90],
        );
        let long = Patch::new("hello:long", Pattern::new(&[0x48]), vec![0x90, 0x90]);
        assert_eq!(
            apply(&mut buffer, &long, 0),
            Err(PatchError::LengthMismatch {
                pattern: 1,
                replacement: 2
            })
        );
        assert_eq!(
            apply(&mut buffer, &patch, 1),
            Err(PatchError::OutOfRange {
                at: 1,
                pattern: 4,
                buffer: 4
            })
        );
        assert_eq!(
            apply(&mut buffer, &patch, 4),
            Err(PatchError::OutOfRange {
                at: 4,
                pattern: 4,
                buffer: 4
            })
        );
        let other = Patch::new(
            "hello:other",
            Pattern::new(&[0x48, 0x8b, 0x02, 0xc3]),
            vec![0xcc, 0xcc, 0xcc, 0xcc],
        );
        assert_eq!(
            apply(&mut buffer, &other, 0),
            Err(PatchError::PatternMismatch {
                at: 0,
                expected: vec![0x48, 0x8b, 0x02, 0xc3],
                found: vec![0x48, 0x8b, 0x01, 0xc3]
            })
        );
        assert_eq!(buffer, [0x48, 0x8b, 0x01, 0xc3], "a refused patch changes nothing");
        assert!(applied().is_empty());
    }

    #[test]
    fn reverting_restores_the_buffer_byte_for_byte() {
        let _serial = serial();
        reset();
        let original = [0x11u8, 0x22, 0x33, 0x44, 0x55];
        let mut buffer = original;
        let first = Patch::new("hello:first", Pattern::new(&[0x11, 0x22]), vec![0xaa, 0xbb]);
        let second = Patch::new("hello:second", Pattern::new(&[0x44, 0x55]), vec![0xcc, 0xdd]);
        let a = apply(&mut buffer, &first, 0).unwrap();
        let b = apply(&mut buffer, &second, 3).unwrap();
        assert_eq!(buffer, [0xaa, 0xbb, 0x33, 0xcc, 0xdd]);
        assert!(revert(&mut buffer, a));
        assert_eq!(buffer, [0x11, 0x22, 0x33, 0xcc, 0xdd]);
        assert!(!revert(&mut buffer, a), "a handle reverts once");
        assert_eq!(revert_all(&mut buffer), 1);
        assert_eq!(buffer, original, "byte for byte");
        assert!(applied().is_empty());
        assert_eq!(rollback(&mut buffer), 0);
        assert!(!verify(&buffer, b));
    }

    #[test]
    fn a_chain_applies_in_order_and_skips_an_overlap() {
        let _serial = serial();
        reset();
        let mut buffer = [0x48u8, 0x8b, 0x01, 0xc3, 0x48, 0x8b, 0x02, 0xc3];
        let a = Patch::new(
            "hello:a",
            Pattern::new(&[0x48, 0x8b, 0x01, 0xc3]),
            vec![0x90, 0x90, 0x90, 0x90],
        );
        let b = Patch::new(
            "hello:b",
            Pattern::new(&[0x90, 0x90, 0x48, 0x8b]),
            vec![0xcc, 0xcc, 0xcc, 0xcc],
        );
        let gone = Patch::new("hello:gone", Pattern::new(&[0xde, 0xad]), vec![0xde, 0xad]);
        let steps = apply_all(&mut buffer, &[a, b, gone]);
        assert_eq!(steps.len(), 3);
        assert!(steps[0].applied);
        assert_eq!(steps[0].at, Some(0));
        assert!(!steps[1].applied);
        assert_eq!(steps[1].at, Some(2));
        assert_eq!(
            steps[1].reason.as_deref(),
            Some("the patch at offset 2 overlaps the patch at offset 0")
        );
        assert!(!steps[2].applied);
        assert_eq!(steps[2].at, None);
        assert!(steps[2].reason.as_deref().unwrap().contains("no match"));
        assert_eq!(&buffer[0..4], &[0x90, 0x90, 0x90, 0x90]);
        assert_eq!(applied().len(), 1);
    }

    #[test]
    fn the_registry_reports_order_conflicts_and_status() {
        let _serial = serial();
        reset();
        let a = Patch::new("hello:a", Pattern::new(&[0x01]), vec![0x11]).target("hello:image");
        let b = Patch::new("hello:b", Pattern::new(&[0x02]), vec![0x22]).target("hello:image");
        let dup = Patch::new("hello:dup", Pattern::new(&[0x01]), vec![0x33]).target("hello:image");
        assert!(register(a));
        assert!(register(b));
        assert!(register(dup));
        assert!(
            !register(Patch::new("hello:b", Pattern::new(&[0x02]), vec![0x22])),
            "replacing answers false"
        );
        assert_eq!(
            get_all().iter().map(|patch| patch.id.clone()).collect::<Vec<_>>(),
            vec!["hello:a", "hello:b", "hello:dup"]
        );
        let conflicts = get_conflicts();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(
            conflicts[0],
            Conflict {
                target: "hello:image".to_owned(),
                first: "hello:a".to_owned(),
                second: "hello:dup".to_owned()
            }
        );
        let reversed = vec![
            "hello:dup".to_owned(),
            "hello:b".to_owned(),
            "hello:a".to_owned(),
        ];
        assert!(set_order(&reversed));
        assert_eq!(get_order(), reversed);
        assert!(!set_order(&["hello:a".to_owned()]), "order must name every patch");
        assert_eq!(get_status("hello:a").unwrap().order, Some(2));
        assert!(unregister("hello:dup"));
        assert!(!unregister("hello:dup"));
        assert!(validate().is_ok());

        register(Patch::new("hello:bad", Pattern::new(&[0x01]), vec![0x11, 0x22]));
        assert!(validate().is_err());
        assert!(get_status("hello:bad").unwrap().problem.is_some());
        assert!(get_status("hello:none").is_none());
    }

    #[test]
    fn a_transformer_renames_source_text_and_reports_it() {
        let _serial = serial();
        reset();
        assert!(register_transformer(Transformer::new(
            "hello:rename",
            "old_name",
            "new_name"
        )));
        assert!(
            !register_transformer(Transformer::new("hello:rename", "x", "y")),
            "replacing answers false, and replaces"
        );
        assert!(!register_transformer(Transformer::new(
            "hello:rename",
            "old_name",
            "new_name"
        )));
        assert!(register_transformer(Transformer::new(
            "hello:add",
            "count",
            "total_value"
        )));
        let source = "let old_name = old_name + count;";
        let report = apply_transformations(source);
        assert_eq!(
            report.applied,
            vec!["hello:add".to_owned(), "hello:rename".to_owned()]
        );
        assert_eq!(report.count, 3);
        assert_eq!(report.input_bytes, 32);
        assert_eq!(report.output_bytes, 38);
        let renamed = report
            .renames
            .iter()
            .find(|rename| rename.from == "old_name")
            .unwrap();
        assert_eq!(renamed.to, "new_name");
        assert_eq!(renamed.count, 2);
        assert_eq!(get_transformation_report(), report);
        let exported = export_report();
        assert!(exported.contains("old_name -> new_name (2)"));
        assert!(exported.contains("count -> total_value (1)"));
        assert_eq!(
            get_transformer("hello:add").unwrap().replace,
            "total_value"
        );
        assert!(unregister_transformer("hello:add"));
        assert!(!unregister_transformer("hello:add"));
    }

    #[test]
    fn patching_the_clients_code_is_refused_with_its_own_reason() {
        assert!(!patch_client_binary("/proc/self/exe"));
        assert!(!patch_client_function("Client.tick"));
        assert!(!patch_client_class("net.minecraft.Client"));
        assert!(!patch_client_memory("Client"));
        assert!(!register_injection_point(
            InjectionPoint::BeforeCall,
            "Client.tick"
        ));
        assert!(!register_redirect("Client.tick", "a", "b"));
        assert!(!register_overwrite("Client.class", "..."));
        assert!(!register_mixin("Client.class"));
        assert_eq!(InjectionPoint::ALL.len(), 17);
        assert_eq!(InjectionPoint::AtRenderStage.to_string(), "at_render_stage");
        let reasons = refusals();
        assert_eq!(reasons.len(), 4);
        for (what, why) in &reasons {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, &format!("{what} is not something this host can patch"));
        }
        assert!(why_unknown("binary").contains("binary"));
        assert!(why_unknown("memory").contains("memory"));
        assert!(why_unknown("nonsense").contains("nonsense"));
    }
}
