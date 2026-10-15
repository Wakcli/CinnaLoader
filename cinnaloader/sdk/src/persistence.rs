//! Persistence: named stores a mod owns, really written to disk and really read back.
//!
//! Unlike [`crate::storage`], which keeps one store per mod, this module lets a mod own
//! several named stores at once, each with its own id, its own format version and its own
//! autosave interval. A store is a file in the mod's own directory ([`crate::fs`]), written
//! atomically — a sibling file first, then a rename — so a save that fails halfway leaves
//! the old file rather than half a new one.
//!
//! Values keep their type across a round trip, nested lists included, because the format
//! records each value's kind beside it. A file written by a newer build than this one is
//! refused rather than guessed at.
//!
//! ```
//! use cinnaloader::persistence;
//! use cinnaloader::config::ConfigValue;
//! use cinnaloader::fs;
//!
//! # let root = std::env::temp_dir().join(format!("cinnaloader-doc-persist-{}", std::process::id()));
//! # let _ = std::fs::remove_dir_all(&root);
//! # std::fs::create_dir_all(&root).unwrap();
//! # fs::set_root(root);
//! persistence::open("seen").expect("a store of the mod's own");
//! persistence::set("runs", ConfigValue::Integer(3));
//! persistence::set(
//!     "best",
//!     ConfigValue::List(vec![ConfigValue::Float(9.5), ConfigValue::Text("first".into())]),
//! );
//! assert!(persistence::save());
//! persistence::close();
//!
//! persistence::open("seen").unwrap();
//! assert_eq!(persistence::get("runs").and_then(|value| value.as_integer()), Some(3));
//! # persistence::close();
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use crate::config::ConfigValue;
use crate::fs::{self, FsError};
use crate::modinfo::ModVersion;

/// The format this build writes and understands. A file version above it is refused.
pub const FORMAT_VERSION: u32 = 1;

/// The name of the file a store lives in.
fn file_for(name: &str) -> String {
    format!("{name}.sav")
}

/// What went wrong, with the store or path it went wrong in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PersistError {
    /// No store is open.
    NoStore,
    /// The name is not one a store may have.
    BadName(String),
    /// The file could not be read or written.
    Io(String, String),
    /// The file is not a store this loader wrote.
    NotAStore(String),
    /// The file was written by a newer format than this build supports.
    VersionTooNew { found: u32, supported: u32 },
    /// A file that was expected is not there.
    Missing(String),
}

impl fmt::Display for PersistError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoStore => formatter.write_str("no store is open"),
            Self::BadName(name) => write!(formatter, "'{name}' is not a usable store name"),
            Self::Io(path, message) => write!(formatter, "{path}: {message}"),
            Self::NotAStore(path) => write!(formatter, "{path} is not a persistence store"),
            Self::VersionTooNew { found, supported } => write!(
                formatter,
                "the store is format {found} and this build writes {supported}"
            ),
            Self::Missing(path) => write!(formatter, "{path} is not there"),
        }
    }
}

impl std::error::Error for PersistError {}

/// A handle to one named store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StoreId(pub u64);

impl fmt::Display for StoreId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// Which part of the world a saved-data declaration belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DataScope {
    World,
    Player,
    Global,
}

impl DataScope {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::World => "world",
            Self::Player => "player",
            Self::Global => "global",
        }
    }
}

/// What a mod declares it wants kept, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedData {
    pub name: String,
    pub scope: DataScope,
    pub store: String,
    pub version: ModVersion,
}

impl SavedData {
    #[must_use]
    pub fn new(name: &str, scope: DataScope, store: &str) -> Self {
        Self {
            name: name.to_owned(),
            scope,
            store: store.to_owned(),
            version: ModVersion::parse("0"),
        }
    }

    #[must_use]
    pub fn version(mut self, version: &str) -> Self {
        self.version = ModVersion::parse(version);
        self
    }
}

/// How one saved-data schema version becomes the next. Compared by the versions it covers,
/// since two closures are never the same function.
#[derive(Debug, Clone)]
pub struct StoreMigration {
    pub from: ModVersion,
    pub to: ModVersion,
    pub description: String,
    apply: fn(&mut BTreeMap<String, ConfigValue>),
}

impl PartialEq for StoreMigration {
    fn eq(&self, other: &Self) -> bool {
        self.from == other.from && self.to == other.to && self.description == other.description
    }
}

impl StoreMigration {
    #[must_use]
    pub fn new(
        from: &str,
        to: &str,
        description: &str,
        apply: fn(&mut BTreeMap<String, ConfigValue>),
    ) -> Self {
        Self {
            from: ModVersion::parse(from),
            to: ModVersion::parse(to),
            description: description.to_owned(),
            apply,
        }
    }

    pub fn run(&self, values: &mut BTreeMap<String, ConfigValue>) {
        (self.apply)(values);
    }
}

/// One named store a mod owns.
#[derive(Debug, Clone)]
pub struct Store {
    id: StoreId,
    name: String,
    file: String,
    format: u32,
    schema: ModVersion,
    values: BTreeMap<String, ConfigValue>,
    dirty: bool,
    interval: Duration,
    saved_at: Instant,
}

impl Store {
    /// Opens a store by name, reading what is already on disk. The file is the mod's own,
    /// beside its config; nothing outside that directory is reachable.
    pub fn open(name: &str) -> Result<Self, PersistError> {
        if !valid_name(name) {
            return Err(PersistError::BadName(name.to_owned()));
        }
        let mut store = Self {
            id: StoreId(next_store_id()),
            name: name.to_owned(),
            file: file_for(name),
            format: FORMAT_VERSION,
            schema: ModVersion::parse("0"),
            values: BTreeMap::new(),
            dirty: false,
            interval: Duration::ZERO,
            saved_at: Instant::now(),
        };
        store.load()?;
        Ok(store)
    }

    #[must_use]
    pub fn id(&self) -> StoreId {
        self.id
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Where the store is written, as a path inside the mod's directory.
    #[must_use]
    pub fn path(&self) -> Option<PathBuf> {
        fs::resolve(&self.file).ok()
    }

    /// The format version this build writes for this store.
    #[must_use]
    pub fn format_version(&self) -> u32 {
        self.format
    }

    /// The mod's own data version, written with the store and checked on load.
    #[must_use]
    pub fn schema_version(&self) -> &ModVersion {
        &self.schema
    }

    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// One value.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<ConfigValue> {
        self.values.get(key).cloned()
    }

    /// One value, or what to use instead.
    #[must_use]
    pub fn get_or(&self, key: &str, fallback: ConfigValue) -> ConfigValue {
        self.values.get(key).cloned().unwrap_or(fallback)
    }

    /// Stores a value. Answers whether the key is one a store accepts. The dirty flag is set
    /// only when the value actually changes, so reading and re-setting the same value is not
    /// a change.
    pub fn set(&mut self, key: &str, value: ConfigValue) -> bool {
        if !valid_key(key) {
            crate::warn!("a mod tried to set the persistence key {key:?}, which is not usable");
            return false;
        }
        if self.values.get(key) != Some(&value) {
            self.values.insert(key.to_owned(), value);
            self.dirty = true;
        }
        true
    }

    /// Removes a key, and answers whether it was there.
    pub fn remove(&mut self, key: &str) -> bool {
        let removed = self.values.remove(key).is_some();
        self.dirty |= removed;
        removed
    }

    #[must_use]
    pub fn contains(&self, key: &str) -> bool {
        self.values.contains_key(key)
    }

    /// Every key, in order.
    #[must_use]
    pub fn keys(&self) -> Vec<String> {
        self.values.keys().cloned().collect()
    }

    /// The whole typed key space, for a caller that wants to walk it.
    #[must_use]
    pub fn all(&self) -> BTreeMap<String, ConfigValue> {
        self.values.clone()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Empties the store, and answers how many keys went. Emptying an empty store is not a
    /// change.
    pub fn clear(&mut self) -> usize {
        let count = self.values.len();
        self.values.clear();
        self.dirty |= count > 0;
        count
    }

    /// How long after a change the store may wait before it saves itself. `Duration::ZERO`
    /// means it never saves on its own.
    #[must_use]
    pub fn interval(&self) -> Duration {
        self.interval
    }

    pub fn set_interval(&mut self, interval: Duration) {
        self.interval = interval;
    }

    /// Whether a save is owed: something changed, an interval is set, and it has elapsed.
    #[must_use]
    pub fn is_due(&self) -> bool {
        self.dirty && !self.interval.is_zero() && self.saved_at.elapsed() >= self.interval
    }

    /// Saves when the interval has elapsed and the store is dirty, and otherwise does
    /// nothing.
    pub fn autosave(&mut self) -> Result<Option<usize>, PersistError> {
        if !self.is_due() {
            return Ok(None);
        }
        self.save().map(Some)
    }

    /// Writes the store to its file, atomically, and answers how many bytes went.
    pub fn save(&mut self) -> Result<usize, PersistError> {
        let bytes = self.encode();
        let written = fs::atomic_write(&self.file, &bytes)
            .map_err(|error| PersistError::Io(self.file.clone(), error.to_string()))?;
        self.dirty = false;
        self.saved_at = Instant::now();
        Ok(written)
    }

    /// Reads the file again over what is in memory, and answers whether there was a file.
    pub fn load(&mut self) -> Result<bool, PersistError> {
        let bytes = match fs::read(&self.file) {
            Ok(bytes) => bytes,
            Err(FsError::Missing(_)) => {
                self.values.clear();
                self.dirty = false;
                return Ok(false);
            }
            Err(error) => return Err(PersistError::Io(self.file.clone(), error.to_string())),
        };
        let (format, schema, values) = self.decode(&bytes)?;
        if format > FORMAT_VERSION {
            return Err(PersistError::VersionTooNew {
                found: format,
                supported: FORMAT_VERSION,
            });
        }
        self.format = format;
        self.schema = schema;
        self.values = values;
        self.dirty = false;
        Ok(true)
    }

    /// Copies the store's file beside itself with `.bak` added, saving first when needed.
    pub fn backup(&mut self) -> Result<PathBuf, PersistError> {
        if self.dirty {
            self.save()?;
        }
        let backup = format!("{}.bak", self.name);
        fs::copy(&self.file, &backup, true)
            .map_err(|error| PersistError::Io(backup.clone(), error.to_string()))?;
        let path = fs::resolve(&backup)
            .map_err(|error| PersistError::Io(backup.clone(), error.to_string()))?;
        Ok(path)
    }

    /// Reads the backup back over the store, and answers how many keys it holds.
    pub fn restore(&mut self) -> Result<usize, PersistError> {
        let backup = format!("{}.bak", self.name);
        if !fs::exists(&backup) {
            return Err(PersistError::Missing(backup));
        }
        fs::copy(&backup, &self.file, true)
            .map_err(|error| PersistError::Io(self.file.clone(), error.to_string()))?;
        self.load()?;
        Ok(self.values.len())
    }

    /// Moves the store's schema version, running the migrations that cover the span.
    pub fn migrate(&mut self, to: &str, migrations: &[StoreMigration]) -> usize {
        let from = self.schema.clone();
        let to = ModVersion::parse(to);
        let mut ran = 0;
        for migration in migrations {
            if migration.from.cmp_parts(&from) != std::cmp::Ordering::Less
                && migration.to.cmp_parts(&to) != std::cmp::Ordering::Greater
            {
                migration.run(&mut self.values);
                ran += 1;
            }
        }
        self.schema = to;
        self.dirty = true;
        ran
    }

    /// Writes the store's bytes to a path of the caller's choosing, atomically.
    pub fn export(&self, path: &str) -> Result<PathBuf, PersistError> {
        let bytes = self.encode();
        fs::atomic_write(path, &bytes)
            .map_err(|error| PersistError::Io(path.to_owned(), error.to_string()))?;
        fs::resolve(path).map_err(|error| PersistError::Io(path.to_owned(), error.to_string()))
    }

    /// Reads a store written by [`Store::export`] and merges its values in.
    pub fn import(&mut self, path: &str) -> Result<usize, PersistError> {
        let bytes =
            fs::read(path).map_err(|error| PersistError::Io(path.to_owned(), error.to_string()))?;
        let (_, _, values) = self.decode(&bytes)?;
        let count = values.len();
        for (key, value) in values {
            if self.values.get(&key) != Some(&value) {
                self.values.insert(key, value);
            }
        }
        self.dirty = true;
        Ok(count)
    }

    fn encode(&self) -> Vec<u8> {
        let mut text = format!(
            "# cinnaloader persistence '{}' format={} schema={}\n",
            self.name, FORMAT_VERSION, self.schema
        );
        for (key, value) in &self.values {
            text.push_str(key);
            text.push_str(" = ");
            text.push_str(&encode_value(value));
            text.push('\n');
        }
        text.into_bytes()
    }

    fn decode(
        &self,
        bytes: &[u8],
    ) -> Result<(u32, ModVersion, BTreeMap<String, ConfigValue>), PersistError> {
        let text = String::from_utf8_lossy(bytes);
        let (format, schema) = header(&text)
            .ok_or_else(|| PersistError::NotAStore(self.file.clone()))?;
        let mut values = BTreeMap::new();
        for line in text.lines().skip(1) {
            let line = line.trim_end();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once(" = ") else {
                return Err(PersistError::NotAStore(self.file.clone()));
            };
            let decoded = decode_value(value)
                .ok_or_else(|| PersistError::NotAStore(format!("{}: {key}", self.file)))?;
            values.insert(key.to_owned(), decoded);
        }
        Ok((format, ModVersion::parse(&schema), values))
    }
}

struct Held {
    stores: BTreeMap<StoreId, Store>,
    current: Option<StoreId>,
    saved: Vec<SavedData>,
    migrations: Vec<StoreMigration>,
    schema: ModVersion,
}

fn held() -> &'static Mutex<Held> {
    static HELD: OnceLock<Mutex<Held>> = OnceLock::new();
    HELD.get_or_init(|| {
        Mutex::new(Held {
            stores: BTreeMap::new(),
            current: None,
            saved: Vec::new(),
            migrations: Vec::new(),
            schema: ModVersion::parse("0"),
        })
    })
}

fn with<T>(body: impl FnOnce(&mut Held) -> T) -> T {
    let mut guard: MutexGuard<'_, Held> = held().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn next_store_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn current<T>(body: impl FnOnce(&Store) -> T, fallback: T) -> T {
    with(|held| match held.current.and_then(|id| held.stores.get(&id)) {
        Some(store) => body(store),
        None => fallback,
    })
}

fn current_mut<T>(body: impl FnOnce(&mut Store) -> T) -> Option<T> {
    with(|held| {
        let id = held.current?;
        let store = held.stores.get_mut(&id)?;
        Some(body(store))
    })
}

/// Opens a named store and makes it the one the plain functions work on.
pub fn open(name: &str) -> Result<StoreId, PersistError> {
    let mut store = Store::open(name)?;
    let id = store.id;
    with(|held| {
        if store.schema == ModVersion::parse("0") {
            store.schema = held.schema.clone();
        }
        held.stores.insert(id, store);
        held.current = Some(id);
    });
    Ok(id)
}

/// Opens a store, whether or not it already exists.
pub fn create(name: &str) -> Result<StoreId, PersistError> {
    open(name)
}

/// Closes the open store, and answers whether there was one.
pub fn close() -> bool {
    with(|held| match held.current.take() {
        Some(id) => held.stores.remove(&id).is_some(),
        None => false,
    })
}

/// Whether a store is open.
#[must_use]
pub fn is_open() -> bool {
    with(|held| held.current.is_some())
}

/// The name of the open store.
#[must_use]
pub fn current_store() -> Option<String> {
    current(|store| Some(store.name.clone()), None)
}

/// Reads the open store's file again, throwing away what was set since the last save.
pub fn load() -> bool {
    match current_mut(Store::load) {
        Some(Ok(_)) => true,
        Some(Err(error)) => {
            crate::warn!("a mod could not load its persistence store: {error}");
            false
        }
        None => false,
    }
}

/// Writes the open store, and answers whether it went.
pub fn save() -> bool {
    match flush() {
        Ok(_) => true,
        Err(error) => {
            crate::warn!("a mod could not save its persistence store: {error}");
            false
        }
    }
}

/// Writes the open store, and answers how many bytes went.
pub fn flush() -> Result<usize, PersistError> {
    current_mut(Store::save).unwrap_or(Err(PersistError::NoStore))
}

/// Reads one value from the open store.
#[must_use]
pub fn get(key: &str) -> Option<ConfigValue> {
    current(|store| store.get(key), None)
}

/// One value, or what to use instead.
#[must_use]
pub fn get_or(key: &str, fallback: ConfigValue) -> ConfigValue {
    with(|held| match held.current.and_then(|id| held.stores.get(&id)) {
        Some(store) => store.get_or(key, fallback.clone()),
        None => fallback,
    })
}

/// Stores a value in the open store, and answers whether it went.
pub fn set(key: &str, value: ConfigValue) -> bool {
    current_mut(|store| store.set(key, value)).unwrap_or(false)
}

/// Removes a key, and answers whether it was there.
pub fn delete(key: &str) -> bool {
    current_mut(|store| store.remove(key)).unwrap_or(false)
}

#[must_use]
pub fn exists(key: &str) -> bool {
    current(|store| store.contains(key), false)
}

/// The open store's keys, in order.
#[must_use]
pub fn keys() -> Vec<String> {
    current(Store::keys, Vec::new())
}

/// The whole open store, for a caller that wants to walk it.
#[must_use]
pub fn get_all() -> BTreeMap<String, ConfigValue> {
    current(Store::all, BTreeMap::new())
}

/// Empties the open store, and answers how many keys went.
pub fn clear() -> usize {
    current_mut(Store::clear).unwrap_or_default()
}

/// Whether anything changed since the last save or load.
#[must_use]
pub fn is_dirty() -> bool {
    current(|store| store.is_dirty(), false)
}

/// Sets how long the open store may wait before it saves itself. Zero means never.
pub fn set_interval(interval: Duration) {
    let _ = current_mut(|store| store.set_interval(interval));
}

/// Saves the open store if its interval has elapsed, and answers how many bytes went.
pub fn autosave_current() -> Result<Option<usize>, PersistError> {
    current_mut(Store::autosave).unwrap_or(Err(PersistError::NoStore))
}

/// Saves every open store whose interval is due, and answers how many saved.
pub fn autosave() -> usize {
    with(|held| {
        let mut saved = 0;
        for store in held.stores.values_mut() {
            match store.autosave() {
                Ok(Some(_)) => saved += 1,
                Ok(None) => {}
                Err(error) => {
                    crate::warn!("a store could not autosave: {error}");
                }
            }
        }
        saved
    })
}

/// Copies the open store's file beside itself, with `.bak` added.
pub fn backup() -> Result<PathBuf, PersistError> {
    current_mut(Store::backup).unwrap_or(Err(PersistError::NoStore))
}

/// Reads the backup back over the open store, and answers how many keys it holds.
pub fn restore() -> Result<usize, PersistError> {
    current_mut(Store::restore).unwrap_or(Err(PersistError::NoStore))
}

/// Moves the open store to a newer schema version, running the registered migrations.
pub fn migrate(to: &str) -> Result<usize, PersistError> {
    with(|held| {
        let id = held.current.ok_or(PersistError::NoStore)?;
        let migrations = &held.migrations;
        let store = held.stores.get_mut(&id).ok_or(PersistError::NoStore)?;
        Ok(store.migrate(to, migrations))
    })
}

/// Writes the open store to a path of the caller's choosing, atomically.
pub fn export(path: &str) -> Result<PathBuf, PersistError> {
    current_mut(|store| store.export(path)).unwrap_or(Err(PersistError::NoStore))
}

/// Reads a store written by [`export`] and merges its values into the open store.
pub fn import(path: &str) -> Result<usize, PersistError> {
    current_mut(|store| store.import(path)).unwrap_or(Err(PersistError::NoStore))
}

/// The mod's data version, written with every store it owns.
#[must_use]
pub fn get_schema_version() -> ModVersion {
    with(|held| match held.current.and_then(|id| held.stores.get(&id)) {
        Some(store) => store.schema.clone(),
        None => held.schema.clone(),
    })
}

/// Sets the mod's data version, on the open store and on stores opened later.
pub fn set_schema_version(version: &str) {
    let version = ModVersion::parse(version);
    with(|held| {
        if let Some(id) = held.current {
            if let Some(store) = held.stores.get_mut(&id) {
                store.schema = version.clone();
                store.dirty = true;
            }
        }
        held.schema = version;
    });
}

/// Registers what a mod wants saved. Answers whether the name was new.
pub fn register_saved_data(data: SavedData) -> bool {
    with(|held| {
        let existed = held.saved.iter().any(|held| held.name == data.name);
        held.saved.retain(|held| held.name != data.name);
        held.saved.push(data);
        !existed
    })
}

/// Removes a saved-data declaration by name, and answers whether it was there.
pub fn unregister_saved_data(name: &str) -> bool {
    with(|held| {
        let before = held.saved.len();
        held.saved.retain(|held| held.name != name);
        held.saved.len() != before
    })
}

/// Declares world data and answers the declaration it made.
pub fn register_world_data(name: &str, store: &str) -> SavedData {
    let data = SavedData::new(name, DataScope::World, store);
    register_saved_data(data.clone());
    data
}

/// Declares player data and answers the declaration it made.
pub fn register_player_data(name: &str, store: &str) -> SavedData {
    let data = SavedData::new(name, DataScope::Player, store);
    register_saved_data(data.clone());
    data
}

/// Declares global data and answers the declaration it made.
pub fn register_global_data(name: &str, store: &str) -> SavedData {
    let data = SavedData::new(name, DataScope::Global, store);
    register_saved_data(data.clone());
    data
}

/// Every declaration a mod made for one scope.
#[must_use]
pub fn saved_data(scope: DataScope) -> Vec<SavedData> {
    with(|held| {
        held.saved
            .iter()
            .filter(|data| data.scope == scope)
            .cloned()
            .collect()
    })
}

/// World data a mod declared.
#[must_use]
pub fn get_world_data() -> Vec<SavedData> {
    saved_data(DataScope::World)
}

/// Player data a mod declared.
#[must_use]
pub fn get_player_data() -> Vec<SavedData> {
    saved_data(DataScope::Player)
}

/// Global data a mod declared.
#[must_use]
pub fn get_global_data() -> Vec<SavedData> {
    saved_data(DataScope::Global)
}

/// Every declaration a mod made, in the order it made them.
#[must_use]
pub fn get_saved_data() -> Vec<SavedData> {
    with(|held| held.saved.clone())
}

/// Registers a migration the open store runs when its schema version moves.
pub fn register_migration(migration: StoreMigration) {
    with(|held| held.migrations.push(migration));
}

/// Why a question about the client's own save data has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "level" => {
            "the client's level and world save is the client's own file format, which this host \
             does not read or write"
                .to_owned()
        }
        "player" => {
            "the client's player data lives in the client's player file and never crosses the \
             mod boundary"
                .to_owned()
        }
        "registry" => {
            "the client's saved-data registry is private to the client and has no loader hook"
                .to_owned()
        }
        "directory" => {
            "the client's save directory is outside every mod's own folder, so nothing is \
             written there"
                .to_owned()
        }
        _ => format!("{what} is not something this host can reach"),
    }
}

/// What this host cannot reach about the client's own saved data.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["level", "player", "registry", "directory"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

/// The client's own save data, which this host cannot read or write. Each call says why.
pub mod client {
    use super::why_unknown;
    use crate::config::ConfigValue;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    /// The client's level and world save.
    #[must_use]
    pub fn level_data() -> Option<BTreeMap<String, ConfigValue>> {
        crate::warn!(
            "a mod asked this host for the client's level data: {}",
            why_unknown("level")
        );
        None
    }

    /// The client's player data.
    #[must_use]
    pub fn player_data() -> Option<BTreeMap<String, ConfigValue>> {
        crate::warn!(
            "a mod asked this host for the client's player data: {}",
            why_unknown("player")
        );
        None
    }

    /// The client's own saved-data registry.
    #[must_use]
    pub fn saved_data_registry() -> Vec<String> {
        crate::warn!(
            "a mod asked this host for the client's saved-data registry: {}",
            why_unknown("registry")
        );
        Vec::new()
    }

    /// A write into the client's save file.
    pub fn write_save(_bytes: &[u8]) -> bool {
        crate::warn!(
            "a mod asked this host to write the client's save file: {}",
            why_unknown("directory")
        );
        false
    }

    /// The client's save directory.
    #[must_use]
    pub fn save_directory() -> Option<PathBuf> {
        crate::warn!(
            "a mod asked this host for the client's save directory: {}",
            why_unknown("directory")
        );
        None
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && !name.contains("..")
        && !name.contains(['/', '\\'])
        && !name.contains('\n')
}

fn valid_key(key: &str) -> bool {
    !key.is_empty() && !key.contains('\n') && !key.starts_with('#') && !key.contains(" = ")
}

fn encode_value(value: &ConfigValue) -> String {
    match value {
        ConfigValue::Boolean(flag) => format!("b:{}", if *flag { "true" } else { "false" }),
        ConfigValue::Integer(number) => format!("i:{number}"),
        ConfigValue::Float(number) => format!("f:{number:?}"),
        ConfigValue::Percentage(number) => format!("G:{number:?}"),
        ConfigValue::Duration(millis) => format!("D:{millis}"),
        ConfigValue::Color(colour) => format!("c:{colour:06x}"),
        ConfigValue::String(text) => format!("s:{}", quote(text)),
        ConfigValue::Text(text) => format!("t:{}", quote(text)),
        ConfigValue::Enum(text) => format!("e:{}", quote(text)),
        ConfigValue::Keybind(text) => format!("k:{}", quote(text)),
        ConfigValue::FilePath(path) => format!("p:{}", quote(&path.display().to_string())),
        ConfigValue::DirectoryPath(path) => format!("d:{}", quote(&path.display().to_string())),
        ConfigValue::ResourceId(text) => format!("r:{}", quote(text)),
        ConfigValue::ItemId(text) => format!("n:{}", quote(text)),
        ConfigValue::BlockId(text) => format!("B:{}", quote(text)),
        ConfigValue::EntityId(text) => format!("E:{}", quote(text)),
        ConfigValue::Position(x, y, z) => format!("P:[{x:?}, {y:?}, {z:?}]"),
        ConfigValue::Vector2(x, y) => format!("V:[{x:?}, {y:?}]"),
        ConfigValue::Vector3(x, y, z) => format!("W:[{x:?}, {y:?}, {z:?}]"),
        ConfigValue::Range(from, to) => format!("R:[{from}, {to}]"),
        ConfigValue::List(items) => format!("l:[{}]", encode_seq(items)),
        ConfigValue::Set(items) => format!("S:[{}]", encode_seq(items)),
        ConfigValue::Map(fields) => format!("M:{{{}}}", encode_map(fields)),
        ConfigValue::Object(fields) => format!("O:{{{}}}", encode_map(fields)),
        ConfigValue::Compound(fields) => format!("C:{{{}}}", encode_map(fields)),
    }
}

fn encode_seq(items: &[ConfigValue]) -> String {
    items
        .iter()
        .map(encode_value)
        .collect::<Vec<_>>()
        .join(", ")
}

fn encode_map(fields: &BTreeMap<String, ConfigValue>) -> String {
    fields
        .iter()
        .map(|(key, value)| format!("{}: {}", quote(key), encode_value(value)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn decode_value(text: &str) -> Option<ConfigValue> {
    let (tag, rest) = text.trim().split_once(':')?;
    Some(match tag {
        "b" => ConfigValue::Boolean(match rest {
            "true" => true,
            "false" => false,
            _ => return None,
        }),
        "i" => ConfigValue::Integer(rest.parse().ok()?),
        "f" => ConfigValue::Float(rest.parse().ok()?),
        "G" => ConfigValue::Percentage(rest.parse().ok()?),
        "D" => ConfigValue::Duration(rest.parse().ok()?),
        "c" => ConfigValue::Color(u32::from_str_radix(rest, 16).ok()?),
        "s" => ConfigValue::String(unquote(rest)?),
        "t" => ConfigValue::Text(unquote(rest)?),
        "e" => ConfigValue::Enum(unquote(rest)?),
        "k" => ConfigValue::Keybind(unquote(rest)?),
        "p" => ConfigValue::FilePath(PathBuf::from(unquote(rest)?)),
        "d" => ConfigValue::DirectoryPath(PathBuf::from(unquote(rest)?)),
        "r" => ConfigValue::ResourceId(unquote(rest)?),
        "n" => ConfigValue::ItemId(unquote(rest)?),
        "B" => ConfigValue::BlockId(unquote(rest)?),
        "E" => ConfigValue::EntityId(unquote(rest)?),
        "P" => {
            let numbers = floats(rest)?;
            match numbers.as_slice() {
                [x, y, z] => ConfigValue::Position(*x, *y, *z),
                _ => return None,
            }
        }
        "V" => {
            let numbers = floats(rest)?;
            match numbers.as_slice() {
                [x, y] => ConfigValue::Vector2(*x, *y),
                _ => return None,
            }
        }
        "W" => {
            let numbers = floats(rest)?;
            match numbers.as_slice() {
                [x, y, z] => ConfigValue::Vector3(*x, *y, *z),
                _ => return None,
            }
        }
        "R" => {
            let inner = delimited(rest, '[', ']')?;
            let parts = split_top(inner);
            match parts.as_slice() {
                [from, to] => ConfigValue::Range(from.trim().parse().ok()?, to.trim().parse().ok()?),
                _ => return None,
            }
        }
        "l" => ConfigValue::List(decode_seq(rest)?),
        "S" => ConfigValue::Set(decode_seq(rest)?),
        "M" => ConfigValue::Map(decode_map(rest)?),
        "O" => ConfigValue::Object(decode_map(rest)?),
        "C" => ConfigValue::Compound(decode_map(rest)?),
        _ => return None,
    })
}

fn decode_seq(rest: &str) -> Option<Vec<ConfigValue>> {
    let inner = delimited(rest, '[', ']')?;
    split_top(inner).into_iter().map(decode_value).collect()
}

fn decode_map(rest: &str) -> Option<BTreeMap<String, ConfigValue>> {
    let inner = delimited(rest, '{', '}')?;
    let mut fields = BTreeMap::new();
    for pair in split_top(inner) {
        let (key, value) = split_pair(pair)?;
        fields.insert(key, value);
    }
    Some(fields)
}

fn floats(rest: &str) -> Option<Vec<f64>> {
    let inner = delimited(rest, '[', ']')?;
    split_top(inner)
        .into_iter()
        .map(|part| part.trim().parse().ok())
        .collect()
}

fn delimited(text: &str, open: char, close: char) -> Option<&str> {
    let text = text.trim();
    text.strip_prefix(open)?.strip_suffix(close)
}

fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

fn unquote(text: &str) -> Option<String> {
    // the closing quote is the last character, so a trailing \" keeps its quote
    let inner = text.trim().strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next()? {
            '\\' => out.push('\\'),
            '"' => out.push('"'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            other => out.push(other),
        }
    }
    Some(out)
}

fn split_pair(pair: &str) -> Option<(String, ConfigValue)> {
    let pair = pair.trim();
    if !pair.starts_with('"') {
        return None;
    }
    let mut escaped = false;
    let mut close = None;
    for (index, ch) in pair[1..].char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '"' => {
                close = Some(index + 2);
                break;
            }
            _ => {}
        }
    }
    let close = close?;
    let key = unquote(&pair[..close])?;
    let rest = pair[close..].trim_start().strip_prefix(':')?.trim();
    Some((key, decode_value(rest)?))
}

fn split_top(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut depth = 0i32;
    let mut quoted = false;
    let mut escaped = false;
    for (index, ch) in text.char_indices() {
        if quoted {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                quoted = false;
            }
            continue;
        }
        match ch {
            '"' => quoted = true,
            '[' | '{' | '(' => depth += 1,
            ']' | '}' | ')' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(text[start..index].trim());
                start = index + 1;
            }
            _ => {}
        }
    }
    let tail = text[start..].trim();
    if !tail.is_empty() {
        parts.push(tail);
    }
    parts
}

fn header(text: &str) -> Option<(u32, String)> {
    let line = text.lines().next()?;
    let rest = line.strip_prefix("# cinnaloader persistence '")?;
    let (_, rest) = rest.split_once('\'')?;
    Some((number_after(rest, "format=")?, text_after(rest, "schema=")))
}

fn number_after(text: &str, key: &str) -> Option<u32> {
    let rest = text.split_once(key)?.1;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

fn text_after(text: &str, key: &str) -> String {
    text.split_once(key)
        .map(|(_, rest)| rest.split_whitespace().next().unwrap_or_default().to_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reset() {
        with(|held| {
            held.stores.clear();
            held.current = None;
            held.saved.clear();
            held.migrations.clear();
            held.schema = ModVersion::parse("0");
        });
    }

    fn rooted(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "cinnaloader-persist-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        fs::set_root(root.clone());
        reset();
        root
    }

    #[test]
    fn a_store_round_trips_a_nested_list_and_a_float() {
        let _serial = crate::test_lock::serial();
        let root = rooted("round-trip");
        open("seen").unwrap();
        let payload = ConfigValue::List(vec![
            ConfigValue::Float(1.5),
            ConfigValue::List(vec![
                ConfigValue::Text("deep".to_owned()),
                ConfigValue::Integer(7),
            ]),
            ConfigValue::Boolean(true),
        ]);
        assert!(set("payload", payload.clone()));
        assert!(set("ratio", ConfigValue::Float(0.25)));
        assert!(set("runs", ConfigValue::Integer(3)));
        let written = flush().unwrap();
        assert!(written > 0, "a store writes some bytes: {written}");
        assert!(!is_dirty(), "a save clears the dirty flag");
        close();

        open("seen").unwrap();
        assert_eq!(get("payload"), Some(payload));
        assert_eq!(get("ratio"), Some(ConfigValue::Float(0.25)));
        assert_eq!(get("runs").and_then(|value| value.as_integer()), Some(3));
        assert_eq!(keys(), vec!["payload", "ratio", "runs"]);
        close();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_save_after_a_load_produces_the_same_bytes() {
        let _serial = crate::test_lock::serial();
        let root = rooted("bytes");
        open("bytes").unwrap();
        set("zero", ConfigValue::Float(2.0));
        set(
            "items",
            ConfigValue::List(vec![ConfigValue::Text("x".to_owned())]),
        );
        flush().unwrap();
        let first = fs::read("bytes.sav").unwrap();
        close();

        open("bytes").unwrap();
        flush().unwrap();
        let second = fs::read("bytes.sav").unwrap();
        assert_eq!(first, second, "a save after a load writes the same bytes");
        close();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_dirty_store_saves_on_its_interval_and_not_before() {
        let _serial = crate::test_lock::serial();
        let root = rooted("autosave");
        open("auto").unwrap();
        set_interval(Duration::from_millis(60));
        assert!(set("blocks", ConfigValue::Integer(1)));
        assert!(is_dirty());
        assert_eq!(
            autosave_current().unwrap(),
            None,
            "not before the interval elapses"
        );
        std::thread::sleep(Duration::from_millis(90));
        let saved = autosave_current().unwrap();
        assert!(matches!(saved, Some(bytes) if bytes > 0));
        assert!(!is_dirty(), "a save clears the dirty flag");

        set_interval(Duration::ZERO);
        assert!(set("blocks", ConfigValue::Integer(2)));
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(autosave_current().unwrap(), None, "zero means never");
        assert!(is_dirty());
        close();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_newer_format_version_is_refused() {
        let _serial = crate::test_lock::serial();
        let root = rooted("version");
        fs::write_text(
            "future.sav",
            "# cinnaloader persistence 'future' format=2 schema=0\n",
        )
        .unwrap();
        match Store::open("future") {
            Err(PersistError::VersionTooNew { found, supported }) => {
                assert_eq!(found, 2);
                assert_eq!(supported, FORMAT_VERSION);
            }
            other => panic!("a newer format was not refused: {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_dirty_flag_is_set_only_by_a_change() {
        let _serial = crate::test_lock::serial();
        let root = rooted("dirty");
        open("dirty").unwrap();
        assert!(!is_dirty());
        let _ = get("missing");
        let _ = get_all();
        assert!(!is_dirty(), "reading is not a change");
        assert!(set("k", ConfigValue::Integer(1)));
        assert!(is_dirty());
        flush().unwrap();
        assert!(!is_dirty());
        assert!(set("k", ConfigValue::Integer(1)));
        assert!(!is_dirty(), "setting the same value is not a change");
        assert!(delete("k"));
        assert!(is_dirty());
        flush().unwrap();
        assert!(!delete("k"), "removing what is not there is not a change");
        assert!(!is_dirty());
        close();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_saved_data_registry_is_scoped() {
        let _serial = crate::test_lock::serial();
        let root = rooted("registry");
        assert!(!save(), "no store is open");
        assert!(get("anything").is_none());
        assert_eq!(clear(), 0);
        assert!(matches!(flush(), Err(PersistError::NoStore)));

        register_world_data("spawns", "world");
        register_player_data("playtime", "player");
        register_global_data("settings", "global");
        assert_eq!(get_world_data().len(), 1);
        assert_eq!(get_world_data()[0].scope, DataScope::World);
        assert_eq!(get_player_data()[0].name, "playtime");
        assert_eq!(get_global_data()[0].store, "global");
        assert!(register_saved_data(
            SavedData::new("achievements", DataScope::Player, "player").version("2")
        ));
        assert_eq!(get_player_data().len(), 2);
        assert!(unregister_saved_data("playtime"));
        assert!(!unregister_saved_data("playtime"));
        assert_eq!(get_player_data().len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_migration_moves_the_schema_version() {
        let _serial = crate::test_lock::serial();
        let root = rooted("migrate");
        open("data").unwrap();
        set("old_name", ConfigValue::Text("kept".to_owned()));
        set_schema_version("1");
        register_migration(StoreMigration::new("1", "2", "rename a key", |values| {
            if let Some(value) = values.remove("old_name") {
                values.insert("new_name".to_owned(), value);
            }
        }));
        assert_eq!(migrate("2").unwrap(), 1);
        assert!(!exists("old_name"));
        assert!(exists("new_name"));
        assert_eq!(get_schema_version().to_string(), "2");
        close();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn what_this_host_cannot_reach_says_why() {
        let _serial = crate::test_lock::serial();
        let rows = refusals();
        assert_eq!(rows.len(), 4);
        assert!(rows.iter().any(|(what, _)| what == "level"));
        assert!(why_unknown("level").contains("level"));
        assert!(why_unknown("directory").contains("save directory"));
        assert!(!why_unknown("nonsense").is_empty());
        assert!(client::level_data().is_none());
        assert!(client::player_data().is_none());
        assert!(client::saved_data_registry().is_empty());
        assert!(!client::write_save(b"nope"));
        assert!(client::save_directory().is_none());
    }
}
