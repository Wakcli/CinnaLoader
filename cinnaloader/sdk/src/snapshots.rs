//! Save states and snapshots: named captures a mod can restore, diff and export.
//!
//! A [`Snapshot`] records a name, a label, a caller-supplied timestamp, a copy of the state
//! as a [`crate::config::ConfigValue`], and — over the state's encoded bytes — a real byte
//! size and a CRC-32 checksum. `create` captures one; `restore` re-reads the stored bytes,
//! checks the checksum and writes the decoded state into a `&mut ConfigValue`, so a value
//! changed since the capture really comes back. A snapshot whose bytes no longer match its
//! checksum is refused, and so is a name the ring does not hold.
//!
//! The ring keeps snapshots in capture order under a capacity; creating past the cap drops
//! the oldest. `get_all` lists in order, `get` finds one by name, `delete` removes and
//! `clear` empties, each answering a real count.
//!
//! Two snapshots are compared with `compare` (equal, and how many paths changed) or detailed
//! with `create_diff` (added, removed, changed, each with its path and both values), which
//! `apply_diff` replays into a state. `export`/`import` and `compress`/`decompress` carry a
//! snapshot through [`crate::serialization`], byte for byte.
//!
//! The client's live world, entities, blocks and inventory are its own state and cannot be
//! captured here; each answers `None` with the reason in [`why_unknown`]. A mod's own config
//! is reachable, and is what `get_config_state` captures.
//!
//! ```
//! use cinnaloader::config::ConfigValue;
//! use cinnaloader::snapshots;
//!
//! snapshots::clear();
//! let full = ConfigValue::Map(
//!     [("hp".to_owned(), ConfigValue::Integer(20))].into_iter().collect(),
//! );
//! snapshots::create("full", "before the fight", 1_700_000_000, &full);
//!
//! let mut state = ConfigValue::Map(
//!     [("hp".to_owned(), ConfigValue::Integer(3))].into_iter().collect(),
//! );
//! snapshots::restore("full", &mut state).expect("a captured snapshot");
//! assert_eq!(state, full);
//!
//! let bytes = snapshots::export("full").expect("a snapshot of its own");
//! let back = snapshots::import(&bytes).expect("a valid export");
//! let restored = snapshots::get("full").expect("still in the ring");
//! assert!(snapshots::compare(&restored, &back).equal);
//! snapshots::clear();
//! ```

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::sync::{Mutex, OnceLock};

use crate::config::ConfigValue;
use crate::serialization::{self, Map, Value};

/// The most snapshots the ring keeps before the oldest is dropped.
pub const DEFAULT_CAPACITY: usize = 8;

/// What went wrong, and with which snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotError {
    /// No snapshot with that name is in the ring.
    Unknown(String),
    /// The name is not one a snapshot may have.
    BadName(String),
    /// The stored bytes no longer checksum to the recorded value.
    ChecksumMismatch {
        name: String,
        expected: u32,
        found: u32,
    },
    /// The bytes are not a snapshot this host can read back.
    Decode(String),
    /// A snapshot that does not validate.
    Invalid(String),
}

impl fmt::Display for SnapshotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown(name) => write!(formatter, "there is no snapshot called '{name}'"),
            Self::BadName(name) => write!(formatter, "'{name}' is not a usable snapshot name"),
            Self::ChecksumMismatch {
                name,
                expected,
                found,
            } => write!(
                formatter,
                "'{name}' checksums to {found:#010x}, not the recorded {expected:#010x}"
            ),
            Self::Decode(reason) => write!(formatter, "the snapshot does not decode: {reason}"),
            Self::Invalid(reason) => write!(formatter, "the snapshot is invalid: {reason}"),
        }
    }
}

impl std::error::Error for SnapshotError {}

/// A capture: what it is called, when a caller says it was taken, the state itself, and the
/// bytes and checksum that restore reads.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub name: String,
    pub label: String,
    pub timestamp: u64,
    pub state: ConfigValue,
    pub bytes: Vec<u8>,
    pub checksum: u32,
}

impl Snapshot {
    /// Captures a state: encodes it and records the checksum over those bytes.
    #[must_use]
    pub fn new(name: &str, label: &str, timestamp: u64, state: &ConfigValue) -> Self {
        let bytes = serialization::to_binary(&to_value(state));
        let checksum = serialization::checksum(&bytes);
        Self {
            name: name.to_owned(),
            label: label.to_owned(),
            timestamp,
            state: state.clone(),
            bytes,
            checksum,
        }
    }

    /// The encoded size, in bytes.
    #[must_use]
    pub fn size(&self) -> usize {
        self.bytes.len()
    }

    /// The snapshot's identifying fields.
    #[must_use]
    pub fn metadata(&self) -> SnapshotMetadata {
        SnapshotMetadata {
            name: self.name.clone(),
            label: self.label.clone(),
            timestamp: self.timestamp,
            size: self.bytes.len(),
            checksum: self.checksum,
        }
    }

    /// Writes the captured state into `state`, refusing bytes that no longer checksum.
    pub fn restore_into(&self, state: &mut ConfigValue) -> Result<(), SnapshotError> {
        let found = serialization::checksum(&self.bytes);
        if found != self.checksum {
            crate::warn!(
                "a mod tried to restore the snapshot {:?}, whose bytes no longer match its checksum",
                self.name
            );
            return Err(SnapshotError::ChecksumMismatch {
                name: self.name.clone(),
                expected: self.checksum,
                found,
            });
        }
        *state = decode_state(&self.bytes)?;
        Ok(())
    }
}

/// The fields that name a snapshot without carrying its state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotMetadata {
    pub name: String,
    pub label: String,
    pub timestamp: u64,
    pub size: usize,
    pub checksum: u32,
}

/// One path's part in a diff, with the value on each side.
#[derive(Debug, Clone, PartialEq)]
pub struct DiffEntry {
    pub path: String,
    pub before: Option<ConfigValue>,
    pub after: Option<ConfigValue>,
}

/// What changed between two states: paths added, removed, and changed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Diff {
    pub added: Vec<DiffEntry>,
    pub removed: Vec<DiffEntry>,
    pub changed: Vec<DiffEntry>,
}

impl Diff {
    /// Whether nothing changed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }

    /// How many paths differ.
    #[must_use]
    pub fn paths(&self) -> usize {
        self.added.len() + self.removed.len() + self.changed.len()
    }
}

/// Whether two states are the same, and how many paths say otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Comparison {
    pub equal: bool,
    pub changed_paths: usize,
}

struct Held {
    ring: VecDeque<Snapshot>,
    capacity: usize,
}

fn held() -> &'static Mutex<Held> {
    static HELD: OnceLock<Mutex<Held>> = OnceLock::new();
    HELD.get_or_init(|| {
        Mutex::new(Held {
            ring: VecDeque::new(),
            capacity: DEFAULT_CAPACITY,
        })
    })
}

fn with<T>(body: impl FnOnce(&mut Held) -> T) -> T {
    let mut guard = held().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Captures a state under a name and puts it in the ring. The oldest goes if the ring is at
/// its capacity, and an existing name is replaced where it stands.
pub fn create(name: &str, label: &str, timestamp: u64, state: &ConfigValue) -> Snapshot {
    let snapshot = Snapshot::new(name, label, timestamp, state);
    store(snapshot.clone());
    snapshot
}

/// Puts a snapshot in the ring, replacing one of its name or dropping the oldest, and answers
/// whether the name was new.
pub fn insert(snapshot: Snapshot) -> bool {
    store(snapshot)
}

/// The snapshot under a name.
#[must_use]
pub fn get(name: &str) -> Option<Snapshot> {
    with(|held| held.ring.iter().find(|held| held.name == name).cloned())
}

/// Every snapshot, in capture order.
#[must_use]
pub fn get_all() -> Vec<Snapshot> {
    with(|held| held.ring.iter().cloned().collect())
}

/// The metadata of the snapshot under a name.
#[must_use]
pub fn get_metadata(name: &str) -> Option<SnapshotMetadata> {
    get(name).map(|snapshot| snapshot.metadata())
}

/// Restores a snapshot by name into a mutable state. An unknown name is refused by name, and
/// corrupt bytes by checksum.
pub fn restore(name: &str, state: &mut ConfigValue) -> Result<(), SnapshotError> {
    match get(name) {
        Some(snapshot) => snapshot.restore_into(state),
        None => {
            crate::warn!("a mod tried to restore the snapshot {name:?}, which is not in the ring");
            Err(SnapshotError::Unknown(name.to_owned()))
        }
    }
}

/// Renames a snapshot, and answers whether it went. An unknown or taken name is refused.
pub fn rename(from: &str, to: &str) -> bool {
    if !valid_name(to) {
        crate::warn!("a mod tried to rename a snapshot to {to:?}, which is not a usable name");
        return false;
    }
    with(|held| {
        if held.ring.iter().any(|held| held.name == to) {
            crate::warn!("a mod tried to rename a snapshot to {to:?}, which is taken");
            return false;
        }
        match held.ring.iter_mut().find(|held| held.name == from) {
            Some(snapshot) => {
                snapshot.name = to.to_owned();
                true
            }
            None => {
                crate::warn!("a mod tried to rename the snapshot {from:?}, which is not in the ring");
                false
            }
        }
    })
}

/// Removes a snapshot, and answers whether it was there.
pub fn delete(name: &str) -> bool {
    with(|held| {
        let before = held.ring.len();
        held.ring.retain(|held| held.name != name);
        held.ring.len() != before
    })
}

/// Removes a snapshot, under the name the wishlist uses for it.
pub fn remove(name: &str) -> bool {
    delete(name)
}

/// Empties the ring, and answers how many snapshots went.
pub fn clear() -> usize {
    with(|held| {
        let count = held.ring.len();
        held.ring.clear();
        count
    })
}

/// How many snapshots the ring holds.
#[must_use]
pub fn len() -> usize {
    with(|held| held.ring.len())
}

/// Whether the ring holds none.
#[must_use]
pub fn is_empty() -> bool {
    with(|held| held.ring.is_empty())
}

/// The cap on the ring.
#[must_use]
pub fn capacity() -> usize {
    with(|held| held.capacity)
}

/// Sets the cap, dropping the oldest if the ring is already past it. Zero keeps nothing.
pub fn set_capacity(capacity: usize) {
    with(|held| {
        held.capacity = capacity;
        if capacity == 0 {
            held.ring.clear();
            return;
        }
        while held.ring.len() > capacity {
            held.ring.pop_front();
        }
    });
}

/// The bytes a snapshot exports to, through [`crate::serialization`].
pub fn export(name: &str) -> Result<Vec<u8>, SnapshotError> {
    let snapshot = get(name).ok_or_else(|| {
        crate::warn!("a mod tried to export the snapshot {name:?}, which is not in the ring");
        SnapshotError::Unknown(name.to_owned())
    })?;
    export_snapshot(&snapshot)
}

/// Reads back what [`export`] wrote, refusing a snapshot that does not validate.
pub fn import(bytes: &[u8]) -> Result<Snapshot, SnapshotError> {
    let value = serialization::from_binary(bytes)
        .map_err(|error| SnapshotError::Decode(error.to_string()))?;
    let doc = value
        .as_map()
        .ok_or_else(|| SnapshotError::Invalid("an export is a map".to_owned()))?;
    let name = doc
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| SnapshotError::Invalid("the export has no name".to_owned()))?;
    let label = doc.get("label").and_then(Value::as_str).unwrap_or_default();
    let timestamp = doc
        .get("timestamp")
        .and_then(Value::as_int)
        .and_then(|number| u64::try_from(number).ok())
        .unwrap_or(0);
    let checksum = doc
        .get("checksum")
        .and_then(Value::as_int)
        .and_then(|number| u32::try_from(number).ok())
        .ok_or_else(|| SnapshotError::Invalid("the export has no checksum".to_owned()))?;
    let state = doc
        .get("state")
        .and_then(from_value)
        .ok_or_else(|| SnapshotError::Invalid("the export holds no state".to_owned()))?;
    let mut snapshot = Snapshot::new(name, label, timestamp, &state);
    snapshot.checksum = checksum;
    let problems = validate(&snapshot);
    if !problems.is_empty() {
        let reason = problems.join("; ");
        crate::warn!("a mod imported a snapshot that does not validate: {reason}");
        return Err(SnapshotError::Invalid(reason));
    }
    store(snapshot.clone());
    Ok(snapshot)
}

/// Compresses [`export`]'s bytes.
pub fn compress(name: &str) -> Result<Vec<u8>, SnapshotError> {
    export(name).map(|bytes| serialization::compress(&bytes))
}

/// Undoes [`compress`] and imports the snapshot.
pub fn decompress(bytes: &[u8]) -> Result<Snapshot, SnapshotError> {
    let raw = serialization::decompress(bytes)
        .map_err(|error| SnapshotError::Decode(error.to_string()))?;
    import(&raw)
}

/// Every problem with a snapshot: bytes that do not decode, a state they do not hold, and a
/// checksum that does not match.
#[must_use]
pub fn validate(snapshot: &Snapshot) -> Vec<String> {
    let mut problems = Vec::new();
    if !valid_name(&snapshot.name) {
        problems.push(format!("{:?} is not a usable name", snapshot.name));
    }
    match serialization::from_binary(&snapshot.bytes) {
        Err(error) => problems.push(format!("the stored bytes do not decode: {error}")),
        Ok(value) if from_value(&value).as_ref() != Some(&snapshot.state) => {
            problems.push("the stored bytes do not hold the recorded state".to_owned());
        }
        Ok(_) => {}
    }
    let found = serialization::checksum(&snapshot.bytes);
    if found != snapshot.checksum {
        problems.push(format!(
            "the bytes checksum to {found:#010x}, not the recorded {:#010x}",
            snapshot.checksum
        ));
    }
    problems
}

/// What changed between two snapshots.
#[must_use]
pub fn create_diff(before: &Snapshot, after: &Snapshot) -> Diff {
    let left = flatten(&before.state);
    let right = flatten(&after.state);
    let mut diff = Diff::default();
    for (path, value) in &left {
        match right.get(path) {
            None => diff.removed.push(DiffEntry {
                path: path.clone(),
                before: Some(value.clone()),
                after: None,
            }),
            Some(other) if other != value => diff.changed.push(DiffEntry {
                path: path.clone(),
                before: Some(value.clone()),
                after: Some(other.clone()),
            }),
            Some(_) => {}
        }
    }
    for (path, value) in &right {
        if !left.contains_key(path) {
            diff.added.push(DiffEntry {
                path: path.clone(),
                before: None,
                after: Some(value.clone()),
            });
        }
    }
    diff
}

/// Whether two snapshots are the same, and how many paths differ.
#[must_use]
pub fn compare(before: &Snapshot, after: &Snapshot) -> Comparison {
    let diff = create_diff(before, after);
    Comparison {
        equal: diff.is_empty(),
        changed_paths: diff.paths(),
    }
}

/// Applies a diff to a state, and answers how many paths it moved.
pub fn apply_diff(state: &mut ConfigValue, diff: &Diff) -> usize {
    let mut applied = 0;
    for entry in &diff.removed {
        if remove_path(state, &entry.path) {
            applied += 1;
        }
    }
    for entry in diff.added.iter().chain(&diff.changed) {
        if let Some(value) = &entry.after {
            if set_path(state, &entry.path, value.clone()) {
                applied += 1;
            }
        }
    }
    applied
}

/// The byte size of a snapshot's encoded state.
#[must_use]
pub fn get_size(snapshot: &Snapshot) -> usize {
    snapshot.bytes.len()
}

/// The client's live world, which this host cannot capture. `None`.
#[must_use]
pub fn get_world_state() -> Option<ConfigValue> {
    refuse("world")
}

/// A live entity's state, which this host cannot capture. `None`.
#[must_use]
pub fn get_entity_state() -> Option<ConfigValue> {
    refuse("entity")
}

/// A live block's state, which this host cannot capture. `None`.
#[must_use]
pub fn get_block_state() -> Option<ConfigValue> {
    refuse("block")
}

/// A player's inventory, which this host cannot capture. `None`.
#[must_use]
pub fn get_inventory_state() -> Option<ConfigValue> {
    refuse("inventory")
}

/// The mod's own configuration, which this host can capture.
#[must_use]
pub fn get_config_state() -> Option<ConfigValue> {
    Some(ConfigValue::Compound(crate::config::all()))
}

/// Why a capture question about the client has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "world" => {
            "the client's world is live game state behind hooks this host does not have, so \
             only a mod's own declarations can be captured"
                .to_owned()
        }
        "save" => {
            "the client's save file is the client's own format, which this host neither reads \
             nor writes"
                .to_owned()
        }
        "entity" => {
            "a live entity is the client's own state, which never crosses the mod boundary"
                .to_owned()
        }
        "block" => {
            "a live block or chunk is the client's own state, which never crosses the mod \
             boundary"
                .to_owned()
        }
        "inventory" => {
            "a player's inventory is the client's own state, which never crosses the mod \
             boundary"
                .to_owned()
        }
        _ => format!("{what} is not something this host can capture"),
    }
}

/// What this host cannot capture about the client's own state.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["world", "save", "entity", "block", "inventory"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

/// The client's own state, which this host cannot capture. Each call says why.
pub mod client {
    use super::why_unknown;
    use crate::config::ConfigValue;

    /// The client's world save.
    #[must_use]
    pub fn save() -> Option<ConfigValue> {
        crate::warn!(
            "a mod asked this host to capture the client's save: {}",
            why_unknown("save")
        );
        None
    }
}

fn refuse(what: &str) -> Option<ConfigValue> {
    crate::warn!("a mod asked this host to capture the client's {what}: {}", why_unknown(what));
    None
}

fn export_snapshot(snapshot: &Snapshot) -> Result<Vec<u8>, SnapshotError> {
    let mut doc = Map::new();
    doc.insert("name", Value::Str(snapshot.name.clone()));
    doc.insert("label", Value::Str(snapshot.label.clone()));
    doc.insert(
        "timestamp",
        Value::Int(i64::try_from(snapshot.timestamp).unwrap_or(i64::MAX)),
    );
    doc.insert("checksum", Value::Int(i64::from(snapshot.checksum)));
    doc.insert("state", to_value(&snapshot.state));
    Ok(serialization::to_binary(&Value::Map(doc)))
}

fn decode_state(bytes: &[u8]) -> Result<ConfigValue, SnapshotError> {
    let value = serialization::from_binary(bytes)
        .map_err(|error| SnapshotError::Decode(error.to_string()))?;
    from_value(&value)
        .ok_or_else(|| SnapshotError::Invalid("the bytes do not hold a config value".to_owned()))
}

fn flat_map(value: &ConfigValue) -> Option<&BTreeMap<String, ConfigValue>> {
    match value {
        ConfigValue::Map(fields) | ConfigValue::Object(fields) | ConfigValue::Compound(fields) => {
            Some(fields)
        }
        _ => None,
    }
}

fn map_mut(value: &mut ConfigValue) -> Option<&mut BTreeMap<String, ConfigValue>> {
    match value {
        ConfigValue::Map(fields) | ConfigValue::Object(fields) | ConfigValue::Compound(fields) => {
            Some(fields)
        }
        _ => None,
    }
}

fn flatten(state: &ConfigValue) -> BTreeMap<String, ConfigValue> {
    let mut out = BTreeMap::new();
    match flat_map(state) {
        Some(fields) if !fields.is_empty() => {
            for (key, held) in fields {
                flatten_into(key, held, &mut out);
            }
        }
        _ => {
            out.insert("$".to_owned(), state.clone());
        }
    }
    out
}

fn flatten_into(prefix: &str, value: &ConfigValue, out: &mut BTreeMap<String, ConfigValue>) {
    match flat_map(value) {
        Some(fields) if !fields.is_empty() => {
            for (key, held) in fields {
                flatten_into(&format!("{prefix}.{key}"), held, out);
            }
        }
        _ => {
            out.insert(prefix.to_owned(), value.clone());
        }
    }
}

fn set_path(root: &mut ConfigValue, path: &str, value: ConfigValue) -> bool {
    let parts: Vec<&str> = path.split('.').collect();
    let Some((last, parents)) = parts.split_last() else {
        return false;
    };
    let mut current = root;
    for part in parents {
        let Some(map) = map_mut(current) else {
            return false;
        };
        current = map
            .entry((*part).to_owned())
            .or_insert_with(|| ConfigValue::Map(BTreeMap::new()));
    }
    match map_mut(current) {
        Some(map) => {
            map.insert((*last).to_owned(), value);
            true
        }
        None => false,
    }
}

fn remove_path(root: &mut ConfigValue, path: &str) -> bool {
    let parts: Vec<&str> = path.split('.').collect();
    let Some((last, parents)) = parts.split_last() else {
        return false;
    };
    let mut current = root;
    for part in parents {
        let Some(map) = map_mut(current) else {
            return false;
        };
        match map.get_mut(*part) {
            Some(held) => current = held,
            None => return false,
        }
    }
    map_mut(current).is_some_and(|map| map.remove(*last).is_some())
}

fn store(snapshot: Snapshot) -> bool {
    with(|held| {
        if let Some(at) = held.ring.iter().position(|held| held.name == snapshot.name) {
            held.ring[at] = snapshot;
            return false;
        }
        held.ring.push_back(snapshot);
        if held.capacity == 0 {
            held.ring.clear();
        } else {
            while held.ring.len() > held.capacity {
                held.ring.pop_front();
            }
        }
        true
    })
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && !name.contains(['/', '\\', '\n']) && !name.contains("..")
}

fn to_value(state: &ConfigValue) -> Value {
    let (kind, payload) = match state {
        ConfigValue::Boolean(flag) => ("boolean", Value::Bool(*flag)),
        ConfigValue::Integer(number) => ("integer", Value::Int(*number)),
        ConfigValue::Float(number) => ("float", Value::Float(*number)),
        ConfigValue::Percentage(number) => ("percentage", Value::Float(*number)),
        ConfigValue::Duration(millis) => ("duration", Value::Int(*millis as i64)),
        ConfigValue::Color(colour) => ("color", Value::Int(i64::from(*colour))),
        ConfigValue::String(text) => ("string", Value::Str(text.clone())),
        ConfigValue::Text(text) => ("text", Value::Str(text.clone())),
        ConfigValue::Enum(text) => ("enum", Value::Str(text.clone())),
        ConfigValue::Keybind(text) => ("keybind", Value::Str(text.clone())),
        ConfigValue::FilePath(path) => ("file_path", Value::Str(path.display().to_string())),
        ConfigValue::DirectoryPath(path) => {
            ("directory_path", Value::Str(path.display().to_string()))
        }
        ConfigValue::ResourceId(text) => ("resource_id", Value::Str(text.clone())),
        ConfigValue::ItemId(text) => ("item_id", Value::Str(text.clone())),
        ConfigValue::BlockId(text) => ("block_id", Value::Str(text.clone())),
        ConfigValue::EntityId(text) => ("entity_id", Value::Str(text.clone())),
        ConfigValue::Position(x, y, z) => (
            "position",
            Value::List(vec![Value::Float(*x), Value::Float(*y), Value::Float(*z)]),
        ),
        ConfigValue::Vector2(x, y) => {
            ("vector2", Value::List(vec![Value::Float(*x), Value::Float(*y)]))
        }
        ConfigValue::Vector3(x, y, z) => (
            "vector3",
            Value::List(vec![Value::Float(*x), Value::Float(*y), Value::Float(*z)]),
        ),
        ConfigValue::Range(from, to) => ("range", Value::List(vec![Value::Int(*from), Value::Int(*to)])),
        ConfigValue::List(items) => ("list", items_value(items)),
        ConfigValue::Set(items) => ("set", items_value(items)),
        ConfigValue::Map(fields) => ("map", fields_value(fields)),
        ConfigValue::Object(fields) => ("object", fields_value(fields)),
        ConfigValue::Compound(fields) => ("compound", fields_value(fields)),
    };
    let mut outer = Map::new();
    outer.insert("kind", Value::Str(kind.to_owned()));
    outer.insert("value", payload);
    Value::Map(outer)
}

fn items_value(items: &[ConfigValue]) -> Value {
    Value::List(items.iter().map(to_value).collect())
}

fn fields_value(fields: &BTreeMap<String, ConfigValue>) -> Value {
    Value::Map(
        fields
            .iter()
            .map(|(key, held)| (key.clone(), to_value(held)))
            .collect(),
    )
}

fn from_value(value: &Value) -> Option<ConfigValue> {
    let map = value.as_map()?;
    let kind = map.get("kind")?.as_str()?;
    let payload = map.get("value")?;
    Some(match kind {
        "boolean" => ConfigValue::Boolean(payload.as_bool()?),
        "integer" => ConfigValue::Integer(payload.as_int()?),
        "float" => ConfigValue::Float(payload.as_float()?),
        "percentage" => ConfigValue::Percentage(payload.as_float()?),
        "duration" => ConfigValue::Duration(u64::try_from(payload.as_int()?).ok()?),
        "color" => ConfigValue::Color(u32::try_from(payload.as_int()?).ok()?),
        "string" => ConfigValue::String(payload.as_str()?.to_owned()),
        "text" => ConfigValue::Text(payload.as_str()?.to_owned()),
        "enum" => ConfigValue::Enum(payload.as_str()?.to_owned()),
        "keybind" => ConfigValue::Keybind(payload.as_str()?.to_owned()),
        "file_path" => ConfigValue::FilePath(payload.as_str()?.into()),
        "directory_path" => ConfigValue::DirectoryPath(payload.as_str()?.into()),
        "resource_id" => ConfigValue::ResourceId(payload.as_str()?.to_owned()),
        "item_id" => ConfigValue::ItemId(payload.as_str()?.to_owned()),
        "block_id" => ConfigValue::BlockId(payload.as_str()?.to_owned()),
        "entity_id" => ConfigValue::EntityId(payload.as_str()?.to_owned()),
        "position" => {
            let numbers = floats(payload)?;
            let [x, y, z] = numbers.as_slice() else {
                return None;
            };
            ConfigValue::Position(*x, *y, *z)
        }
        "vector2" => {
            let numbers = floats(payload)?;
            let [x, y] = numbers.as_slice() else {
                return None;
            };
            ConfigValue::Vector2(*x, *y)
        }
        "vector3" => {
            let numbers = floats(payload)?;
            let [x, y, z] = numbers.as_slice() else {
                return None;
            };
            ConfigValue::Vector3(*x, *y, *z)
        }
        "range" => {
            let list = payload.as_list()?;
            let [from, to] = list else { return None };
            ConfigValue::Range(from.as_int()?, to.as_int()?)
        }
        "list" => ConfigValue::List(values(payload)?),
        "set" => ConfigValue::Set(values(payload)?),
        "map" => ConfigValue::Map(fields(payload)?),
        "object" => ConfigValue::Object(fields(payload)?),
        "compound" => ConfigValue::Compound(fields(payload)?),
        _ => return None,
    })
}

fn values(payload: &Value) -> Option<Vec<ConfigValue>> {
    payload.as_list()?.iter().map(from_value).collect()
}

fn fields(payload: &Value) -> Option<BTreeMap<String, ConfigValue>> {
    payload
        .as_map()?
        .iter()
        .map(|(key, held)| from_value(held).map(|value| (key.to_owned(), value)))
        .collect()
}

fn floats(payload: &Value) -> Option<Vec<f64>> {
    payload.as_list()?.iter().map(Value::as_float).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reset() {
        with(|held| {
            held.ring.clear();
            held.capacity = DEFAULT_CAPACITY;
        });
    }

    fn map(pairs: &[(&str, ConfigValue)]) -> ConfigValue {
        ConfigValue::Map(
            pairs
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone()))
                .collect(),
        )
    }

    #[test]
    fn a_capture_restores_a_changed_value() {
        let _serial = crate::test_lock::serial();
        reset();
        let before = map(&[
            ("hp", ConfigValue::Integer(20)),
            ("name", ConfigValue::Text("wisp".to_owned())),
        ]);
        let snapshot = create("before", "full health", 1_700_000_000, &before);
        assert_eq!(snapshot.name, "before");
        assert_eq!(snapshot.label, "full health");
        assert_eq!(snapshot.timestamp, 1_700_000_000);
        assert_eq!(snapshot.checksum, serialization::checksum(&snapshot.bytes));
        assert_eq!(get_size(&snapshot), snapshot.bytes.len());
        assert!(!snapshot.bytes.is_empty(), "a capture writes some bytes");

        let mut state = map(&[
            ("hp", ConfigValue::Integer(3)),
            ("name", ConfigValue::Text("wisp".to_owned())),
        ]);
        assert_ne!(state, before, "the value really changed");
        restore("before", &mut state).expect("a captured snapshot");
        assert_eq!(state, before, "the captured value is back");

        state = ConfigValue::Integer(0);
        assert_eq!(restore("before", &mut state), Ok(()));
        assert_eq!(state, before);
    }

    #[test]
    fn an_unknown_or_corrupt_snapshot_is_refused() {
        let _serial = crate::test_lock::serial();
        reset();
        let mut state = ConfigValue::Integer(0);
        match restore("absent", &mut state) {
            Err(SnapshotError::Unknown(name)) => assert_eq!(name, "absent"),
            other => panic!("an unknown snapshot was not refused: {other:?}"),
        }
        let good = create("good", "a state", 1, &map(&[("a", ConfigValue::Integer(1))]));
        let mut broken = good.clone();
        broken.bytes[3] ^= 0xff;
        insert(broken);
        match restore("good", &mut state) {
            Err(SnapshotError::ChecksumMismatch {
                name,
                expected,
                found,
            }) => {
                assert_eq!(name, "good");
                assert_eq!(expected, good.checksum);
                assert_ne!(found, expected);
            }
            other => panic!("corrupt bytes were not refused: {other:?}"),
        }
        assert_eq!(state, ConfigValue::Integer(0), "a refused restore changes nothing");
    }

    #[test]
    fn a_diff_names_added_removed_and_changed_paths() {
        let _serial = crate::test_lock::serial();
        reset();
        let before = create(
            "before",
            "",
            1,
            &map(&[("a", ConfigValue::Integer(1)), ("b", ConfigValue::Integer(2))]),
        );
        let after = create(
            "after",
            "",
            2,
            &map(&[("a", ConfigValue::Integer(3)), ("c", ConfigValue::Integer(4))]),
        );
        let diff = create_diff(&before, &after);
        assert_eq!(
            diff.removed,
            vec![DiffEntry {
                path: "b".to_owned(),
                before: Some(ConfigValue::Integer(2)),
                after: None,
            }]
        );
        assert_eq!(
            diff.changed,
            vec![DiffEntry {
                path: "a".to_owned(),
                before: Some(ConfigValue::Integer(1)),
                after: Some(ConfigValue::Integer(3)),
            }]
        );
        assert_eq!(
            diff.added,
            vec![DiffEntry {
                path: "c".to_owned(),
                before: None,
                after: Some(ConfigValue::Integer(4)),
            }]
        );
        assert_eq!(diff.paths(), 3);
        let mut state = before.state.clone();
        assert_eq!(apply_diff(&mut state, &diff), 3);
        assert_eq!(state, after.state);
    }

    #[test]
    fn two_identical_snapshots_diff_empty() {
        let _serial = crate::test_lock::serial();
        reset();
        let state = map(&[
            ("x", ConfigValue::Integer(1)),
            ("y", ConfigValue::List(vec![ConfigValue::Integer(2)])),
        ]);
        let first = create("first", "", 1, &state);
        let second = create("second", "", 2, &state);
        let diff = create_diff(&first, &second);
        assert!(diff.is_empty());
        assert_eq!(diff.paths(), 0);
        assert_eq!(
            compare(&first, &second),
            Comparison {
                equal: true,
                changed_paths: 0,
            }
        );
        let other = create("other", "", 3, &map(&[("x", ConfigValue::Integer(9))]));
        assert_eq!(
            compare(&first, &other),
            Comparison {
                equal: false,
                changed_paths: 2,
            }
        );
    }

    #[test]
    fn the_ring_drops_the_oldest_at_its_capacity_and_counts_the_rest() {
        let _serial = crate::test_lock::serial();
        reset();
        set_capacity(2);
        assert_eq!(capacity(), 2);
        create("one", "", 1, &ConfigValue::Integer(1));
        create("two", "", 2, &ConfigValue::Integer(2));
        assert!(rename("one", "one-renamed"));
        assert!(!rename("one", "two"), "a taken name is refused");
        create("three", "", 3, &ConfigValue::Integer(3));
        assert_eq!(
            get_all()
                .iter()
                .map(|snapshot| snapshot.name.clone())
                .collect::<Vec<_>>(),
            vec!["two".to_owned(), "three".to_owned()]
        );
        assert!(get("one-renamed").is_none(), "the oldest went");
        assert_eq!(len(), 2);
        assert!(!is_empty());
        assert!(delete("two"));
        assert!(!delete("two"));
        assert!(remove("three"));
        assert_eq!(clear(), 0);
        assert!(is_empty());
    }

    #[test]
    fn a_snapshot_round_trips_through_export_and_import() {
        let _serial = crate::test_lock::serial();
        reset();
        let state = map(&[
            ("a", ConfigValue::Integer(1)),
            ("b", ConfigValue::Text("two".to_owned())),
        ]);
        let original = create("orig", "a label", 42, &state);
        let bytes = export("orig").expect("a snapshot of its own");
        let imported = import(&bytes).expect("a valid export");
        assert_eq!(imported, original, "byte for byte");
        assert_eq!(imported.bytes, original.bytes);
        assert_eq!(export("orig").unwrap(), bytes, "export is deterministic");
        assert_eq!(
            compare(&original, &imported),
            Comparison {
                equal: true,
                changed_paths: 0,
            }
        );

        let mut doc = serialization::from_binary(&bytes)
            .unwrap()
            .as_map()
            .unwrap()
            .clone();
        doc.insert("checksum", Value::Int(0));
        let forged = serialization::to_binary(&Value::Map(doc));
        assert!(matches!(import(&forged), Err(SnapshotError::Invalid(_))));

        match export("absent") {
            Err(SnapshotError::Unknown(name)) => assert_eq!(name, "absent"),
            other => panic!("exporting an unknown snapshot was not refused: {other:?}"),
        }
    }

    #[test]
    fn a_snapshot_survives_compression() {
        let _serial = crate::test_lock::serial();
        reset();
        let state = map(&[("tiles", ConfigValue::Text("a".repeat(200)))]);
        let original = create("packed", "", 7, &state);
        let raw = export("packed").expect("a snapshot of its own");
        let packed = compress("packed").expect("a snapshot of its own");
        assert!(packed.len() < raw.len(), "a run of bytes compresses");
        assert_eq!(decompress(&packed).expect("a compressed snapshot"), original);
    }

    #[test]
    fn metadata_and_size_come_from_the_bytes() {
        let _serial = crate::test_lock::serial();
        reset();
        let small = create("small", "one field", 10, &map(&[("a", ConfigValue::Integer(1))]));
        let large = create(
            "large",
            "two fields",
            20,
            &map(&[("a", ConfigValue::Integer(1)), ("b", ConfigValue::Integer(2))]),
        );
        assert!(get_size(&large) > get_size(&small), "more state is more bytes");
        let meta = get_metadata("small").expect("just captured");
        assert_eq!(
            meta,
            SnapshotMetadata {
                name: "small".to_owned(),
                label: "one field".to_owned(),
                timestamp: 10,
                size: get_size(&small),
                checksum: small.checksum,
            }
        );
        assert_eq!(small.metadata(), meta);
        assert!(validate(&small).is_empty());
        assert!(get_metadata("absent").is_none());
    }

    #[test]
    fn what_this_host_cannot_capture_says_why() {
        let _serial = crate::test_lock::serial();
        reset();
        assert!(get_world_state().is_none());
        assert!(get_entity_state().is_none());
        assert!(get_block_state().is_none());
        assert!(get_inventory_state().is_none());
        assert!(client::save().is_none());
        assert!(get_config_state().is_some(), "a mod's own config is reachable");
        let rows = refusals();
        assert_eq!(rows.len(), 5);
        for (what, why) in &rows {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, &format!("{what} is not something this host can capture"));
        }
        assert!(why_unknown("world").contains("world"));
        assert!(why_unknown("save").contains("save"));
    }
}
