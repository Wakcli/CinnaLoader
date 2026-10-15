//! Configuration: typed values, a schema that says what they may be, and one file they live
//! in.
//!
//! A mod's settings (the toggles, sliders and choices the loader draws) persist in
//! `<mods_dir>/<mod>.cfg`. This is the other half: values a mod keeps for itself, with a
//! type each, validated against a schema the mod registered, saved in
//! `<config_dir>/config.ini` and changeable at runtime with a callback.
//!
//! ```
//! use cinnaloader::prelude::*;
//!
//! config::register_schema(ConfigSchema::new("hello").entry(
//!     ConfigEntry::new("greeting", ConfigValue::Text("hello".into()))
//!         .comment("what the mod says on start")
//!         .constraint(config::max_length(32))
//!         .constraint(config::required()),
//! ));
//! config::load();
//! config::set("greeting", ConfigValue::Text("hi".into()));
//! config::watch("greeting", |key, value| info!("{key} is now {value}"));
//! config::save();
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::modinfo::ModVersion;

/// One configuration value. The wishlist's list of types, spelled as variants.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigValue {
    Boolean(bool),
    Integer(i64),
    Float(f64),
    String(String),
    Text(String),
    Enum(String),
    Color(u32),
    Keybind(String),
    FilePath(PathBuf),
    DirectoryPath(PathBuf),
    ResourceId(String),
    ItemId(String),
    BlockId(String),
    EntityId(String),
    Position(f64, f64, f64),
    Vector2(f64, f64),
    Vector3(f64, f64, f64),
    Range(i64, i64),
    List(Vec<ConfigValue>),
    Set(Vec<ConfigValue>),
    Map(BTreeMap<String, ConfigValue>),
    Object(BTreeMap<String, ConfigValue>),
    Compound(BTreeMap<String, ConfigValue>),
    Duration(u64),
    Percentage(f64),
}

impl ConfigValue {
    /// What kind of value it is, for a schema.
    #[must_use]
    pub fn kind(&self) -> ConfigType {
        match self {
            Self::Boolean(_) => ConfigType::Boolean,
            Self::Integer(_) => ConfigType::Integer,
            Self::Float(_) => ConfigType::Float,
            Self::String(_) => ConfigType::String,
            Self::Text(_) => ConfigType::Text,
            Self::Enum(_) => ConfigType::Enum,
            Self::Color(_) => ConfigType::Color,
            Self::Keybind(_) => ConfigType::Keybind,
            Self::FilePath(_) => ConfigType::FilePath,
            Self::DirectoryPath(_) => ConfigType::DirectoryPath,
            Self::ResourceId(_) => ConfigType::ResourceId,
            Self::ItemId(_) => ConfigType::ItemId,
            Self::BlockId(_) => ConfigType::BlockId,
            Self::EntityId(_) => ConfigType::EntityId,
            Self::Position(..) => ConfigType::Position,
            Self::Vector2(..) => ConfigType::Vector2,
            Self::Vector3(..) => ConfigType::Vector3,
            Self::Range(..) => ConfigType::Range,
            Self::List(_) => ConfigType::List,
            Self::Set(_) => ConfigType::Set,
            Self::Map(_) => ConfigType::Map,
            Self::Object(_) => ConfigType::Object,
            Self::Compound(_) => ConfigType::Compound,
            Self::Duration(_) => ConfigType::Duration,
            Self::Percentage(_) => ConfigType::Percentage,
        }
    }

    /// As text, one line, which is how it is written to the file.
    #[must_use]
    pub fn render(&self) -> String {
        match self {
            Self::Boolean(flag) => flag.to_string(),
            Self::Integer(number) => number.to_string(),
            Self::Float(number) | Self::Percentage(number) => number.to_string(),
            Self::Duration(millis) => format!("{millis}ms"),
            Self::Color(colour) => format!("#{colour:06x}"),
            Self::String(text)
            | Self::Text(text)
            | Self::Enum(text)
            | Self::Keybind(text)
            | Self::ResourceId(text)
            | Self::ItemId(text)
            | Self::BlockId(text)
            | Self::EntityId(text) => text.clone(),
            Self::FilePath(path) | Self::DirectoryPath(path) => path.display().to_string(),
            Self::Position(x, y, z) => format!("{x},{y},{z}"),
            Self::Vector2(x, y) => format!("{x},{y}"),
            Self::Vector3(x, y, z) => format!("{x},{y},{z}"),
            Self::Range(from, to) => format!("{from}..{to}"),
            Self::List(items) | Self::Set(items) => format!(
                "[{}]",
                items
                    .iter()
                    .map(Self::render)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Map(fields) | Self::Object(fields) | Self::Compound(fields) => format!(
                "{{{}}}",
                fields
                    .iter()
                    .map(|(key, value)| format!("{key}: {}", value.render()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }

    /// Reads back what [`ConfigValue::render`] wrote, as the given kind where that is known.
    #[must_use]
    pub fn parse(text: &str, kind: ConfigType) -> Option<Self> {
        let text = text.trim();
        let flag = |text: &str| match text {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        };
        Some(match kind {
            ConfigType::Boolean => Self::Boolean(flag(text)?),
            ConfigType::Integer => Self::Integer(text.parse().ok()?),
            ConfigType::Float => Self::Float(text.parse().ok()?),
            ConfigType::Percentage => Self::Percentage(text.parse().ok()?),
            ConfigType::Duration => {
                Self::Duration(text.trim_end_matches("ms").parse().ok()?)
            }
            ConfigType::Color => Self::Color(u32::from_str_radix(text.trim_start_matches('#'), 16).ok()?),
            ConfigType::String => Self::String(text.to_owned()),
            ConfigType::Text => Self::Text(text.to_owned()),
            ConfigType::Enum => Self::Enum(text.to_owned()),
            ConfigType::Keybind => Self::Keybind(text.to_owned()),
            ConfigType::ResourceId => Self::ResourceId(text.to_owned()),
            ConfigType::ItemId => Self::ItemId(text.to_owned()),
            ConfigType::BlockId => Self::BlockId(text.to_owned()),
            ConfigType::EntityId => Self::EntityId(text.to_owned()),
            ConfigType::FilePath => Self::FilePath(PathBuf::from(text)),
            ConfigType::DirectoryPath => Self::DirectoryPath(PathBuf::from(text)),
            ConfigType::Position => {
                let (x, y, z) = triple(text)?;
                Self::Position(x, y, z)
            }
            ConfigType::Vector2 => {
                let (x, y, _) = triple(text)?;
                Self::Vector2(x, y)
            }
            ConfigType::Vector3 => {
                let (x, y, z) = triple(text)?;
                Self::Vector3(x, y, z)
            }
            ConfigType::Range => {
                let (from, to) = text.split_once("..")?;
                Self::Range(from.trim().parse().ok()?, to.trim().parse().ok()?)
            }
            ConfigType::List | ConfigType::Set => {
                let inner = text.strip_prefix('[')?.strip_suffix(']')?;
                let items: Vec<ConfigValue> = inner
                    .split(',')
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .map(|item| Self::String(item.to_owned()))
                    .collect();
                match kind {
                    ConfigType::List => Self::List(items),
                    _ => Self::Set(items),
                }
            }
            ConfigType::Map | ConfigType::Object | ConfigType::Compound => {
                let mut fields = BTreeMap::new();
                let inner = text.strip_prefix('{')?.strip_suffix('}')?;
                for pair in inner.split(',').map(str::trim).filter(|pair| !pair.is_empty()) {
                    let (key, value) = pair.split_once(':')?;
                    fields.insert(
                        key.trim().to_owned(),
                        Self::String(value.trim().to_owned()),
                    );
                }
                match kind {
                    ConfigType::Map => Self::Map(fields),
                    ConfigType::Object => Self::Object(fields),
                    _ => Self::Compound(fields),
                }
            }
        })
    }

    /// The value as a boolean, when that is what it is.
    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Boolean(flag) => Some(*flag),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_integer(&self) -> Option<i64> {
        match self {
            Self::Integer(number) => Some(*number),
            Self::Duration(millis) => i64::try_from(*millis).ok(),
            Self::Percentage(percent) => Some(*percent as i64),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_float(&self) -> Option<f64> {
        match self {
            Self::Float(number) | Self::Percentage(number) => Some(*number),
            Self::Integer(number) => Some(*number as f64),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::String(text)
            | Self::Text(text)
            | Self::Enum(text)
            | Self::Keybind(text)
            | Self::ResourceId(text)
            | Self::ItemId(text)
            | Self::BlockId(text)
            | Self::EntityId(text) => Some(text),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_path(&self) -> Option<&PathBuf> {
        match self {
            Self::FilePath(path) | Self::DirectoryPath(path) => Some(path),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_list(&self) -> Option<&[ConfigValue]> {
        match self {
            Self::List(items) | Self::Set(items) => Some(items),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_map(&self) -> Option<&BTreeMap<String, ConfigValue>> {
        match self {
            Self::Map(fields) | Self::Object(fields) | Self::Compound(fields) => Some(fields),
            _ => None,
        }
    }
}

impl fmt::Display for ConfigValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.render())
    }
}

fn triple(text: &str) -> Option<(f64, f64, f64)> {
    let mut parts = text.split(',').map(|part| part.trim().parse::<f64>());
    let x = parts.next()?.ok()?;
    let y = parts.next()?.ok()?;
    let z = parts.next().and_then(Result::ok).unwrap_or(0.0);
    Some((x, y, z))
}

/// The kind of value an entry holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ConfigType {
    Boolean,
    Integer,
    Float,
    String,
    Text,
    Enum,
    Color,
    Keybind,
    FilePath,
    DirectoryPath,
    ResourceId,
    ItemId,
    BlockId,
    EntityId,
    Position,
    Vector2,
    Vector3,
    Range,
    List,
    Set,
    Map,
    Object,
    Compound,
    Duration,
    Percentage,
}

impl ConfigType {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Boolean => "boolean",
            Self::Integer => "integer",
            Self::Float => "float",
            Self::String => "string",
            Self::Text => "text",
            Self::Enum => "enum",
            Self::Color => "color",
            Self::Keybind => "keybind",
            Self::FilePath => "file_path",
            Self::DirectoryPath => "directory_path",
            Self::ResourceId => "resource_id",
            Self::ItemId => "item_id",
            Self::BlockId => "block_id",
            Self::EntityId => "entity_id",
            Self::Position => "position",
            Self::Vector2 => "vector2",
            Self::Vector3 => "vector3",
            Self::Range => "range",
            Self::List => "list",
            Self::Set => "set",
            Self::Map => "map",
            Self::Object => "object",
            Self::Compound => "compound",
            Self::Duration => "duration",
            Self::Percentage => "percentage",
        }
    }

    /// Whether a value of this kind can be stored in an entry of that kind.
    #[must_use]
    pub fn accepts(self, other: Self) -> bool {
        self == other
            || matches!(
                (self, other),
                (Self::Float, Self::Integer)
                    | (Self::String, Self::Text)
                    | (Self::Text, Self::String)
            )
    }
}

impl fmt::Display for ConfigType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// When an entry is shown, for a panel that hides what does not apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ConfigVisibility {
    #[default]
    Always,
    /// Only in a development build.
    Development,
    /// Only when another entry says so; see [`visible_when`].
    Conditional,
    /// Never shown, still stored.
    Hidden,
}

/// What changing an entry costs the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ConfigRestartRequirement {
    #[default]
    None,
    /// The client has to be started again.
    Restart,
    /// The world has to be loaded again.
    WorldReload,
    /// Resources have to be reloaded.
    ResourceReload,
}

impl ConfigRestartRequirement {
    /// Whether the change takes effect straight away.
    #[must_use]
    pub fn is_immediate(self) -> bool {
        self == Self::None
    }
}

/// A heading a panel groups entries under.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConfigCategory {
    pub name: String,
    pub description: String,
}

impl ConfigCategory {
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            description: String::new(),
        }
    }

    #[must_use]
    pub fn description(mut self, description: &str) -> Self {
        self.description = description.to_owned();
        self
    }
}

/// A group of entries inside a category.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConfigGroup {
    pub name: String,
    pub category: String,
    pub collapsed: bool,
}

impl ConfigGroup {
    #[must_use]
    pub fn new(name: &str, category: &str) -> Self {
        Self {
            name: name.to_owned(),
            category: category.to_owned(),
            collapsed: false,
        }
    }
}

/// A note shown beside an entry.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConfigComment {
    pub text: String,
}

impl ConfigComment {
    #[must_use]
    pub fn new(text: &str) -> Self {
        Self {
            text: text.to_owned(),
        }
    }
}

/// One thing a value has to satisfy. Built by the helpers below.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigConstraint {
    Required,
    Min(f64),
    Max(f64),
    Range(f64, f64),
    MinLength(usize),
    MaxLength(usize),
    Pattern(String),
    OneOf(Vec<String>),
    Unique,
    /// This entry only matters when `key` equals one of these texts.
    DependsOn(String, Vec<String>),
    /// Shown only when `key` is one of these.
    VisibleWhen(String, Vec<String>),
    /// Editable only when `key` is one of these.
    EnabledWhen(String, Vec<String>),
    RequiresRestart,
    RequiresWorldReload,
    RequiresResourceReload,
}

impl ConfigConstraint {
    /// One line saying what the constraint is, for a message.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Required => "is required".to_owned(),
            Self::Min(minimum) => format!("must be at least {minimum}"),
            Self::Max(maximum) => format!("must be at most {maximum}"),
            Self::Range(from, to) => format!("must be between {from} and {to}"),
            Self::MinLength(length) => format!("must be at least {length} characters"),
            Self::MaxLength(length) => format!("must be at most {length} characters"),
            Self::Pattern(pattern) => format!("must match {pattern}"),
            Self::OneOf(options) => format!("must be one of {}", options.join(", ")),
            Self::Unique => "must not repeat a value another entry has".to_owned(),
            Self::DependsOn(key, values) => {
                format!("only applies when {key} is one of {}", values.join(", "))
            }
            Self::VisibleWhen(key, values) => {
                format!("is shown when {key} is one of {}", values.join(", "))
            }
            Self::EnabledWhen(key, values) => {
                format!("is editable when {key} is one of {}", values.join(", "))
            }
            Self::RequiresRestart => "needs a restart to take effect".to_owned(),
            Self::RequiresWorldReload => "needs the world to be reloaded".to_owned(),
            Self::RequiresResourceReload => "needs resources to be reloaded".to_owned(),
        }
    }

    /// Whether `value` satisfies it, given what the rest of the config holds.
    #[must_use]
    pub fn check(&self, key: &str, value: &ConfigValue, config: &BTreeMap<String, ConfigValue>) -> Option<String> {
        let number = |value: &ConfigValue| value.as_float();
        let fails = |what: String| Some(format!("{key} {what}"));
        match self {
            Self::Required => match value {
                ConfigValue::Boolean(_) => None,
                ConfigValue::List(items) | ConfigValue::Set(items) => {
                    items.is_empty().then(|| format!("{key} is required"))
                }
                other if other.render().is_empty() => Some(format!("{key} is required")),
                _ => None,
            },
            Self::Min(minimum) => match number(value) {
                Some(found) if found < *minimum => fails(format!("is {found}, below {minimum}")),
                _ => None,
            },
            Self::Max(maximum) => match number(value) {
                Some(found) if found > *maximum => fails(format!("is {found}, above {maximum}")),
                _ => None,
            },
            Self::Range(from, to) => match number(value) {
                Some(found) if found < *from || found > *to => {
                    fails(format!("is {found}, outside {from}..{to}"))
                }
                _ => None,
            },
            Self::MinLength(length) => {
                let found = value.render().chars().count();
                (found < *length).then(|| format!("{key} is {found} characters, under {length}"))
            }
            Self::MaxLength(length) => {
                let found = value.render().chars().count();
                (found > *length).then(|| format!("{key} is {found} characters, over {length}"))
            }
            Self::Pattern(pattern) => {
                if simple_match(pattern, &value.render()) {
                    None
                } else {
                    fails(format!("does not match {pattern}"))
                }
            }
            Self::OneOf(options) => match options.contains(&value.render()) {
                true => None,
                false => fails(format!("is not one of {}", options.join(", "))),
            },
            Self::Unique => match config
                .iter()
                .filter(|(other, held)| other.as_str() != key && held.render() == value.render())
                .count()
            {
                0 => None,
                _ => fails("repeats a value another entry has".to_owned()),
            },
            Self::DependsOn(other, values) | Self::VisibleWhen(other, values) | Self::EnabledWhen(other, values) => {
                // A gate says when an entry matters, not what it may hold.
                let _ = (other, values);
                None
            }
            Self::RequiresRestart | Self::RequiresWorldReload | Self::RequiresResourceReload => None,
        }
    }
}

/// A `*` that stands for any run of characters, which is enough for a config pattern
/// without pulling in a regex engine.
fn simple_match(pattern: &str, text: &str) -> bool {
    if !pattern.contains('*') {
        return pattern == text;
    }
    let mut rest = text;
    let mut parts = pattern.split('*').peekable();
    if let Some(first) = parts.next() {
        if !rest.starts_with(first) {
            return false;
        }
        rest = &rest[first.len()..];
    }
    while let Some(part) = parts.next() {
        match parts.peek() {
            // The last piece has to be at the end.
            None => return rest.ends_with(part) || part.is_empty(),
            Some(_) => match rest.find(part) {
                Some(at) => rest = &rest[at + part.len()..],
                None => return false,
            },
        }
    }
    true
}

/// One entry in a schema: what it holds, what it may hold, and how it is shown.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigEntry {
    pub key: String,
    pub default: ConfigValue,
    pub comment: Option<ConfigComment>,
    pub group: Option<ConfigGroup>,
    pub visibility: ConfigVisibility,
    pub restart: ConfigRestartRequirement,
    pub constraints: Vec<ConfigConstraint>,
}

impl ConfigEntry {
    #[must_use]
    pub fn new(key: &str, default: ConfigValue) -> Self {
        Self {
            key: key.to_owned(),
            default,
            comment: None,
            group: None,
            visibility: ConfigVisibility::Always,
            restart: ConfigRestartRequirement::None,
            constraints: Vec::new(),
        }
    }

    /// The kind this entry holds, taken from its default.
    #[must_use]
    pub fn kind(&self) -> ConfigType {
        self.default.kind()
    }

    #[must_use]
    pub fn comment(mut self, text: &str) -> Self {
        self.comment = Some(ConfigComment::new(text));
        self
    }

    #[must_use]
    pub fn group(mut self, group: ConfigGroup) -> Self {
        self.group = Some(group);
        self
    }

    #[must_use]
    pub fn visibility(mut self, visibility: ConfigVisibility) -> Self {
        self.visibility = visibility;
        self
    }

    #[must_use]
    pub fn restart(mut self, restart: ConfigRestartRequirement) -> Self {
        self.restart = restart;
        self
    }

    #[must_use]
    pub fn constraint(mut self, constraint: ConfigConstraint) -> Self {
        self.constraints.push(constraint);
        self
    }

    /// Whether a value may be stored here.
    #[must_use]
    pub fn accepts(&self, value: &ConfigValue, config: &BTreeMap<String, ConfigValue>) -> Option<String> {
        if !self.kind().accepts(value.kind()) {
            return Some(format!(
                "{} holds {}, not {}",
                self.key,
                self.kind(),
                value.kind()
            ));
        }
        self.constraints
            .iter()
            .find_map(|constraint| constraint.check(&self.key, value, config))
    }
}

/// A mod's set of entries, with the migrations that move one version's config to the next.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigSchema {
    pub name: String,
    pub version: ModVersion,
    pub entries: Vec<ConfigEntry>,
    pub migrations: Vec<ConfigMigration>,
}

impl ConfigSchema {
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            version: ModVersion::parse("0"),
            entries: Vec::new(),
            migrations: Vec::new(),
        }
    }

    #[must_use]
    pub fn version(mut self, version: &str) -> Self {
        self.version = ModVersion::parse(version);
        self
    }

    #[must_use]
    pub fn entry(mut self, entry: ConfigEntry) -> Self {
        self.entries.push(entry);
        self
    }

    #[must_use]
    pub fn migration(mut self, migration: ConfigMigration) -> Self {
        self.migrations.push(migration);
        self
    }

    /// The defaults every entry declares.
    #[must_use]
    pub fn defaults(&self) -> BTreeMap<String, ConfigValue> {
        self.entries
            .iter()
            .map(|entry| (entry.key.clone(), entry.default.clone()))
            .collect()
    }

    #[must_use]
    pub fn entry_for(&self, key: &str) -> Option<&ConfigEntry> {
        self.entries.iter().find(|entry| entry.key == key)
    }

    /// What is wrong with the config as it stands: unknown keys, wrong kinds, constraints.
    #[must_use]
    pub fn validate(&self, config: &BTreeMap<String, ConfigValue>) -> Vec<String> {
        let mut problems = Vec::new();
        for (key, value) in config {
            match self.entry_for(key) {
                Some(entry) => problems.extend(entry.accepts(value, config)),
                None => problems.push(format!("{key} is not part of the schema")),
            }
        }
        for entry in &self.entries {
            if entry.constraints.contains(&ConfigConstraint::Required) && !config.contains_key(&entry.key) {
                problems.push(format!("{} is required and missing", entry.key));
            }
        }
        problems
    }
}

/// How one version's config becomes the next version's. Compared by the versions it
/// covers, since two closures are never the same function.
#[derive(Debug, Clone)]
pub struct ConfigMigration {
    pub from: ModVersion,
    pub to: ModVersion,
    /// What it does, in one line, for a report.
    pub description: String,
    apply: fn(&mut BTreeMap<String, ConfigValue>),
}

impl PartialEq for ConfigMigration {
    fn eq(&self, other: &Self) -> bool {
        self.from == other.from && self.to == other.to && self.description == other.description
    }
}

impl ConfigMigration {
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

    /// Runs it over the config.
    pub fn run(&self, config: &mut BTreeMap<String, ConfigValue>) {
        (self.apply)(config);
    }
}

/// A rule that checks a whole config at once, beside the per-entry constraints.
#[derive(Debug, Clone)]
pub struct ConfigValidator {
    pub name: String,
    check: fn(&BTreeMap<String, ConfigValue>) -> Vec<String>,
}

impl ConfigValidator {
    #[must_use]
    pub fn new(name: &str, check: fn(&BTreeMap<String, ConfigValue>) -> Vec<String>) -> Self {
        Self {
            name: name.to_owned(),
            check,
        }
    }

    #[must_use]
    pub fn check(&self, config: &BTreeMap<String, ConfigValue>) -> Vec<String> {
        (self.check)(config)
    }
}

type Watcher = Box<dyn FnMut(&str, &ConfigValue) + Send>;
/// A callback that wants the whole config whenever anything changes.
type Changed = Box<dyn FnMut(&BTreeMap<String, ConfigValue>) + Send>;

#[derive(Default)]
struct Held {
    config: BTreeMap<String, ConfigValue>,
    schemas: Vec<ConfigSchema>,
    validators: Vec<ConfigValidator>,
    watchers: Vec<(u64, String, Watcher)>,
    on_changed: Vec<Changed>,
    errors: Vec<String>,
    dirty: bool,
    transaction: Option<BTreeMap<String, ConfigValue>>,
}

static NEXT_WATCHER: AtomicU64 = AtomicU64::new(1);

fn held() -> &'static Mutex<Held> {
    static HELD: OnceLock<Mutex<Held>> = OnceLock::new();
    HELD.get_or_init(|| Mutex::new(Held::default()))
}

fn with<T>(body: impl FnOnce(&mut Held) -> T) -> T {
    let mut guard = held().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Where this mod's config file is. `config.ini` beside the settings the loader keeps.
#[must_use]
pub fn path() -> Option<PathBuf> {
    crate::metadata::config_directory().map(|dir| dir.join("config.ini"))
}

/// Loads the file over the defaults, and answers whether it read one.
pub fn load() -> bool {
    let Some(path) = path() else {
        return false;
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        // No file yet: the defaults stand.
        with(|held| {
            held.config = defaults_of(held);
            held.errors.clear();
            held.dirty = false;
        });
        return false;
    };
    with(|held| {
        let mut config = defaults_of(held);
        let schema = held.schemas.last().cloned();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let value = value.trim();
            let kind = schema
                .as_ref()
                .and_then(|schema| schema.entry_for(key))
                .map_or(ConfigType::String, ConfigEntry::kind);
            if let Some(value) = ConfigValue::parse(value, kind) {
                config.insert(key.to_owned(), value);
            }
        }
        held.config = config;
        held.errors = validate_with(held);
        held.dirty = false;
    });
    let changes: Vec<(String, ConfigValue)> = with(|held| held.config.clone().into_iter().collect());
    for (key, value) in changes {
        notify(key, value);
    }
    true
}

/// Writes the file, and answers whether it was written.
pub fn save() -> bool {
    let Some(path) = path() else {
        return false;
    };
    let text = with(|held| {
        let mut text = String::from("# written by cinnaloader; edits are read on the next load\n");
        for (key, value) in &held.config {
            text.push_str(&format!("{key} = {}\n", value.render()));
        }
        text
    });
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return false;
        }
    }
    if std::fs::write(&path, text).is_err() {
        return false;
    }
    with(|held| held.dirty = false);
    true
}

/// Reads the file again, throwing away what was set since the last save.
pub fn reload() -> bool {
    load()
}

/// Puts every entry back to its default.
pub fn reset() -> usize {
    let count = with(|held| {
        let defaults = defaults_of(held);
        let count = held.config.len();
        held.config = defaults;
        held.dirty = true;
        held.errors = validate_with(held);
        count
    });
    for (key, value) in snapshot() {
        notify(key, value);
    }
    count
}

/// Puts one entry back to its default, and answers whether it had one.
pub fn reset_key(key: &str) -> bool {
    let reset = with(|held| {
        let default = held
            .schemas
            .last()
            .and_then(|schema| schema.entry_for(key))
            .map(|entry| entry.default.clone());
        match default {
            Some(default) => {
                held.config.insert(key.to_owned(), default);
                held.dirty = true;
                true
            }
            None => {
                held.config.remove(key);
                held.dirty = true;
                false
            }
        }
    });
    if let Some(value) = get(key) {
        notify(key.to_owned(), value);
    }
    reset
}

fn defaults_of(held: &Held) -> BTreeMap<String, ConfigValue> {
    held.schemas
        .last()
        .map(ConfigSchema::defaults)
        .unwrap_or_default()
}

fn validate_with(held: &Held) -> Vec<String> {
    let mut problems = Vec::new();
    if let Some(schema) = held.schemas.last() {
        problems.extend(schema.validate(&held.config));
    }
    for validator in &held.validators {
        problems.extend(validator.check(&held.config));
    }
    problems
}

/// One value, if it is set.
#[must_use]
pub fn get(key: &str) -> Option<ConfigValue> {
    with(|held| held.config.get(key).cloned())
}

/// One value, or what to use instead.
#[must_use]
pub fn get_or(key: &str, fallback: ConfigValue) -> ConfigValue {
    with(|held| {
        held.config
            .get(key)
            .cloned()
            .or_else(|| {
                held.schemas
                    .last()
                    .and_then(|schema| schema.entry_for(key))
                    .map(|entry| entry.default.clone())
            })
            .unwrap_or(fallback)
    })
}

/// Sets a value. A value the schema refuses is not stored, and `validate` says why.
pub fn set(key: &str, value: ConfigValue) -> bool {
    let accepted = with(|held| {
        let refusal = held
            .schemas
            .last()
            .and_then(|schema| schema.entry_for(key))
            .and_then(|entry| entry.accepts(&value, &held.config));
        match refusal {
            Some(problem) => {
                held.errors.push(problem);
                false
            }
            None => {
                held.config.insert(key.to_owned(), value.clone());
                held.dirty = true;
                held.errors = validate_with(held);
                true
            }
        }
    });
    if accepted {
        notify(key.to_owned(), value);
    }
    accepted
}

/// Sets several values at once, stopping at the first the schema refuses.
pub fn set_many(values: &[(&str, ConfigValue)]) -> bool {
    for (key, value) in values {
        if !set(key, value.clone()) {
            return false;
        }
    }
    true
}

/// Removes a key, and answers whether it was there.
pub fn remove(key: &str) -> bool {
    let removed = with(|held| {
        let removed = held.config.remove(key).is_some();
        held.dirty |= removed;
        removed
    });
    if removed {
        notify(key.to_owned(), ConfigValue::String(String::new()));
    }
    removed
}

#[must_use]
pub fn contains(key: &str) -> bool {
    with(|held| held.config.contains_key(key))
}

#[must_use]
pub fn keys() -> Vec<String> {
    with(|held| held.config.keys().cloned().collect())
}

#[must_use]
pub fn values() -> Vec<ConfigValue> {
    with(|held| held.config.values().cloned().collect())
}

/// The whole config, for a caller that wants to walk it.
#[must_use]
pub fn all() -> BTreeMap<String, ConfigValue> {
    with(|held| held.config.clone())
}

/// Calls `watcher` whenever `key` changes. Returns an id for [`unwatch`].
pub fn watch(key: &str, watcher: impl FnMut(&str, &ConfigValue) + Send + 'static) -> u64 {
    let id = NEXT_WATCHER.fetch_add(1, Ordering::Relaxed);
    with(|held| held.watchers.push((id, key.to_owned(), Box::new(watcher))));
    id
}

/// Stops watching, and answers whether that watcher existed.
pub fn unwatch(id: u64) -> bool {
    with(|held| {
        let before = held.watchers.len();
        held.watchers.retain(|(held, _, _)| *held != id);
        held.watchers.len() != before
    })
}

/// Calls `watcher` whenever anything changes, with the whole config.
pub fn on_changed(watcher: impl FnMut(&BTreeMap<String, ConfigValue>) + Send + 'static) -> u64 {
    with(|held| held.on_changed.push(Box::new(watcher)));
    NEXT_WATCHER.fetch_add(1, Ordering::Relaxed)
}

fn notify(key: String, value: ConfigValue) {
    with(|held| {
        for (_, watched, watcher) in &mut held.watchers {
            if watched == &key {
                watcher(&key, &value);
            }
        }
        let config = held.config.clone();
        for watcher in &mut held.on_changed {
            watcher(&config);
        }
    });
}

fn snapshot() -> Vec<(String, ConfigValue)> {
    with(|held| held.config.clone().into_iter().collect())
}

/// Runs every check and keeps what it found, for [`get_errors`].
pub fn validate() -> Vec<String> {
    with(|held| {
        held.errors = validate_with(held);
        held.errors.clone()
    })
}

/// Adds a validator that checks the whole config.
pub fn register_validator(validator: ConfigValidator) {
    with(|held| held.validators.push(validator));
}

/// Moves the config from one version to another, running the migrations in between.
pub fn migrate(from: &str, to: &str) -> usize {
    let from = ModVersion::parse(from);
    let to = ModVersion::parse(to);
    let (ran, problems) = with(|held| {
        let schema = held.schemas.last().cloned().unwrap_or_else(|| ConfigSchema::new("none"));
        let mut ran = 0;
        for migration in &schema.migrations {
            if migration.from.cmp_parts(&from) != std::cmp::Ordering::Less
                && migration.to.cmp_parts(&to) != std::cmp::Ordering::Greater
            {
                migration.run(&mut held.config);
                ran += 1;
            }
        }
        held.dirty |= ran > 0;
        held.errors = validate_with(held);
        (ran, held.errors.clone())
    });
    let _ = problems;
    for (key, value) in snapshot() {
        notify(key, value);
    }
    ran
}

/// The schema in force, as it was registered.
#[must_use]
pub fn get_schema() -> Option<ConfigSchema> {
    with(|held| held.schemas.last().cloned())
}

/// Registers a schema, replacing one of the same name.
pub fn register_schema(schema: ConfigSchema) {
    with(|held| {
        held.schemas.retain(|held| held.name != schema.name);
        held.schemas.push(schema);
        let defaults = defaults_of(held);
        for (key, value) in defaults {
            held.config.entry(key).or_insert(value);
        }
        held.errors = validate_with(held);
    });
}

/// Removes a schema by name, keeping the values.
pub fn unregister_schema(name: &str) -> bool {
    with(|held| {
        let before = held.schemas.len();
        held.schemas.retain(|schema| schema.name != name);
        held.schemas.len() != before
    })
}

/// Every default the schema declares.
#[must_use]
pub fn get_defaults() -> BTreeMap<String, ConfigValue> {
    with(|held| defaults_of(held))
}

/// What the last check found: empty means the config is fine.
#[must_use]
pub fn get_errors() -> Vec<String> {
    with(|held| held.errors.clone())
}

/// Writes the config where asked, or to `config.ini` beside it.
pub fn export(path: Option<&std::path::Path>) -> std::io::Result<PathBuf> {
    let path = match path {
        Some(path) => path.to_owned(),
        None => path_or_error()?,
    };
    let text = with(|held| {
        held.config
            .iter()
            .map(|(key, value)| format!("{key} = {}\n", value.render()))
            .collect::<String>()
    });
    std::fs::write(&path, text)?;
    Ok(path)
}

/// Reads a config back from a file, as [`load`] does for its own.
pub fn import(path: &std::path::Path) -> std::io::Result<usize> {
    let text = std::fs::read_to_string(path)?;
    let schema = get_schema();
    let mut count = 0;
    with(|held| {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let kind = schema
                .as_ref()
                .and_then(|schema| schema.entry_for(key.trim()))
                .map_or(ConfigType::String, ConfigEntry::kind);
            if let Some(value) = ConfigValue::parse(value.trim(), kind) {
                held.config.insert(key.trim().to_owned(), value);
                count += 1;
            }
        }
        held.dirty |= count > 0;
        held.errors = validate_with(held);
    });
    Ok(count)
}

/// Copies the file beside itself, with `.bak` added, and answers where the copy went.
pub fn backup() -> std::io::Result<PathBuf> {
    let path = path_or_error()?;
    let backup = path.with_extension("ini.bak");
    std::fs::write(&backup, config_text())?;
    Ok(backup)
}

/// Reads the backup back over the config.
pub fn restore() -> std::io::Result<usize> {
    let path = path_or_error()?;
    let backup = path.with_extension("ini.bak");
    import(&backup)
}

fn path_or_error() -> std::io::Result<PathBuf> {
    path().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no directory is known for this mod",
        )
    })
}

fn config_text() -> String {
    with(|held| {
        held.config
            .iter()
            .map(|(key, value)| format!("{key} = {}\n", value.render()))
            .collect()
    })
}

/// A set of changes that either all happen or none do.
#[derive(Debug, Clone, Default)]
pub struct Transaction {
    open: bool,
}

impl Transaction {
    /// Starts one, keeping what the config holds now.
    #[must_use]
    pub fn begin() -> Self {
        with(|held| {
            held.transaction = Some(held.config.clone());
        });
        Self { open: true }
    }

    /// Keeps everything set since it began.
    pub fn commit(&mut self) -> bool {
        if !self.open {
            return false;
        }
        with(|held| {
            held.transaction = None;
            held.dirty = true;
        });
        self.open = false;
        true
    }

    /// Puts the config back as it was.
    pub fn rollback(&mut self) -> bool {
        if !self.open {
            return false;
        }
        with(|held| {
            if let Some(previous) = held.transaction.take() {
                held.config = previous;
            }
            held.errors = validate_with(held);
        });
        self.open = false;
        for (key, value) in snapshot() {
            notify(key, value);
        }
        true
    }

    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open
    }
}

/// Starts a transaction, and answers whether one was already open.
pub fn begin_transaction() -> bool {
    with(|held| {
        let already = held.transaction.is_some();
        if !already {
            held.transaction = Some(held.config.clone());
        }
        !already
    })
}

/// Keeps everything set since [`begin_transaction`].
pub fn commit() -> bool {
    with(|held| {
        let was_open = held.transaction.is_some();
        held.transaction = None;
        if was_open {
            held.dirty = true;
        }
        was_open
    })
}

/// Puts the config back as it was when [`begin_transaction`] ran.
pub fn rollback() -> bool {
    let restored = with(|held| {
        match held.transaction.take() {
            Some(previous) => {
                held.config = previous;
                held.errors = validate_with(held);
                true
            }
            None => false,
        }
    });
    if restored {
        for (key, value) in snapshot() {
            notify(key, value);
        }
    }
    restored
}

/// The same as [`begin_transaction`], as a value.
#[must_use]
pub fn transaction() -> Transaction {
    Transaction::begin()
}

/// Whether anything changed since the last save or load.
#[must_use]
pub fn is_dirty() -> bool {
    with(|held| held.dirty)
}

// The validation helpers the wishlist asks for, each one constraint.

#[must_use]
pub fn required() -> ConfigConstraint {
    ConfigConstraint::Required
}

#[must_use]
pub fn min(least: f64) -> ConfigConstraint {
    ConfigConstraint::Min(least)
}

#[must_use]
pub fn max(most: f64) -> ConfigConstraint {
    ConfigConstraint::Max(most)
}

#[must_use]
pub fn range(from: f64, to: f64) -> ConfigConstraint {
    ConfigConstraint::Range(from, to)
}

#[must_use]
pub fn min_length(length: usize) -> ConfigConstraint {
    ConfigConstraint::MinLength(length)
}

#[must_use]
pub fn max_length(length: usize) -> ConfigConstraint {
    ConfigConstraint::MaxLength(length)
}

#[must_use]
pub fn pattern(pattern: &str) -> ConfigConstraint {
    ConfigConstraint::Pattern(pattern.to_owned())
}

#[must_use]
pub fn one_of(options: &[&str]) -> ConfigConstraint {
    ConfigConstraint::OneOf(options.iter().map(|option| (*option).to_owned()).collect())
}

#[must_use]
pub fn unique() -> ConfigConstraint {
    ConfigConstraint::Unique
}

#[must_use]
pub fn depends_on(key: &str, values: &[&str]) -> ConfigConstraint {
    ConfigConstraint::DependsOn(key.to_owned(), texts(values))
}

#[must_use]
pub fn visible_when(key: &str, values: &[&str]) -> ConfigConstraint {
    ConfigConstraint::VisibleWhen(key.to_owned(), texts(values))
}

#[must_use]
pub fn enabled_when(key: &str, values: &[&str]) -> ConfigConstraint {
    ConfigConstraint::EnabledWhen(key.to_owned(), texts(values))
}

#[must_use]
pub fn requires_restart() -> ConfigConstraint {
    ConfigConstraint::RequiresRestart
}

#[must_use]
pub fn requires_world_reload() -> ConfigConstraint {
    ConfigConstraint::RequiresWorldReload
}

#[must_use]
pub fn requires_resource_reload() -> ConfigConstraint {
    ConfigConstraint::RequiresResourceReload
}

fn texts(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with(|held| {
            held.config.clear();
            held.schemas.clear();
            held.validators.clear();
            held.watchers.clear();
            held.on_changed.clear();
            held.errors.clear();
            held.dirty = false;
            held.transaction = None;
        });
    }

    #[test]
    fn a_registered_schema_supplies_the_defaults() {
        let _serial = serial();
        fresh();
        register_schema(
            ConfigSchema::new("probe")
                .version("1.2.0")
                .entry(ConfigEntry::new("loud", ConfigValue::Boolean(true)))
                .entry(ConfigEntry::new("volume", ConfigValue::Percentage(50.0))),
        );
        assert_eq!(get("loud"), Some(ConfigValue::Boolean(true)));
        assert_eq!(get_defaults().len(), 2);
        assert_eq!(
            get_schema().expect("a schema is registered").version.to_string(),
            "1.2.0"
        );
        assert!(unregister_schema("probe"));
        assert!(get_schema().is_none());
    }

    #[test]
    fn a_value_the_schema_refuses_is_not_stored() {
        let _serial = serial();
        fresh();
        register_schema(
            ConfigSchema::new("probe").entry(
                ConfigEntry::new("count", ConfigValue::Integer(1)).constraint(range(1.0, 10.0)),
            ),
        );
        assert!(set("count", ConfigValue::Integer(5)));
        assert!(!set("count", ConfigValue::Integer(50)));
        assert_eq!(get("count"), Some(ConfigValue::Integer(5)));
        assert!(get_errors().iter().any(|error| error.contains("10")));
        // A value of the wrong kind is refused too.
        assert!(!set("count", ConfigValue::Text("five".to_owned())));
    }

    #[test]
    fn a_watcher_hears_about_a_change() {
        let _serial = serial();
        fresh();
        let heard = std::sync::Arc::new(Mutex::new(Vec::new()));
        let into = std::sync::Arc::clone(&heard);
        let id = watch("greeting", move |key, value| {
            into.lock()
                .unwrap()
                .push(format!("{key}={}", value.render()));
        });
        set("greeting", ConfigValue::Text("hi".to_owned()));
        assert_eq!(
            *heard.lock().unwrap(),
            vec!["greeting=hi".to_owned()]
        );
        assert!(unwatch(id));
        set("greeting", ConfigValue::Text("hello".to_owned()));
        assert_eq!(heard.lock().unwrap().len(), 1, "the watcher was removed");
    }

    #[test]
    fn a_transaction_keeps_or_discards_the_batch() {
        let _serial = serial();
        fresh();
        set("one", ConfigValue::Integer(1));
        assert!(begin_transaction());
        assert!(!begin_transaction(), "one is already open");
        set("one", ConfigValue::Integer(2));
        set("two", ConfigValue::Integer(3));
        assert!(rollback());
        assert_eq!(get("one"), Some(ConfigValue::Integer(1)));
        assert_eq!(get("two"), None);

        let mut transaction = transaction();
        set("one", ConfigValue::Integer(9));
        assert!(transaction.is_open());
        assert!(transaction.commit());
        assert_eq!(get("one"), Some(ConfigValue::Integer(9)));
        assert!(!transaction.rollback(), "a committed transaction is closed");
    }

    #[test]
    fn a_migration_moves_one_version_to_the_next() {
        let _serial = serial();
        fresh();
        fn rename(config: &mut BTreeMap<String, ConfigValue>) {
            if let Some(value) = config.remove("old_name") {
                config.insert("new_name".to_owned(), value);
            }
        }
        register_schema(
            ConfigSchema::new("probe")
                .migration(ConfigMigration::new("1.0.0", "2.0.0", "rename a key", rename)),
        );
        set("old_name", ConfigValue::Text("kept".to_owned()));
        assert_eq!(migrate("1.0.0", "2.0.0"), 1);
        assert_eq!(get("new_name"), Some(ConfigValue::Text("kept".to_owned())));
        assert_eq!(get("old_name"), None);
        assert_eq!(migrate("2.0.0", "3.0.0"), 0, "nothing applies outside its range");
    }

    #[test]
    fn a_value_round_trips_through_its_text_form() {
        for value in [
            ConfigValue::Boolean(true),
            ConfigValue::Integer(-4),
            ConfigValue::Float(1.5),
            ConfigValue::Text("some words".to_owned()),
            ConfigValue::Color(0x12ab34),
            ConfigValue::Duration(1500),
            ConfigValue::Percentage(25.0),
            ConfigValue::Range(1, 8),
            ConfigValue::List(vec![ConfigValue::String("a".to_owned())]),
        ] {
            let rendered = value.render();
            let parsed = ConfigValue::parse(&rendered, value.kind());
            assert_eq!(parsed, Some(value.clone()), "{rendered} did not read back");
        }
        assert_eq!(
            ConfigValue::parse("3,4,5", ConfigType::Position),
            Some(ConfigValue::Position(3.0, 4.0, 5.0))
        );
        assert_eq!(ConfigValue::parse("nonsense", ConfigType::Integer), None);
    }

    #[test]
    fn a_whole_config_can_be_checked_at_once() {
        let _serial = serial();
        fresh();
        register_validator(ConfigValidator::new("pair", |config| {
            match (
                config.get("min").and_then(ConfigValue::as_integer),
                config.get("max").and_then(ConfigValue::as_integer),
            ) {
                (Some(min), Some(max)) if min > max => vec!["min is above max".to_owned()],
                _ => Vec::new(),
            }
        }));
        register_schema(
            ConfigSchema::new("probe")
                .entry(ConfigEntry::new("min", ConfigValue::Integer(0)))
                .entry(ConfigEntry::new("max", ConfigValue::Integer(10))),
        );
        set("min", ConfigValue::Integer(20));
        assert!(validate().iter().any(|error| error == "min is above max"));
    }

    #[test]
    fn a_config_missing_a_required_entry_is_reported() {
        let _serial = serial();
        fresh();
        register_schema(
            ConfigSchema::new("probe").entry(
                ConfigEntry::new("mandatory", ConfigValue::Text(String::new()))
                    .constraint(required()),
            ),
        );
        let problems = validate();
        assert!(problems.iter().any(|problem| problem.contains("mandatory")));
        assert!(set("mandatory", ConfigValue::Text("here".to_owned())));
        assert!(validate().iter().all(|problem| !problem.contains("mandatory")));
    }

    #[test]
    fn a_pattern_with_a_star_matches_the_way_a_config_expects() {
        let _serial = serial();
        fresh();
        register_schema(ConfigSchema::new("probe").entry(
            ConfigEntry::new("id", ConfigValue::String(String::new()))
                .constraint(pattern("minecraft:*")),
        ));
        assert!(set("id", ConfigValue::String("minecraft:stone".to_owned())));
        assert!(!set("id", ConfigValue::String("other:stone".to_owned())));
    }
}
