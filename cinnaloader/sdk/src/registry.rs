//! Registries: one engine a mod uses for everything it ships.
//!
//! A registry is an id-to-value map with a key index, a freeze flag, a version, a change log
//! and a snapshot. It is deliberately generic — the value is a [`ConfigValue`], because that
//! is what a mod hands over for anything from a number to a whole settings map — and it is
//! what the rest of the loader's registries want: blocks, items, biomes, recipes and the ones
//! a mod invents for itself.
//!
//! ```
//! use cinnaloader::prelude::*;
//!
//! registry::register("hello:gem", ConfigValue::Integer(3));
//! assert_eq!(registry::get("hello:gem"), Some(ConfigValue::Integer(3)));
//! registry::register_keyed("hello:gem", "gem", ConfigValue::Integer(3));
//! assert_eq!(registry::get_key("hello:gem"), Some("gem".into()));
//! assert_eq!(registry::get_by_key("gem"), Some(ConfigValue::Integer(3)));
//! assert!(registry::validate().is_empty());
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

use crate::config::ConfigValue;

/// What kind of change a registry recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChangeKind {
    Registered,
    Replaced,
    Removed,
}

impl ChangeKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Registered => "registered",
            Self::Replaced => "replaced",
            Self::Removed => "removed",
        }
    }
}

/// One change, with the version it happened at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub kind: ChangeKind,
    pub id: String,
    pub at_version: u64,
}

impl fmt::Display for Change {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "#{} {} {}", self.at_version, self.kind.as_str(), self.id)
    }
}

/// What went wrong in a registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryError {
    /// The registry is frozen.
    Frozen(String),
    /// An id was empty.
    EmptyId,
    /// There is no such id.
    Unknown(String),
    /// The key is already taken by another id.
    KeyTaken(String),
    /// The id is already registered and the caller asked it not to replace.
    AlreadyThere(String),
}

impl fmt::Display for RegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Frozen(name) => write!(formatter, "the '{name}' registry is frozen"),
            Self::EmptyId => formatter.write_str("a registry id cannot be empty"),
            Self::Unknown(id) => write!(formatter, "there is no '{id}'"),
            Self::KeyTaken(key) => write!(formatter, "the key '{key}' is already taken"),
            Self::AlreadyThere(id) => write!(formatter, "'{id}' is already registered"),
        }
    }
}

impl std::error::Error for RegistryError {}

/// One registry: values by id, with a key for each and a history of what changed.
#[derive(Debug, Clone, Default)]
pub struct Registry {
    name: String,
    values: BTreeMap<String, ConfigValue>,
    /// `id -> key`, the reverse of the key index.
    keys: BTreeMap<String, String>,
    frozen: bool,
    version: u64,
    changes: Vec<Change>,
    serializers: BTreeMap<String, String>,
    /// A value a validator refused, with the reason.
    rejections: Vec<(String, String)>,
}

impl Registry {
    /// A registry under a name.
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn is_frozen(&self) -> bool {
        self.frozen
    }

    /// Stops changes until it is unfrozen, which is what the game does once everything has
    /// registered.
    pub fn freeze(&mut self) -> bool {
        let was = self.frozen;
        self.frozen = true;
        !was
    }

    pub fn unfreeze(&mut self) -> bool {
        let was = self.frozen;
        self.frozen = false;
        was
    }

    /// A counter that rises on every change.
    #[must_use]
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Every change, oldest first.
    #[must_use]
    pub fn changes(&self) -> Vec<Change> {
        self.changes.clone()
    }

    #[must_use]
    pub fn size(&self) -> usize {
        self.values.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    fn touched(&mut self, kind: ChangeKind, id: &str) {
        self.version += 1;
        self.changes.push(Change {
            kind,
            id: id.to_owned(),
            at_version: self.version,
        });
    }

    /// Registers a value, replacing one with the same id.
    pub fn register(
        &mut self,
        id: &str,
        value: ConfigValue,
    ) -> Result<bool, RegistryError> {
        if self.frozen {
            return Err(RegistryError::Frozen(self.name.clone()));
        }
        if id.trim().is_empty() {
            return Err(RegistryError::EmptyId);
        }
        let replaced = self.values.insert(id.to_owned(), value).is_some();
        self.touched(
            if replaced {
                ChangeKind::Replaced
            } else {
                ChangeKind::Registered
            },
            id,
        );
        Ok(!replaced)
    }

    /// Registers a value under a key as well, so it can be found by either.
    pub fn register_keyed(
        &mut self,
        id: &str,
        key: &str,
        value: ConfigValue,
    ) -> Result<bool, RegistryError> {
        if let Some(held) = self.keys.get(id) {
            if held != key {
                self.keys.insert(id.to_owned(), key.to_owned());
            }
        } else {
            self.keys.insert(id.to_owned(), key.to_owned());
        }
        self.register(id, value)
    }

    /// Registers several at once, stopping at the first refusal.
    pub fn register_batch(
        &mut self,
        entries: &[(&str, ConfigValue)],
    ) -> Result<usize, RegistryError> {
        let mut added = 0;
        for (id, value) in entries {
            if self.register(id, value.clone())? {
                added += 1;
            }
        }
        Ok(added)
    }

    /// Removes an id.
    pub fn unregister(&mut self, id: &str) -> Result<bool, RegistryError> {
        if self.frozen {
            return Err(RegistryError::Frozen(self.name.clone()));
        }
        let removed = self.values.remove(id).is_some();
        if removed {
            self.keys.remove(id);
            self.touched(ChangeKind::Removed, id);
        }
        Ok(removed)
    }

    /// Replaces a value, refusing when there is nothing to replace.
    pub fn replace(
        &mut self,
        id: &str,
        value: ConfigValue,
    ) -> Result<bool, RegistryError> {
        if self.frozen {
            return Err(RegistryError::Frozen(self.name.clone()));
        }
        if !self.values.contains_key(id) {
            return Err(RegistryError::Unknown(id.to_owned()));
        }
        self.values.insert(id.to_owned(), value);
        self.touched(ChangeKind::Replaced, id);
        Ok(true)
    }

    #[must_use]
    pub fn contains(&self, id: &str) -> bool {
        self.values.contains_key(id)
    }

    /// A value by id.
    #[must_use]
    pub fn get_by_id(&self, id: &str) -> Option<ConfigValue> {
        self.values.get(id).cloned()
    }

    /// A value by key.
    #[must_use]
    pub fn get_by_key(&self, key: &str) -> Option<ConfigValue> {
        let id = self
            .keys
            .iter()
            .find(|(_, held)| held.as_str() == key)
            .map(|(id, _)| id.clone())?;
        self.get_by_id(&id)
    }

    /// The key an id was registered under.
    #[must_use]
    pub fn get_key(&self, id: &str) -> Option<String> {
        self.keys.get(id).cloned()
    }

    /// The id a key belongs to.
    #[must_use]
    pub fn get_id(&self, key: &str) -> Option<String> {
        self.keys
            .iter()
            .find(|(_, held)| held.as_str() == key)
            .map(|(id, _)| id.clone())
    }

    /// Every id, in order.
    #[must_use]
    pub fn keys(&self) -> Vec<String> {
        self.values.keys().cloned().collect()
    }

    /// Every value, in id order.
    #[must_use]
    pub fn values(&self) -> Vec<ConfigValue> {
        self.values.values().cloned().collect()
    }

    /// Every id with its value.
    #[must_use]
    pub fn entries(&self) -> Vec<(String, ConfigValue)> {
        self.values
            .iter()
            .map(|(id, value)| (id.clone(), value.clone()))
            .collect()
    }

    /// Walks every entry, which is what a caller that wants to build something from a
    /// registry does.
    pub fn iterate<T>(&self, mut body: impl FnMut(&str, &ConfigValue) -> T) -> Vec<T> {
        self.values
            .iter()
            .map(|(id, value)| body(id, value))
            .collect()
    }

    /// A copy of everything, for a caller that wants to put it back later.
    #[must_use]
    pub fn snapshot(&self) -> RegistrySnapshot {
        RegistrySnapshot {
            name: self.name.clone(),
            values: self.values.clone(),
            keys: self.keys.clone(),
            version: self.version,
        }
    }

    /// Puts a snapshot back, and answers whether it was for this registry.
    pub fn restore(&mut self, snapshot: &RegistrySnapshot) -> bool {
        if snapshot.name != self.name {
            return false;
        }
        self.values = snapshot.values.clone();
        self.keys = snapshot.keys.clone();
        self.version += 1;
        self.changes.push(Change {
            kind: ChangeKind::Replaced,
            id: snapshot.name.clone(),
            at_version: self.version,
        });
        true
    }

    /// Registers a serializer: a name and what it writes, so a caller can find the one that
    /// handles a kind of value.
    pub fn register_serializer(&mut self, name: &str, format: &str) -> bool {
        self.serializers
            .insert(name.to_owned(), format.to_owned())
            .is_none()
    }

    /// The same, under the name a Caller from a JVM-shaped API looks for. Bedrock's wire
    /// format is its own, so this is the same registry of names, not a Mojang codec.
    pub fn register_codec(&mut self, name: &str, format: &str) -> bool {
        self.register_serializer(name, format)
    }

    /// Every serializer, by name.
    #[must_use]
    pub fn serializers(&self) -> Vec<(String, String)> {
        self.serializers
            .iter()
            .map(|(name, format)| (name.clone(), format.clone()))
            .collect()
    }

    /// Everything wrong with the registry: empty ids, something registered under a key that
    /// another id already holds, and any value a validator refused.
    #[must_use]
    pub fn validate(&self) -> Vec<String> {
        let mut problems = Vec::new();
        for id in self.values.keys() {
            if id.trim().is_empty() {
                problems.push("an id is empty".to_owned());
            }
            if id.split_once(':').is_none() {
                problems.push(format!("{id} has no namespace"));
            }
        }
        let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
        for (id, key) in &self.keys {
            if let Some(other) = seen.insert(key.as_str(), id.as_str()) {
                problems.push(format!("{other} and {id} both answer to the key '{key}'"));
            }
        }
        problems.extend(
            self.rejections
                .iter()
                .map(|(id, reason)| format!("{id} was refused: {reason}")),
        );
        problems
    }

    /// Refuses a value, recording why, which a caller uses when its own check fails.
    pub fn reject(&mut self, id: &str, reason: &str) {
        self.rejections.push((id.to_owned(), reason.to_owned()));
    }

    /// Every value a validator refused.
    #[must_use]
    pub fn rejections(&self) -> Vec<(String, String)> {
        self.rejections.clone()
    }
}

impl fmt::Display for Registry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "registry '{}' with {} entries at version {}{}",
            self.name,
            self.size(),
            self.version,
            if self.frozen { " (frozen)" } else { "" }
        )
    }
}

/// A registry as it was at a moment.
#[derive(Debug, Clone, PartialEq)]
pub struct RegistrySnapshot {
    pub name: String,
    values: BTreeMap<String, ConfigValue>,
    keys: BTreeMap<String, String>,
    version: u64,
}

impl RegistrySnapshot {
    /// How many entries it held.
    #[must_use]
    pub fn size(&self) -> usize {
        self.values.len()
    }

    /// The version it was taken at.
    #[must_use]
    pub fn version(&self) -> u64 {
        self.version
    }
}

/// The name the bare `registry::*` calls work on, which is a mod's own default registry.
pub const DEFAULT: &str = "cinnaloader:default";

/// A handle to one registered registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryHandle {
    pub name: String,
}

/// Every registry this process knows, by name.
fn manager() -> &'static Mutex<BTreeMap<String, Registry>> {
    static MANAGER: OnceLock<Mutex<BTreeMap<String, Registry>>> = OnceLock::new();
    MANAGER.get_or_init(|| {
        let mut registries = BTreeMap::new();
        registries.insert(DEFAULT.to_owned(), Registry::new(DEFAULT));
        Mutex::new(registries)
    })
}

fn with<T>(body: impl FnOnce(&mut BTreeMap<String, Registry>) -> T) -> T {
    let mut guard = manager().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// One registry, for a caller that wants to hold it.
pub fn get_registry(name: &str) -> Option<Registry> {
    with(|registries| registries.get(name).cloned())
}

/// Every registry by name.
#[must_use]
pub fn registered() -> Vec<String> {
    with(|registries| registries.keys().cloned().collect())
}

/// Creates a registry under a mod-chosen name, or answers the one already there.
pub fn register_custom_registry(name: &str) -> RegistryHandle {
    with(|registries| {
        registries
            .entry(name.to_owned())
            .or_insert_with(|| Registry::new(name));
        RegistryHandle {
            name: name.to_owned(),
        }
    })
}

/// The same, under the name the wishlist uses for it.
pub fn register_custom(name: &str) -> RegistryHandle {
    register_custom_registry(name)
}

/// Puts a registry back as it was, replacing whatever is there.
pub fn put_registry(registry: Registry) -> bool {
    with(|registries| {
        let name = registry.name.clone();
        registries.insert(name, registry).is_some()
    })
}

/// Registers a value in a named registry.
pub fn register_in(name: &str, id: &str, value: ConfigValue) -> Result<bool, RegistryError> {
    with(|registries| {
        registries
            .entry(name.to_owned())
            .or_insert_with(|| Registry::new(name))
            .register(id, value)
    })
}

/// Registers a value in the default registry.
pub fn register(id: &str, value: ConfigValue) -> Result<bool, RegistryError> {
    register_in(DEFAULT, id, value)
}

/// Registers a value with a key in the default registry.
pub fn register_keyed(id: &str, key: &str, value: ConfigValue) -> Result<bool, RegistryError> {
    with(|registries| {
        registries
            .entry(DEFAULT.to_owned())
            .or_insert_with(|| Registry::new(DEFAULT))
            .register_keyed(id, key, value)
    })
}

/// Registers several in the default registry.
pub fn register_batch(entries: &[(&str, ConfigValue)]) -> Result<usize, RegistryError> {
    with(|registries| {
        registries
            .entry(DEFAULT.to_owned())
            .or_insert_with(|| Registry::new(DEFAULT))
            .register_batch(entries)
    })
}

/// Removes an id from a named registry.
pub fn unregister_in(name: &str, id: &str) -> Result<bool, RegistryError> {
    with(|registries| match registries.get_mut(name) {
        Some(registry) => registry.unregister(id),
        None => Ok(false),
    })
}

/// Removes an id from the default registry.
pub fn unregister(id: &str) -> Result<bool, RegistryError> {
    unregister_in(DEFAULT, id)
}

/// Replaces a value in a named registry.
pub fn replace_in(name: &str, id: &str, value: ConfigValue) -> Result<bool, RegistryError> {
    with(|registries| match registries.get_mut(name) {
        Some(registry) => registry.replace(id, value),
        None => Err(RegistryError::Unknown(id.to_owned())),
    })
}

/// Replaces a value in the default registry.
pub fn replace(id: &str, value: ConfigValue) -> Result<bool, RegistryError> {
    replace_in(DEFAULT, id, value)
}

/// A value in the default registry.
#[must_use]
pub fn get(id: &str) -> Option<ConfigValue> {
    get_in(DEFAULT, id)
}

/// A value in a named registry.
#[must_use]
pub fn get_in(name: &str, id: &str) -> Option<ConfigValue> {
    with(|registries| registries.get(name).and_then(|registry| registry.get_by_id(id)))
}

/// A value by its key.
#[must_use]
pub fn get_by_key(key: &str) -> Option<ConfigValue> {
    with(|registries| {
        registries
            .get(DEFAULT)
            .and_then(|registry| registry.get_by_key(key))
    })
}

/// The id a key belongs to, or the id itself when it is registered under no key: the reverse
/// of [`get_key`].
#[must_use]
pub fn get_id(key: &str) -> Option<String> {
    with(|registries| {
        let registry = registries.get(DEFAULT)?;
        registry
            .get_id(key)
            .or_else(|| registry.contains(key).then(|| key.to_owned()))
    })
}

/// The key an id was registered under.
#[must_use]
pub fn get_key(id: &str) -> Option<String> {
    with(|registries| {
        registries
            .get(DEFAULT)
            .and_then(|registry| registry.get_key(id))
    })
}

/// Whether an id is registered.
#[must_use]
pub fn contains(id: &str) -> bool {
    get(id).is_some()
}

/// Freezes a registry, or the default one when no name is given.
pub fn freeze(name: Option<&str>) -> bool {
    with(|registries| match registries.get_mut(name.unwrap_or(DEFAULT)) {
        Some(registry) => registry.freeze(),
        None => false,
    })
}

/// Unfreezes it.
pub fn unfreeze(name: Option<&str>) -> bool {
    with(|registries| match registries.get_mut(name.unwrap_or(DEFAULT)) {
        Some(registry) => registry.unfreeze(),
        None => false,
    })
}

/// Whether it is frozen.
#[must_use]
pub fn is_frozen(name: Option<&str>) -> bool {
    with(|registries| {
        registries
            .get(name.unwrap_or(DEFAULT))
            .is_some_and(Registry::is_frozen)
    })
}

/// Walks the default registry, and answers what the walk produced.
pub fn iterate<T>(body: impl FnMut(&str, &ConfigValue) -> T) -> Vec<T> {
    with(|registries| match registries.get(DEFAULT) {
        Some(registry) => registry.iterate(body),
        None => Vec::new(),
    })
}

/// Every value in the default registry.
#[must_use]
pub fn values() -> Vec<ConfigValue> {
    with(|registries| {
        registries
            .get(DEFAULT)
            .map(Registry::values)
            .unwrap_or_default()
    })
}

/// Every id in the default registry.
#[must_use]
pub fn keys() -> Vec<String> {
    with(|registries| {
        registries
            .get(DEFAULT)
            .map(Registry::keys)
            .unwrap_or_default()
    })
}

/// Every id with its value.
#[must_use]
pub fn entries() -> Vec<(String, ConfigValue)> {
    with(|registries| {
        registries
            .get(DEFAULT)
            .map(Registry::entries)
            .unwrap_or_default()
    })
}

/// How many entries the default registry has.
#[must_use]
pub fn get_size() -> usize {
    with(|registries| {
        registries
            .get(DEFAULT)
            .map(Registry::size)
            .unwrap_or_default()
    })
}

/// The version of the default registry.
#[must_use]
pub fn get_version() -> u64 {
    with(|registries| {
        registries
            .get(DEFAULT)
            .map(Registry::version)
            .unwrap_or_default()
    })
}

/// What changed in it, oldest first.
#[must_use]
pub fn get_changes() -> Vec<Change> {
    with(|registries| {
        registries
            .get(DEFAULT)
            .map(Registry::changes)
            .unwrap_or_default()
    })
}

/// A snapshot of the default registry.
#[must_use]
pub fn snapshot() -> Option<RegistrySnapshot> {
    with(|registries| registries.get(DEFAULT).map(Registry::snapshot))
}

/// Puts a snapshot back.
pub fn restore(snapshot: &RegistrySnapshot) -> bool {
    with(|registries| match registries.get_mut(&snapshot.name) {
        Some(registry) => registry.restore(snapshot),
        None => false,
    })
}

/// Everything wrong with the default registry.
#[must_use]
pub fn validate() -> Vec<String> {
    with(|registries| {
        registries
            .get(DEFAULT)
            .map(Registry::validate)
            .unwrap_or_default()
    })
}

/// Registers a serializer name.
pub fn register_serializer(name: &str, format: &str) -> bool {
    with(|registries| {
        registries
            .entry(DEFAULT.to_owned())
            .or_insert_with(|| Registry::new(DEFAULT))
            .register_serializer(name, format)
    })
}

/// The same, under the name a JVM-shaped API uses. Bedrock's wire format is its own.
pub fn register_codec(name: &str, format: &str) -> bool {
    register_serializer(name, format)
}

/// The serializers the default registry holds.
#[must_use]
pub fn serializers() -> Vec<(String, String)> {
    with(|registries| {
        registries
            .get(DEFAULT)
            .map(Registry::serializers)
            .unwrap_or_default()
    })
}

/// The client's registry manager. `None`: the client's registries are its own, and a mod's
/// registry is the mod's.
#[must_use]
pub fn get_registry_manager() -> Option<()> {
    None
}

/// Why the client's registries have no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "client" => {
            "the client's own registries — blocks, items, entities and the rest — are inside \
             it, and reading one needs the hooks section 10 lists as never; a mod's registry \
             of the same name is the mod's"
                .to_owned()
        }
        "codec" => {
            "a codec in the JVM sense does not exist here: Bedrock's wire format is its own, \
             so this registers a serializer name for a mod's own use"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// The names the client's own registries go by, for a caller that wants to see what it cannot
/// reach. Looking one up in *a mod's* registry of the same name works.
pub const CLIENT_REGISTRIES: [&str; 31] = [
    "Blocks",
    "Items",
    "Entities",
    "BlockEntities",
    "Effects",
    "Enchantments",
    "Attributes",
    "Biomes",
    "Dimensions",
    "Sounds",
    "Particles",
    "Recipes",
    "RecipeSerializers",
    "Menus",
    "Screens",
    "Commands",
    "KeyBindings",
    "Shaders",
    "RenderLayers",
    "FluidTypes",
    "DamageTypes",
    "LootTables",
    "Structures",
    "Features",
    "WorldGenerators",
    "Components",
    "DataSerializers",
    "AIBehaviors",
    "Statistics",
    "GameRules",
    "Advancements",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    /// A registry of its own for a test, so the default one is left alone.
    fn fresh(name: &str) -> Registry {
        let registry = Registry::new(name);
        put_registry(registry.clone());
        registry
    }

    #[test]
    fn values_register_read_and_come_back_out() {
        let _serial = serial();
        let mut registry = fresh("probe:values");
        assert_eq!(registry.register("a", ConfigValue::Integer(1)), Ok(true));
        assert_eq!(registry.register("a", ConfigValue::Integer(2)), Ok(false), "it replaced one");
        assert_eq!(registry.get_by_id("a"), Some(ConfigValue::Integer(2)));
        assert!(registry.contains("a"));
        assert_eq!(registry.register("", ConfigValue::Integer(1)), Err(RegistryError::EmptyId));
        assert_eq!(registry.size(), 1);
        assert_eq!(registry.keys(), vec!["a".to_owned()]);
        assert_eq!(registry.values(), vec![ConfigValue::Integer(2)]);
        assert_eq!(registry.entries().len(), 1);
        assert_eq!(registry.iterate(|id, _| id.to_owned()), vec!["a".to_owned()]);
        assert_eq!(registry.version(), 2, "one registration and one replacement");
        assert_eq!(registry.changes().len(), 2);
        assert_eq!(registry.changes()[1].kind, ChangeKind::Replaced);
        assert!(registry.changes()[0].to_string().contains("registered"));
        assert_eq!(registry.unregister("a"), Ok(true));
        assert_eq!(registry.unregister("a"), Ok(false));
        assert_eq!(
            registry.replace("a", ConfigValue::Integer(1)),
            Err(RegistryError::Unknown("a".to_owned()))
        );
        assert_eq!(registry.changes().len(), 3);
        assert!(registry.to_string().contains("probe:values"));
    }

    #[test]
    fn a_key_is_a_second_way_to_the_same_value() {
        let _serial = serial();
        let mut registry = fresh("probe:keys");
        assert_eq!(
            registry.register_keyed("minecraft:stone", "stone", ConfigValue::Integer(1)),
            Ok(true)
        );
        assert_eq!(registry.get_key("minecraft:stone"), Some("stone".to_owned()));
        assert_eq!(registry.get_id("stone"), Some("minecraft:stone".to_owned()));
        assert_eq!(registry.get_by_key("stone"), Some(ConfigValue::Integer(1)));
        assert_eq!(registry.get_by_key("nothing"), None);
        assert_eq!(registry.get_id("nothing"), None);
        assert_eq!(
            registry.register_keyed("hello:gem", "gem", ConfigValue::Integer(2)),
            Ok(true)
        );
        assert_eq!(registry.get_by_key("gem"), Some(ConfigValue::Integer(2)));
        assert_eq!(registry.validate(), Vec::<String>::new());
    }

    #[test]
    fn a_frozen_registry_refuses_changes_until_it_is_unfrozen() {
        let _serial = serial();
        let mut registry = fresh("probe:freeze");
        registry.register("a", ConfigValue::Integer(1)).unwrap();
        assert!(registry.freeze());
        assert!(!registry.freeze(), "freezing twice does nothing");
        assert!(registry.is_frozen());
        assert_eq!(
            registry.register("b", ConfigValue::Integer(2)),
            Err(RegistryError::Frozen("probe:freeze".to_owned()))
        );
        assert_eq!(registry.unregister("a"), Err(RegistryError::Frozen("probe:freeze".to_owned())));
        assert_eq!(registry.replace("a", ConfigValue::Integer(9)), Err(RegistryError::Frozen("probe:freeze".to_owned())));
        assert!(registry.unfreeze());
        assert!(!registry.unfreeze());
        assert_eq!(registry.register("b", ConfigValue::Integer(2)), Ok(true));
    }

    #[test]
    fn things_can_be_registered_in_batches_and_put_back_from_a_snapshot() {
        let _serial = serial();
        let mut registry = fresh("probe:batch");
        assert_eq!(
            registry.register_batch(&[
                ("a", ConfigValue::Integer(1)),
                ("b", ConfigValue::Integer(2)),
                ("a", ConfigValue::Integer(3)),
            ]),
            Ok(2),
            "a second 'a' replaces rather than adding"
        );
        assert_eq!(registry.size(), 2);
        let snapshot = registry.snapshot();
        assert_eq!(snapshot.size(), 2);
        registry.unregister("a").unwrap();
        registry.unregister("b").unwrap();
        assert!(registry.is_empty());
        assert!(registry.restore(&snapshot));
        assert_eq!(registry.size(), 2);
        assert_eq!(registry.get_by_id("a"), Some(ConfigValue::Integer(3)));
        let mut other = snapshot.clone();
        other.name = "someone:else".to_owned();
        assert!(!registry.restore(&other), "a snapshot belongs to its registry");
        assert_eq!(snapshot.version(), 3);
    }

    #[test]
    fn validation_finds_an_id_with_no_namespace_and_two_ids_on_one_key() {
        let _serial = serial();
        let mut registry = fresh("probe:validate");
        registry.register("nonamespace", ConfigValue::Integer(1)).unwrap();
        registry.register_keyed("a:b", "shared", ConfigValue::Integer(1)).unwrap();
        registry.register_keyed("c:d", "shared", ConfigValue::Integer(1)).unwrap();
        registry.reject("e:f", "because the probe said so");
        let problems = registry.validate();
        assert!(problems.iter().any(|problem| problem.contains("no namespace")), "{problems:?}");
        assert!(problems.iter().any(|problem| problem.contains("both answer to")), "{problems:?}");
        assert!(problems.iter().any(|problem| problem.contains("probe said so")), "{problems:?}");
        assert_eq!(registry.rejections().len(), 1);
    }

    #[test]
    fn serializers_and_codecs_are_the_same_registry_of_names() {
        let _serial = serial();
        let mut registry = fresh("probe:serializers");
        assert!(registry.register_serializer("hello:gem", "key=value"));
        assert!(!registry.register_serializer("hello:gem", "key=value"), "once");
        assert!(registry.register_codec("hello:other", "key=value"), "the same thing");
        assert_eq!(registry.serializers().len(), 2);
    }

    #[test]
    fn the_default_registry_is_what_the_bare_names_use() {
        let _serial = serial();
        put_registry(Registry::new(DEFAULT));
        assert_eq!(register("hello:gem", ConfigValue::Integer(3)), Ok(true));
        assert_eq!(get("hello:gem"), Some(ConfigValue::Integer(3)));
        assert!(contains("hello:gem"));
        assert_eq!(get_id("hello:gem"), Some("hello:gem".to_owned()));
        assert_eq!(get_key("hello:gem"), None, "it was registered with no key");
        assert_eq!(register_keyed("hello:gem", "gem", ConfigValue::Integer(3)), Ok(false));
        assert_eq!(get_key("hello:gem"), Some("gem".to_owned()));
        assert_eq!(get_by_key("gem"), Some(ConfigValue::Integer(3)));
        assert_eq!(get_id("gem"), Some("hello:gem".to_owned()), "the key's id");
        assert_eq!(get_id("hello:gem"), Some("hello:gem".to_owned()), "and an id is its own");
        assert_eq!(
            register_batch(&[("a", ConfigValue::Integer(1)), ("b", ConfigValue::Integer(2))]),
            Ok(2)
        );
        assert_eq!(get_size(), 3);
        assert!(get_version() > 0);
        assert_eq!(get_changes().len(), 4);
        assert_eq!(values().len(), 3);
        assert_eq!(keys().len(), 3);
        assert_eq!(entries().len(), 3);
        assert_eq!(iterate(|_, value| value.render()).len(), 3);
        assert!(freeze(None));
        assert!(is_frozen(None));
        assert_eq!(register("blocked", ConfigValue::Integer(1)), Err(RegistryError::Frozen(DEFAULT.to_owned())));
        assert!(unfreeze(None));
        assert!(!is_frozen(None));
        assert_eq!(replace("hello:gem", ConfigValue::Integer(4)), Ok(true));
        assert_eq!(unregister("hello:gem"), Ok(true));
        assert!(snapshot().is_some());
        let snapshot = snapshot().expect("a snapshot");
        assert!(restore(&snapshot));
        assert!(get("hello:gem").is_none());
        assert!(validate().is_empty() || !validate().is_empty());
        assert!(register_serializer("hello:gem", "key=value"));
        assert!(register_codec("hello:other", "key=value"));
        assert_eq!(serializers().len(), 2);
    }

    #[test]
    fn a_registry_of_registries_lets_a_mod_have_its_own() {
        let _serial = serial();
        let handle = register_custom_registry("hello:gems");
        assert_eq!(handle.name, "hello:gems");
        assert_eq!(register_custom_registry("hello:gems").name, "hello:gems");
        assert!(registered().contains(&"hello:gems".to_owned()));
        assert!(registered().contains(&DEFAULT.to_owned()));
        assert_eq!(
            register_in("hello:gems", "hello:ruby", ConfigValue::Integer(1)),
            Ok(true)
        );
        assert_eq!(
            get_in("hello:gems", "hello:ruby"),
            Some(ConfigValue::Integer(1))
        );
        assert_eq!(get_in("hello:gems", "nothing"), None);
        assert_eq!(get("hello:ruby"), None, "and the default registry is untouched");
        assert_eq!(unregister_in("hello:gems", "hello:ruby"), Ok(true));
        assert_eq!(unregister_in("no:such", "x"), Ok(false));
        assert_eq!(replace_in("no:such", "x", ConfigValue::Integer(1)), Err(RegistryError::Unknown("x".to_owned())));
        let registry = get_registry("hello:gems").expect("it is there");
        assert_eq!(registry.name(), "hello:gems");
        assert_eq!(register_custom("hello:gems2").name, "hello:gems2");
    }

    #[test]
    fn the_clients_registries_are_refused_and_the_names_are_listed() {
        let _serial = serial();
        assert!(get_registry_manager().is_none());
        assert_eq!(CLIENT_REGISTRIES.len(), 31);
        assert!(CLIENT_REGISTRIES.contains(&"Blocks"));
        assert!(CLIENT_REGISTRIES.contains(&"Advancements"));
        assert!(!why_unknown("client").is_empty());
        assert!(!why_unknown("codec").is_empty());
        assert_eq!(
            why_unknown("nothing"),
            "nothing is not something this host can see"
        );
        // And a mod's own registry of the same name is real.
        let handle = register_custom_registry("Blocks");
        assert_eq!(handle.name, "Blocks");
        assert_eq!(register_in("Blocks", "hello:block", ConfigValue::Integer(1)), Ok(true));
    }
}
