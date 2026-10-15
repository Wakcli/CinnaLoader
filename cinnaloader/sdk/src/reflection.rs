//! Reflection over what a mod declares, and honest refusals for the client's.
//!
//! A mod can describe the types it owns. A [`TypeDescriptor`] names a type, its fields (name,
//! kind, whether each is readable and writable, a default) and its methods (name, arity, return
//! kind), and a registry of them answers [`get_type`], [`get_field`], [`get_method`],
//! [`list_fields`], [`list_methods`] and the metadata lookups beside them. A value of a declared
//! type is a [`ReflectValue`]: a map of field to [`ConfigValue`]. [`get_value`] and [`set_value`]
//! read and write it through the descriptor, enforcing each field's kind and answering a real
//! [`ReflectError`] — an unknown field, a wrong kind, or a read-only field — rather than a
//! silent default. [`invoke`] calls a method by name against an implementation the mod
//! registered, after an arity check.
//!
//! What this host cannot do is reflect into the *client*: its private fields, its classes and
//! objects, and its own reflection. Each refuses with a sentence per kind, reachable through
//! [`refusals`].
//!
//! ```
//! use cinnaloader::config::ConfigValue;
//! use cinnaloader::reflection::{self, TypeDescriptor};
//!
//! reflection::register(TypeDescriptor::new("hello:point")
//!     .field("x", ConfigValue::Integer(0))
//!     .read_only_field("label", ConfigValue::Text("origin".into())));
//! let mut point = reflection::create_instance("hello:point").expect("registered");
//! assert_eq!(reflection::get_value(&point, "x"), Ok(ConfigValue::Integer(0)));
//! assert!(reflection::set_value(&mut point, "label", ConfigValue::Text("moved".into())).is_err());
//! reflection::unregister("hello:point");
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

use crate::config::{ConfigType, ConfigValue};

/// The kind of access being checked: a read or a write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessKind {
    Read,
    Write,
}

impl AccessKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
        }
    }
}

impl fmt::Display for AccessKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One field of a declared type.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldDescriptor {
    pub name: String,
    pub kind: ConfigType,
    pub readable: bool,
    pub writable: bool,
    pub default: ConfigValue,
}

impl FieldDescriptor {
    /// A readable and writable field. The kind comes from the default.
    #[must_use]
    pub fn new(name: &str, default: ConfigValue) -> Self {
        Self {
            name: name.to_owned(),
            kind: default.kind(),
            readable: true,
            writable: true,
            default,
        }
    }

    /// A field a value may be read but not written through.
    #[must_use]
    pub fn read_only(name: &str, default: ConfigValue) -> Self {
        let mut field = Self::new(name, default);
        field.writable = false;
        field
    }

    /// A field a value may be written but not read through.
    #[must_use]
    pub fn write_only(name: &str, default: ConfigValue) -> Self {
        let mut field = Self::new(name, default);
        field.readable = false;
        field
    }
}

/// One method of a declared type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodDescriptor {
    pub name: String,
    pub arity: usize,
    pub return_kind: ConfigType,
}

impl MethodDescriptor {
    #[must_use]
    pub fn new(name: &str, arity: usize, return_kind: ConfigType) -> Self {
        Self {
            name: name.to_owned(),
            arity,
            return_kind,
        }
    }
}

/// A type a mod declares: its name, its fields and its methods.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeDescriptor {
    pub name: String,
    pub owner: String,
    pub description: String,
    pub superclass: Option<String>,
    pub interfaces: Vec<String>,
    pub constructor: Option<usize>,
    pub fields: Vec<FieldDescriptor>,
    pub methods: Vec<MethodDescriptor>,
}

impl TypeDescriptor {
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            owner: crate::metadata::id(),
            description: String::new(),
            superclass: None,
            interfaces: Vec::new(),
            constructor: None,
            fields: Vec::new(),
            methods: Vec::new(),
        }
    }

    #[must_use]
    pub fn described(mut self, description: &str) -> Self {
        self.description = description.to_owned();
        self
    }

    #[must_use]
    pub fn extends(mut self, superclass: &str) -> Self {
        self.superclass = Some(superclass.to_owned());
        self
    }

    #[must_use]
    pub fn implements(mut self, interface: &str) -> Self {
        self.interfaces.push(interface.to_owned());
        self
    }

    #[must_use]
    pub fn constructor(mut self, arity: usize) -> Self {
        self.constructor = Some(arity);
        self
    }

    #[must_use]
    pub fn field(mut self, name: &str, default: ConfigValue) -> Self {
        self.fields.retain(|field| field.name != name);
        self.fields.push(FieldDescriptor::new(name, default));
        self
    }

    #[must_use]
    pub fn read_only_field(mut self, name: &str, default: ConfigValue) -> Self {
        self.fields.retain(|field| field.name != name);
        self.fields.push(FieldDescriptor::read_only(name, default));
        self
    }

    #[must_use]
    pub fn method(mut self, name: &str, arity: usize, return_kind: ConfigType) -> Self {
        self.methods.retain(|method| method.name != name);
        self.methods.push(MethodDescriptor::new(name, arity, return_kind));
        self
    }

    #[must_use]
    pub fn field_of(&self, name: &str) -> Option<&FieldDescriptor> {
        self.fields.iter().find(|field| field.name == name)
    }

    #[must_use]
    pub fn method_of(&self, name: &str) -> Option<&MethodDescriptor> {
        self.methods.iter().find(|method| method.name == name)
    }

    #[must_use]
    pub fn field_names(&self) -> Vec<String> {
        self.fields.iter().map(|field| field.name.clone()).collect()
    }

    #[must_use]
    pub fn method_names(&self) -> Vec<String> {
        self.methods
            .iter()
            .map(|method| method.name.clone())
            .collect()
    }

    /// Every field at its default, which is the value a new instance starts from.
    #[must_use]
    pub fn defaults(&self) -> BTreeMap<String, ConfigValue> {
        self.fields
            .iter()
            .map(|field| (field.name.clone(), field.default.clone()))
            .collect()
    }

    /// The descriptor as text: a header, then one line per field and per method.
    #[must_use]
    pub fn summary(&self) -> String {
        let mut out = format!(
            "{}: {} field(s), {} method(s)",
            self.name,
            self.fields.len(),
            self.methods.len()
        );
        for field in &self.fields {
            out.push_str(&format!(
                "\n  field {}: {} {} default {}",
                field.name,
                field.kind,
                access_str(field),
                field.default.render()
            ));
        }
        for method in &self.methods {
            out.push_str(&format!(
                "\n  method {}({}) -> {}",
                method.name, method.arity, method.return_kind
            ));
        }
        out
    }
}

impl fmt::Display for TypeDescriptor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.summary())
    }
}

fn access_str(field: &FieldDescriptor) -> &'static str {
    match (field.readable, field.writable) {
        (true, true) => "read/write",
        (true, false) => "read-only",
        (false, true) => "write-only",
        (false, false) => "hidden",
    }
}

/// A value of a declared type: its fields and the [`ConfigValue`] each holds.
#[derive(Debug, Clone, PartialEq)]
pub struct ReflectValue {
    type_name: String,
    fields: BTreeMap<String, ConfigValue>,
}

impl ReflectValue {
    #[must_use]
    pub fn new(type_name: &str) -> Self {
        Self {
            type_name: type_name.to_owned(),
            fields: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn type_name(&self) -> &str {
        &self.type_name
    }

    /// Reads a field through the descriptor, refusing an unknown field and a stored value of
    /// the wrong kind.
    pub fn get_field(&self, name: &str) -> Result<ConfigValue, ReflectError> {
        let descriptor = type_of(&self.type_name)?;
        let field = descriptor.field_of(name).ok_or_else(|| ReflectError::UnknownField {
            type_name: self.type_name.clone(),
            field: name.to_owned(),
        })?;
        let found = self
            .fields
            .get(name)
            .cloned()
            .unwrap_or_else(|| field.default.clone());
        if !field.kind.accepts(found.kind()) {
            return Err(ReflectError::WrongKind {
                field: name.to_owned(),
                wanted: field.kind.as_str(),
                found: found.kind().as_str(),
            });
        }
        Ok(found)
    }

    /// Writes a field through the descriptor, refusing an unknown field, a read-only field and
    /// a value of the wrong kind.
    pub fn set_field(&mut self, name: &str, value: ConfigValue) -> Result<(), ReflectError> {
        let descriptor = type_of(&self.type_name)?;
        let field = descriptor.field_of(name).ok_or_else(|| ReflectError::UnknownField {
            type_name: self.type_name.clone(),
            field: name.to_owned(),
        })?;
        if !field.writable {
            return Err(ReflectError::ReadOnly(name.to_owned()));
        }
        if !field.kind.accepts(value.kind()) {
            return Err(ReflectError::WrongKind {
                field: name.to_owned(),
                wanted: field.kind.as_str(),
                found: value.kind().as_str(),
            });
        }
        self.fields.insert(name.to_owned(), value);
        Ok(())
    }

    #[must_use]
    pub fn fields(&self) -> Vec<(String, ConfigValue)> {
        self.fields
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect()
    }

    #[must_use]
    pub fn has(&self, name: &str) -> bool {
        self.fields.contains_key(name)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.fields.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }
}

/// What can go wrong describing or using a declared type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReflectError {
    /// No type with that name is registered.
    UnknownType { type_name: String },
    /// The type is registered but has no such field.
    UnknownField { type_name: String, field: String },
    /// The type is registered but has no such method.
    UnknownMethod { type_name: String, method: String },
    /// A value does not hold a field's declared kind.
    WrongKind {
        field: String,
        wanted: &'static str,
        found: &'static str,
    },
    /// The field is declared read-only and a write was asked for.
    ReadOnly(String),
    /// A method was called with the wrong number of arguments.
    WrongArity {
        method: String,
        wanted: usize,
        found: usize,
    },
    /// The method is declared but no implementation is registered for it.
    NoImplementation { method: String },
}

impl fmt::Display for ReflectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownType { type_name } => {
                write!(formatter, "{type_name} is not a registered type")
            }
            Self::UnknownField { type_name, field } => {
                write!(formatter, "{field} is not a field of {type_name}")
            }
            Self::UnknownMethod { type_name, method } => {
                write!(formatter, "{method} is not a method of {type_name}")
            }
            Self::WrongKind {
                field,
                wanted,
                found,
            } => write!(formatter, "{field} holds {found}, not {wanted}"),
            Self::ReadOnly(field) => write!(formatter, "{field} is read-only"),
            Self::WrongArity {
                method,
                wanted,
                found,
            } => write!(formatter, "{method} takes {wanted} argument(s), got {found}"),
            Self::NoImplementation { method } => {
                write!(formatter, "no implementation is registered for {method}")
            }
        }
    }
}

impl std::error::Error for ReflectError {}

/// A lightweight view of a type, for a caller that wants the shape and not the members.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeMetadata {
    pub name: String,
    pub owner: String,
    pub fields: usize,
    pub methods: usize,
    pub superclass: Option<String>,
    pub interfaces: Vec<String>,
}

/// A lightweight view of a method.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodMetadata {
    pub name: String,
    pub arity: usize,
    pub return_kind: ConfigType,
}

/// A lightweight view of a field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldMetadata {
    pub name: String,
    pub kind: ConfigType,
    pub readable: bool,
    pub writable: bool,
}

/// A named adapter for a type, which a mod declares beside the type itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Adapter {
    pub name: String,
    pub target: String,
}

impl Adapter {
    #[must_use]
    pub fn new(name: &str, target: &str) -> Self {
        Self {
            name: name.to_owned(),
            target: target.to_owned(),
        }
    }
}

/// A named accessor a mod declares for one field of one type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldAccessor {
    pub name: String,
    pub target: String,
    pub field: String,
}

impl FieldAccessor {
    #[must_use]
    pub fn new(name: &str, target: &str, field: &str) -> Self {
        Self {
            name: name.to_owned(),
            target: target.to_owned(),
            field: field.to_owned(),
        }
    }
}

/// An implementation of a method, registered by name against a declared method.
#[derive(Debug, Clone)]
pub struct Invoker {
    pub name: String,
    pub target: String,
    pub method: String,
    call: fn(&[ConfigValue]) -> ConfigValue,
}

impl Invoker {
    #[must_use]
    pub fn new(
        name: &str,
        target: &str,
        method: &str,
        call: fn(&[ConfigValue]) -> ConfigValue,
    ) -> Self {
        Self {
            name: name.to_owned(),
            target: target.to_owned(),
            method: method.to_owned(),
            call,
        }
    }

    #[must_use]
    pub fn call(&self, args: &[ConfigValue]) -> ConfigValue {
        (self.call)(args)
    }
}

/// A resolved symbol: a type, or a field or method of one.
#[derive(Debug, Clone, PartialEq)]
pub enum Symbol {
    Type(TypeDescriptor),
    Field {
        type_name: String,
        field: FieldDescriptor,
    },
    Method {
        type_name: String,
        method: MethodDescriptor,
    },
}

impl Symbol {
    #[must_use]
    pub fn name(&self) -> String {
        match self {
            Self::Type(descriptor) => descriptor.name.clone(),
            Self::Field { type_name, field } => format!("{type_name}.{}", field.name),
            Self::Method { type_name, method } => format!("{type_name}.{}", method.name),
        }
    }
}

#[derive(Default)]
struct Registries {
    types: BTreeMap<String, TypeDescriptor>,
    invokers: BTreeMap<(String, String), Invoker>,
    adapters: BTreeMap<String, Adapter>,
    accessors: BTreeMap<String, FieldAccessor>,
    report: Vec<String>,
}

fn registries() -> &'static Mutex<Registries> {
    static REGISTRIES: OnceLock<Mutex<Registries>> = OnceLock::new();
    REGISTRIES.get_or_init(|| Mutex::new(Registries::default()))
}

fn with<T>(body: impl FnOnce(&mut Registries) -> T) -> T {
    let mut guard = registries()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn type_of(name: &str) -> Result<TypeDescriptor, ReflectError> {
    get_type(name).ok_or_else(|| ReflectError::UnknownType {
        type_name: name.to_owned(),
    })
}

/// Registers a type, or replaces the one with that name. Answers whether it was new.
pub fn register(descriptor: TypeDescriptor) -> bool {
    with(|registries| {
        registries
            .types
            .insert(descriptor.name.clone(), descriptor)
            .is_none()
    })
}

/// Forgets a type.
pub fn unregister(name: &str) -> bool {
    with(|registries| registries.types.remove(name).is_some())
}

/// A registered type.
#[must_use]
pub fn get_type(name: &str) -> Option<TypeDescriptor> {
    with(|registries| registries.types.get(name).cloned())
}

/// A registered type, under the wishlist's `find` name.
#[must_use]
pub fn find(name: &str) -> Option<TypeDescriptor> {
    get_type(name)
}

/// Every registered type's name.
#[must_use]
pub fn list() -> Vec<String> {
    with(|registries| registries.types.keys().cloned().collect())
}

/// A type as text: its header, its fields and its methods.
#[must_use]
pub fn describe(name: &str) -> Option<String> {
    get_type(name).map(|descriptor| descriptor.summary())
}

/// A field of a registered type.
#[must_use]
pub fn get_field(type_name: &str, field: &str) -> Option<FieldDescriptor> {
    get_type(type_name).and_then(|descriptor| descriptor.field_of(field).cloned())
}

/// A field of a registered type, under the wishlist's `property` name.
#[must_use]
pub fn get_property(type_name: &str, property: &str) -> Option<FieldDescriptor> {
    get_field(type_name, property)
}

/// A method of a registered type.
#[must_use]
pub fn get_method(type_name: &str, method: &str) -> Option<MethodDescriptor> {
    get_type(type_name).and_then(|descriptor| descriptor.method_of(method).cloned())
}

/// Every field name of a registered type.
#[must_use]
pub fn list_fields(type_name: &str) -> Vec<String> {
    get_type(type_name)
        .map(|descriptor| descriptor.field_names())
        .unwrap_or_default()
}

/// Every method name of a registered type.
#[must_use]
pub fn list_methods(type_name: &str) -> Vec<String> {
    get_type(type_name)
        .map(|descriptor| descriptor.method_names())
        .unwrap_or_default()
}

/// A registered type's declared superclass.
#[must_use]
pub fn get_superclass(type_name: &str) -> Option<String> {
    get_type(type_name).and_then(|descriptor| descriptor.superclass)
}

/// A registered type's declared interfaces.
#[must_use]
pub fn get_interfaces(type_name: &str) -> Vec<String> {
    get_type(type_name)
        .map(|descriptor| descriptor.interfaces)
        .unwrap_or_default()
}

/// A registered type's declared constructor arity.
#[must_use]
pub fn get_constructor(type_name: &str) -> Option<usize> {
    get_type(type_name).and_then(|descriptor| descriptor.constructor)
}

/// A new value of a declared type, every field at its default.
pub fn create_instance(type_name: &str) -> Result<ReflectValue, ReflectError> {
    let descriptor = type_of(type_name)?;
    let mut value = ReflectValue::new(type_name);
    value.fields = descriptor.defaults();
    Ok(value)
}

/// Reads a field of a value: see [`ReflectValue::get_field`].
pub fn get_value(value: &ReflectValue, field: &str) -> Result<ConfigValue, ReflectError> {
    value.get_field(field)
}

/// Writes a field of a value: see [`ReflectValue::set_field`].
pub fn set_value(
    value: &mut ReflectValue,
    field: &str,
    new: ConfigValue,
) -> Result<(), ReflectError> {
    value.set_field(field, new)
}

/// Calls a declared method against a registered implementation, checking the arity first.
pub fn invoke(
    type_name: &str,
    method: &str,
    args: &[ConfigValue],
) -> Result<ConfigValue, ReflectError> {
    let descriptor = type_of(type_name)?;
    let declared = descriptor
        .method_of(method)
        .ok_or_else(|| ReflectError::UnknownMethod {
            type_name: type_name.to_owned(),
            method: method.to_owned(),
        })?;
    if declared.arity != args.len() {
        return Err(ReflectError::WrongArity {
            method: method.to_owned(),
            wanted: declared.arity,
            found: args.len(),
        });
    }
    let invoker = with(|registries| {
        registries
            .invokers
            .get(&(type_name.to_owned(), method.to_owned()))
            .cloned()
    })
    .ok_or_else(|| ReflectError::NoImplementation {
        method: method.to_owned(),
    })?;
    Ok(invoker.call(args))
}

/// Registers an implementation for a declared method.
pub fn register_invoker(invoker: Invoker) -> bool {
    with(|registries| {
        registries
            .invokers
            .insert((invoker.target.clone(), invoker.method.clone()), invoker)
            .is_none()
    })
}

/// Registers an adapter for a declared type.
pub fn register_adapter(adapter: Adapter) -> bool {
    with(|registries| {
        registries
            .adapters
            .insert(adapter.name.clone(), adapter)
            .is_none()
    })
}

/// Registers an accessor for a declared field.
pub fn register_accessor(accessor: FieldAccessor) -> bool {
    with(|registries| {
        registries
            .accessors
            .insert(accessor.name.clone(), accessor)
            .is_none()
    })
}

/// A registered adapter by name.
#[must_use]
pub fn get_adapter(name: &str) -> Option<Adapter> {
    with(|registries| registries.adapters.get(name).cloned())
}

/// A registered accessor by name.
#[must_use]
pub fn get_accessor(name: &str) -> Option<FieldAccessor> {
    with(|registries| registries.accessors.get(name).cloned())
}

/// A type's metadata: the shape without the members.
#[must_use]
pub fn get_type_metadata(type_name: &str) -> Option<TypeMetadata> {
    get_type(type_name).map(|descriptor| TypeMetadata {
        name: descriptor.name,
        owner: descriptor.owner,
        fields: descriptor.fields.len(),
        methods: descriptor.methods.len(),
        superclass: descriptor.superclass,
        interfaces: descriptor.interfaces,
    })
}

/// A method's metadata.
#[must_use]
pub fn get_method_metadata(type_name: &str, method: &str) -> Option<MethodMetadata> {
    get_method(type_name, method).map(|declared| MethodMetadata {
        name: declared.name,
        arity: declared.arity,
        return_kind: declared.return_kind,
    })
}

/// A field's metadata.
#[must_use]
pub fn get_field_metadata(type_name: &str, field: &str) -> Option<FieldMetadata> {
    get_field(type_name, field).map(|declared| FieldMetadata {
        name: declared.name,
        kind: declared.kind,
        readable: declared.readable,
        writable: declared.writable,
    })
}

/// Resolves `type.member` or a bare type name to a symbol.
#[must_use]
pub fn resolve_symbol(symbol: &str) -> Option<Symbol> {
    let Some((type_name, member)) = symbol.split_once('.') else {
        return get_type(symbol).map(Symbol::Type);
    };
    let descriptor = get_type(type_name)?;
    if let Some(field) = descriptor.field_of(member) {
        return Some(Symbol::Field {
            type_name: type_name.to_owned(),
            field: field.clone(),
        });
    }
    descriptor
        .method_of(member)
        .map(|method| Symbol::Method {
            type_name: type_name.to_owned(),
            method: method.clone(),
        })
}

/// Every type, field and method whose name contains `needle`.
#[must_use]
pub fn find_symbol(needle: &str) -> Vec<Symbol> {
    let mut found = Vec::new();
    for descriptor in get_type_all() {
        if descriptor.name.contains(needle) {
            found.push(Symbol::Type(descriptor.clone()));
        }
        for field in &descriptor.fields {
            if field.name.contains(needle) {
                found.push(Symbol::Field {
                    type_name: descriptor.name.clone(),
                    field: field.clone(),
                });
            }
        }
        for method in &descriptor.methods {
            if method.name.contains(needle) {
                found.push(Symbol::Method {
                    type_name: descriptor.name.clone(),
                    method: method.clone(),
                });
            }
        }
    }
    found
}

/// Every registered type, as descriptors.
#[must_use]
pub fn get_type_all() -> Vec<TypeDescriptor> {
    with(|registries| registries.types.values().cloned().collect())
}

/// Whether a field may be read or written, recording the answer in the access report.
pub fn validate_access(type_name: &str, field: &str, access: AccessKind) -> Vec<String> {
    let mut problems = Vec::new();
    match get_type(type_name) {
        None => problems.push(format!("{type_name} is not a registered type")),
        Some(descriptor) => match descriptor.field_of(field) {
            None => problems.push(format!("{field} is not a field")),
            Some(declared) => match access {
                AccessKind::Read if !declared.readable => {
                    problems.push(format!("{field} is not readable"))
                }
                AccessKind::Write if !declared.writable => {
                    problems.push(format!("{field} is read-only"))
                }
                _ => {}
            },
        },
    }
    let line = match problems.first() {
        Some(reason) => format!(
            "{type_name}.{field} {} refused: {reason}",
            access.as_str()
        ),
        None => format!("{type_name}.{field} {} ok", access.as_str()),
    };
    with(|registries| registries.report.push(line));
    problems
}

/// Every line the last [`validate_access`] calls recorded.
#[must_use]
pub fn get_access_report() -> Vec<String> {
    with(|registries| registries.report.clone())
}

/// Forgets the access report, and answers how many lines it held.
pub fn clear_access_report() -> usize {
    with(|registries| {
        let count = registries.report.len();
        registries.report.clear();
        count
    })
}

/// Why a reflection question about the client has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "private" => {
            "a client field is private to a running client, behind a hook the project records \
             as never"
                .to_owned()
        }
        "class" => {
            "the client's classes and objects are its runtime's own; this host has no handle \
             on them"
                .to_owned()
        }
        "reflection" => {
            "reading the client's own reflection means walking its metadata, which needs the \
             same missing hook"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// A private field of the client. `None`: see [`why_unknown`].
#[must_use]
pub fn get_private_field(_type_name: &str, _field: &str) -> Option<ConfigValue> {
    crate::warn!(
        "a mod asked for a private client field: {}",
        why_unknown("private")
    );
    None
}

/// Writes a private field of the client. `false`.
pub fn set_private_field(_type_name: &str, _field: &str, _value: ConfigValue) -> bool {
    crate::warn!(
        "a mod asked to write a private client field: {}",
        why_unknown("private")
    );
    false
}

/// A class of the client by name. `None`.
#[must_use]
pub fn get_client_class(_path: &str) -> Option<String> {
    crate::warn!(
        "a mod asked for a client class: {}",
        why_unknown("class")
    );
    None
}

/// An object of the client by name. `None`.
#[must_use]
pub fn get_client_object(_path: &str) -> Option<String> {
    crate::warn!(
        "a mod asked for a client object: {}",
        why_unknown("class")
    );
    None
}

/// The client's own reflection for a type. `None`.
#[must_use]
pub fn get_client_reflection(_type_name: &str) -> Option<TypeDescriptor> {
    crate::warn!(
        "a mod asked for the client's own reflection: {}",
        why_unknown("reflection")
    );
    None
}

/// A member of the client's own reflection. `None`.
#[must_use]
pub fn get_client_member(_type_name: &str, _member: &str) -> Option<String> {
    crate::warn!(
        "a mod asked for a member of the client's reflection: {}",
        why_unknown("reflection")
    );
    None
}

/// What this host cannot reach about reflection, for a panel or a report.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["private", "class", "reflection"]
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
        with(|registries| {
            registries.types.clear();
            registries.invokers.clear();
            registries.adapters.clear();
            registries.accessors.clear();
            registries.report.clear();
        });
    }

    fn point() -> TypeDescriptor {
        TypeDescriptor::new("hello:point")
            .described("a point a mod owns")
            .extends("hello:shape")
            .implements("hello:movable")
            .constructor(2)
            .field("x", ConfigValue::Integer(0))
            .field("y", ConfigValue::Integer(0))
            .read_only_field("label", ConfigValue::Text("origin".into()))
            .method("move_by", 2, ConfigType::Integer)
    }

    fn sum_args(args: &[ConfigValue]) -> ConfigValue {
        ConfigValue::Integer(
            args.iter()
                .filter_map(ConfigValue::as_integer)
                .sum::<i64>(),
        )
    }

    #[test]
    fn a_declared_type_registers_with_its_fields_and_methods() {
        let _serial = serial();
        fresh();
        assert!(register(point()));
        let descriptor = get_type("hello:point").expect("registered");
        assert_eq!(descriptor.fields.len(), 3);
        assert_eq!(descriptor.methods.len(), 1);
        assert_eq!(list(), vec!["hello:point".to_owned()]);
        assert_eq!(find("hello:point"), Some(descriptor.clone()));
        assert_eq!(list_fields("hello:point"), vec!["x".to_owned(), "y".to_owned(), "label".to_owned()]);
        assert_eq!(list_methods("hello:point"), vec!["move_by".to_owned()]);
        assert_eq!(get_superclass("hello:point"), Some("hello:shape".to_owned()));
        assert_eq!(get_interfaces("hello:point"), vec!["hello:movable".to_owned()]);
        assert_eq!(get_constructor("hello:point"), Some(2));
        assert_eq!(
            describe("hello:point"),
            Some(
                [
                    "hello:point: 3 field(s), 1 method(s)",
                    "  field x: integer read/write default 0",
                    "  field y: integer read/write default 0",
                    "  field label: text read-only default origin",
                    "  method move_by(2) -> integer",
                ]
                .join("\n")
            )
        );
        assert_eq!(
            get_field("hello:point", "label").map(|field| field.writable),
            Some(false)
        );
        assert_eq!(
            get_property("hello:point", "x").map(|field| field.kind),
            Some(ConfigType::Integer)
        );
        assert_eq!(
            get_method("hello:point", "move_by").map(|method| method.arity),
            Some(2)
        );
        assert!(!register(point()), "the second registration replaces the first");
        assert!(unregister("hello:point"));
        assert!(!unregister("hello:point"));
    }

    #[test]
    fn building_and_reading_a_value_uses_the_declared_kinds() {
        let _serial = serial();
        fresh();
        register(point());
        let mut value = create_instance("hello:point").expect("registered");
        assert_eq!(value.len(), 3);
        assert_eq!(get_value(&value, "x"), Ok(ConfigValue::Integer(0)));
        assert_eq!(get_value(&value, "label"), Ok(ConfigValue::Text("origin".to_owned())));
        assert!(set_value(&mut value, "x", ConfigValue::Integer(5)).is_ok());
        assert_eq!(get_value(&value, "x"), Ok(ConfigValue::Integer(5)));
        assert!(value.has("x"));
        assert_eq!(value.type_name(), "hello:point");
        assert_eq!(value.fields().len(), 3);
        let metadata = get_type_metadata("hello:point").expect("registered");
        assert_eq!(metadata.fields, 3);
        assert_eq!(metadata.methods, 1);
        assert_eq!(
            get_method_metadata("hello:point", "move_by").map(|method| method.arity),
            Some(2)
        );
        assert_eq!(
            get_field_metadata("hello:point", "label").map(|field| field.writable),
            Some(false)
        );
    }

    #[test]
    fn an_unknown_type_or_field_is_named() {
        let _serial = serial();
        fresh();
        register(point());
        assert_eq!(
            create_instance("nothing:here"),
            Err(ReflectError::UnknownType {
                type_name: "nothing:here".to_owned(),
            })
        );
        let mut value = create_instance("hello:point").expect("registered");
        assert_eq!(
            get_value(&value, "z"),
            Err(ReflectError::UnknownField {
                type_name: "hello:point".to_owned(),
                field: "z".to_owned(),
            })
        );
        assert_eq!(
            set_value(&mut value, "z", ConfigValue::Integer(1)),
            Err(ReflectError::UnknownField {
                type_name: "hello:point".to_owned(),
                field: "z".to_owned(),
            })
        );
        assert_eq!(
            invoke("hello:point", "nope", &[]),
            Err(ReflectError::UnknownMethod {
                type_name: "hello:point".to_owned(),
                method: "nope".to_owned(),
            })
        );
    }

    #[test]
    fn a_value_of_the_wrong_kind_is_refused_with_both_kinds() {
        let _serial = serial();
        fresh();
        register(point());
        let mut value = create_instance("hello:point").expect("registered");
        assert_eq!(
            set_value(&mut value, "x", ConfigValue::Text("five".to_owned())),
            Err(ReflectError::WrongKind {
                field: "x".to_owned(),
                wanted: "integer",
                found: "text",
            })
        );
        // A stored value of the wrong kind is caught on the read too.
        value.fields.insert("x".to_owned(), ConfigValue::Text("five".to_owned()));
        assert_eq!(
            get_value(&value, "x"),
            Err(ReflectError::WrongKind {
                field: "x".to_owned(),
                wanted: "integer",
                found: "text",
            })
        );
    }

    #[test]
    fn a_read_only_field_refuses_a_write() {
        let _serial = serial();
        fresh();
        register(point());
        let mut value = create_instance("hello:point").expect("registered");
        assert_eq!(
            set_value(&mut value, "label", ConfigValue::Text("moved".to_owned())),
            Err(ReflectError::ReadOnly("label".to_owned()))
        );
        assert_eq!(
            get_value(&value, "label"),
            Ok(ConfigValue::Text("origin".to_owned()))
        );
    }

    #[test]
    fn a_method_is_invoked_by_name_and_a_wrong_arity_is_refused() {
        let _serial = serial();
        fresh();
        register(point());
        assert!(register_invoker(Invoker::new(
            "hello:sum",
            "hello:point",
            "move_by",
            sum_args
        )));
        assert_eq!(
            invoke(
                "hello:point",
                "move_by",
                &[ConfigValue::Integer(3), ConfigValue::Integer(4)]
            ),
            Ok(ConfigValue::Integer(7))
        );
        assert_eq!(
            invoke("hello:point", "move_by", &[ConfigValue::Integer(3)]),
            Err(ReflectError::WrongArity {
                method: "move_by".to_owned(),
                wanted: 2,
                found: 1,
            })
        );
        register(
            TypeDescriptor::new("hello:silent").method("run", 0, ConfigType::Boolean),
        );
        assert_eq!(
            invoke("hello:silent", "run", &[]),
            Err(ReflectError::NoImplementation {
                method: "run".to_owned(),
            })
        );
    }

    #[test]
    fn symbols_resolve_and_find_over_the_registry() {
        let _serial = serial();
        fresh();
        register(point());
        assert_eq!(
            resolve_symbol("hello:point").map(|symbol| symbol.name()),
            Some("hello:point".to_owned())
        );
        assert_eq!(
            resolve_symbol("hello:point.x").map(|symbol| symbol.name()),
            Some("hello:point.x".to_owned())
        );
        assert_eq!(
            resolve_symbol("hello:point.move_by").map(|symbol| symbol.name()),
            Some("hello:point.move_by".to_owned())
        );
        assert!(resolve_symbol("hello:point.z").is_none());
        assert!(resolve_symbol("nothing:here").is_none());
        let found = find_symbol("x");
        assert_eq!(
            found.iter().map(Symbol::name).collect::<Vec<_>>(),
            vec!["hello:point.x".to_owned()]
        );
        assert_eq!(get_type_all().len(), 1);
    }

    #[test]
    fn the_client_is_refused_with_a_reason_and_access_is_validated() {
        let _serial = serial();
        fresh();
        register(point());
        assert!(get_private_field("hello:point", "x").is_none());
        assert!(!set_private_field("hello:point", "x", ConfigValue::Integer(1)));
        assert!(get_client_class("hello:point").is_none());
        assert!(get_client_object("hello:point").is_none());
        assert!(get_client_reflection("hello:point").is_none());
        assert!(get_client_member("hello:point", "x").is_none());
        assert_eq!(refusals().len(), 3);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
        assert!(validate_access("hello:point", "x", AccessKind::Read).is_empty());
        assert!(validate_access("hello:point", "x", AccessKind::Write).is_empty());
        assert_eq!(
            validate_access("hello:point", "label", AccessKind::Write),
            vec!["label is read-only".to_owned()]
        );
        assert_eq!(
            validate_access("hello:point", "z", AccessKind::Read),
            vec!["z is not a field".to_owned()]
        );
        assert_eq!(
            get_access_report(),
            vec![
                "hello:point.x read ok".to_owned(),
                "hello:point.x write ok".to_owned(),
                "hello:point.label write refused: label is read-only".to_owned(),
                "hello:point.z read refused: z is not a field".to_owned(),
            ]
        );
        assert_eq!(clear_access_report(), 4);
        assert!(register_adapter(Adapter::new("hello:point_adapter", "hello:point")));
        assert!(!register_adapter(Adapter::new("hello:point_adapter", "hello:point")));
        assert_eq!(
            get_adapter("hello:point_adapter").map(|adapter| adapter.target),
            Some("hello:point".to_owned())
        );
        assert!(register_accessor(FieldAccessor::new("hello:x", "hello:point", "x")));
        assert_eq!(
            get_accessor("hello:x").map(|accessor| accessor.field),
            Some("x".to_owned())
        );
    }
}
