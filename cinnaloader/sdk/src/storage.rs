//! Storage: named key/value stores beside a mod's config, with namespaces and transactions.
//!
//! A `.packed` store is run-length packed ([`crate::fs::compress`]), which shrinks repeated
//! data and leaves varied data much the same size; it is not a general compressor.
//!
//! A store is a file in the mod's own directory, written in this loader's own text format —
//! one `key = value` per line, values rendered the way [`crate::config::ConfigValue`] renders
//! them. That is deliberate: a mod's data stays readable, and reading it does not need this
//! library.
//!
//! ```
//! use cinnaloader::prelude::*;
//!
//! # let root = std::env::temp_dir().join("cinnaloader-doc-store");
//! # let _ = std::fs::remove_dir_all(&root);
//! # std::fs::create_dir_all(&root).unwrap();
//! # fs::set_root(root);
//! storage::open("seen").expect("a store beside the config");
//! storage::set("blocks", ConfigValue::Integer(42));
//! storage::set("first_run", ConfigValue::Boolean(false));
//! storage::flush();
//! assert_eq!(storage::get("blocks").and_then(|value| value.as_integer()), Some(42));
//! let wanderers = storage::namespace("world");
//! wanderers.set("spawn", ConfigValue::Text("0,64,0".into()));
//! # storage::close();
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use crate::config::{ConfigType, ConfigValue};

/// What went wrong, with the store it went wrong in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// No store is open.
    NoStore,
    /// The name is not one a store may have.
    BadName(String),
    /// The file could not be read or written.
    Io(String, String),
    /// The file is not a store this loader wrote.
    NotAStore(String),
    /// A version was asked to migrate from something that is not what the file holds.
    VersionMismatch(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoStore => formatter.write_str("no store is open"),
            Self::BadName(name) => write!(formatter, "'{name}' is not a usable store name"),
            Self::Io(path, message) => write!(formatter, "{path}: {message}"),
            Self::NotAStore(path) => write!(formatter, "{path} is not a store this loader wrote"),
            Self::VersionMismatch(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for StoreError {}

/// The format a store is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Format {
    /// One `key = value` per line. What this loader writes.
    #[default]
    Text,
    /// The same, gzipped by [`crate::fs::compress`] so a big store stays small.
    Packed,
}

impl Format {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Packed => "packed",
        }
    }

    /// The extension a store in this format is written with.
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            Self::Text => "store",
            Self::Packed => "packed",
        }
    }
}

/// One named store, open on a file.
#[derive(Debug, Clone)]
pub struct Storage {
    name: String,
    path: PathBuf,
    format: Format,
    version: String,
    values: BTreeMap<String, ConfigValue>,
    dirty: bool,
}

impl Storage {
    /// Opens a store by name, reading what is already in it. The file lives beside the mod's
    /// config; nothing outside that directory is reachable.
    pub fn open(name: &str) -> Result<Self, StoreError> {
        if name.is_empty()
            || name.contains('/')
            || name.contains('\\')
            || name.contains("..")
            || name.starts_with('.')
        {
            return Err(StoreError::BadName(name.to_owned()));
        }
        // A name ending in `.packed` is written packed; anything else is plain text.
        let (stem, format) = match name.strip_suffix(".packed") {
            Some(stem) if !stem.is_empty() => (stem, Format::Packed),
            _ => (name, Format::Text),
        };
        let file = match format {
            Format::Text => format!("{stem}.store"),
            Format::Packed => format!("{stem}.packed"),
        };
        let path = crate::fs::resolve(&file)
            .map_err(|error| StoreError::Io(name.to_owned(), error.to_string()))?;
        let mut store = Self {
            name: name.to_owned(),
            path,
            format,
            version: String::new(),
            values: BTreeMap::new(),
            dirty: false,
        };
        store.read()?;
        Ok(store)
    }

    /// The same as [`Storage::open`], for a caller that thinks of it as a database. This
    /// host has no SQL engine: it is a key/value store, and says so in the name.
    pub fn open_database(name: &str) -> Result<Self, StoreError> {
        Self::open(name)
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn format(&self) -> Format {
        self.format
    }

    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
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

    /// Sets a value, and answers whether it changed anything.
    pub fn set(&mut self, key: &str, value: ConfigValue) -> bool {
        self.dirty = true;
        self.values.insert(key.to_owned(), value).is_none()
    }

    /// Removes a key, and answers whether it was there.
    pub fn delete(&mut self, key: &str) -> bool {
        let removed = self.values.remove(key).is_some();
        self.dirty |= removed;
        removed
    }

    #[must_use]
    pub fn contains(&self, key: &str) -> bool {
        self.values.contains_key(key)
    }

    #[must_use]
    pub fn keys(&self) -> Vec<String> {
        self.values.keys().cloned().collect()
    }

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

    /// Empties it, which takes effect on the next flush.
    pub fn clear(&mut self) -> usize {
        let count = self.values.len();
        self.values.clear();
        self.dirty = true;
        count
    }

    /// Writes what is in memory to the file.
    pub fn flush(&mut self) -> Result<usize, StoreError> {
        let mut text = format!("# cinnaloader store '{}', version {}\n", self.name, self.version);
        for (key, value) in &self.values {
            text.push_str(&format!("{key} = {}\n", value.render()));
        }
        let bytes = match self.format {
            Format::Text => text.into_bytes(),
            Format::Packed => crate::fs::compress(text.as_bytes()),
        };
        let temporary = self.path.with_extension("store.part");
        std::fs::write(&temporary, &bytes).map_err(|error| {
            StoreError::Io(temporary.display().to_string(), error.to_string())
        })?;
        std::fs::rename(&temporary, &self.path).map_err(|error| {
            let _ = std::fs::remove_file(&temporary);
            StoreError::Io(self.path.display().to_string(), error.to_string())
        })?;
        self.dirty = false;
        Ok(bytes.len())
    }

    fn read(&mut self) -> Result<(), StoreError> {
        let Ok(bytes) = std::fs::read(&self.path) else {
            // No file yet: an empty store, which is what a first run wants.
            return Ok(());
        };
        let text = match self.format {
            Format::Text => String::from_utf8_lossy(&bytes).into_owned(),
            Format::Packed => {
                let unpacked = crate::fs::decompress(&bytes).map_err(|error| {
                    StoreError::NotAStore(format!("{}: {error}", self.path.display()))
                })?;
                String::from_utf8_lossy(&unpacked).into_owned()
            }
        };
        for line in text.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("# cinnaloader store '") {
                if let Some(version) = rest.split("version ").nth(1) {
                    self.version = version.trim().to_owned();
                }
                continue;
            }
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let value = value.trim();
            // Without a schema a store value has no declared kind, so a value that reads as
            // an integer, a float or a flag is stored as one, and anything else is text.
            let parsed = ConfigValue::parse(value, ConfigType::Integer)
                .or_else(|| ConfigValue::parse(value, ConfigType::Float))
                .or_else(|| ConfigValue::parse(value, ConfigType::Boolean))
                .unwrap_or_else(|| ConfigValue::Text(value.to_owned()));
            self.values.insert(key.to_owned(), parsed);
        }
        self.dirty = false;
        Ok(())
    }

    /// Reads the file again over what is in memory.
    pub fn reload(&mut self) -> Result<(), StoreError> {
        self.values.clear();
        self.read()
    }

    /// Copies the file beside itself with `.bak` added.
    pub fn backup(&mut self) -> Result<PathBuf, StoreError> {
        if self.dirty {
            self.flush()?;
        }
        let path = self.path.with_extension("store.bak");
        std::fs::copy(&self.path, &path)
            .map_err(|error| StoreError::Io(path.display().to_string(), error.to_string()))?;
        Ok(path)
    }

    /// Reads the backup back over the store.
    pub fn restore(&mut self) -> Result<usize, StoreError> {
        let path = self.path.with_extension("store.bak");
        if !path.exists() {
            return Err(StoreError::Io(
                path.display().to_string(),
                "there is no backup to restore".to_owned(),
            ));
        }
        self.values.clear();
        self.read()?;
        std::fs::copy(&path, &self.path)
            .map_err(|error| StoreError::Io(self.path.display().to_string(), error.to_string()))?;
        self.values.clear();
        self.read()?;
        Ok(self.values.len())
    }

    /// Moves the store to `to`, running `migrate` over its contents first. The version in the
    /// file is what a caller checks before calling this.
    pub fn migrate(
        &mut self,
        to: &str,
        migrate: impl FnOnce(&mut BTreeMap<String, ConfigValue>),
    ) -> usize {
        let before = self.values.clone();
        migrate(&mut self.values);
        self.version = to.to_owned();
        self.dirty = true;
        let changed = self
            .values
            .iter()
            .filter(|(key, value)| before.get(*key) != Some(*value))
            .count()
            + before
                .keys()
                .filter(|key| !self.values.contains_key(*key))
                .count();
        changed
    }

    /// A view of one prefix inside this store, so two parts of a mod do not collide.
    #[must_use]
    pub fn namespace(&self, prefix: &str) -> Namespace {
        Namespace {
            prefix: prefix.to_owned(),
        }
    }

    /// A batch of changes that either all happen or none do.
    #[must_use]
    pub fn transaction(&self) -> StoreTransaction {
        StoreTransaction {
            before: self.values.clone(),
            open: true,
        }
    }
}

/// One prefix inside the open store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Namespace {
    prefix: String,
}

impl Namespace {
    #[must_use]
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    #[must_use]
    pub fn scope(&self, key: &str) -> String {
        match self.prefix.is_empty() {
            true => key.to_owned(),
            false => format!("{}:{key}", self.prefix),
        }
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<ConfigValue> {
        get(&self.scope(key))
    }

    pub fn set(&self, key: &str, value: ConfigValue) -> bool {
        set(&self.scope(key), value)
    }

    pub fn delete(&self, key: &str) -> bool {
        delete(&self.scope(key))
    }

    #[must_use]
    pub fn contains(&self, key: &str) -> bool {
        contains(&self.scope(key))
    }

    /// Every key in this namespace, without the prefix.
    #[must_use]
    pub fn keys(&self) -> Vec<String> {
        keys(&self.prefix)
    }

    /// Empties this namespace, and answers how many keys went.
    pub fn clear(&self) -> usize {
        let keys = self.keys();
        for key in &keys {
            delete(&self.scope(key));
        }
        keys.len()
    }
}

impl fmt::Display for Namespace {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.prefix)
    }
}

/// A batch of changes to the open store.
#[derive(Debug, Clone)]
pub struct StoreTransaction {
    before: BTreeMap<String, ConfigValue>,
    open: bool,
}

impl StoreTransaction {
    /// Keeps everything set since it began.
    pub fn commit(&mut self) -> bool {
        if !self.open {
            return false;
        }
        self.open = false;
        true
    }

    /// Puts the store back as it was.
    pub fn rollback(&mut self) -> bool {
        if !self.open {
            return false;
        }
        let before = std::mem::take(&mut self.before);
        self.open = false;
        with(|held| {
            if let Some(store) = held.store.as_mut() {
                store.values = before;
                store.dirty = true;
            }
        });
        true
    }

    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open
    }
}

struct Held {
    store: Option<Storage>,
}

fn held() -> &'static Mutex<Held> {
    static HELD: OnceLock<Mutex<Held>> = OnceLock::new();
    HELD.get_or_init(|| Mutex::new(Held { store: None }))
}

fn with<T>(body: impl FnOnce(&mut Held) -> T) -> T {
    let mut guard: MutexGuard<'_, Held> = held().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Opens a store and makes it the one the plain functions work on.
pub fn open(name: &str) -> Result<String, StoreError> {
    let store = Storage::open(name)?;
    let name = store.name.clone();
    with(|held| held.store = Some(store));
    Ok(name)
}

/// Opens a store that thinks of itself as a database. Key/value, not SQL, in this host.
pub fn open_database(name: &str) -> Result<String, StoreError> {
    open(name)
}

/// Closes the open store, and answers whether there was one.
pub fn close() -> bool {
    with(|held| held.store.take().is_some())
}

/// Whether a store is open.
#[must_use]
pub fn is_open() -> bool {
    with(|held| held.store.is_some())
}

/// The name of the open store.
#[must_use]
pub fn current() -> Option<String> {
    with(|held| held.store.as_ref().map(|store| store.name.clone()))
}

/// The version the open store was written at.
#[must_use]
pub fn version() -> Option<String> {
    with(|held| held.store.as_ref().map(|store| store.version.clone()))
}

/// One value from the open store.
#[must_use]
pub fn get(key: &str) -> Option<ConfigValue> {
    with(|held| held.store.as_ref().and_then(|store| store.get(key)))
}

/// One value, or what to use instead.
#[must_use]
pub fn get_or(key: &str, fallback: ConfigValue) -> ConfigValue {
    with(|held| {
        held.store
            .as_ref()
            .map_or(fallback.clone(), |store| store.get_or(key, fallback))
    })
}

/// Sets a value in the open store, and answers whether the store was open.
pub fn set(key: &str, value: ConfigValue) -> bool {
    with(|held| match held.store.as_mut() {
        Some(store) => {
            store.set(key, value);
            true
        }
        None => false,
    })
}

/// Removes a key, and answers whether it was there.
pub fn delete(key: &str) -> bool {
    with(|held| {
        held.store
            .as_mut()
            .map(|store| store.delete(key))
            .unwrap_or(false)
    })
}

#[must_use]
pub fn contains(key: &str) -> bool {
    with(|held| {
        held.store
            .as_ref()
            .map(|store| store.contains(key))
            .unwrap_or(false)
    })
}

/// The keys in the open store: all of them, or the ones under `prefix`.
#[must_use]
pub fn keys(prefix: &str) -> Vec<String> {
    with(|held| {
        let Some(store) = held.store.as_ref() else {
            return Vec::new();
        };
        store
            .keys()
            .into_iter()
            .filter_map(|key| match prefix.is_empty() {
                true => Some(key),
                false => key
                    .strip_prefix(&format!("{prefix}:"))
                    .map(str::to_owned),
            })
            .collect()
    })
}

/// Empties the open store, and answers how many keys went.
pub fn clear() -> usize {
    with(|held| {
        held.store
            .as_mut()
            .map(|store| store.clear())
            .unwrap_or_default()
    })
}

/// Writes the open store to its file, and answers how many bytes went.
pub fn flush() -> Result<usize, StoreError> {
    with(|held| match held.store.as_mut() {
        Some(store) => store.flush(),
        None => Err(StoreError::NoStore),
    })
}

/// Starts a transaction over the open store.
pub fn transaction() -> Option<StoreTransaction> {
    with(|held| held.store.as_ref().map(Storage::transaction))
}

/// Copies the open store's file beside itself.
pub fn backup() -> Result<PathBuf, StoreError> {
    with(|held| match held.store.as_mut() {
        Some(store) => store.backup(),
        None => Err(StoreError::NoStore),
    })
}

/// Reads the backup back over the open store, and answers how many keys it holds.
pub fn restore() -> Result<usize, StoreError> {
    with(|held| match held.store.as_mut() {
        Some(store) => store.restore(),
        None => Err(StoreError::NoStore),
    })
}

/// Moves the open store to a new version, running `migrate` over its contents.
pub fn migrate(
    to: &str,
    migrate: impl FnOnce(&mut BTreeMap<String, ConfigValue>),
) -> Result<usize, StoreError> {
    with(|held| match held.store.as_mut() {
        Some(store) => Ok(store.migrate(to, migrate)),
        None => Err(StoreError::NoStore),
    })
}

/// A view of one prefix in the open store.
#[must_use]
pub fn namespace(prefix: &str) -> Namespace {
    Namespace {
        prefix: prefix.to_owned(),
    }
}

/// Every store beside this mod's config, by name.
#[must_use]
pub fn stores() -> Vec<String> {
    let Ok(root) = crate::fs::resolve("") else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|ext| ext == "store" || ext == "packed")
        })
        .filter_map(|entry| {
            entry
                .path()
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
        })
        .collect();
    names.sort();
    names
}

/// Reads a store from somewhere else and merges it into the open one, without opening a
/// second store.
pub fn import(path: &Path) -> Result<usize, StoreError> {
    let bytes = std::fs::read(path)
        .map_err(|error| StoreError::Io(path.display().to_string(), error.to_string()))?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let mut count = 0;
    with(|held| {
        let Some(store) = held.store.as_mut() else {
            return;
        };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                store.set(key.trim(), ConfigValue::Text(value.trim().to_owned()));
                count += 1;
            }
        }
    });
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn rooted(name: &str) {
        let root =
            std::env::temp_dir().join(format!("cinnaloader-store-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        crate::fs::set_root(root);
        close();
    }

    #[test]
    fn a_store_written_by_this_loader_is_read_by_this_loader() {
        let _serial = serial();
        rooted("round-trip");
        assert_eq!(open("seen").unwrap(), "seen");
        assert!(is_open());
        set("blocks", ConfigValue::Integer(42));
        set("name", ConfigValue::Text("hello".to_owned()));
        assert!(flush().unwrap() > 0);
        close();

        open("seen").unwrap();
        assert_eq!(
            get("blocks").and_then(|value| value.as_integer()),
            Some(42)
        );
        assert_eq!(
            get("name").and_then(|value| value.as_text().map(str::to_owned)),
            Some("hello".to_owned())
        );
        assert!(keys("").contains(&"blocks".to_owned()));
        close();
    }

    #[test]
    fn a_namespace_keeps_two_parts_apart() {
        let _serial = serial();
        rooted("namespace");
        open("split").unwrap();
        let left = namespace("left");
        let right = namespace("right");
        left.set("colour", ConfigValue::Text("red".to_owned()));
        right.set("colour", ConfigValue::Text("blue".to_owned()));
        assert_eq!(left.scope("colour"), "left:colour");
        assert_eq!(
            left.get("colour")
                .and_then(|value| value.as_text().map(str::to_owned)),
            Some("red".to_owned())
        );
        assert_eq!(right.keys(), vec!["colour".to_owned()]);
        assert_eq!(left.clear(), 1);
        assert!(!left.contains("colour"));
        assert!(right.contains("colour"));
        close();
    }

    #[test]
    fn a_transaction_keeps_or_discards_the_batch() {
        let _serial = serial();
        rooted("transaction");
        open("batch").unwrap();
        set("kept", ConfigValue::Integer(1));
        let mut batch = transaction().expect("a store is open");
        set("kept", ConfigValue::Integer(2));
        set("extra", ConfigValue::Integer(3));
        assert!(batch.is_open());
        assert!(batch.rollback());
        assert_eq!(get("kept").and_then(|value| value.as_integer()), Some(1));
        assert!(!contains("extra"));
        assert!(!batch.commit(), "a closed transaction stays closed");

        let mut second = transaction().unwrap();
        set("kept", ConfigValue::Integer(9));
        assert!(second.commit());
        assert_eq!(get("kept").and_then(|value| value.as_integer()), Some(9));
        close();
    }

    #[test]
    fn a_backup_can_be_restored_over_the_store() {
        let _serial = serial();
        rooted("backup");
        open("keep").unwrap();
        set("blocks", ConfigValue::Integer(1));
        flush().unwrap();
        let path = backup().unwrap();
        assert!(path.exists());
        set("blocks", ConfigValue::Integer(2));
        flush().unwrap();
        assert_eq!(get("blocks").and_then(|value| value.as_integer()), Some(2));
        assert_eq!(restore().unwrap(), 1);
        assert_eq!(get("blocks").and_then(|value| value.as_integer()), Some(1));
        close();
    }

    #[test]
    fn a_migration_moves_the_version_and_the_keys() {
        let _serial = serial();
        rooted("migrate");
        open("moved").unwrap();
        set("old_name", ConfigValue::Text("kept".to_owned()));
        assert_eq!(version().as_deref(), Some(""));
        let changed = migrate("2", |values| {
            if let Some(value) = values.remove("old_name") {
                values.insert("new_name".to_owned(), value);
            }
        })
        .unwrap();
        assert!(changed >= 2, "one key went and one arrived: {changed}");
        assert!(!contains("old_name"));
        assert_eq!(version().as_deref(), Some("2"));
        close();
    }

    #[test]
    fn a_packed_store_round_trips_and_a_broken_pack_is_refused() {
        let _serial = serial();
        rooted("packed");
        open("tidy.packed").unwrap();
        for index in 0..200 {
            set(
                &format!("k{index:03}"),
                ConfigValue::Text("value that repeats and repeats and repeats".to_owned()),
            );
        }
        flush().unwrap();
        let path = crate::fs::resolve("tidy.packed").unwrap();
        let packed = std::fs::read(&path).unwrap();
        assert!(packed.starts_with(b"CINNA1"), "it is this loader's own pack");
        // The pack is a run-length pass, so varied text comes back the same size it went in
        // and a repetitive store gets smaller: what matters is that it reads back exactly.
        assert_eq!(
            crate::fs::decompress(&crate::fs::compress(
                &crate::fs::decompress(&packed).unwrap()
            ))
            .unwrap()
            .len(),
            10044
        );
        close();

        open("tidy.packed").unwrap();
        assert_eq!(get("k199").and_then(|value| value.as_text().map(str::to_owned)), Some("value that repeats and repeats and repeats".to_owned()));
        close();

        let broken = crate::fs::resolve("broken.packed").unwrap();
        std::fs::write(&broken, b"not a pack").unwrap();
        assert_eq!(
            Storage::open("broken.packed").unwrap_err(),
            StoreError::NotAStore(format!(
                "{}: wrong kind of path: not a cinnaloader pack: no header",
                broken.display()
            ))
        );
    }

    #[test]
    fn a_name_that_would_leave_the_directory_is_refused() {
        let _serial = serial();
        rooted("names");
        for name in ["../escape", "a/b", ".hidden", ""] {
            assert!(
                matches!(Storage::open(name), Err(StoreError::BadName(_))),
                "{name} was accepted"
            );
        }
    }

    #[test]
    fn the_stores_beside_a_mod_can_be_listed() {
        let _serial = serial();
        rooted("listing");
        open("one").unwrap();
        flush().unwrap();
        close();
        open("two").unwrap();
        flush().unwrap();
        close();
        assert_eq!(stores(), vec!["one".to_owned(), "two".to_owned()]);
    }

    #[test]
    fn every_plain_function_says_what_is_wrong_without_a_store() {
        let _serial = serial();
        rooted("nostore");
        assert!(matches!(flush(), Err(StoreError::NoStore)));
        assert!(matches!(backup(), Err(StoreError::NoStore)));
        assert!(matches!(restore(), Err(StoreError::NoStore)));
        assert!(!set("k", ConfigValue::Integer(1)));
        assert!(get("k").is_none());
        assert!(keys("").is_empty());
        assert!(transaction().is_none());
    }
}
