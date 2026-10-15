//! Networking and cross-version translation: a version registry, real translation rules and
//! the chain that carries a packet's fields from one protocol to another.
//!
//! A **version registry** maps protocol numbers to game versions and back, ordered by
//! [`crate::version`], refusing a duplicate and naming an unknown number. A **rule**
//! ([`Rule`]) spans two protocols for one packet and carries real transforms — a field
//! rename, a field added with a default, a field dropped, a type change — applied to packet
//! data shaped as [`crate::config::ConfigValue`]. [`translate_packet`] builds the chain from
//! one protocol to another, applies the intermediate rules **in order**, and refuses a gap
//! with [`ProtocolError::MissingTranslation`] naming from, to and the packet.
//!
//! [`dry_run`] reports what each rule would change without touching the packet, and the
//! [`Report`] renders as text or JSON through [`crate::serialization`]. A rule registered
//! without a reverse transform is one way only; a chain that would have to walk it backwards
//! is refused. Two versions with no path at all are refused with [`ProtocolError::NoPath`].
//!
//! What this host cannot do is touch the client's real traffic — intercept it, read it or
//! rewrite it — refused at each entry point with a sentence in [`why_unknown`].
//!
//! ```
//! use cinnaloader::protocol::{self, Rule};
//! use cinnaloader::config::ConfigValue;
//! use std::collections::BTreeMap;
//!
//! protocol::clear();
//! protocol::register_version(765, "1.20.4").unwrap();
//! protocol::register_version(766, "1.20.5").unwrap();
//! protocol::register_version(767, "1.21.0").unwrap();
//! protocol::set_version(767);
//!
//! protocol::register_translator(Rule::new(765, 766, "demo:block", "rename marker").rename("old", "mid"));
//! protocol::register_translator(
//!     Rule::new(766, 767, "demo:block", "settle the new name")
//!         .rename("mid", "name")
//!         .add("region", ConfigValue::Integer(0)),
//! );
//!
//! let mut fields = BTreeMap::new();
//! fields.insert("old".to_owned(), ConfigValue::Integer(7));
//! let packet = ConfigValue::Map(fields);
//!
//! let run = protocol::translate_packet("demo:block", &packet, 765, 767).expect("a chain");
//! let map = run.as_map().expect("a map");
//! assert_eq!(map.get("name"), Some(&ConfigValue::Integer(7)));
//! assert!(!map.contains_key("old") && !map.contains_key("mid"));
//! assert_eq!(map.get("region"), Some(&ConfigValue::Integer(0)));
//! assert_eq!(protocol::get_translation_report().expect("a report").applied.len(), 2);
//! assert!(protocol::get_packet_map().is_empty());
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Mutex, OnceLock};

use crate::config::{ConfigType, ConfigValue};
use crate::serialization::{Map as SerMap, Value};
use crate::version::Version;

/// The protocol numbers and game versions this host knows, the rules and maps registered
/// against them, the last report and the mappings a caller asked for and did not find.
#[derive(Default)]
struct Held {
    versions: BTreeMap<u32, Version>,
    by_game: BTreeMap<Version, u32>,
    current: u32,
    rules: Vec<Rule>,
    mappings: Vec<Mapping>,
    missing: BTreeSet<String>,
    report: Option<Report>,
}

fn held() -> &'static Mutex<Held> {
    static HELD: OnceLock<Mutex<Held>> = OnceLock::new();
    HELD.get_or_init(|| Mutex::new(Held::default()))
}

fn with<T>(body: impl FnOnce(&mut Held) -> T) -> T {
    let mut guard = held().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// The game version a protocol number names, or a version parsed from the number so two
/// unregistered protocols still order sensibly.
fn version_of(protocol: u32) -> Version {
    with(|held| held.versions.get(&protocol).cloned())
        .unwrap_or_else(|| Version::parse(&protocol.to_string()))
}

/// What a rule can do to one field of a packet.
#[derive(Debug, Clone, PartialEq)]
pub enum Transform {
    Rename { from: String, to: String },
    Add { field: String, value: ConfigValue },
    Drop { field: String },
    Retype { field: String, to: ConfigType },
}

/// One translation: the protocols it spans, the packet it applies to, and the transforms it
/// carries. Without a reverse transform it is one way only.
#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    pub from: u32,
    pub to: u32,
    pub packet: String,
    pub name: String,
    transforms: Vec<Transform>,
    reverse: Vec<Transform>,
    one_way: bool,
}

impl Rule {
    #[must_use]
    pub fn new(from: u32, to: u32, packet: &str, name: &str) -> Self {
        Self {
            from,
            to,
            packet: packet.to_owned(),
            name: name.to_owned(),
            transforms: Vec::new(),
            reverse: Vec::new(),
            one_way: false,
        }
    }

    #[must_use]
    pub fn rename(mut self, from: &str, to: &str) -> Self {
        self.transforms.push(Transform::Rename {
            from: from.to_owned(),
            to: to.to_owned(),
        });
        self
    }

    #[must_use]
    pub fn add(mut self, field: &str, value: ConfigValue) -> Self {
        self.transforms.push(Transform::Add {
            field: field.to_owned(),
            value,
        });
        self
    }

    #[must_use]
    pub fn remove(mut self, field: &str) -> Self {
        self.transforms.push(Transform::Drop {
            field: field.to_owned(),
        });
        self
    }

    #[must_use]
    pub fn retype(mut self, field: &str, to: ConfigType) -> Self {
        self.transforms.push(Transform::Retype {
            field: field.to_owned(),
            to,
        });
        self
    }

    #[must_use]
    pub fn transform(mut self, transform: Transform) -> Self {
        self.transforms.push(transform);
        self
    }

    /// Gives the rule the transform that walks it backwards, which also makes it reversible.
    #[must_use]
    pub fn reverse(mut self, transform: Transform) -> Self {
        self.reverse.push(transform);
        self
    }

    /// Marks the rule one way only, even if it has a reverse transform.
    #[must_use]
    pub fn one_way(mut self) -> Self {
        self.one_way = true;
        self
    }

    #[must_use]
    pub fn reversible(&self) -> bool {
        !self.one_way && !self.reverse.is_empty()
    }
}

/// A category of id that changes between versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MapKind {
    Packet,
    Registry,
    Item,
    Block,
    Entity,
    Component,
    Feature,
    Resource,
}

impl MapKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Packet => "packet",
            Self::Registry => "registry",
            Self::Item => "item",
            Self::Block => "block",
            Self::Entity => "entity",
            Self::Component => "component",
            Self::Feature => "feature",
            Self::Resource => "resource",
        }
    }
}

impl fmt::Display for MapKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One id, translated from one protocol to another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mapping {
    pub kind: MapKind,
    pub from: u32,
    pub to: u32,
    pub from_id: String,
    pub to_id: String,
}

impl Mapping {
    #[must_use]
    pub fn new(kind: MapKind, from: u32, to: u32, from_id: &str, to_id: &str) -> Self {
        Self {
            kind,
            from,
            to,
            from_id: from_id.to_owned(),
            to_id: to_id.to_owned(),
        }
    }
}

/// One rule's part of a report: the rule, the protocols it spans, the packet and the fields
/// it changed, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleChange {
    pub rule: String,
    pub from: u32,
    pub to: u32,
    pub packet: String,
    pub fields: Vec<String>,
}

/// What a translation or a dry run did, rule by rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub packet: String,
    pub from: u32,
    pub to: u32,
    pub applied: Vec<String>,
    pub changes: Vec<RuleChange>,
}

impl Report {
    /// The report as a [`Value`], the tree [`Report::json`] and [`Report::text`] render.
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut map = SerMap::new();
        map.insert("packet".to_owned(), Value::str(self.packet.clone()));
        map.insert("from".to_owned(), Value::Int(i64::from(self.from)));
        map.insert("to".to_owned(), Value::Int(i64::from(self.to)));
        map.insert(
            "applied".to_owned(),
            Value::List(self.applied.iter().map(|name| Value::str(name.clone())).collect()),
        );
        let changes = self
            .changes
            .iter()
            .map(|change| {
                let mut entry = SerMap::new();
                entry.insert("rule".to_owned(), Value::str(change.rule.clone()));
                entry.insert("from".to_owned(), Value::Int(i64::from(change.from)));
                entry.insert("to".to_owned(), Value::Int(i64::from(change.to)));
                entry.insert("packet".to_owned(), Value::str(change.packet.clone()));
                entry.insert(
                    "fields".to_owned(),
                    Value::List(
                        change
                            .fields
                            .iter()
                            .map(|field| Value::str(field.clone()))
                            .collect(),
                    ),
                );
                Value::Map(entry)
            })
            .collect();
        map.insert("changes".to_owned(), Value::List(changes));
        Value::Map(map)
    }

    /// The report as JSON, through [`crate::serialization`].
    #[must_use]
    pub fn json(&self) -> String {
        crate::serialization::to_json(&self.to_value())
            .unwrap_or_else(|error| format!("{{\"error\":\"{error}\"}}"))
    }

    /// The report as the module's own text form, through [`crate::serialization`].
    #[must_use]
    pub fn text(&self) -> String {
        crate::serialization::to_text(&self.to_value())
    }
}

/// Whether two protocol numbers are the same, a rule apart, or unreachable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Compatibility {
    Native,
    Translatable,
    Unsupported,
    Unknown,
}

impl Compatibility {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Translatable => "translatable",
            Self::Unsupported => "unsupported",
            Self::Unknown => "unknown",
        }
    }
}

impl fmt::Display for Compatibility {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Why a version or a translation could not go ahead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    /// Nothing is registered under that protocol number.
    UnknownProtocol(u32),
    DuplicateProtocol(u32),
    DuplicateVersion(String),
    /// No rule covers this version step for this packet.
    MissingTranslation { from: u32, to: u32, packet: String },
    /// No rule joins the two versions at all.
    NoPath { from: u32, to: u32 },
    /// No id map covers this id between the two versions.
    MissingMapping {
        kind: String,
        id: String,
        from: u32,
        to: u32,
    },
    /// A transform names a field the packet does not have, or cannot convert.
    BadField(String),
    /// Packet data is not a map of fields.
    NotAMap(String),
    /// The client's real traffic, with [`why_unknown`] as the reason.
    Refused(String),
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownProtocol(protocol) => {
                write!(formatter, "protocol {protocol} is not registered")
            }
            Self::DuplicateProtocol(protocol) => {
                write!(formatter, "protocol {protocol} is already registered")
            }
            Self::DuplicateVersion(version) => {
                write!(formatter, "game version {version} is already registered")
            }
            Self::MissingTranslation { from, to, packet } => write!(
                formatter,
                "no translation for packet {packet} from protocol {from} to {to}"
            ),
            Self::NoPath { from, to } => {
                write!(formatter, "no translation path from protocol {from} to {to}")
            }
            Self::MissingMapping {
                kind,
                id,
                from,
                to,
            } => write!(
                formatter,
                "no {kind} mapping for {id} from protocol {from} to {to}"
            ),
            Self::BadField(field) => write!(formatter, "the packet has no usable field {field}"),
            Self::NotAMap(kind) => write!(formatter, "packet data is {kind}, not a map of fields"),
            Self::Refused(why) => formatter.write_str(why),
        }
    }
}

impl std::error::Error for ProtocolError {}

fn apply(
    transform: &Transform,
    fields: &mut BTreeMap<String, ConfigValue>,
    changes: &mut Vec<String>,
) -> Result<(), ProtocolError> {
    match transform {
        Transform::Rename { from, to } => {
            let value = fields
                .remove(from)
                .ok_or_else(|| ProtocolError::BadField(from.clone()))?;
            fields.insert(to.clone(), value);
            changes.push(format!("{from} -> {to}"));
        }
        Transform::Add { field, value } => {
            fields.insert(field.clone(), value.clone());
            changes.push(format!("+{field} = {}", value.render()));
        }
        Transform::Drop { field } => {
            fields
                .remove(field)
                .ok_or_else(|| ProtocolError::BadField(field.clone()))?;
            changes.push(format!("-{field}"));
        }
        Transform::Retype { field, to } => {
            let converted = match fields.get(field) {
                Some(value) => {
                    let kind_before = value.kind();
                    let converted = ConfigValue::parse(&value.render(), *to)
                        .ok_or_else(|| ProtocolError::BadField(field.clone()))?;
                    changes.push(format!("{field}: {kind_before} -> {to}"));
                    converted
                }
                None => return Err(ProtocolError::BadField(field.clone())),
            };
            fields.insert(field.clone(), converted);
        }
    }
    Ok(())
}

fn fields_of(data: &ConfigValue) -> Result<BTreeMap<String, ConfigValue>, ProtocolError> {
    match data {
        ConfigValue::Map(fields) => Ok(fields.clone()),
        other => Err(ProtocolError::NotAMap(other.kind().as_str().to_owned())),
    }
}

struct Step {
    rule: Rule,
    reversed: bool,
}

fn path_exists(from: u32, to: u32) -> bool {
    if from == to {
        return true;
    }
    let rules = with(|held| held.rules.clone());
    let mut seen = vec![from];
    let mut stack = vec![from];
    while let Some(current) = stack.pop() {
        for rule in &rules {
            let next = if rule.from == current {
                Some(rule.to)
            } else if rule.to == current && rule.reversible() {
                Some(rule.from)
            } else {
                None
            };
            if let Some(next) = next {
                if next == to {
                    return true;
                }
                if !seen.contains(&next) {
                    seen.push(next);
                    stack.push(next);
                }
            }
        }
    }
    false
}

fn build_chain(packet: &str, from: u32, to: u32) -> Result<Vec<Step>, ProtocolError> {
    if from == to {
        return Ok(Vec::new());
    }
    let rules = with(|held| held.rules.clone());
    let forward = version_of(from) <= version_of(to);
    let mut steps = Vec::new();
    let mut current = from;
    let mut visited = vec![from];
    while current != to {
        let candidate = if forward {
            rules
                .iter()
                .filter(|rule| rule.packet == packet && rule.from == current)
                .filter(|rule| {
                    let reach = version_of(rule.to);
                    reach > version_of(current) && reach <= version_of(to)
                })
                .min_by_key(|rule| version_of(rule.to))
        } else {
            rules
                .iter()
                .filter(|rule| rule.packet == packet && rule.to == current && rule.reversible())
                .filter(|rule| {
                    let start = version_of(rule.from);
                    start < version_of(current) && start >= version_of(to)
                })
                .max_by_key(|rule| version_of(rule.from))
        };
        let Some(rule) = candidate else {
            return Err(ProtocolError::MissingTranslation {
                from: current,
                to,
                packet: packet.to_owned(),
            });
        };
        let next = if forward { rule.to } else { rule.from };
        if visited.contains(&next) {
            return Err(ProtocolError::MissingTranslation {
                from: current,
                to,
                packet: packet.to_owned(),
            });
        }
        visited.push(next);
        steps.push(Step {
            rule: rule.clone(),
            reversed: !forward,
        });
        current = next;
    }
    Ok(steps)
}

fn run_chain(
    packet: &str,
    data: &ConfigValue,
    from: u32,
    to: u32,
) -> Result<(ConfigValue, Report), ProtocolError> {
    if !is_supported(from) {
        return Err(ProtocolError::UnknownProtocol(from));
    }
    if !is_supported(to) {
        return Err(ProtocolError::UnknownProtocol(to));
    }
    if !path_exists(from, to) {
        return Err(ProtocolError::NoPath { from, to });
    }
    let steps = build_chain(packet, from, to)?;
    let mut fields = fields_of(data)?;
    let mut changes = Vec::new();
    for step in &steps {
        let transforms = if step.reversed {
            &step.rule.reverse
        } else {
            &step.rule.transforms
        };
        let mut touched = Vec::new();
        for transform in transforms {
            apply(transform, &mut fields, &mut touched)?;
        }
        changes.push(RuleChange {
            rule: step.rule.name.clone(),
            from: step.rule.from,
            to: step.rule.to,
            packet: step.rule.packet.clone(),
            fields: touched,
        });
    }
    let report = Report {
        packet: packet.to_owned(),
        from,
        to,
        applied: steps.iter().map(|step| step.rule.name.clone()).collect(),
        changes,
    };
    Ok((ConfigValue::Map(fields), report))
}

/// Registers a protocol number and the game version it names. A duplicate either way is
/// refused, naming what is already there.
pub fn register_version(protocol: u32, game: &str) -> Result<(), ProtocolError> {
    let version = Version::parse(game);
    with(|held| {
        if held.versions.contains_key(&protocol) {
            return Err(ProtocolError::DuplicateProtocol(protocol));
        }
        if held.by_game.contains_key(&version) {
            return Err(ProtocolError::DuplicateVersion(game.to_owned()));
        }
        held.by_game.insert(version.clone(), protocol);
        held.versions.insert(protocol, version);
        Ok(())
    })
}

/// Forgets a protocol number, and answers whether it was there.
pub fn unregister_version(protocol: u32) -> bool {
    with(|held| match held.versions.remove(&protocol) {
        Some(version) => {
            held.by_game.remove(&version);
            true
        }
        None => false,
    })
}

/// The game version a protocol number names. An unknown number is refused by number.
pub fn get_game_version(protocol: u32) -> Result<Version, ProtocolError> {
    with(|held| held.versions.get(&protocol).cloned())
        .ok_or(ProtocolError::UnknownProtocol(protocol))
}

/// The protocol number a game version names.
#[must_use]
pub fn get_protocol(game: &str) -> Option<u32> {
    let version = Version::parse(game);
    with(|held| held.by_game.get(&version).copied())
}

/// The protocol number this build speaks.
#[must_use]
pub fn get_version() -> u32 {
    with(|held| held.current)
}

/// Sets the protocol number this build speaks.
pub fn set_version(protocol: u32) {
    with(|held| held.current = protocol);
}

/// Whether a protocol number is registered.
#[must_use]
pub fn is_supported(protocol: u32) -> bool {
    with(|held| held.versions.contains_key(&protocol))
}

/// Every registered protocol and its game version, in game-version order.
#[must_use]
pub fn get_supported_versions() -> Vec<(u32, Version)> {
    with(|held| {
        held.by_game
            .iter()
            .map(|(version, protocol)| (*protocol, version.clone()))
            .collect()
    })
}

/// Declares a translation rule. A blank or taken name, or a second rule for the same
/// protocols and packet, is refused.
pub fn register_translator(rule: Rule) -> bool {
    with(|held| {
        if rule.name.trim().is_empty() {
            crate::warn!("a translation rule needs a name");
            return false;
        }
        if held.rules.iter().any(|held| held.name == rule.name) {
            crate::warn!("a translation rule named {} is already registered", rule.name);
            return false;
        }
        if held
            .rules
            .iter()
            .any(|held| held.from == rule.from && held.to == rule.to && held.packet == rule.packet)
        {
            crate::warn!(
                "a translation rule for {} from protocol {} to {} is already registered",
                rule.packet,
                rule.from,
                rule.to
            );
            return false;
        }
        held.rules.push(rule);
        true
    })
}

/// Forgets a translation rule by name, and answers whether it was there.
pub fn unregister_translator(name: &str) -> bool {
    with(|held| {
        let before = held.rules.len();
        held.rules.retain(|rule| rule.name != name);
        held.rules.len() != before
    })
}

/// Every registered rule, in registration order.
#[must_use]
pub fn get_translators() -> Vec<Rule> {
    with(|held| held.rules.clone())
}

/// Declares one id's translation between two protocols. A duplicate is refused.
pub fn register_mapping(mapping: Mapping) -> bool {
    with(|held| {
        if held.mappings.iter().any(|held| {
            held.kind == mapping.kind
                && held.from == mapping.from
                && held.to == mapping.to
                && held.from_id == mapping.from_id
        }) {
            crate::warn!(
                "an {} mapping for {} from protocol {} to {} is already registered",
                mapping.kind,
                mapping.from_id,
                mapping.from,
                mapping.to
            );
            return false;
        }
        held.mappings.push(mapping);
        true
    })
}

/// Forgets an id's translation, and answers whether it was there.
pub fn unregister_mapping(kind: MapKind, from: u32, to: u32, id: &str) -> bool {
    with(|held| {
        let before = held.mappings.len();
        held.mappings.retain(|mapping| {
            !(mapping.kind == kind && mapping.from == from && mapping.to == to && mapping.from_id == id)
        });
        held.mappings.len() != before
    })
}

fn map_of(kind: MapKind) -> Vec<Mapping> {
    with(|held| {
        held.mappings
            .iter()
            .filter(|mapping| mapping.kind == kind)
            .cloned()
            .collect()
    })
}

#[must_use]
pub fn get_packet_map() -> Vec<Mapping> {
    map_of(MapKind::Packet)
}

#[must_use]
pub fn get_registry_map() -> Vec<Mapping> {
    map_of(MapKind::Registry)
}

#[must_use]
pub fn get_item_map() -> Vec<Mapping> {
    map_of(MapKind::Item)
}

#[must_use]
pub fn get_block_map() -> Vec<Mapping> {
    map_of(MapKind::Block)
}

#[must_use]
pub fn get_entity_map() -> Vec<Mapping> {
    map_of(MapKind::Entity)
}

#[must_use]
pub fn get_component_map() -> Vec<Mapping> {
    map_of(MapKind::Component)
}

#[must_use]
pub fn get_feature_map() -> Vec<Mapping> {
    map_of(MapKind::Feature)
}

/// Carries a packet's data from one protocol to another, applying each rule in order and
/// refusing a gap with [`ProtocolError::MissingTranslation`].
pub fn translate_packet(
    packet: &str,
    data: &ConfigValue,
    from: u32,
    to: u32,
) -> Result<ConfigValue, ProtocolError> {
    let (value, report) = run_chain(packet, data, from, to)?;
    with(|held| held.report = Some(report));
    Ok(value)
}

/// Reports what each rule would change on the way from one protocol to another, without
/// touching the packet.
pub fn dry_run(packet: &str, data: &ConfigValue, from: u32, to: u32) -> Result<Report, ProtocolError> {
    let (_value, report) = run_chain(packet, data, from, to)?;
    Ok(report)
}

fn translate_id(
    kind: MapKind,
    id: &str,
    from: u32,
    to: u32,
) -> Result<String, ProtocolError> {
    if !is_supported(from) {
        return Err(ProtocolError::UnknownProtocol(from));
    }
    if !is_supported(to) {
        return Err(ProtocolError::UnknownProtocol(to));
    }
    if from == to {
        return Ok(id.to_owned());
    }
    let mappings = with(|held| held.mappings.clone());
    let mut current = id.to_owned();
    let mut step = from;
    let mut guard = 0;
    while step != to {
        let found = mappings.iter().find(|mapping| {
            mapping.kind == kind && mapping.from == step && mapping.from_id == current
        });
        match found {
            Some(mapping) => {
                current = mapping.to_id.clone();
                step = mapping.to;
            }
            None => {
                let description = format!("{} {current} {step}->{to}", kind.as_str());
                with(|held| {
                    held.missing.insert(description);
                });
                return Err(ProtocolError::MissingMapping {
                    kind: kind.as_str().to_owned(),
                    id: current,
                    from: step,
                    to,
                });
            }
        }
        guard += 1;
        if guard > 64 {
            let description = format!("{} {current} {step}->{to}", kind.as_str());
            with(|held| {
                held.missing.insert(description);
            });
            return Err(ProtocolError::MissingMapping {
                kind: kind.as_str().to_owned(),
                id: current,
                from: step,
                to,
            });
        }
    }
    Ok(current)
}

pub fn translate_item(id: &str, from: u32, to: u32) -> Result<String, ProtocolError> {
    translate_id(MapKind::Item, id, from, to)
}

pub fn translate_block(id: &str, from: u32, to: u32) -> Result<String, ProtocolError> {
    translate_id(MapKind::Block, id, from, to)
}

pub fn translate_entity(id: &str, from: u32, to: u32) -> Result<String, ProtocolError> {
    translate_id(MapKind::Entity, id, from, to)
}

pub fn translate_component(id: &str, from: u32, to: u32) -> Result<String, ProtocolError> {
    translate_id(MapKind::Component, id, from, to)
}

pub fn translate_resource(id: &str, from: u32, to: u32) -> Result<String, ProtocolError> {
    translate_id(MapKind::Resource, id, from, to)
}

pub fn translate_registry_id(id: &str, from: u32, to: u32) -> Result<String, ProtocolError> {
    translate_id(MapKind::Registry, id, from, to)
}

/// The last translation's report, if one has run.
#[must_use]
pub fn get_translation_report() -> Option<Report> {
    with(|held| held.report.clone())
}

/// The id mappings a caller asked for and none covered, oldest first.
#[must_use]
pub fn get_missing_mappings() -> Vec<String> {
    with(|held| held.missing.iter().cloned().collect())
}

/// What is wrong with the registered rules and maps: one that never advances, names an
/// unregistered protocol, or changes no fields.
#[must_use]
pub fn validate_mappings() -> Vec<(String, String)> {
    with(|held| {
        let mut problems = Vec::new();
        for rule in &held.rules {
            if rule.from == rule.to {
                problems.push((
                    rule.name.clone(),
                    format!("{} never moves off protocol {}", rule.name, rule.from),
                ));
            }
            if !held.versions.contains_key(&rule.from) {
                problems.push((
                    rule.name.clone(),
                    format!("{} starts at unregistered protocol {}", rule.name, rule.from),
                ));
            }
            if !held.versions.contains_key(&rule.to) {
                problems.push((
                    rule.name.clone(),
                    format!("{} ends at unregistered protocol {}", rule.name, rule.to),
                ));
            }
            if rule.transforms.is_empty() {
                problems.push((
                    rule.name.clone(),
                    format!("{} changes no fields", rule.name),
                ));
            }
        }
        for mapping in &held.mappings {
            let name = format!("{}:{}", mapping.kind.as_str(), mapping.from_id);
            if mapping.from == mapping.to {
                problems.push((name.clone(), "a mapping from a version to itself".to_owned()));
            }
            if !held.versions.contains_key(&mapping.from) {
                problems.push((
                    name.clone(),
                    format!("a mapping from unregistered protocol {}", mapping.from),
                ));
            }
            if !held.versions.contains_key(&mapping.to) {
                problems.push((
                    name,
                    format!("a mapping to unregistered protocol {}", mapping.to),
                ));
            }
        }
        problems
    })
}

/// Whether two protocol numbers are the same, joined by rules, or unreachable.
#[must_use]
pub fn get_compatibility_status(from: u32, to: u32) -> Compatibility {
    if !is_supported(from) || !is_supported(to) {
        return Compatibility::Unknown;
    }
    if from == to {
        return Compatibility::Native;
    }
    if path_exists(from, to) {
        Compatibility::Translatable
    } else {
        Compatibility::Unsupported
    }
}

/// Clears every registered version, rule, map, report and missing entry.
pub fn clear() {
    with(|held| {
        held.versions.clear();
        held.by_game.clear();
        held.current = 0;
        held.rules.clear();
        held.mappings.clear();
        held.missing.clear();
        held.report = None;
    });
}

/// Why touching the client's real traffic has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "intercept" => {
            "observing the client's traffic means intercepting its connection, and this host \
             never sees the client's socket"
                .to_owned()
        }
        "read" => {
            "the client frames and reads its own traffic as it runs; this host has no handle \
             on that connection, so it cannot read it"
                .to_owned()
        }
        "rewrite" => {
            "rewriting a packet on the client's real connection would desynchronise it from \
             the server, so it is refused"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach about the client's traffic.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["intercept", "read", "rewrite"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

fn refused(what: &str) -> bool {
    crate::warn!(
        "a mod asked this host to touch the client's real traffic: {}",
        why_unknown(what)
    );
    false
}

/// Intercepting the client's real traffic. `false`, with [`why_unknown`].
pub fn intercept(_packet: &str) -> bool {
    refused("intercept")
}

/// Reading the client's real traffic. Empty, with [`why_unknown`].
#[must_use]
pub fn read_traffic() -> Vec<ConfigValue> {
    crate::warn!(
        "a mod asked this host to touch the client's real traffic: {}",
        why_unknown("read")
    );
    Vec::new()
}

/// Rewriting the client's real traffic. `false`, with [`why_unknown`].
pub fn rewrite_traffic(_packet: &str, _data: &ConfigValue) -> bool {
    refused("rewrite")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn packet(fields: &[(&str, ConfigValue)]) -> ConfigValue {
        let mut map = BTreeMap::new();
        for (key, value) in fields {
            map.insert((*key).to_owned(), value.clone());
        }
        ConfigValue::Map(map)
    }

    fn field<'a>(value: &'a ConfigValue, key: &str) -> Option<&'a ConfigValue> {
        value.as_map()?.get(key)
    }

    fn bytes(value: &ConfigValue) -> Vec<u8> {
        value.render().into_bytes()
    }

    #[test]
    fn the_version_registry_reads_both_ways_and_refuses_a_duplicate() {
        let _serial = serial();
        clear();
        assert!(register_version(765, "1.20.4").is_ok());
        assert!(register_version(766, "1.20.5").is_ok());
        assert_eq!(
            register_version(765, "1.21.0"),
            Err(ProtocolError::DuplicateProtocol(765))
        );
        assert_eq!(
            register_version(767, "1.20.5"),
            Err(ProtocolError::DuplicateVersion("1.20.5".to_owned()))
        );
        assert_eq!(
            get_game_version(765).map(|version| version.to_string()),
            Ok("1.20.4".to_owned())
        );
        assert_eq!(get_protocol("1.20.5"), Some(766));
        assert_eq!(get_game_version(999), Err(ProtocolError::UnknownProtocol(999)));
        assert!(is_supported(765));
        assert!(!is_supported(999));
        set_version(766);
        assert_eq!(get_version(), 766);
        assert_eq!(
            get_supported_versions()
                .first()
                .map(|(protocol, version)| (*protocol, version.to_string())),
            Some((765, "1.20.4".to_owned()))
        );
        assert!(unregister_version(765));
        assert!(!unregister_version(765));
    }

    #[test]
    fn id_maps_translate_between_versions_and_record_a_missing_one() {
        let _serial = serial();
        clear();
        register_version(1, "1.0.0").unwrap();
        register_version(2, "1.1.0").unwrap();
        set_version(2);
        assert!(register_mapping(Mapping::new(
            MapKind::Item,
            1,
            2,
            "demo:old_gem",
            "demo:gem"
        )));
        assert!(
            !register_mapping(Mapping::new(MapKind::Item, 1, 2, "demo:old_gem", "demo:other")),
            "a duplicate from-id is refused"
        );
        assert_eq!(
            translate_item("demo:old_gem", 1, 2),
            Ok("demo:gem".to_owned())
        );
        assert_eq!(
            translate_item("demo:missing", 1, 2),
            Err(ProtocolError::MissingMapping {
                kind: "item".to_owned(),
                id: "demo:missing".to_owned(),
                from: 1,
                to: 2,
            })
        );
        assert_eq!(get_missing_mappings(), vec!["item demo:missing 1->2".to_owned()]);
        assert_eq!(get_item_map().len(), 1);
        assert!(get_block_map().is_empty());
        assert!(unregister_mapping(MapKind::Item, 1, 2, "demo:old_gem"));
    }

    #[test]
    fn a_rule_applies_a_real_transform_to_packet_data() {
        let _serial = serial();
        clear();
        register_version(1, "1.0.0").unwrap();
        register_version(2, "1.1.0").unwrap();
        set_version(2);
        register_translator(Rule::new(1, 2, "demo:rename", "rename").rename("old", "name"));
        register_translator(Rule::new(1, 2, "demo:add", "add").add("region", ConfigValue::Integer(0)));
        register_translator(Rule::new(1, 2, "demo:drop", "drop").remove("temp"));
        register_translator(Rule::new(1, 2, "demo:retype", "retype").retype("count", ConfigType::String));

        let renamed = translate_packet("demo:rename", &packet(&[("old", ConfigValue::Integer(7))]), 1, 2)
            .expect("the rule exists");
        assert_eq!(field(&renamed, "name"), Some(&ConfigValue::Integer(7)));
        assert!(field(&renamed, "old").is_none());

        let added = translate_packet("demo:add", &packet(&[("id", ConfigValue::Text("x".into()))]), 1, 2)
            .expect("the rule exists");
        assert_eq!(field(&added, "region"), Some(&ConfigValue::Integer(0)));

        let dropped = translate_packet(
            "demo:drop",
            &packet(&[("temp", ConfigValue::Integer(9)), ("keep", ConfigValue::Integer(1))]),
            1,
            2,
        )
        .expect("the rule exists");
        assert!(field(&dropped, "temp").is_none());
        assert_eq!(field(&dropped, "keep"), Some(&ConfigValue::Integer(1)));

        let retyped = translate_packet("demo:retype", &packet(&[("count", ConfigValue::Integer(5))]), 1, 2)
            .expect("the rule exists");
        assert_eq!(field(&retyped, "count"), Some(&ConfigValue::String("5".to_owned())));

        let missing_field =
            translate_packet("demo:rename", &packet(&[("other", ConfigValue::Integer(1))]), 1, 2);
        assert_eq!(missing_field, Err(ProtocolError::BadField("old".to_owned())));
    }

    #[test]
    fn a_two_step_chain_applies_each_rule_in_order() {
        let _serial = serial();
        clear();
        register_version(1, "1.0.0").unwrap();
        register_version(2, "1.1.0").unwrap();
        register_version(3, "1.2.0").unwrap();
        set_version(3);
        register_translator(Rule::new(1, 2, "demo:p", "first").rename("old", "mid"));
        register_translator(Rule::new(2, 3, "demo:p", "second").rename("mid", "new"));

        let data = packet(&[("old", ConfigValue::Integer(7))]);
        let one = translate_packet("demo:p", &data, 1, 2).expect("one step");
        assert!(field(&one, "mid").is_some() && field(&one, "new").is_none());

        let two = translate_packet("demo:p", &data, 1, 3).expect("two steps");
        assert_eq!(field(&two, "new"), Some(&ConfigValue::Integer(7)));
        assert!(field(&two, "old").is_none() && field(&two, "mid").is_none());
        assert_eq!(
            get_translation_report().expect("a report").applied,
            vec!["first".to_owned(), "second".to_owned()]
        );
    }

    #[test]
    fn a_chain_with_a_gap_is_refused_and_names_the_packet() {
        let _serial = serial();
        clear();
        register_version(1, "1.0.0").unwrap();
        register_version(2, "1.1.0").unwrap();
        register_version(3, "1.2.0").unwrap();
        register_translator(Rule::new(1, 2, "demo:p", "first").rename("a", "b"));
        register_translator(Rule::new(2, 3, "demo:other", "other").rename("a", "b"));

        assert_eq!(
            translate_packet("demo:p", &packet(&[("a", ConfigValue::Integer(1))]), 1, 3),
            Err(ProtocolError::MissingTranslation {
                from: 2,
                to: 3,
                packet: "demo:p".to_owned(),
            })
        );
    }

    #[test]
    fn a_dry_run_reports_rule_by_rule_and_leaves_the_packet_byte_identical() {
        let _serial = serial();
        clear();
        register_version(1, "1.0.0").unwrap();
        register_version(2, "1.1.0").unwrap();
        register_version(3, "1.2.0").unwrap();
        register_translator(Rule::new(1, 2, "demo:p", "rename").rename("old", "name"));
        register_translator(Rule::new(2, 3, "demo:p", "add").add("count", ConfigValue::Integer(4)));

        let data = packet(&[("old", ConfigValue::Integer(7))]);
        let before = bytes(&data);
        let preview = dry_run("demo:p", &data, 1, 3).expect("a chain");
        assert_eq!(bytes(&data), before, "the dry run did not move the packet");
        assert_eq!(preview.applied, vec!["rename".to_owned(), "add".to_owned()]);
        assert_eq!(preview.changes.len(), 2);
        assert!(preview.changes[0].fields.iter().any(|field| field == "old -> name"));
        assert!(preview.changes[1].fields.iter().any(|field| field.starts_with("+count")));
        assert!(field(&data, "old").is_some(), "the real packet is intact");
    }

    #[test]
    fn the_report_renders_as_text_and_json_through_serialization() {
        let _serial = serial();
        clear();
        register_version(1, "1.0.0").unwrap();
        register_version(2, "1.1.0").unwrap();
        register_translator(Rule::new(1, 2, "demo:p", "reshape").rename("old", "name"));
        translate_packet("demo:p", &packet(&[("old", ConfigValue::Integer(7))]), 1, 2).expect("a chain");

        let report = get_translation_report().expect("a report");
        let json = report.json();
        assert!(json.contains(r#""applied":["reshape"]"#), "{json}");
        assert!(json.contains(r#""fields":["old -> name"]"#), "{json}");
        assert!(json.contains(r#""packet":"demo:p""#), "{json}");
        assert!(report.text().contains("old -> name"));
    }

    #[test]
    fn an_asymmetric_rule_is_not_walked_backwards() {
        let _serial = serial();
        clear();
        register_version(1, "1.0.0").unwrap();
        register_version(2, "1.1.0").unwrap();
        register_translator(Rule::new(1, 2, "demo:p", "forward only").rename("a", "b"));
        assert!(translate_packet("demo:p", &packet(&[("a", ConfigValue::Integer(1))]), 1, 2).is_ok());
        assert_eq!(
            translate_packet("demo:p", &packet(&[("b", ConfigValue::Integer(1))]), 2, 1),
            Err(ProtocolError::NoPath { from: 2, to: 1 })
        );

        clear();
        register_version(1, "1.0.0").unwrap();
        register_version(2, "1.1.0").unwrap();
        register_translator(
            Rule::new(1, 2, "demo:p", "both ways")
                .rename("a", "b")
                .reverse(Transform::Rename {
                    from: "b".to_owned(),
                    to: "a".to_owned(),
                }),
        );
        let back = translate_packet("demo:p", &packet(&[("b", ConfigValue::Integer(5))]), 2, 1)
            .expect("the rule is reversible");
        assert_eq!(field(&back, "a"), Some(&ConfigValue::Integer(5)));
        assert_eq!(get_compatibility_status(1, 2), Compatibility::Translatable);
    }

    #[test]
    fn an_unsupported_pair_is_refused_and_the_status_says_which() {
        let _serial = serial();
        clear();
        register_version(1, "1.0.0").unwrap();
        register_version(2, "1.1.0").unwrap();
        register_version(3, "1.2.0").unwrap();
        register_translator(Rule::new(1, 2, "demo:p", "first").rename("a", "b"));
        assert_eq!(
            translate_packet("demo:p", &packet(&[("a", ConfigValue::Integer(1))]), 1, 3),
            Err(ProtocolError::NoPath { from: 1, to: 3 })
        );
        assert_eq!(get_compatibility_status(1, 1), Compatibility::Native);
        assert_eq!(get_compatibility_status(1, 2), Compatibility::Translatable);
        assert_eq!(get_compatibility_status(1, 3), Compatibility::Unsupported);
        assert_eq!(get_compatibility_status(9, 1), Compatibility::Unknown);
    }

    #[test]
    fn validate_flags_a_stuck_rule_and_the_client_traffic_is_refused() {
        let _serial = serial();
        clear();
        register_version(1, "1.0.0").unwrap();
        register_translator(Rule::new(1, 1, "demo:p", "stuck"));
        register_mapping(Mapping::new(MapKind::Registry, 1, 7, "demo:id", "demo:renamed"));
        let problems = validate_mappings();
        assert!(
            problems
                .iter()
                .any(|(name, why)| name == "stuck" && why.contains("never moves off protocol 1")),
            "{problems:?}"
        );
        assert!(problems
            .iter()
            .any(|(name, why)| name == "stuck" && why.contains("changes no fields")));
        assert!(problems
            .iter()
            .any(|(name, why)| name == "registry:demo:id" && why.contains("unregistered protocol 7")));

        assert_eq!(refusals().len(), 3);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has a sentence");
            assert_eq!(why_unknown(&what), why);
        }
        assert!(why_unknown("intercept").contains("connection"));
        assert!(!intercept("demo:p"));
        assert!(read_traffic().is_empty());
        assert!(!rewrite_traffic("demo:p", &packet(&[])));
    }
}
