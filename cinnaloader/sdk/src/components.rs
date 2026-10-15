//! Entity components and attributes: the registry a mod can own, and the entity it cannot.
//!
//! Two halves, and the split is worth being precise about because this section is where the
//! wishlist borrows most heavily from an ECS that a Bedrock client does not have.
//!
//! **Real:** a component *type* registry. A mod registers a component with its name, its
//! fields and the values those fields may hold, and then creates, clones, serialises,
//! deserialises and validates component *values* — all of it in this process, over a value
//! tree that renders the way [`crate::config::ConfigValue`] renders. Attributes are the same
//! shape: a value with a base, modifiers and a computed current value, which a mod can
//! register, set, modify and read on a set it owns.
//!
//! **Refused:** attaching a component to a live entity, querying entities for components, and
//! an attribute's value *on an entity*. Those need the client's entity store — section 19's
//! hook, which does not exist.
//!
//! **Not applicable:** `components::register_system`, `unregister_system`, `run_system`,
//! `get_system_order`, `set_system_order` and `attributes::register`/`unregister`. They name
//! an ECS and a system scheduler for mods; a Bedrock client has neither, so there is no
//! target rather than a missing hook.
//!
//! ```
//! use cinnaloader::prelude::*;
//!
//! components::register(ComponentType::new("hello:glow")
//!     .field("colour", ConfigValue::Integer(0xffffff))
//!     .field("strength", ConfigValue::Float(1.0)));
//! let mut values = ComponentSet::new();
//! let value = ComponentData::new(&[("colour", ConfigValue::Integer(0xff0000))]);
//! components::set(&mut values, "hello:glow", value);
//! assert!(components::has(&values, "hello:glow"));
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

use crate::config::{ConfigType, ConfigValue};

/// The same type [`crate::config::ConfigValue`] is, under a name this section reads better
/// with.
pub type ComponentValue = ConfigValue;

/// The value a component holds: named fields, rendered like a config value.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ComponentData {
    fields: BTreeMap<String, ConfigValue>,
}

impl ComponentData {
    /// A component value from `name = value` pairs.
    #[must_use]
    pub fn new(fields: &[(&str, ConfigValue)]) -> Self {
        Self {
            fields: fields
                .iter()
                .map(|(name, value)| ((*name).to_owned(), value.clone()))
                .collect(),
        }
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<ConfigValue> {
        self.fields.get(name).cloned()
    }

    pub fn set(&mut self, name: &str, value: ConfigValue) {
        self.fields.insert(name.to_owned(), value);
    }

    #[must_use]
    pub fn fields(&self) -> Vec<(String, ConfigValue)> {
        self.fields.iter().map(|(name, value)| (name.clone(), value.clone())).collect()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// As text, one line per field, which is how it is written down.
    #[must_use]
    pub fn serialize(&self) -> String {
        self.fields
            .iter()
            .map(|(name, value)| format!("{name}={}", value.render()))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Reads back what [`ComponentData::serialize`] wrote, using a schema to know the types.
    #[must_use]
    pub fn deserialize(text: &str, schema: Option<&ComponentType>) -> Self {
        let mut data = Self::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((name, value)) = line.split_once('=') else {
                continue;
            };
            let name = name.trim();
            let kind = schema
                .and_then(|schema| schema.field_of(name))
                .map_or(ConfigType::String, |field| field.kind());
            if let Some(value) = ConfigValue::parse(value.trim(), kind) {
                data.set(name, value);
            }
        }
        data
    }
}

impl fmt::Display for ComponentData {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.serialize())
    }
}

/// One field of a component.
#[derive(Debug, Clone, PartialEq)]
pub struct ComponentField {
    pub name: String,
    pub default: ConfigValue,
    pub required: bool,
    pub description: String,
}

impl ComponentField {
    #[must_use]
    pub fn new(name: &str, default: ConfigValue) -> Self {
        Self {
            name: name.to_owned(),
            default,
            required: false,
            description: String::new(),
        }
    }

    #[must_use]
    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    #[must_use]
    pub fn described(mut self, description: &str) -> Self {
        self.description = description.to_owned();
        self
    }

    /// The kind of value the field holds, taken from its default.
    #[must_use]
    pub fn kind(&self) -> ConfigType {
        self.default.kind()
    }
}

/// A component type a mod registers: what it is called and what its fields may hold.
#[derive(Debug, Clone, PartialEq)]
pub struct ComponentType {
    pub name: String,
    pub owner: String,
    pub description: String,
    pub fields: Vec<ComponentField>,
    /// Whether a value of this component has to be validated before it is stored.
    pub validated: bool,
}

impl ComponentType {
    /// A component with no fields yet.
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            owner: crate::metadata::id(),
            description: String::new(),
            fields: Vec::new(),
            validated: false,
        }
    }

    #[must_use]
    pub fn described(mut self, description: &str) -> Self {
        self.description = description.to_owned();
        self
    }

    #[must_use]
    pub fn field(mut self, name: &str, default: ConfigValue) -> Self {
        self.fields.retain(|field| field.name != name);
        self.fields
            .push(ComponentField::new(name, default));
        self
    }

    #[must_use]
    pub fn required_field(mut self, name: &str, default: ConfigValue) -> Self {
        self.fields.retain(|field| field.name != name);
        self.fields
            .push(ComponentField::new(name, default).required());
        self
    }

    #[must_use]
    pub fn validated(mut self) -> Self {
        self.validated = true;
        self
    }

    #[must_use]
    pub fn field_of(&self, name: &str) -> Option<&ComponentField> {
        self.fields.iter().find(|field| field.name == name)
    }

    /// A value with every field at its default.
    #[must_use]
    pub fn defaults(&self) -> ComponentData {
        ComponentData {
            fields: self
                .fields
                .iter()
                .map(|field| (field.name.clone(), field.default.clone()))
                .collect(),
        }
    }

    /// What is wrong with a value for this component: missing required fields, unknown
    /// fields, or a field holding the wrong kind of value.
    #[must_use]
    pub fn validate(&self, data: &ComponentData) -> Vec<String> {
        let mut problems = Vec::new();
        for (name, value) in data.fields() {
            match self.field_of(&name) {
                Some(field) if !field.kind().accepts(value.kind()) => problems.push(format!(
                    "{name} holds {}, not {}",
                    value.kind(),
                    field.kind()
                )),
                Some(_) => {}
                None => problems.push(format!("{name} is not a field of {}", self.name)),
            }
        }
        for field in &self.fields {
            if field.required && !data.fields.contains_key(&field.name) {
                problems.push(format!("{} is required and missing", field.name));
            }
        }
        problems
    }

    /// The same, under the name the wishlist uses for a component's schema.
    #[must_use]
    pub fn schema(&self) -> Vec<(String, ConfigType)> {
        self.fields
            .iter()
            .map(|feld| (feld.name.clone(), feld.kind()))
            .collect()
    }
}

impl fmt::Display for ComponentType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ({} field(s))",
            self.name,
            self.fields.len()
        )
    }
}

/// A set of components a mod owns: a value, not an entity's.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ComponentSet {
    components: BTreeMap<String, ComponentData>,
}

impl ComponentSet {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn names(&self) -> Vec<String> {
        self.components.keys().cloned().collect()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.components.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.components.is_empty()
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<ComponentData> {
        self.components.get(name).cloned()
    }

    pub fn insert(&mut self, name: &str, data: ComponentData) {
        self.components.insert(name.to_owned(), data);
    }

    pub fn remove(&mut self, name: &str) -> bool {
        self.components.remove(name).is_some()
    }

    #[must_use]
    pub fn has(&self, name: &str) -> bool {
        self.components.contains_key(name)
    }
}

fn registry() -> &'static Mutex<BTreeMap<String, ComponentType>> {
    static REGISTRY: OnceLock<Mutex<BTreeMap<String, ComponentType>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with<T>(body: impl FnOnce(&mut BTreeMap<String, ComponentType>) -> T) -> T {
    let mut guard = registry().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Registers a component type, or replaces the one with that name.
pub fn register(component: ComponentType) -> bool {
    with(|registry| registry.insert(component.name.clone(), component).is_none())
}

/// Forgets a component type.
pub fn unregister(name: &str) -> bool {
    with(|registry| registry.remove(name).is_some())
}

/// A registered component type.
#[must_use]
pub fn get_schema(name: &str) -> Option<ComponentType> {
    with(|registry| registry.get(name).cloned())
}

/// Every registered component type.
#[must_use]
pub fn get_all() -> Vec<ComponentType> {
    with(|registry| registry.values().cloned().collect())
}

/// A new value for a component: its defaults when it is registered, or a value with no
/// fields when it is not.
#[must_use]
pub fn create(name: &str) -> ComponentData {
    get_schema(name)
        .map(|component| component.defaults())
        .unwrap_or_default()
}

/// A copy of a value.
#[must_use]
pub fn clone_value(data: &ComponentData) -> ComponentData {
    data.clone()
}

/// Writes a value down, one `field=value` per line.
#[must_use]
pub fn serialize(data: &ComponentData) -> String {
    data.serialize()
}

/// Reads a value back, using the registered schema where there is one.
#[must_use]
pub fn deserialize(name: &str, text: &str) -> ComponentData {
    let schema = get_schema(name);
    ComponentData::deserialize(text, schema.as_ref())
}

/// Registers a serializer for a component, which in this host means checking that the value
/// can be written and read back.
pub fn register_serializer(name: &str) -> bool {
    if let Some(component) = get_schema(name) {
        let defaults = component.defaults();
        return ComponentData::deserialize(&defaults.serialize(), Some(&component)) == defaults;
    }
    false
}

/// Registers a validator for a component: values are checked before they are stored.
pub fn register_validator(name: &str) -> bool {
    with(|registry| match registry.get_mut(name) {
        Some(component) => {
            component.validated = true;
            true
        }
        None => false,
    })
}

/// Checks a value against its component's schema, if one is registered.
#[must_use]
pub fn validate(name: &str, data: &ComponentData) -> Vec<String> {
    match get_schema(name) {
        Some(component) => component.validate(data),
        None => vec![format!("{name} is not a registered component")],
    }
}

/// Whether a set of component values has one.
#[must_use]
pub fn has(set: &ComponentSet, name: &str) -> bool {
    set.has(name)
}

/// One component value out of a set.
#[must_use]
pub fn get_value(set: &ComponentSet, name: &str) -> Option<ComponentData> {
    set.get(name)
}

/// Puts a component value into a set, refusing one the schema rejects when the component
/// asked to be validated.
pub fn set(set: &mut ComponentSet, name: &str, data: ComponentData) -> bool {
    if let Some(component) = get_schema(name) {
        if component.validated && !component.validate(&data).is_empty() {
            return false;
        }
    }
    set.insert(name, data);
    true
}

/// Takes a component out of a set.
pub fn remove_from(set: &mut ComponentSet, name: &str) -> bool {
    set.remove(name)
}

/// Every component name in a set.
#[must_use]
pub fn names_in(set: &ComponentSet) -> Vec<String> {
    set.names()
}

/// Why a component question about an entity has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "entity" => {
            "attaching a component to an entity needs the client's entity store, which \
             section 19 refuses for want of a hook"
                .to_owned()
        }
        "query" => "querying entities needs that same store, and there is no entity source here".to_owned(),
        "attribute" => "an attribute's value on an entity is synced to the client, which this host cannot read".to_owned(),
        "write" => "attributes are the server's; the client only reads what it is told".to_owned(),
        "ecs" => {
            "this names an ECS and a system scheduler for mods; a Bedrock client has neither, \
             so there is no target rather than a missing hook"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// One component of a live entity. `None`: see [`why_unknown`].
#[must_use]
pub fn get_on_entity(_entity: &str, _name: &str) -> Option<ComponentData> {
    None
}

/// Attaches a component to a live entity. `false`.
pub fn set_on_entity(_entity: &str, _name: &str, _data: ComponentData) -> bool {
    crate::warn!("a mod asked this host to attach a component: {}", why_unknown("entity"));
    false
}

/// Takes a component off a live entity. `false`.
pub fn remove_from_entity(_entity: &str, _name: &str) -> bool {
    crate::warn!(
        "a mod asked this host to take a component off an entity: {}",
        why_unknown("entity")
    );
    false
}

/// Whether a live entity has a component. `None`.
#[must_use]
pub fn entity_has(_entity: &str, _name: &str) -> Option<bool> {
    None
}

/// Queries entities by component. Empty.
#[must_use]
pub fn query_entities(_name: &str) -> Vec<String> {
    Vec::new()
}

/// Queries the components of an entity. Empty.
#[must_use]
pub fn query_components(_entity: &str) -> Vec<String> {
    Vec::new()
}

/// A system, which this host has no scheduler for. Always `false`.
pub fn register_system(_name: &str) -> bool {
    crate::warn!("a mod asked this host to register a system: {}", why_unknown("ecs"));
    false
}

/// Removes a system. Always `false`.
pub fn unregister_system(_name: &str) -> bool {
    false
}

/// Runs a system. Always `false`.
pub fn run_system(_name: &str) -> bool {
    false
}

/// The order systems run in. Empty.
#[must_use]
pub fn get_system_order() -> Vec<String> {
    Vec::new()
}

/// Sets the order systems run in. Always `false`.
pub fn set_system_order(_names: &[&str]) -> bool {
    false
}

/// An attribute a mod keeps on a set it owns: a base value and the modifiers applied to it.
#[derive(Debug, Clone, PartialEq)]
pub struct Attribute {
    pub name: String,
    pub base: f64,
    pub minimum: f64,
    pub maximum: f64,
    pub modifiers: Vec<AttributeModifier>,
}

/// How a modifier changes an attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModifierKind {
    /// Adds to the base value.
    Add,
    /// Adds to the base value after the additive ones, as `base * amount`.
    Multiply,
    /// The final multiplier, applied last.
    MultiplyTotal,
}

/// One change to an attribute.
#[derive(Debug, Clone, PartialEq)]
pub struct AttributeModifier {
    pub name: String,
    pub kind: ModifierKind,
    pub amount: f64,
}

impl Attribute {
    /// An attribute with a base and no modifiers, clamped between two bounds.
    #[must_use]
    pub fn new(name: &str, base: f64, minimum: f64, maximum: f64) -> Self {
        Self {
            name: name.to_owned(),
            base,
            minimum: minimum.min(maximum),
            maximum: minimum.max(maximum),
            modifiers: Vec::new(),
        }
    }

    /// The value, with the modifiers applied in the order the game applies them: additive
    /// first, then multipliers, then the total multiplier.
    #[must_use]
    pub fn current(&self) -> f64 {
        let mut value = self.base
            + self
                .modifiers
                .iter()
                .filter(|modifier| modifier.kind == ModifierKind::Add)
                .map(|modifier| modifier.amount)
                .sum::<f64>();
        for modifier in self
            .modifiers
            .iter()
            .filter(|modifier| modifier.kind == ModifierKind::Multiply)
        {
            value *= 1.0 + modifier.amount;
        }
        for modifier in self
            .modifiers
            .iter()
            .filter(|modifier| modifier.kind == ModifierKind::MultiplyTotal)
        {
            value *= 1.0 + modifier.amount;
        }
        value.clamp(self.minimum, self.maximum)
    }

    pub fn add_modifier(&mut self, modifier: AttributeModifier) {
        self.modifiers.retain(|held| held.name != modifier.name);
        self.modifiers.push(modifier);
    }

    pub fn remove_modifier(&mut self, name: &str) -> bool {
        let before = self.modifiers.len();
        self.modifiers.retain(|modifier| modifier.name != name);
        self.modifiers.len() != before
    }
}

/// An attribute set a mod owns, keyed by attribute name.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AttributeSet {
    attributes: BTreeMap<String, Attribute>,
}

impl AttributeSet {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<Attribute> {
        self.attributes.get(name).cloned()
    }

    pub fn insert(&mut self, attribute: Attribute) {
        self.attributes.insert(attribute.name.clone(), attribute);
    }

    pub fn remove(&mut self, name: &str) -> bool {
        self.attributes.remove(name).is_some()
    }

    #[must_use]
    pub fn names(&self) -> Vec<String> {
        self.attributes.keys().cloned().collect()
    }
}

/// A registered attribute *definition*, which a mod owns. Registering a new *engine*
/// attribute is not possible: see [`why_unknown`] for `ecs`.
#[derive(Debug, Clone, PartialEq)]
pub struct AttributeDefinition {
    pub name: String,
    pub owner: String,
    pub default: f64,
    pub minimum: f64,
    pub maximum: f64,
}

fn definitions() -> &'static Mutex<BTreeMap<String, AttributeDefinition>> {
    static DEFINITIONS: OnceLock<Mutex<BTreeMap<String, AttributeDefinition>>> = OnceLock::new();
    DEFINITIONS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// This half of the section, under the name the wishlist uses.
pub mod attributes {
    use super::{with_definitions, Attribute, AttributeModifier};

    /// A value in a set a mod owns.
    #[must_use]
    pub fn get(set: &super::AttributeSet, name: &str) -> Option<Attribute> {
        set.get(name)
    }

    /// Puts an attribute into a set.
    pub fn set(set: &mut super::AttributeSet, attribute: Attribute) -> bool {
        set.insert(attribute);
        true
    }

    /// Adds an attribute, or updates the base of one that is already there.
    pub fn add(set: &mut super::AttributeSet, attribute: Attribute) {
        set.insert(attribute);
    }

    /// Removes an attribute.
    pub fn remove(set: &mut super::AttributeSet, name: &str) -> bool {
        set.remove(name)
    }

    /// Changes an attribute's base value.
    pub fn modify(set: &mut super::AttributeSet, name: &str, base: f64) -> bool {
        match set.attributes.get_mut(name) {
            Some(attribute) => {
                attribute.base = base;
                true
            }
            None => false,
        }
    }

    /// An attribute's base value.
    #[must_use]
    pub fn get_base_value(set: &super::AttributeSet, name: &str) -> Option<f64> {
        set.get(name).map(|attribute| attribute.base)
    }

    /// Sets an attribute's base value.
    pub fn set_base_value(set: &mut super::AttributeSet, name: &str, base: f64) -> bool {
        modify(set, name, base)
    }

    /// An attribute's current value, with its modifiers applied.
    #[must_use]
    pub fn get_current_value(set: &super::AttributeSet, name: &str) -> Option<f64> {
        set.get(name).map(|attribute| attribute.current())
    }

    /// An attribute's modifiers.
    #[must_use]
    pub fn get_modifiers(set: &super::AttributeSet, name: &str) -> Vec<AttributeModifier> {
        set.get(name)
            .map(|attribute| attribute.modifiers)
            .unwrap_or_default()
    }

    /// Adds a modifier.
    pub fn add_modifier(set: &mut super::AttributeSet, name: &str, modifier: AttributeModifier) -> bool {
        match set.attributes.get_mut(name) {
            Some(attribute) => {
                attribute.add_modifier(modifier);
                true
            }
            None => false,
        }
    }

    /// Removes a modifier.
    pub fn remove_modifier(set: &mut super::AttributeSet, name: &str, modifier: &str) -> bool {
        match set.attributes.get_mut(name) {
            Some(attribute) => attribute.remove_modifier(modifier),
            None => false,
        }
    }

    /// Registers an attribute *definition*, which a mod owns. This host cannot add an
    /// attribute to the client's engine.
    pub fn register(definition: super::AttributeDefinition) -> bool {
        with_definitions(|definitions| {
            definitions
                .insert(definition.name.clone(), definition)
                .is_none()
        })
    }

    /// Forgets an attribute definition.
    pub fn unregister(name: &str) -> bool {
        with_definitions(|definitions| definitions.remove(name).is_some())
    }

    /// A registered attribute definition.
    #[must_use]
    pub fn definition(name: &str) -> Option<super::AttributeDefinition> {
        with_definitions(|definitions| definitions.get(name).cloned())
    }

    /// Every registered attribute definition.
    #[must_use]
    pub fn definitions() -> Vec<super::AttributeDefinition> {
        with_definitions(|definitions| definitions.values().cloned().collect())
    }

    /// An attribute's value on a live entity. `None`: the client owns it.
    #[must_use]
    pub fn get_live(_entity: &str, _name: &str) -> Option<f64> {
        None
    }

    /// Sets an attribute on a live entity. `false`.
    pub fn set_live(_entity: &str, _name: &str, _value: f64) -> bool {
        super::crate_warn("attribute");
        false
    }
}

/// A helper so the nested module can reach the refusal sentence without a long path.
pub(crate) fn crate_warn(what: &str) {
    crate::warn!("a mod asked this host about a live attribute: {}", why_unknown(what));
}

fn with_definitions<T>(
    body: impl FnOnce(&mut BTreeMap<String, AttributeDefinition>) -> T,
) -> T {
    let mut guard = definitions()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// What this host cannot reach about components and attributes.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["entity", "query", "attribute", "write", "ecs"]
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
        with(|registry| registry.clear());
    }

    fn glow() -> ComponentType {
        ComponentType::new("hello:glow")
            .described("a light a mod puts on its own entity")
            .field("colour", ConfigValue::Integer(0xff_ffff))
            .required_field("strength", ConfigValue::Float(1.0))
    }

    #[test]
    fn a_component_type_is_registered_with_its_fields_and_defaults() {
        let _serial = serial();
        fresh();
        assert!(register(glow()));
        let component = get_schema("hello:glow").expect("registered");
        assert_eq!(component.fields.len(), 2);
        assert_eq!(component.schema().len(), 2);
        assert!(component
            .field_of("strength")
            .is_some_and(|field| field.required));
        assert_eq!(
            component.field_of("strength").map(ComponentField::kind),
            Some(ConfigType::Float)
        );
        let defaults = component.defaults();
        assert_eq!(defaults.get("colour"), Some(ConfigValue::Integer(0xff_ffff)));
        assert!(component.validate(&defaults).is_empty());
        assert!(register_validator("hello:glow"));
        assert!(!register_validator("nothing"));
        assert_eq!(create("hello:glow").fields().len(), 2);
        assert!(create("nothing").is_empty());
        assert!(!register(glow()), "the second registration replaces the first");
        assert!(unregister("hello:glow"));
    }

    #[test]
    fn a_component_value_round_trips_and_is_validated() {
        let _serial = serial();
        fresh();
        register(glow().validated());
        let mut values = ComponentSet::new();
        let mut value = ComponentData::new(&[("colour", ConfigValue::Integer(0x00_ff00))]);
        value.set("strength", ConfigValue::Float(2.0));
        assert!(set(&mut values, "hello:glow", value.clone()));
        assert!(has(&values, "hello:glow"));
        assert_eq!(get_value(&values, "hello:glow"), Some(value.clone()));
        assert_eq!(names_in(&values), vec!["hello:glow".to_owned()]);
        assert_eq!(values.len(), 1);

        // A field the schema does not know, and a field of the wrong kind, are both refused
        // once the component asked to be validated.
        let mut wrong = ComponentData::new(&[("colour", ConfigValue::Text("red".to_owned()))]);
        wrong.set("strength", ConfigValue::Float(1.0));
        assert!(!set(&mut values, "hello:glow", wrong.clone()));
        assert!(!validate("hello:glow", &wrong).is_empty());
        let mut unknown = clone_value(&value);
        unknown.set("nothing", ConfigValue::Integer(1));
        assert!(!validate("hello:glow", &unknown).is_empty());

        let text = serialize(&value);
        assert!(text.contains("colour="), "{text}");
        assert_eq!(deserialize("hello:glow", &text), value);
        assert!(register_serializer("hello:glow"));
        assert!(!register_serializer("nothing"));
        assert!(remove_from(&mut values, "hello:glow"));
        assert!(!remove_from(&mut values, "hello:glow"));
    }

    #[test]
    fn a_required_field_that_is_missing_is_reported() {
        let _serial = serial();
        fresh();
        register(glow());
        let component = get_schema("hello:glow").expect("registered");
        let only_colour = ComponentData::new(&[("colour", ConfigValue::Integer(1))]);
        let problems = component.validate(&only_colour);
        assert!(problems.iter().any(|problem| problem.contains("strength")), "{problems:?}");
    }

    #[test]
    fn a_component_a_mod_owns_is_not_an_entitys() {
        let _serial = serial();
        fresh();
        register(glow());
        assert!(get_on_entity("wisp", "hello:glow").is_none());
        assert!(!set_on_entity("wisp", "hello:glow", ComponentData::default()));
        assert!(!remove_from_entity("wisp", "hello:glow"));
        assert_eq!(entity_has("wisp", "hello:glow"), None);
        assert!(query_entities("hello:glow").is_empty());
        assert!(query_components("wisp").is_empty());
    }

    #[test]
    fn an_attribute_applies_its_modifiers_the_way_the_game_does() {
        let _serial = serial();
        fresh();
        let mut set = AttributeSet::new();
        assert!(attributes::set(
            &mut set,
            Attribute::new("minecraft:movement", 0.25, 0.0, 1.0)
        ));
        assert_eq!(attributes::get(&set, "minecraft:movement").map(|a| a.base), Some(0.25));
        assert_eq!(attributes::get_base_value(&set, "minecraft:movement"), Some(0.25));
        assert_eq!(attributes::get_current_value(&set, "minecraft:movement"), Some(0.25));

        assert!(attributes::add_modifier(
            &mut set,
            "minecraft:movement",
            AttributeModifier {
                name: "boots".to_owned(),
                kind: ModifierKind::Add,
                amount: 0.05,
            }
        ));
        assert_eq!(attributes::get_current_value(&set, "minecraft:movement"), Some(0.3));
        assert!(attributes::add_modifier(
            &mut set,
            "minecraft:movement",
            AttributeModifier {
                name: "swift".to_owned(),
                kind: ModifierKind::Multiply,
                amount: 0.5,
            }
        ));
        assert!((attributes::get_current_value(&set, "minecraft:movement").unwrap() - 0.45).abs() < 1e-9);
        assert_eq!(attributes::get_modifiers(&set, "minecraft:movement").len(), 2);
        assert!(attributes::remove_modifier(&mut set, "minecraft:movement", "swift"));
        assert_eq!(attributes::get_current_value(&set, "minecraft:movement"), Some(0.3));
        assert!(attributes::set_base_value(&mut set, "minecraft:movement", 2.0));
        assert_eq!(
            attributes::get_current_value(&set, "minecraft:movement"),
            Some(1.0),
            "clamped to the maximum"
        );
        assert!(attributes::modify(&mut set, "minecraft:movement", 0.25));
        assert!(!attributes::modify(&mut set, "nothing", 1.0));
        assert!(attributes::remove(&mut set, "minecraft:movement"));
        assert!(!attributes::remove(&mut set, "minecraft:movement"));
    }

    #[test]
    fn an_attribute_definition_is_a_mods_own_and_the_engine_is_refused() {
        let _serial = serial();
        fresh();
        with_definitions(|definitions| definitions.clear());
        assert!(attributes::register(AttributeDefinition {
            name: "hello:focus".to_owned(),
            owner: crate::metadata::id(),
            default: 0.0,
            minimum: 0.0,
            maximum: 10.0,
        }));
        assert_eq!(
            attributes::definition("hello:focus").map(|definition| definition.maximum),
            Some(10.0)
        );
        assert_eq!(attributes::definitions().len(), 1);
        assert!(attributes::unregister("hello:focus"));
        assert!(!attributes::unregister("hello:focus"));
        assert!(attributes::get_live("wisp", "minecraft:movement").is_none());
        assert!(!attributes::set_live("wisp", "minecraft:movement", 0.5));
    }

    #[test]
    fn a_system_is_not_applicable_rather_than_refused_for_want_of_a_hook() {
        let _serial = serial();
        assert!(!register_system("hello:tick"));
        assert!(!unregister_system("hello:tick"));
        assert!(!run_system("hello:tick"));
        assert!(get_system_order().is_empty());
        assert!(!set_system_order(&["a", "b"]));
        assert_eq!(refusals().len(), 5);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
    }

    #[test]
    fn a_component_value_renders_and_reads_back() {
        let value = ComponentData::new(&[
            ("colour", ConfigValue::Integer(0xff0000)),
            ("strength", ConfigValue::Float(1.5)),
        ]);
        let text = value.to_string();
        assert!(text.contains("colour=16711680"), "{text}");
        // Without a schema a written value has no declared kind, so it reads back as text;
        // with one, it comes back as the integer it was.
        assert_eq!(
            ComponentData::deserialize(&text, None).get("colour"),
            Some(ConfigValue::String("16711680".to_owned()))
        );
        register(glow());
        let colour = ComponentType::new("hello:colour")
            .field("colour", ConfigValue::Integer(0))
            .field("strength", ConfigValue::Float(0.0));
        assert_eq!(
            ComponentData::deserialize(&text, Some(&colour)).get("colour"),
            Some(ConfigValue::Integer(0xff0000))
        );
    }
}
