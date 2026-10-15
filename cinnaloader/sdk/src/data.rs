//! Data components a mod declares, and the values it stores against an owner.
//!
//! A **component type** is a value a mod owns: an id, a name, one of six value kinds
//! (integer, float, string, bool, list, map) and a default. A **component map** holds values
//! of those types against an owner — an item id, an entity id, or any string a mod chooses —
//! and reads them back *typed*, refusing a value of the wrong kind with the component and
//! both kinds named. A **patch** is a set of changes that merges into a map, and the map
//! itself serialises through [`crate::config::ConfigValue`] so it round trips exactly, empty
//! or nested.
//!
//! Equality is on the owner and the *present* entries: a map holding a component at its
//! default is not equal to the empty map, so a caller can tell "absent" from "present and
//! default" with `has`, `get_component` or [`ComponentMap::fingerprint`]. The client's own
//! item and entity components, a live stack's components, and writing one the client would
//! read answer `None`/`false` with the reason in [`why_unknown`].
//!
//! ```
//! use cinnaloader::config::ConfigValue;
//! use cinnaloader::data::{self, ComponentMap, ComponentType, ValueKind};
//!
//! data::register_component(ComponentType::new(
//!     "hello:durability",
//!     "Durability",
//!     ValueKind::Integer,
//!     ConfigValue::Integer(100),
//! ));
//! let mut map = ComponentMap::new("minecraft:iron_pickaxe");
//! data::set_component(&mut map, "hello:durability", ConfigValue::Integer(42)).unwrap();
//! assert_eq!(data::get_component(&map, "hello:durability").unwrap(), ConfigValue::Integer(42));
//! data::unregister_component("hello:durability");
//! ```

use std::cmp::Ordering;
use std::collections::hash_map::DefaultHasher;
use std::collections::BTreeMap;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Mutex, OnceLock};

use crate::config::{ConfigType, ConfigValue};
use crate::modinfo::ModVersion;

/// The kind of value a component holds, the six the wishlist names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ValueKind {
    Integer,
    Float,
    String,
    Bool,
    List,
    Map,
}

impl ValueKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Integer => "integer",
            Self::Float => "float",
            Self::String => "string",
            Self::Bool => "bool",
            Self::List => "list",
            Self::Map => "map",
        }
    }

    /// The kind a value is, when it is one of the six.
    #[must_use]
    pub fn of(value: &ConfigValue) -> Option<Self> {
        Some(match value {
            ConfigValue::Boolean(_) => Self::Bool,
            ConfigValue::Integer(_) => Self::Integer,
            ConfigValue::Float(_) => Self::Float,
            ConfigValue::String(_) | ConfigValue::Text(_) => Self::String,
            ConfigValue::List(_) | ConfigValue::Set(_) => Self::List,
            ConfigValue::Map(_) | ConfigValue::Object(_) | ConfigValue::Compound(_) => Self::Map,
            _ => return None,
        })
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "integer" => Some(Self::Integer),
            "float" => Some(Self::Float),
            "string" => Some(Self::String),
            "bool" => Some(Self::Bool),
            "list" => Some(Self::List),
            "map" => Some(Self::Map),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_config_type(self) -> ConfigType {
        match self {
            Self::Integer => ConfigType::Integer,
            Self::Float => ConfigType::Float,
            Self::String => ConfigType::String,
            Self::Bool => ConfigType::Boolean,
            Self::List => ConfigType::List,
            Self::Map => ConfigType::Map,
        }
    }

    /// The value a missing component of this kind reads as.
    #[must_use]
    pub fn default_value(self) -> ConfigValue {
        match self {
            Self::Integer => ConfigValue::Integer(0),
            Self::Float => ConfigValue::Float(0.0),
            Self::String => ConfigValue::String(String::new()),
            Self::Bool => ConfigValue::Boolean(false),
            Self::List => ConfigValue::List(Vec::new()),
            Self::Map => ConfigValue::Map(BTreeMap::new()),
        }
    }

    #[must_use]
    pub fn accepts(self, value: &ConfigValue) -> bool {
        Self::of(value) == Some(self)
    }
}

impl fmt::Display for ValueKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The name of a value's kind, for a message: the six when it is one of them, otherwise the
/// config kind it actually is.
#[must_use]
fn kind_name(value: &ConfigValue) -> &'static str {
    match ValueKind::of(value) {
        Some(kind) => kind.as_str(),
        None => value.kind().as_str(),
    }
}

/// What can go wrong reading or writing a component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataError {
    /// No component type with that id is registered.
    UnknownComponent { component: String },
    /// The component is registered but the map holds nothing for it.
    Absent { component: String },
    /// The value's kind is not the component's declared kind.
    TypeMismatch {
        component: String,
        wanted: &'static str,
        found: &'static str,
    },
    /// A serialised map was not a map.
    NotAMap { found: &'static str },
}

impl fmt::Display for DataError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownComponent { component } => {
                write!(formatter, "{component} is not a registered data component")
            }
            Self::Absent { component } => write!(formatter, "{component} is not set here"),
            Self::TypeMismatch {
                component,
                wanted,
                found,
            } => write!(
                formatter,
                "{component} holds {found}, not {wanted}"
            ),
            Self::NotAMap { found } => write!(formatter, "a component map was {found}, not a map"),
        }
    }
}

impl std::error::Error for DataError {}

/// A component type a mod declares: what it is called, what it holds, and what a missing one
/// reads as.
#[derive(Debug, Clone, PartialEq)]
pub struct ComponentType {
    pub id: String,
    pub name: String,
    pub owner: String,
    pub kind: ValueKind,
    pub default: ConfigValue,
    pub description: String,
}

impl ComponentType {
    /// A component type. A default of the wrong kind is replaced with the kind's own default,
    /// because the default is what a typed read answers when the component is absent.
    #[must_use]
    pub fn new(id: &str, name: &str, kind: ValueKind, default: ConfigValue) -> Self {
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            owner: crate::metadata::id(),
            kind,
            default: if kind.accepts(&default) {
                default
            } else {
                kind.default_value()
            },
            description: String::new(),
        }
    }

    #[must_use]
    pub fn described(mut self, description: &str) -> Self {
        self.description = description.to_owned();
        self
    }

    #[must_use]
    pub fn accepts(&self, value: &ConfigValue) -> bool {
        self.kind.accepts(value)
    }
}

impl fmt::Display for ComponentType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} ({}, {})", self.name, self.id, self.kind)
    }
}

/// A value type a mod names, beside the component types that use one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataType {
    pub name: String,
    pub kind: ValueKind,
    pub description: String,
}

impl DataType {
    #[must_use]
    pub fn new(name: &str, kind: ValueKind) -> Self {
        Self {
            name: name.to_owned(),
            kind,
            description: String::new(),
        }
    }

    #[must_use]
    pub fn described(mut self, description: &str) -> Self {
        self.description = description.to_owned();
        self
    }
}

/// A codec: how a value kind is written as text and read back. The six built-in ones render
/// and parse the way [`crate::config::ConfigValue`] does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Codec {
    pub name: String,
    pub kind: ValueKind,
}

impl Codec {
    #[must_use]
    pub fn new(name: &str, kind: ValueKind) -> Self {
        Self {
            name: name.to_owned(),
            kind,
        }
    }

    /// The built-in codec for a kind, by the kind's name.
    #[must_use]
    pub fn builtin(name: &str) -> Option<Self> {
        ValueKind::from_name(name).map(|kind| Self::new(name, kind))
    }

    #[must_use]
    pub fn encode(&self, value: &ConfigValue) -> Option<String> {
        self.kind.accepts(value).then(|| value.render())
    }

    #[must_use]
    pub fn decode(&self, text: &str) -> Option<ConfigValue> {
        ConfigValue::parse(text, self.kind.as_config_type())
    }
}

/// A component map: the values an owner actually carries, keyed by component id. Equality is
/// on the owner and the present entries, so an owner carrying a component at its default is
/// not equal to one that does not carry it.
#[derive(Debug, Clone, PartialEq)]
pub struct ComponentMap {
    owner: String,
    values: BTreeMap<String, ConfigValue>,
}

impl ComponentMap {
    #[must_use]
    pub fn new(owner: &str) -> Self {
        Self {
            owner: owner.to_owned(),
            values: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn owner(&self) -> &str {
        &self.owner
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Sets a component, refusing one whose kind is not the registered component's.
    pub fn set(&mut self, id: &str, value: ConfigValue) -> Result<(), DataError> {
        let component = get_component_schema(id)
            .ok_or_else(|| DataError::UnknownComponent { component: id.to_owned() })?;
        if !component.accepts(&value) {
            return Err(DataError::TypeMismatch {
                component: id.to_owned(),
                wanted: component.kind.as_str(),
                found: kind_name(&value),
            });
        }
        self.values.insert(id.to_owned(), value);
        Ok(())
    }

    /// Puts a value in without checking its kind, which is how persisted or foreign data
    /// arrives; a typed read is what catches it afterwards.
    pub fn insert_raw(&mut self, id: &str, value: ConfigValue) {
        self.values.insert(id.to_owned(), value);
    }

    #[must_use]
    pub fn get(&self, id: &str) -> Option<ConfigValue> {
        self.values.get(id).cloned()
    }

    /// The typed read: the component's value when it is present and the declared kind, and an
    /// error naming the component and both kinds when it is not.
    pub fn get_typed(&self, id: &str) -> Result<ConfigValue, DataError> {
        let component = get_component_schema(id)
            .ok_or_else(|| DataError::UnknownComponent { component: id.to_owned() })?;
        match self.values.get(id) {
            None => Err(DataError::Absent {
                component: id.to_owned(),
            }),
            Some(value) if component.accepts(value) => Ok(value.clone()),
            Some(value) => Err(DataError::TypeMismatch {
                component: id.to_owned(),
                wanted: component.kind.as_str(),
                found: kind_name(value),
            }),
        }
    }

    /// The value when present, otherwise the component's declared default. `has` is what tells
    /// the two apart.
    pub fn present_or_default(&self, id: &str) -> Result<ConfigValue, DataError> {
        let component = get_component_schema(id)
            .ok_or_else(|| DataError::UnknownComponent { component: id.to_owned() })?;
        Ok(self.values.get(id).cloned().unwrap_or(component.default))
    }

    pub fn remove(&mut self, id: &str) -> bool {
        self.values.remove(id).is_some()
    }

    #[must_use]
    pub fn has(&self, id: &str) -> bool {
        self.values.contains_key(id)
    }

    pub fn clear(&mut self) {
        self.values.clear();
    }

    #[must_use]
    pub fn keys(&self) -> Vec<String> {
        self.values.keys().cloned().collect()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &ConfigValue)> {
        self.values
            .iter()
            .map(|(id, value)| (id.as_str(), value))
    }

    /// Merges another map in, with the rule this section documents: an entry from `other`
    /// wins over one of the same id here, and ids only here are kept. Returns `true` when
    /// anything changed.
    pub fn merge(&mut self, other: &ComponentMap) -> bool {
        let before = self.values.clone();
        for (id, value) in &other.values {
            self.values.insert(id.clone(), value.clone());
        }
        self.values != before
    }

    #[must_use]
    pub fn merged(&self, other: &ComponentMap) -> ComponentMap {
        let mut merged = self.clone();
        merged.merge(other);
        merged
    }

    /// A hash over the owner and every present component's rendered value. Equal maps share a
    /// fingerprint; a map carrying a default hashes differently from the empty map.
    #[must_use]
    pub fn fingerprint(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.owner.hash(&mut hasher);
        for (id, value) in &self.values {
            id.hash(&mut hasher);
            value.render().hash(&mut hasher);
        }
        hasher.finish()
    }
}

/// A set of component changes. Applying it merges into a map: a value sets, a `None` removes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Patch {
    changes: BTreeMap<String, Option<ConfigValue>>,
}

impl Patch {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, id: &str, value: ConfigValue) -> &mut Self {
        self.changes.insert(id.to_owned(), Some(value));
        self
    }

    pub fn remove(&mut self, id: &str) -> &mut Self {
        self.changes.insert(id.to_owned(), None);
        self
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    #[must_use]
    pub fn ids(&self) -> Vec<String> {
        self.changes.keys().cloned().collect()
    }

    /// Applies the changes to a map in place.
    pub fn apply(&self, map: &mut ComponentMap) -> Result<(), DataError> {
        for (id, change) in &self.changes {
            match change {
                Some(value) => map.set(id, value.clone())?,
                None => {
                    map.remove(id);
                }
            }
        }
        Ok(())
    }

    /// Applies the changes to a copy, leaving the map it was handed untouched.
    pub fn applied(&self, map: &ComponentMap) -> Result<ComponentMap, DataError> {
        let mut copy = map.clone();
        self.apply(&mut copy)?;
        Ok(copy)
    }

    /// This patch with another merged over it: the same precedence as [`ComponentMap::merge`],
    /// so `other` wins.
    #[must_use]
    pub fn merged(&self, other: &Patch) -> Patch {
        let mut merged = self.clone();
        for (id, change) in &other.changes {
            merged.changes.insert(id.clone(), change.clone());
        }
        merged
    }
}

/// How one version of a component's value becomes the next. Compared by the versions it
/// covers, since two closures are never the same function.
#[derive(Debug, Clone)]
pub struct DataMigration {
    pub component: String,
    pub from: ModVersion,
    pub to: ModVersion,
    pub description: String,
    apply: fn(&mut ConfigValue),
}

impl PartialEq for DataMigration {
    fn eq(&self, other: &Self) -> bool {
        self.component == other.component
            && self.from == other.from
            && self.to == other.to
            && self.description == other.description
    }
}

impl DataMigration {
    #[must_use]
    pub fn new(
        component: &str,
        from: &str,
        to: &str,
        description: &str,
        apply: fn(&mut ConfigValue),
    ) -> Self {
        Self {
            component: component.to_owned(),
            from: ModVersion::parse(from),
            to: ModVersion::parse(to),
            description: description.to_owned(),
            apply,
        }
    }

    /// Whether a move from `from` to `to` crosses this migration.
    #[must_use]
    pub fn covers(&self, from: &ModVersion, to: &ModVersion) -> bool {
        self.from.cmp_parts(from) != Ordering::Less && self.to.cmp_parts(to) != Ordering::Greater
    }

    pub fn run(&self, value: &mut ConfigValue) {
        (self.apply)(value);
    }
}

type Watcher = Box<dyn FnMut(&str, &ConfigValue) + Send>;

#[derive(Default)]
struct Registries {
    components: BTreeMap<String, ComponentType>,
    data_types: BTreeMap<String, DataType>,
    codecs: BTreeMap<String, Codec>,
    migrations: Vec<DataMigration>,
    custom: BTreeMap<String, BTreeMap<String, ConfigValue>>,
    watchers: Vec<(u64, String, Watcher)>,
}

fn registries() -> &'static Mutex<Registries> {
    static REGISTRIES: OnceLock<Mutex<Registries>> = OnceLock::new();
    REGISTRIES.get_or_init(|| Mutex::new(Registries::default()))
}

fn with_registries<T>(body: impl FnOnce(&mut Registries) -> T) -> T {
    let mut guard = registries()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Declares a component type, or replaces the one with that id. Answers whether it was new.
pub fn register_component(component: ComponentType) -> bool {
    with_registries(|registries| {
        registries
            .components
            .insert(component.id.clone(), component)
            .is_none()
    })
}

/// Forgets a component type.
pub fn unregister_component(id: &str) -> bool {
    with_registries(|registries| registries.components.remove(id).is_some())
}

/// A declared component type.
#[must_use]
pub fn get_component_schema(id: &str) -> Option<ComponentType> {
    with_registries(|registries| registries.components.get(id).cloned())
}

/// Every declared component type.
#[must_use]
pub fn all_components() -> Vec<ComponentType> {
    with_registries(|registries| registries.components.values().cloned().collect())
}

/// A component's declared default.
#[must_use]
pub fn default_of(id: &str) -> Option<ConfigValue> {
    get_component_schema(id).map(|component| component.default)
}

/// The typed read from a map: see [`ComponentMap::get_typed`].
pub fn get_component(map: &ComponentMap, id: &str) -> Result<ConfigValue, DataError> {
    map.get_typed(id)
}

/// The raw read from a map: some value, whoever it was put there by.
#[must_use]
pub fn get_component_raw(map: &ComponentMap, id: &str) -> Option<ConfigValue> {
    map.get(id)
}

/// Sets a component on a map and tells the owner's watchers.
pub fn set_component(
    map: &mut ComponentMap,
    id: &str,
    value: ConfigValue,
) -> Result<(), DataError> {
    map.set(id, value.clone())?;
    notify(map.owner(), id, &value);
    Ok(())
}

/// Removes a component from a map and tells the owner's watchers.
pub fn remove_component(map: &mut ComponentMap, id: &str) -> bool {
    let removed = map.remove(id);
    if removed {
        notify(map.owner(), id, &ConfigValue::String(String::new()));
    }
    removed
}

#[must_use]
pub fn has_component(map: &ComponentMap, id: &str) -> bool {
    map.has(id)
}

/// Registers a value type a mod names.
pub fn register_data_type(data_type: DataType) -> bool {
    with_registries(|registries| {
        registries
            .data_types
            .insert(data_type.name.clone(), data_type)
            .is_none()
    })
}

/// A registered value type.
#[must_use]
pub fn get_data_type(name: &str) -> Option<DataType> {
    with_registries(|registries| registries.data_types.get(name).cloned())
}

/// Registers a codec. The six built-in ones are always reachable through [`get_codec`].
pub fn register_codec(codec: Codec) -> bool {
    with_registries(|registries| {
        registries
            .codecs
            .insert(codec.name.clone(), codec)
            .is_none()
    })
}

/// A codec by name: a registered one, or a built-in for a kind's name.
#[must_use]
pub fn get_codec(name: &str) -> Option<Codec> {
    with_registries(|registries| registries.codecs.get(name).cloned())
        .or_else(|| Codec::builtin(name))
}

/// Checks a value against a component's declared kind.
#[must_use]
pub fn validate_component(id: &str, value: &ConfigValue) -> Vec<String> {
    match get_component_schema(id) {
        Some(component) if component.accepts(value) => Vec::new(),
        Some(component) => vec![format!(
            "{id} holds {}, not {}",
            kind_name(value),
            component.kind
        )],
        None => vec![format!("{id} is not a registered data component")],
    }
}

/// Wraps a whole map as a config value: the owner under the reserved `owner` key (a component
/// id always carries a `:`), then each present component. Reading it back is exact, empty or
/// nested.
#[must_use]
pub fn serialize_component(map: &ComponentMap) -> ConfigValue {
    let mut fields = BTreeMap::new();
    fields.insert("owner".to_owned(), ConfigValue::String(map.owner.clone()));
    for (id, value) in &map.values {
        fields.insert(id.clone(), value.clone());
    }
    ConfigValue::Compound(fields)
}

/// Reads a map back from [`serialize_component`]'s value.
pub fn deserialize_component(value: &ConfigValue) -> Result<ComponentMap, DataError> {
    let fields = match value {
        ConfigValue::Map(fields)
        | ConfigValue::Object(fields)
        | ConfigValue::Compound(fields) => fields,
        other => {
            return Err(DataError::NotAMap {
                found: kind_name(other),
            })
        }
    };
    let owner = fields
        .get("owner")
        .and_then(ConfigValue::as_text)
        .unwrap_or_default();
    let mut map = ComponentMap::new(owner);
    for (id, value) in fields {
        if id != "owner" {
            map.insert_raw(id, value.clone());
        }
    }
    Ok(map)
}

/// Registers a migration that moves a component's value from one version to the next.
pub fn register_migration(migration: DataMigration) {
    with_registries(|registries| registries.migrations.push(migration));
}

/// Every registered migration.
#[must_use]
pub fn migrations() -> Vec<DataMigration> {
    with_registries(|registries| registries.migrations.clone())
}

/// Runs the registered migrations over a map, and answers how many ran.
pub fn migrate_component(map: &mut ComponentMap, from: &str, to: &str) -> usize {
    let migrations = migrations();
    let from = ModVersion::parse(from);
    let to = ModVersion::parse(to);
    let mut ran = 0;
    for migration in &migrations {
        if !migration.covers(&from, &to) {
            continue;
        }
        if let Some(value) = map.values.get_mut(&migration.component) {
            migration.run(value);
            ran += 1;
        }
    }
    ran
}

/// Reads arbitrary data a mod keeps against an owner. Answers whether the key was new.
pub fn set_custom_data(owner: &str, key: &str, value: ConfigValue) -> bool {
    with_registries(|registries| {
        registries
            .custom
            .entry(owner.to_owned())
            .or_default()
            .insert(key.to_owned(), value)
            .is_none()
    })
}

#[must_use]
pub fn get_custom_data(owner: &str, key: &str) -> Option<ConfigValue> {
    with_registries(|registries| {
        registries
            .custom
            .get(owner)
            .and_then(|data| data.get(key))
            .cloned()
    })
}

pub fn remove_custom_data(owner: &str, key: &str) -> bool {
    with_registries(|registries| {
        registries
            .custom
            .get_mut(owner)
            .is_some_and(|data| data.remove(key).is_some())
    })
}

/// The keys an owner has custom data under.
#[must_use]
pub fn custom_data_keys(owner: &str) -> Vec<String> {
    with_registries(|registries| {
        registries
            .custom
            .get(owner)
            .map(|data| data.keys().cloned().collect())
            .unwrap_or_default()
    })
}

/// Calls `watcher` whenever a component is set or removed on `owner`. Returns an id for
/// [`unwatch`].
pub fn watch(owner: &str, watcher: impl FnMut(&str, &ConfigValue) + Send + 'static) -> u64 {
    static NEXT_WATCHER: AtomicU64 = AtomicU64::new(1);
    let id = NEXT_WATCHER.fetch_add(1, AtomicOrdering::Relaxed);
    with_registries(|registries| {
        registries
            .watchers
            .push((id, owner.to_owned(), Box::new(watcher)))
    });
    id
}

/// Stops watching, and answers whether that watcher existed.
pub fn unwatch(id: u64) -> bool {
    with_registries(|registries| {
        let before = registries.watchers.len();
        registries.watchers.retain(|(held, _, _)| *held != id);
        registries.watchers.len() != before
    })
}

fn notify(owner: &str, id: &str, value: &ConfigValue) {
    with_registries(|registries| {
        for (_, watched, watcher) in &mut registries.watchers {
            if watched == owner {
                watcher(id, value);
            }
        }
    });
}

/// Why a component question about the client has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "item" => {
            "an item's components are the client's own registry data, decoded from the wire; \
             this host has no hook for them"
                .to_owned()
        }
        "entity" => {
            "an entity's components live in the client's entity store, which section 19 \
             refuses for want of a hook"
                .to_owned()
        }
        "stack" => {
            "the components of a live stack are the client's player state, which needs the \
             hooks section 10 lists as never"
                .to_owned()
        }
        "write" => {
            "writing a component the client would read means rebuilding its wire data, which \
             this host cannot do"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// The components of the client's own item or entity, which this host cannot read. `None`.
#[must_use]
pub fn get_live_component(_owner: &str, _id: &str) -> Option<ConfigValue> {
    crate::warn!("a mod asked for a live component: {}", why_unknown("item"));
    None
}

/// A component of the live stack in the player's hand. `None`.
#[must_use]
pub fn get_stack_component(_id: &str) -> Option<ConfigValue> {
    crate::warn!(
        "a mod asked for a component of a live stack: {}",
        why_unknown("stack")
    );
    None
}

/// Writes a component the client would read. `false`.
pub fn set_live_component(_owner: &str, _id: &str, _value: ConfigValue) -> bool {
    crate::warn!(
        "a mod asked to write a live component: {}",
        why_unknown("write")
    );
    false
}

/// Removes a component the client reads. `false`.
pub fn remove_live_component(_owner: &str, _id: &str) -> bool {
    crate::warn!(
        "a mod asked to remove a live component: {}",
        why_unknown("write")
    );
    false
}

/// What this host cannot reach about data components, for a panel or a report.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["item", "entity", "stack", "write"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with_registries(|registries| {
            registries.components.clear();
            registries.data_types.clear();
            registries.codecs.clear();
            registries.migrations.clear();
            registries.custom.clear();
            registries.watchers.clear();
        });
    }

    fn durability() -> ComponentType {
        ComponentType::new(
            "hello:durability",
            "Durability",
            ValueKind::Integer,
            ConfigValue::Integer(100),
        )
        .described("how much use is left")
    }

    #[test]
    fn a_component_type_is_registered_with_its_kind_and_default() {
        let _serial = serial();
        fresh();
        let declared = durability();
        assert!(register_component(declared.clone()));
        let component = get_component_schema("hello:durability").expect("registered");
        assert_eq!(component.kind, ValueKind::Integer);
        assert_eq!(component.default, ConfigValue::Integer(100));
        assert_eq!(component.owner, declared.owner);
        assert_eq!(all_components().len(), 1);
        assert_eq!(default_of("hello:durability"), Some(ConfigValue::Integer(100)));
        assert!(register_data_type(DataType::new("hello:count", ValueKind::Integer).described("a tally")));
        assert_eq!(get_data_type("hello:count").map(|data| data.kind), Some(ValueKind::Integer));
        assert!(register_codec(Codec::new("hello:shout", ValueKind::String)));
        assert!(!register_codec(Codec::new("hello:shout", ValueKind::String)), "the second replaces the first");
        assert!(get_codec("integer").is_some(), "the built-in codecs are always reachable");
        assert!(!register_component(durability()), "the second registration replaces the first");
        assert!(unregister_component("hello:durability"));
        assert!(!unregister_component("hello:durability"));
    }

    #[test]
    fn a_typed_read_refuses_a_wrong_kind_with_a_real_error() {
        let _serial = serial();
        fresh();
        register_component(durability());
        let mut map = ComponentMap::new("minecraft:iron_pickaxe");
        assert!(map.set("hello:durability", ConfigValue::Integer(42)).is_ok());
        assert_eq!(map.get_typed("hello:durability"), Ok(ConfigValue::Integer(42)));
        // Persisted or foreign data can arrive with the wrong kind; the typed read is what
        // catches it, naming the component and both kinds.
        map.insert_raw("hello:durability", ConfigValue::String("lots".to_owned()));
        assert_eq!(
            map.get_typed("hello:durability"),
            Err(DataError::TypeMismatch {
                component: "hello:durability".to_owned(),
                wanted: "integer",
                found: "string",
            })
        );
        assert_eq!(
            map.set("hello:durability", ConfigValue::String("lots".to_owned())),
            Err(DataError::TypeMismatch {
                component: "hello:durability".to_owned(),
                wanted: "integer",
                found: "string",
            })
        );
        assert_eq!(
            map.get_typed("nothing:here"),
            Err(DataError::UnknownComponent {
                component: "nothing:here".to_owned(),
            })
        );
        assert_eq!(
            validate_component("hello:durability", &ConfigValue::Text("lots".to_owned())),
            vec!["hello:durability holds string, not integer".to_owned()]
        );
    }

    #[test]
    fn the_map_operations_read_write_and_remove_and_notify_a_watcher() {
        let _serial = serial();
        fresh();
        register_component(durability());
        let mut map = ComponentMap::new("wisp");
        let heard = std::sync::Arc::new(Mutex::new(Vec::new()));
        let into = std::sync::Arc::clone(&heard);
        let id = watch("wisp", move |id, value| {
            into.lock()
                .unwrap()
                .push(format!("{id}={}", value.render()));
        });
        assert!(set_component(&mut map, "hello:durability", ConfigValue::Integer(7)).is_ok());
        assert!(has_component(&map, "hello:durability"));
        assert_eq!(get_component_raw(&map, "hello:durability"), Some(ConfigValue::Integer(7)));
        assert_eq!(map.keys(), vec!["hello:durability".to_owned()]);
        assert_eq!(map.len(), 1);
        assert_eq!(
            *heard.lock().unwrap(),
            vec!["hello:durability=7".to_owned()]
        );
        assert!(remove_component(&mut map, "hello:durability"));
        assert!(!remove_component(&mut map, "hello:durability"));
        assert!(!has_component(&map, "hello:durability"));
        map.insert_raw("hello:durability", ConfigValue::Integer(1));
        map.clear();
        assert!(map.is_empty());
        assert_eq!(heard.lock().unwrap().len(), 2, "the watcher heard both changes");
        assert!(unwatch(id));
        assert!(!unwatch(id));
    }

    #[test]
    fn a_patch_applies_to_a_copy_and_leaves_the_original_untouched() {
        let _serial = serial();
        fresh();
        register_component(durability());
        register_component(ComponentType::new(
            "hello:traits",
            "Traits",
            ValueKind::List,
            ConfigValue::List(Vec::new()),
        ));
        let mut original = ComponentMap::new("wisp");
        original.set("hello:traits", ConfigValue::List(vec![ConfigValue::String("swift".to_owned())])).unwrap();
        let mut patch = Patch::new();
        patch.set("hello:durability", ConfigValue::Integer(9));
        patch.remove("hello:traits");
        assert_eq!(patch.ids(), vec!["hello:durability".to_owned(), "hello:traits".to_owned()]);
        let patched = patch.applied(&original).expect("the patch applies");
        assert_eq!(patched.get("hello:durability"), Some(ConfigValue::Integer(9)));
        assert!(!patched.has("hello:traits"));
        assert_eq!(original.get("hello:durability"), None, "the original is untouched");
        assert!(original.has("hello:traits"), "the original is untouched");
        assert!(!patch.is_empty());
    }

    #[test]
    fn merging_two_maps_lets_the_right_hand_win() {
        let _serial = serial();
        fresh();
        register_component(durability());
        register_component(ComponentType::new(
            "hello:traits",
            "Traits",
            ValueKind::List,
            ConfigValue::List(Vec::new()),
        ));
        let mut self_map = ComponentMap::new("wisp");
        self_map.set("hello:durability", ConfigValue::Integer(1)).unwrap();
        let mut other = ComponentMap::new("wisp");
        other.set("hello:durability", ConfigValue::Integer(2)).unwrap();
        other.set("hello:traits", ConfigValue::List(Vec::new())).unwrap();
        assert!(self_map.merge(&other));
        assert_eq!(self_map.get("hello:durability"), Some(ConfigValue::Integer(2)), "other wins");
        assert!(self_map.has("hello:traits"), "ids only here are kept");
        assert_eq!(other.merged(&self_map).get("hello:durability"), Some(ConfigValue::Integer(2)));
        assert!(!self_map.merge(&other), "nothing changes the second time");
    }

    #[test]
    fn a_map_round_trips_through_config_values_and_migrates() {
        let _serial = serial();
        fresh();
        register_component(durability());
        register_component(ComponentType::new(
            "hello:traits",
            "Traits",
            ValueKind::List,
            ConfigValue::List(Vec::new()),
        ));
        assert_eq!(
            deserialize_component(&serialize_component(&ComponentMap::new("minecraft:stone")))
                .expect("an empty map reads back"),
            ComponentMap::new("minecraft:stone")
        );
        let mut map = ComponentMap::new("minecraft:iron_pickaxe");
        map.set("hello:durability", ConfigValue::Integer(5)).unwrap();
        map.set(
            "hello:traits",
            ConfigValue::List(vec![
                ConfigValue::List(vec![ConfigValue::Integer(1), ConfigValue::Integer(2)]),
                ConfigValue::String("nested".to_owned()),
            ]),
        )
        .unwrap();
        let value = serialize_component(&map);
        assert!(matches!(&value, ConfigValue::Compound(_)));
        assert_eq!(deserialize_component(&value).expect("the map reads back"), map);
        assert_eq!(
            deserialize_component(&ConfigValue::Integer(1)),
            Err(DataError::NotAMap { found: "integer" })
        );
        fn bump(value: &mut ConfigValue) {
            if let ConfigValue::Integer(number) = value {
                *number += 1;
            }
        }
        register_migration(DataMigration::new(
            "hello:durability",
            "1.0.0",
            "2.0.0",
            "bump the durability",
            bump,
        ));
        assert_eq!(migrate_component(&mut map, "1.0.0", "2.0.0"), 1);
        assert_eq!(map.get("hello:durability"), Some(ConfigValue::Integer(6)));
        assert_eq!(migrate_component(&mut map, "2.0.0", "3.0.0"), 0, "outside its range");
        assert_eq!(migrations().len(), 1);
    }

    #[test]
    fn absent_and_present_default_are_distinguishable() {
        let _serial = serial();
        fresh();
        register_component(durability());
        let empty = ComponentMap::new("wisp");
        let mut defaulted = ComponentMap::new("wisp");
        defaulted.set("hello:durability", ConfigValue::Integer(100)).unwrap();
        assert!(!empty.has("hello:durability"));
        assert!(defaulted.has("hello:durability"));
        assert_eq!(
            empty.get_typed("hello:durability"),
            Err(DataError::Absent {
                component: "hello:durability".to_owned(),
            })
        );
        assert_eq!(empty.present_or_default("hello:durability"), Ok(ConfigValue::Integer(100)));
        assert_eq!(defaulted.get_typed("hello:durability"), Ok(ConfigValue::Integer(100)));
        assert_ne!(empty, defaulted, "present at the default is not absent");
        assert_ne!(empty.fingerprint(), defaulted.fingerprint());
        assert_eq!(defaulted.fingerprint(), defaulted.clone().fingerprint());
    }

    #[test]
    fn what_the_client_owns_is_refused_with_a_reason() {
        let _serial = serial();
        fresh();
        assert!(get_live_component("minecraft:stone", "minecraft:damage").is_none());
        assert!(get_stack_component("minecraft:damage").is_none());
        assert!(!set_live_component("wisp", "hello:glow", ConfigValue::Integer(1)));
        assert!(!remove_live_component("wisp", "hello:glow"));
        assert_eq!(refusals().len(), 4);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
    }

    #[test]
    fn custom_data_is_a_mods_own_and_a_watcher_hears_one_owner() {
        let _serial = serial();
        fresh();
        register_component(durability());
        assert!(set_custom_data("wisp", "nickname", ConfigValue::Text("Puff".to_owned())));
        assert!(!set_custom_data("wisp", "nickname", ConfigValue::Text("Puff".to_owned())));
        assert_eq!(
            get_custom_data("wisp", "nickname"),
            Some(ConfigValue::Text("Puff".to_owned()))
        );
        assert_eq!(custom_data_keys("wisp"), vec!["nickname".to_owned()]);
        assert!(remove_custom_data("wisp", "nickname"));
        assert!(!remove_custom_data("wisp", "nickname"));
        let heard = std::sync::Arc::new(Mutex::new(Vec::new()));
        let into = std::sync::Arc::clone(&heard);
        watch("wisp", move |id, _| into.lock().unwrap().push(id.to_owned()));
        let mut other = ComponentMap::new("stone");
        set_component(&mut other, "hello:durability", ConfigValue::Integer(1)).unwrap();
        assert!(heard.lock().unwrap().is_empty(), "a watcher hears only its own owner");
        let mut wisp = ComponentMap::new("wisp");
        set_component(&mut wisp, "hello:durability", ConfigValue::Integer(1)).unwrap();
        assert_eq!(*heard.lock().unwrap(), vec!["hello:durability".to_owned()]);
    }
}
