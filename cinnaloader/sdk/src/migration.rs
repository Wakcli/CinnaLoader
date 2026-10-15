//! Resource and API migration: named steps that carry a document from one schema version to
//! the next, run for real against a [`Value`] tree.
//!
//! A [`Step`] names the kind of thing it migrates ([`Kind`]), the versions it spans and what
//! it changes — a declared rule ([`Step::rename`], [`Step::move_to`], [`Step::remove`],
//! [`Step::set`]) or a closure ([`Step::run`]). Registered by name, [`run_all`] builds the
//! chain from a document's version to a target and applies each step in order, [`run`] applies
//! one, and [`dry_run`] reports what each step would change while leaving the document exactly
//! as it found it.
//!
//! A step the chain needs but none covers is [`Error::MissingStep`] naming the gap; a chain
//! that revisits a version is [`Error::Cycle`]. [`deprecate`] maps an old id to its
//! replacement, and migration rewrites the key as it warns. The rule [`check`] applies is the
//! document version against this build's supported version ([`get_version`]): equal is
//! [`Support::Supported`], below is [`Support::Older`], above is the future and is refused.
//!
//! What this host cannot migrate is the client's own data — the level save, player data,
//! resource packs and its internal API — refused by [`migrate_client`] with [`why_unknown`].
//!
//! ```
//! use cinnaloader::migration::{self, Kind, Support, Step};
//! use cinnaloader::serialization::{Map, Value, to_binary};
//!
//! migration::clear();
//! migration::set_version(3);
//! migration::deprecate("demo:old", "demo:new");
//! migration::register(Step::of(Kind::Config, 1, 2, "rename old_name").rename("old_name", "name"));
//! migration::register(Step::of(Kind::Config, 2, 3, "stamp schema").set("schema", Value::Int(3)));
//!
//! let mut document = Value::Map(Map::new());
//! if let Value::Map(map) = &mut document {
//!     map.insert("version", Value::Int(1));
//!     map.insert("old_name", Value::Str("gem".into()));
//!     map.insert("demo:old", Value::Int(7));
//! }
//!
//! let before = to_binary(&document);
//! let preview = migration::dry_run(Kind::Config, &document, 1, 3).expect("the chain exists");
//! assert_eq!(to_binary(&document), before, "the dry run changed nothing");
//! assert!(preview.changes.iter().any(|change| change.path == "old_name"));
//!
//! let run = migration::run_all(Kind::Config, &mut document, 1, 3).expect("the chain exists");
//! assert_eq!(run.applied, vec!["rename old_name".to_owned(), "stamp schema".to_owned()]);
//! let map = document.as_map().expect("a map");
//! assert!(map.contains_key("name") && !map.contains_key("old_name"));
//! assert!(map.contains_key("demo:new") && !map.contains_key("demo:old"));
//! assert_eq!(migration::check(4), Err(migration::Error::FromFuture { found: 4, supported: 3 }));
//! assert_eq!(migration::compatibility(2), Support::Older);
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

use crate::serialization::{Map, Value};

/// The kinds of thing a step can migrate, one per wishlist register call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Config,
    Save,
    Resource,
    Registry,
    Api,
    Manifest,
}

impl Kind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Config => "config",
            Self::Save => "save",
            Self::Resource => "resource",
            Self::Registry => "registry",
            Self::Api => "api",
            Self::Manifest => "manifest",
        }
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Whether a document's version is the one this build writes, older, or from the future.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Support {
    Supported,
    Older,
    Newer,
}

impl Support {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Supported => "supported",
            Self::Older => "older",
            Self::Newer => "newer",
        }
    }
}

/// Where a step stands: never run, done, or failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Status {
    Idle,
    Pending,
    Completed,
    Failed,
}

impl Status {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Pending => "pending",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }
}

/// What one declared rule did to a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    Renamed,
    Moved,
    Removed,
    Set,
    RenamedId,
    Custom,
}

impl Action {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Renamed => "rename",
            Self::Moved => "move",
            Self::Removed => "remove",
            Self::Set => "set",
            Self::RenamedId => "rename_id",
            Self::Custom => "custom",
        }
    }
}

/// One change, named by the path it touched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub path: String,
    pub action: Action,
    pub detail: String,
}

impl Change {
    fn new(path: &str, action: Action, detail: &str) -> Self {
        Self {
            path: path.to_owned(),
            action,
            detail: detail.to_owned(),
        }
    }
}

/// A chain that ran: the steps applied, in order, and every change each made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub applied: Vec<String>,
    pub changes: Vec<Change>,
}

impl Run {
    /// The run as a [`Value`], the shape [`Run::json`] writes.
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert(
            "applied",
            Value::List(
                self.applied
                    .iter()
                    .map(|name| Value::str(name.as_str()))
                    .collect(),
            ),
        );
        let changes = self
            .changes
            .iter()
            .map(|change| {
                let mut entry = Map::new();
                entry.insert("path", Value::str(change.path.as_str()));
                entry.insert("action", Value::str(change.action.as_str()));
                entry.insert("detail", Value::str(change.detail.as_str()));
                Value::Map(entry)
            })
            .collect();
        map.insert("changes", Value::List(changes));
        Value::Map(map)
    }

    /// The run as JSON, through section 55's writer.
    #[must_use]
    pub fn json(&self) -> String {
        crate::serialization::to_json(&self.to_value())
            .unwrap_or_else(|error| format!("{{\"error\":\"{error}\"}}"))
    }
}

/// Why a migration could not go ahead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// No step covers this version change.
    MissingStep { from: u64, to: u64 },
    /// The chain would revisit a version.
    Cycle { at: u64 },
    /// The document is from a build newer than this one.
    FromFuture { found: u64, supported: u64 },
    /// No step is registered under that name.
    UnknownStep(String),
    /// A declared rule names a path the document does not have.
    BadPath(String),
    /// The client's own data, with [`why_unknown`] as the reason.
    Refused(String),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingStep { from, to } => {
                write!(formatter, "no migration step from version {from} to {to}")
            }
            Self::Cycle { at } => write!(formatter, "the migration chain loops at version {at}"),
            Self::FromFuture { found, supported } => write!(
                formatter,
                "version {found} is newer than this build's {supported}"
            ),
            Self::UnknownStep(name) => write!(formatter, "no migration step named {name}"),
            Self::BadPath(path) => write!(formatter, "the document has no path {path}"),
            Self::Refused(why) => formatter.write_str(why),
        }
    }
}

impl std::error::Error for Error {}

#[derive(Clone)]
enum Op {
    Rename { from: String, to: String },
    Move { from: String, to: String },
    Remove { path: String },
    Set { path: String, value: Value },
}

type Apply = fn(&mut Value) -> Result<(), Error>;

/// One migration: the versions it spans and the change it makes.
#[derive(Clone)]
pub struct Step {
    pub kind: Kind,
    pub from: u64,
    pub to: u64,
    pub name: String,
    ops: Vec<Op>,
    apply: Option<Apply>,
}

impl fmt::Debug for Step {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Step")
            .field("kind", &self.kind)
            .field("from", &self.from)
            .field("to", &self.to)
            .field("name", &self.name)
            .finish()
    }
}

impl PartialEq for Step {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.from == other.from
            && self.to == other.to
            && self.name == other.name
    }
}

impl Eq for Step {}

impl Step {
    #[must_use]
    pub fn of(kind: Kind, from: u64, to: u64, name: &str) -> Self {
        Self {
            kind,
            from,
            to,
            name: name.to_owned(),
            ops: Vec::new(),
            apply: None,
        }
    }

    #[must_use]
    pub fn new(from: u64, to: u64, name: &str) -> Self {
        Self::of(Kind::Config, from, to, name)
    }

    #[must_use]
    pub fn with_kind(mut self, kind: Kind) -> Self {
        self.kind = kind;
        self
    }

    /// Renames the key at `from` to `to`, under the same parent.
    #[must_use]
    pub fn rename(mut self, from: &str, to: &str) -> Self {
        self.ops.push(Op::Rename {
            from: from.to_owned(),
            to: to.to_owned(),
        });
        self
    }

    /// Moves the value at `from` to a full path `to`, maps made along the way.
    #[must_use]
    pub fn move_to(mut self, from: &str, to: &str) -> Self {
        self.ops.push(Op::Move {
            from: from.to_owned(),
            to: to.to_owned(),
        });
        self
    }

    #[must_use]
    pub fn remove(mut self, path: &str) -> Self {
        self.ops.push(Op::Remove {
            path: path.to_owned(),
        });
        self
    }

    #[must_use]
    pub fn set(mut self, path: &str, value: Value) -> Self {
        self.ops.push(Op::Set {
            path: path.to_owned(),
            value,
        });
        self
    }

    /// Adds a closure that runs against the whole document after the declared rules.
    #[must_use]
    pub fn run(mut self, apply: Apply) -> Self {
        self.apply = Some(apply);
        self
    }
}

struct Registry {
    steps: Vec<Step>,
    state: BTreeMap<String, Status>,
    deprecations: Vec<(String, String)>,
    version: u64,
    status: Status,
    backup: Option<Value>,
    report: Option<Run>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            steps: Vec::new(),
            state: BTreeMap::new(),
            deprecations: Vec::new(),
            version: 3,
            status: Status::Idle,
            backup: None,
            report: None,
        }
    }
}

fn held() -> &'static Mutex<Registry> {
    static HELD: OnceLock<Mutex<Registry>> = OnceLock::new();
    HELD.get_or_init(|| Mutex::new(Registry::default()))
}

fn with<T>(body: impl FnOnce(&mut Registry) -> T) -> T {
    let mut guard = held().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn split_path(path: &str) -> (&str, &str) {
    match path.rfind('.') {
        Some(at) => (&path[..at], &path[at + 1..]),
        None => ("", path),
    }
}

fn split_first(path: &str) -> (&str, &str) {
    match path.find('.') {
        Some(at) => (&path[..at], &path[at + 1..]),
        None => (path, ""),
    }
}

fn value_at<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = root;
    if path.is_empty() {
        return Some(current);
    }
    for segment in path.split('.') {
        current = match current {
            Value::Map(map) => map.get(segment)?,
            Value::List(items) => items.get(segment.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current)
}

fn value_at_mut<'a>(root: &'a mut Value, path: &str) -> Option<&'a mut Value> {
    let mut current = root;
    if path.is_empty() {
        return Some(current);
    }
    for segment in path.split('.') {
        current = match current {
            Value::Map(map) => map.get_mut(segment)?,
            Value::List(items) => {
                let at: usize = segment.parse().ok()?;
                items.get_mut(at)?
            }
            _ => return None,
        };
    }
    Some(current)
}

fn ensure_map<'a>(value: &'a mut Value, path: &str) -> Result<&'a mut Map, Error> {
    let Value::Map(map) = value else {
        return Err(Error::BadPath(path.to_owned()));
    };
    if path.is_empty() {
        return Ok(map);
    }
    let (head, rest) = split_first(path);
    if !map.contains_key(head) {
        map.insert(head, Value::Map(Map::new()));
    }
    let entry = map
        .get_mut(head)
        .ok_or_else(|| Error::BadPath(head.to_owned()))?;
    ensure_map(entry, rest)
}

fn take_at(root: &mut Value, path: &str) -> Option<Value> {
    let (parent_path, key) = split_path(path);
    let parent = if parent_path.is_empty() {
        Some(root)
    } else {
        value_at_mut(root, parent_path)
    }?;
    match parent {
        Value::Map(map) => map.remove(key),
        Value::List(items) => {
            let at: usize = key.parse().ok()?;
            if at < items.len() {
                Some(items.remove(at))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn put_at(root: &mut Value, path: &str, value: Value) -> Result<(), Error> {
    let (parent_path, key) = split_path(path);
    if parent_path.is_empty() {
        let Value::Map(map) = root else {
            return Err(Error::BadPath(path.to_owned()));
        };
        map.insert(key, value);
        return Ok(());
    }
    let parent = ensure_map(root, parent_path)?;
    parent.insert(key, value);
    Ok(())
}

fn rename_at(root: &mut Value, from: &str, to: &str) -> Result<(), Error> {
    let held = take_at(root, from).ok_or_else(|| Error::BadPath(from.to_owned()))?;
    let (parent_path, _) = split_path(from);
    let new_path = if parent_path.is_empty() {
        to.to_owned()
    } else {
        format!("{parent_path}.{to}")
    };
    put_at(root, &new_path, held)
}

fn apply_step(step: &Step, document: &mut Value, changes: &mut Vec<Change>) -> Result<(), Error> {
    for op in &step.ops {
        match op {
            Op::Rename { from, to } => {
                rename_at(document, from, to)?;
                changes.push(Change::new(from, Action::Renamed, to));
            }
            Op::Move { from, to } => {
                let held = take_at(document, from).ok_or_else(|| Error::BadPath(from.clone()))?;
                put_at(document, to, held)?;
                changes.push(Change::new(from, Action::Moved, to));
            }
            Op::Remove { path } => {
                take_at(document, path).ok_or_else(|| Error::BadPath(path.clone()))?;
                changes.push(Change::new(path, Action::Removed, ""));
            }
            Op::Set { path, value } => {
                put_at(document, path, value.clone())?;
                changes.push(Change::new(
                    path,
                    Action::Set,
                    &crate::serialization::to_text(value),
                ));
            }
        }
    }
    if let Some(apply) = step.apply {
        apply(document)?;
        changes.push(Change::new(&step.name, Action::Custom, ""));
    }
    Ok(())
}

fn chain(steps: &[Step], kind: Kind, from: u64, to: u64) -> Result<Vec<Step>, Error> {
    let mut current = from;
    let mut seen = vec![from];
    let mut out = Vec::new();
    while current != to {
        let Some(step) = steps
            .iter()
            .find(|step| step.kind == kind && step.from == current)
        else {
            return Err(Error::MissingStep { from: current, to });
        };
        if seen.contains(&step.to) {
            return Err(Error::Cycle { at: step.to });
        }
        out.push(step.clone());
        current = step.to;
        seen.push(current);
    }
    Ok(out)
}

fn apply_deprecations(document: &mut Value, changes: &mut Vec<Change>) {
    let pairs = with(|registry| registry.deprecations.clone());
    let Value::Map(map) = document else {
        return;
    };
    for (old, _) in pairs {
        let Some(new) = renamed(&old) else { continue };
        if let Some(held) = map.remove(&old) {
            map.insert(new.clone(), held);
            changes.push(Change::new(&old, Action::RenamedId, &new));
        }
    }
}

/// Registers a step by name. A blank or taken name is refused.
pub fn register(step: Step) -> bool {
    with(|registry| {
        if step.name.trim().is_empty() {
            crate::warn!("a migration step needs a name");
            return false;
        }
        if registry.steps.iter().any(|held| held.name == step.name) {
            crate::warn!("a migration step named {} is already registered", step.name);
            return false;
        }
        registry.state.insert(step.name.clone(), Status::Pending);
        registry.steps.push(step);
        true
    })
}

/// Removes a step by name, and answers whether it was there.
pub fn unregister(name: &str) -> bool {
    with(|registry| {
        let before = registry.steps.len();
        registry.steps.retain(|step| step.name != name);
        registry.state.remove(name);
        registry.steps.len() != before
    })
}

/// Runs the one step registered under a name.
pub fn run(name: &str, document: &mut Value) -> Result<Run, Error> {
    let step = get(name).ok_or_else(|| Error::UnknownStep(name.to_owned()))?;
    with(|registry| registry.backup = Some(document.clone()));
    let mut changes = Vec::new();
    let result = apply_step(&step, document, &mut changes);
    let state = if result.is_ok() {
        Status::Completed
    } else {
        Status::Failed
    };
    with(|registry| {
        registry.state.insert(step.name.clone(), state);
        registry.status = state;
    });
    result?;
    let run = Run {
        applied: vec![step.name.clone()],
        changes,
    };
    with(|registry| registry.report = Some(run.clone()));
    Ok(run)
}

/// Applies every step from `from` to `to` in order, refusing a missing step or a cycle.
pub fn run_all(kind: Kind, document: &mut Value, from: u64, to: u64) -> Result<Run, Error> {
    let supported = get_version();
    if from > supported {
        return Err(Error::FromFuture {
            found: from,
            supported,
        });
    }
    let steps = with(|registry| chain(&registry.steps, kind, from, to))?;
    with(|registry| registry.backup = Some(document.clone()));
    let mut applied = Vec::new();
    let mut changes = Vec::new();
    for step in &steps {
        match apply_step(step, document, &mut changes) {
            Ok(()) => {
                applied.push(step.name.clone());
                with(|registry| {
                    registry
                        .state
                        .insert(step.name.clone(), Status::Completed);
                });
            }
            Err(error) => {
                with(|registry| {
                    registry.state.insert(step.name.clone(), Status::Failed);
                    registry.status = Status::Failed;
                });
                return Err(error);
            }
        }
    }
    apply_deprecations(document, &mut changes);
    let run = Run { applied, changes };
    with(|registry| {
        registry.status = Status::Completed;
        registry.report = Some(run.clone());
    });
    Ok(run)
}

/// Reports what the chain from `from` to `to` would change, without changing the document.
pub fn dry_run(kind: Kind, document: &Value, from: u64, to: u64) -> Result<Run, Error> {
    let supported = get_version();
    if from > supported {
        return Err(Error::FromFuture {
            found: from,
            supported,
        });
    }
    let steps = with(|registry| chain(&registry.steps, kind, from, to))?;
    let mut copy = document.clone();
    let mut applied = Vec::new();
    let mut changes = Vec::new();
    for step in &steps {
        apply_step(step, &mut copy, &mut changes)?;
        applied.push(step.name.clone());
    }
    apply_deprecations(&mut copy, &mut changes);
    Ok(Run { applied, changes })
}

/// The step registered under a name.
#[must_use]
pub fn get(name: &str) -> Option<Step> {
    with(|registry| {
        registry
            .steps
            .iter()
            .find(|step| step.name == name)
            .cloned()
    })
}

/// Every registered step, in registration order.
#[must_use]
pub fn get_all() -> Vec<Step> {
    with(|registry| registry.steps.clone())
}

/// Where the last run left the registry.
#[must_use]
pub fn get_status() -> Status {
    with(|registry| registry.status)
}

/// This build's supported document version.
#[must_use]
pub fn get_version() -> u64 {
    with(|registry| registry.version)
}

/// Sets the version this build writes; [`check`] measures a document against it.
pub fn set_version(version: u64) {
    with(|registry| registry.version = version);
}

/// The steps that have not run yet.
#[must_use]
pub fn get_pending() -> Vec<String> {
    with(|registry| {
        registry
            .steps
            .iter()
            .filter(|step| registry.state.get(&step.name) == Some(&Status::Pending))
            .map(|step| step.name.clone())
            .collect()
    })
}

/// The steps that ran without error.
#[must_use]
pub fn get_completed() -> Vec<String> {
    with(|registry| {
        registry
            .steps
            .iter()
            .filter(|step| registry.state.get(&step.name) == Some(&Status::Completed))
            .map(|step| step.name.clone())
            .collect()
    })
}

/// The steps that ran and failed.
#[must_use]
pub fn get_failed() -> Vec<String> {
    with(|registry| {
        registry
            .steps
            .iter()
            .filter(|step| registry.state.get(&step.name) == Some(&Status::Failed))
            .map(|step| step.name.clone())
            .collect()
    })
}

/// Registers a step that carries configuration forward.
pub fn register_config_migration(step: Step) -> bool {
    register(step.with_kind(Kind::Config))
}

/// Registers a step that carries save data forward.
pub fn register_save_migration(step: Step) -> bool {
    register(step.with_kind(Kind::Save))
}

/// Registers a step that carries a resource forward.
pub fn register_resource_migration(step: Step) -> bool {
    register(step.with_kind(Kind::Resource))
}

/// Registers a step that carries a registry forward.
pub fn register_registry_migration(step: Step) -> bool {
    register(step.with_kind(Kind::Registry))
}

/// Registers a step that carries an API shape forward.
pub fn register_api_migration(step: Step) -> bool {
    register(step.with_kind(Kind::Api))
}

/// Registers a step that carries a manifest forward.
pub fn register_manifest_migration(step: Step) -> bool {
    register(step.with_kind(Kind::Manifest))
}

/// Problems in the registered steps: a blank name, a step that never advances, or two steps
/// covering the same kind and from-version.
#[must_use]
pub fn validate() -> Vec<(String, String)> {
    with(|registry| {
        let mut problems = Vec::new();
        for (at, step) in registry.steps.iter().enumerate() {
            if step.name.trim().is_empty() {
                problems.push((format!("step {at}"), "a step needs a name".to_owned()));
            }
            if step.to == step.from {
                problems.push((
                    step.name.clone(),
                    format!("{} never moves off version {}", step.name, step.from),
                ));
            }
            if registry.steps[..at]
                .iter()
                .any(|earlier| earlier.kind == step.kind && earlier.from == step.from)
            {
                problems.push((
                    step.name.clone(),
                    format!("another {} step already starts at version {}", step.kind, step.from),
                ));
            }
        }
        problems
    })
}

/// Restores the document from the most recent backup; answers whether there was one.
pub fn rollback(document: &mut Value) -> bool {
    let backup = with(|registry| registry.backup.clone());
    match backup {
        Some(held) => {
            *document = held;
            true
        }
        None => false,
    }
}

/// Keeps a copy of the document for [`rollback`], taken before a run.
pub fn create_backup(document: &Value) -> bool {
    with(|registry| {
        registry.backup = Some(document.clone());
        true
    })
}

/// The last run's report, if one has run.
#[must_use]
pub fn get_report() -> Option<Run> {
    with(|registry| registry.report.clone())
}

/// The version a document declares under `version`, if it has one.
#[must_use]
pub fn version(document: &Value) -> Option<u64> {
    value_at(document, "version")?.as_int().map(|number| number as u64)
}

/// Maps an old id to its replacement, replacing any earlier mapping for the old id.
pub fn deprecate(old: &str, new: &str) -> bool {
    if old.is_empty() || new.is_empty() || old == new {
        return false;
    }
    with(|registry| {
        registry.deprecations.retain(|(held, _)| held != old);
        registry
            .deprecations
            .push((old.to_owned(), new.to_owned()));
        true
    })
}

/// Drops a deprecation, and answers whether it was there.
pub fn undeprecate(old: &str) -> bool {
    with(|registry| {
        let before = registry.deprecations.len();
        registry.deprecations.retain(|(held, _)| held != old);
        registry.deprecations.len() != before
    })
}

/// The id an old one now migrates to, warning that it is deprecated.
#[must_use]
pub fn renamed(id: &str) -> Option<String> {
    let found = with(|registry| {
        registry
            .deprecations
            .iter()
            .find(|(old, _)| old == id)
            .map(|(_, new)| new.clone())
    });
    if let Some(new) = &found {
        crate::warn!("{id} is deprecated and migration renames it to {new}");
    }
    found
}

/// Every registered deprecation as an old id and its replacement.
#[must_use]
pub fn deprecations() -> Vec<(String, String)> {
    with(|registry| registry.deprecations.clone())
}

/// Clears every step, deprecation, backup and report. The supported version is kept.
pub fn clear() {
    with(|registry| {
        registry.steps.clear();
        registry.state.clear();
        registry.deprecations.clear();
        registry.status = Status::Idle;
        registry.backup = None;
        registry.report = None;
    });
}

/// Whether a document version is the one this build writes, older, or newer.
#[must_use]
pub fn compatibility(version: u64) -> Support {
    let supported = get_version();
    if version > supported {
        Support::Newer
    } else if version < supported {
        Support::Older
    } else {
        Support::Supported
    }
}

/// [`compatibility`] that refuses the future rather than answer it.
pub fn check(version: u64) -> Result<Support, Error> {
    let supported = get_version();
    if version > supported {
        return Err(Error::FromFuture {
            found: version,
            supported,
        });
    }
    Ok(compatibility(version))
}

/// Why a migration of the client's own data has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "level" => {
            "the client's level save is the client's own format, which this host neither reads \
             nor rewrites"
                .to_owned()
        }
        "player" => {
            "the client's player data is written only by the client and never crosses the mod \
             boundary"
                .to_owned()
        }
        "resource_pack" => {
            "a client resource pack is packed and signed by the client, so this host cannot \
             migrate one"
                .to_owned()
        }
        "api" => {
            "the client's internal API is private to the client and has no shape this host can \
             migrate"
                .to_owned()
        }
        _ => format!("{what} is not something this host can migrate"),
    }
}

/// What this host cannot migrate about the client's own data.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["level", "player", "resource_pack", "api"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

/// Migrating a document the client owns. Refused, with [`why_unknown`] and a warning.
pub fn migrate_client(what: &str) -> Result<Value, Error> {
    let why = why_unknown(what);
    crate::warn!("a mod asked to migrate the client's own data: {why}");
    Err(Error::Refused(why))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serialization::to_binary;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn document(version: i64) -> Value {
        let mut map = Map::new();
        map.insert("version", Value::Int(version));
        map.insert("old_name", Value::Str("gem".to_owned()));
        map.insert("temp", Value::Int(9));
        Value::Map(map)
    }

    fn map_of(value: &Value) -> &Map {
        value.as_map().expect("a map")
    }

    #[test]
    fn a_two_step_chain_runs_in_order_and_a_backup_rolls_it_back() {
        let _serial = serial();
        clear();
        set_version(3);
        assert!(register(
            Step::of(Kind::Config, 1, 2, "rename old_name").rename("old_name", "name")
        ));
        fn stamp(document: &mut Value) -> Result<(), Error> {
            put_at(document, "stamped", Value::Bool(true))
        }
        assert!(register(
            Step::of(Kind::Config, 2, 3, "stamp schema")
                .set("schema", Value::Int(3))
                .run(stamp)
        ));

        let mut doc = document(1);
        let before = to_binary(&doc);
        let run = run_all(Kind::Config, &mut doc, 1, 3).expect("the chain exists");
        assert_eq!(
            run.applied,
            vec!["rename old_name".to_owned(), "stamp schema".to_owned()]
        );
        assert_eq!(map_of(&doc).get("name"), Some(&Value::Str("gem".to_owned())));
        assert!(!map_of(&doc).contains_key("old_name"));
        assert_eq!(map_of(&doc).get("schema"), Some(&Value::Int(3)));
        assert_eq!(map_of(&doc).get("stamped"), Some(&Value::Bool(true)));
        assert_eq!(
            run.changes
                .iter()
                .map(|change| (change.path.as_str(), change.action))
                .collect::<Vec<_>>(),
            vec![
                ("old_name", Action::Renamed),
                ("schema", Action::Set),
                ("stamp schema", Action::Custom),
            ]
        );

        assert!(rollback(&mut doc));
        assert_eq!(to_binary(&doc), before, "rollback restored the original bytes");
    }

    #[test]
    fn a_chain_that_cannot_be_built_names_its_gap_or_its_loop() {
        let _serial = serial();
        clear();
        set_version(3);
        register(Step::of(Kind::Config, 1, 2, "one to two").rename("old_name", "name"));

        let mut doc = document(1);
        assert_eq!(
            run_all(Kind::Config, &mut doc, 2, 3),
            Err(Error::MissingStep { from: 2, to: 3 })
        );
        assert_eq!(
            run_all(Kind::Config, &mut doc, 1, 3),
            Err(Error::MissingStep { from: 2, to: 3 })
        );

        clear();
        register(Step::of(Kind::Config, 1, 2, "up"));
        register(Step::of(Kind::Config, 2, 1, "down"));
        assert_eq!(
            run_all(Kind::Config, &mut doc, 1, 3),
            Err(Error::Cycle { at: 1 })
        );
    }

    #[test]
    fn a_dry_run_names_the_paths_and_leaves_the_document_byte_identical() {
        let _serial = serial();
        clear();
        set_version(3);
        register(Step::of(Kind::Config, 1, 2, "rename old_name").rename("old_name", "name"));
        register(Step::of(Kind::Config, 2, 3, "drop temp").remove("temp"));

        let doc = document(1);
        let before = to_binary(&doc);
        let preview = dry_run(Kind::Config, &doc, 1, 3).expect("the chain exists");
        assert_eq!(to_binary(&doc), before, "the document did not move");
        assert_eq!(
            preview.applied,
            vec!["rename old_name".to_owned(), "drop temp".to_owned()]
        );
        assert!(preview
            .changes
            .iter()
            .any(|change| change.path == "old_name" && change.action == Action::Renamed));
        assert!(preview
            .changes
            .iter()
            .any(|change| change.path == "temp" && change.action == Action::Removed));
        assert!(map_of(&doc).contains_key("old_name"), "the real one is intact");
    }

    #[test]
    fn a_deprecated_id_is_renamed_during_migration_with_the_old_one_gone() {
        let _serial = serial();
        clear();
        set_version(3);
        assert!(deprecate("demo:old", "demo:new"));

        let mut doc = Value::Map(Map::new());
        if let Value::Map(map) = &mut doc {
            map.insert("version", Value::Int(3));
            map.insert("demo:old", Value::Int(7));
        }
        let run = run_all(Kind::Config, &mut doc, 3, 3).expect("an empty chain still migrates");
        assert_eq!(renamed("demo:old"), Some("demo:new".to_owned()));
        assert_eq!(map_of(&doc).get("demo:new"), Some(&Value::Int(7)));
        assert!(!map_of(&doc).contains_key("demo:old"));
        assert_eq!(
            run.changes,
            vec![Change::new("demo:old", Action::RenamedId, "demo:new")]
        );
        assert_eq!(deprecations(), vec![("demo:old".to_owned(), "demo:new".to_owned())]);
        assert!(undeprecate("demo:old"));
    }

    #[test]
    fn compatibility_answers_three_ways_and_refuses_the_future() {
        let _serial = serial();
        clear();
        set_version(3);
        assert_eq!(compatibility(3), Support::Supported);
        assert_eq!(compatibility(2), Support::Older);
        assert_eq!(compatibility(4), Support::Newer);
        assert_eq!(check(3), Ok(Support::Supported));
        assert_eq!(check(2), Ok(Support::Older));
        assert_eq!(
            check(4),
            Err(Error::FromFuture {
                found: 4,
                supported: 3
            })
        );

        let mut doc = document(4);
        assert_eq!(
            run_all(Kind::Config, &mut doc, 4, 4),
            Err(Error::FromFuture {
                found: 4,
                supported: 3
            })
        );
    }

    #[test]
    fn the_report_renders_as_json_through_serialization() {
        let _serial = serial();
        clear();
        set_version(3);
        register(Step::of(Kind::Config, 1, 2, "rename old_name").rename("old_name", "name"));

        let mut doc = document(1);
        let run = run_all(Kind::Config, &mut doc, 1, 2).expect("the step exists");
        let json = run.json();
        assert!(json.contains(r#""applied":["rename old_name"]"#), "{json}");
        assert!(json.contains(r#""path":"old_name""#), "{json}");
        assert!(json.contains(r#""action":"rename""#), "{json}");
        assert_eq!(get_report().as_ref(), Some(&run));
        assert_eq!(get_status(), Status::Completed);
        assert_eq!(get_completed(), vec!["rename old_name".to_owned()]);
    }

    #[test]
    fn steps_register_by_name_and_validate_flags_a_step_that_never_advances() {
        let _serial = serial();
        clear();
        set_version(3);
        assert!(register_manifest_migration(Step::new(1, 1, "stuck")));
        assert!(get("stuck").is_some());
        assert_eq!(get("stuck").map(|step| step.kind), Some(Kind::Manifest));
        assert!(!register(Step::new(2, 3, "stuck")), "a taken name is refused");
        assert_eq!(get_pending(), vec!["stuck".to_owned()]);

        let problems = validate();
        assert!(
            problems
                .iter()
                .any(|(name, why)| name == "stuck" && why.contains("never moves off version 1")),
            "{problems:?}"
        );
        assert!(unregister("stuck"));
        assert!(!unregister("stuck"));
        assert!(get_all().is_empty());
        assert_eq!(run("missing", &mut document(1)), Err(Error::UnknownStep("missing".to_owned())));
    }

    #[test]
    fn the_clients_own_data_is_refused_with_a_sentence_and_a_warning() {
        let _serial = serial();
        clear();
        assert_eq!(refusals().len(), 4);
        assert!(why_unknown("level").contains("level save"));
        assert!(why_unknown("resource_pack").contains("resource pack"));
        assert!(matches!(migrate_client("level"), Err(Error::Refused(_))));
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has a sentence");
            assert_eq!(why_unknown(&what), why);
        }
        assert!(why_unknown("something_else").contains("something_else"));
    }
}
