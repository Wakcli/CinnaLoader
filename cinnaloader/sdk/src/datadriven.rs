//! Data-driven systems: documents a mod loads, the schemas that say what they hold, and the
//! registries they feed.
//!
//! A [`Document`] is an id, the kind of registry it feeds, its content as a [`ConfigValue`],
//! and the ids it depends on. A [`DocumentSchema`] per kind says which fields are required,
//! which are optional with a default, and what kind each holds; [`validate`] answers every
//! problem it finds with the field's path and the reason, not just the first. [`load`]
//! validates a batch, orders it so a document that names a parent comes after it, refuses a
//! cycle by naming it, and registers each document's entries in the [`crate::registry`] its
//! kind names. [`reload`] and [`unload`] touch exactly one document's content.
//!
//! What this host cannot reach is the client's own data pack documents: reading one, or
//! writing content the client would load. Those live in [`known_only_to_the_client`],
//! refused with their reasons and a warning.
//!
//! ```
//! use cinnaloader::config::{ConfigType, ConfigValue};
//! use cinnaloader::datadriven::{self, Document, DocumentSchema};
//!
//! datadriven::register_schema(DocumentSchema::new("item").required("stack", ConfigType::Integer));
//! let content = ConfigValue::Object(
//!     [("stack".to_owned(), ConfigValue::Integer(64))].into_iter().collect(),
//! );
//! let doc = Document::new("hello:gem", "item", content.clone());
//! assert_eq!(datadriven::load(&[doc]).unwrap(), vec!["hello:gem".to_owned()]);
//! assert_eq!(datadriven::get("item", "hello:gem"), Some(content));
//! assert_eq!(datadriven::count("item"), 1);
//! assert!(datadriven::unload("hello:gem"));
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

use crate::config::{ConfigType, ConfigValue};

/// One problem with a document's content: where it is and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Validation {
    pub path: String,
    pub reason: String,
}

impl fmt::Display for Validation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.path, self.reason)
    }
}

/// One field a schema says a document of its kind holds.
#[derive(Debug, Clone, PartialEq)]
pub struct SchemaField {
    pub name: String,
    pub kind: ConfigType,
    pub required: bool,
    pub default: Option<ConfigValue>,
    /// The fields of a map/object/compound field, checked inside it.
    pub fields: Vec<SchemaField>,
    /// The shape every element of a list/set field has.
    pub element: Option<Box<SchemaField>>,
}

impl Default for SchemaField {
    fn default() -> Self {
        Self {
            name: String::new(),
            kind: ConfigType::String,
            required: false,
            default: None,
            fields: Vec::new(),
            element: None,
        }
    }
}

impl SchemaField {
    #[must_use]
    pub fn new(name: &str, kind: ConfigType) -> Self {
        Self {
            name: name.to_owned(),
            kind,
            ..Self::default()
        }
    }

    #[must_use]
    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    #[must_use]
    pub fn with_default(mut self, value: ConfigValue) -> Self {
        self.default = Some(value);
        self
    }

    #[must_use]
    pub fn field(mut self, child: SchemaField) -> Self {
        self.fields.push(child);
        self
    }

    #[must_use]
    pub fn element(mut self, child: SchemaField) -> Self {
        self.element = Some(Box::new(child));
        self
    }
}

/// What a document of one kind must look like.
#[derive(Debug, Clone, PartialEq)]
pub struct DocumentSchema {
    pub kind: String,
    pub version: u64,
    pub fields: Vec<SchemaField>,
}

impl DocumentSchema {
    #[must_use]
    pub fn new(kind: &str) -> Self {
        Self {
            kind: kind.to_owned(),
            version: 1,
            fields: Vec::new(),
        }
    }

    #[must_use]
    pub fn version(mut self, version: u64) -> Self {
        self.version = version;
        self
    }

    #[must_use]
    pub fn field(mut self, field: SchemaField) -> Self {
        self.fields.push(field);
        self
    }

    #[must_use]
    pub fn required(mut self, name: &str, kind: ConfigType) -> Self {
        self.fields.push(SchemaField::new(name, kind).required());
        self
    }

    #[must_use]
    pub fn optional(mut self, name: &str, kind: ConfigType, default: ConfigValue) -> Self {
        self.fields.push(SchemaField::new(name, kind).with_default(default));
        self
    }
}

/// A document a mod loads: an id, the registry kind it feeds, its content and its parents.
#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    pub id: String,
    pub kind: String,
    pub version: u64,
    pub content: ConfigValue,
    pub depends_on: Vec<String>,
}

impl Document {
    #[must_use]
    pub fn new(id: &str, kind: &str, content: ConfigValue) -> Self {
        Self {
            id: id.to_owned(),
            kind: kind.to_owned(),
            version: 1,
            content,
            depends_on: Vec::new(),
        }
    }

    #[must_use]
    pub fn version(mut self, version: u64) -> Self {
        self.version = version;
        self
    }

    #[must_use]
    pub fn depends_on(mut self, id: &str) -> Self {
        self.depends_on.push(id.to_owned());
        self
    }

    /// Reads a document's content through the codec registered for `format`.
    pub fn from_text(id: &str, kind: &str, format: &str, text: &str) -> Result<Self, DocumentError> {
        let content = decode(format, text).map_err(|reason| DocumentError::Decode {
            format: format.to_owned(),
            reason,
        })?;
        Ok(Self::new(id, kind, content))
    }
}

/// A reference a document's content makes to another id, and whether it is loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    pub target: String,
    pub found: bool,
}

/// Why a load, reload or decode was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentError {
    EmptyId,
    Duplicate(String),
    UnknownSchema(String),
    MissingParent { id: String, parent: String },
    Invalid { id: String, problems: Vec<Validation> },
    Cycle(Vec<String>),
    NoMigration { kind: String, from: u64, to: u64 },
    Rejected { id: String, reason: String },
    UnknownCodec(String),
    Decode { format: String, reason: String },
    Unknown(String),
}

impl fmt::Display for DocumentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyId => formatter.write_str("a document id cannot be empty"),
            Self::Duplicate(id) => write!(formatter, "{id} is already loaded"),
            Self::UnknownSchema(kind) => write!(formatter, "no schema is registered for kind '{kind}'"),
            Self::MissingParent { id, parent } => {
                write!(formatter, "'{id}' depends on '{parent}', which is not loaded")
            }
            Self::Invalid { id, problems } => {
                write!(formatter, "'{id}' is invalid: {}", join(problems))
            }
            Self::Cycle(path) => write!(formatter, "cycle: {}", path.join(" -> ")),
            Self::NoMigration { kind, from, to } => {
                write!(formatter, "no migration of kind '{kind}' from version {from} to {to}")
            }
            Self::Rejected { id, reason } => write!(formatter, "processor refused '{id}': {reason}"),
            Self::UnknownCodec(format) => write!(formatter, "no codec is registered for '{format}'"),
            Self::Decode { format, reason } => {
                write!(formatter, "'{format}' did not decode: {reason}")
            }
            Self::Unknown(id) => write!(formatter, "there is no document '{id}'"),
        }
    }
}

impl std::error::Error for DocumentError {}

fn join(problems: &[Validation]) -> String {
    problems
        .iter()
        .map(Validation::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

type Codec = fn(&str) -> Result<ConfigValue, String>;
type Loader = fn(&Document) -> Result<Vec<(String, ConfigValue)>, Vec<Validation>>;
type Processor = fn(&str, &ConfigValue) -> Result<ConfigValue, String>;
type Migration = fn(&ConfigValue) -> ConfigValue;
type WatcherBody = Box<dyn FnMut(&str, &str) + Send>;

struct Watcher {
    kind: String,
    id: u64,
    body: WatcherBody,
}

struct Loaded {
    document: Document,
    entries: Vec<String>,
}

#[derive(Default)]
struct Store {
    schemas: BTreeMap<String, DocumentSchema>,
    loaded: BTreeMap<String, Loaded>,
    codecs: BTreeMap<String, Codec>,
    loaders: BTreeMap<String, Loader>,
    processors: BTreeMap<String, Processor>,
    migrations: Vec<(String, u64, u64, Migration)>,
    watchers: Vec<Watcher>,
    errors: Vec<String>,
    next_watch: u64,
}

fn store() -> &'static Mutex<Store> {
    static STORE: OnceLock<Mutex<Store>> = OnceLock::new();
    STORE.get_or_init(|| {
        let mut store = Store::default();
        store.codecs.insert("json".to_owned(), json_codec);
        store.codecs.insert("text".to_owned(), text_codec);
        Mutex::new(store)
    })
}

fn with<T>(body: impl FnOnce(&mut Store) -> T) -> T {
    let mut guard = store().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn json_codec(text: &str) -> Result<ConfigValue, String> {
    crate::serialization::from_json(text)
        .map(from_serialized)
        .map_err(|error| error.to_string())
}

fn text_codec(text: &str) -> Result<ConfigValue, String> {
    crate::serialization::from_text(text)
        .map(from_serialized)
        .map_err(|error| error.to_string())
}

fn from_serialized(value: crate::serialization::Value) -> ConfigValue {
    use crate::serialization::Value;
    match value {
        Value::Null => ConfigValue::Text(String::new()),
        Value::Bool(flag) => ConfigValue::Boolean(flag),
        Value::Int(number) => ConfigValue::Integer(number),
        Value::Float(number) => ConfigValue::Float(number),
        Value::Str(text) => ConfigValue::Text(text),
        Value::List(items) => ConfigValue::List(items.into_iter().map(from_serialized).collect()),
        Value::Map(map) => ConfigValue::Object(
            map.iter()
                .map(|(key, value)| (key.to_owned(), from_serialized(value.clone())))
                .collect(),
        ),
    }
}

/// Registers the schema for a kind. The last call for a kind wins.
pub fn register_schema(schema: DocumentSchema) -> bool {
    if schema.kind.is_empty() {
        return false;
    }
    with(|store| {
        store.schemas.insert(schema.kind.clone(), schema);
        true
    })
}

/// The schema registered for a kind, if any.
#[must_use]
pub fn get_schema(kind: &str) -> Option<DocumentSchema> {
    with(|store| store.schemas.get(kind).cloned())
}

/// Every problem with `content` for `kind`, each with its path and reason.
#[must_use]
pub fn validate(kind: &str, content: &ConfigValue) -> Vec<Validation> {
    let schema = with(|store| store.schemas.get(kind).cloned());
    let Some(schema) = schema else {
        return vec![Validation {
            path: kind.to_owned(),
            reason: "no schema is registered for this kind".to_owned(),
        }];
    };
    validate_against(&schema, content)
}

fn validate_against(schema: &DocumentSchema, content: &ConfigValue) -> Vec<Validation> {
    let mut problems = Vec::new();
    validate_fields(&schema.kind, &schema.fields, content, &mut problems);
    problems
}

fn validate_fields(kind: &str, fields: &[SchemaField], value: &ConfigValue, out: &mut Vec<Validation>) {
    let Some(map) = value.as_map() else {
        out.push(Validation {
            path: kind.to_owned(),
            reason: format!("expected object, found {}", value.kind().as_str()),
        });
        return;
    };
    for field in fields {
        let path = format!("{kind}.{}", field.name);
        match map.get(&field.name) {
            Some(held) => check_field(&path, field, held, out),
            None if field.required => out.push(Validation {
                path,
                reason: "required, but missing".to_owned(),
            }),
            None => {}
        }
    }
}

fn check_field(path: &str, field: &SchemaField, value: &ConfigValue, out: &mut Vec<Validation>) {
    let mismatch = |field: &SchemaField, value: &ConfigValue| Validation {
        path: path.to_owned(),
        reason: format!("expected {}, found {}", field.kind.as_str(), value.kind().as_str()),
    };
    match field.kind {
        ConfigType::List | ConfigType::Set => match &field.element {
            Some(element) => match value.as_list() {
                Some(items) => {
                    for (index, item) in items.iter().enumerate() {
                        check_field(&format!("{path}[{index}]"), element, item, out);
                    }
                }
                None => out.push(mismatch(field, value)),
            },
            None => {
                if !field.kind.accepts(value.kind()) {
                    out.push(mismatch(field, value));
                }
            }
        },
        ConfigType::Map | ConfigType::Object | ConfigType::Compound if !field.fields.is_empty() => {
            if value.as_map().is_some() {
                validate_fields(path, &field.fields, value, out);
            } else {
                out.push(mismatch(field, value));
            }
        }
        _ => {
            if !field.kind.accepts(value.kind()) {
                out.push(mismatch(field, value));
            }
        }
    }
}

fn fill_defaults(content: &mut ConfigValue, fields: &[SchemaField]) {
    let Some(map) = object_mut(content) else {
        return;
    };
    for field in fields {
        if let Some(default) = &field.default {
            map.entry(field.name.clone()).or_insert_with(|| default.clone());
        }
    }
}

fn object_mut(value: &mut ConfigValue) -> Option<&mut BTreeMap<String, ConfigValue>> {
    match value {
        ConfigValue::Map(map) | ConfigValue::Object(map) | ConfigValue::Compound(map) => Some(map),
        _ => None,
    }
}

/// Registers a codec for a format name: its text to a [`ConfigValue`].
pub fn register_codec(format: &str, codec: fn(&str) -> Result<ConfigValue, String>) -> bool {
    if format.is_empty() {
        return false;
    }
    with(|store| {
        store.codecs.insert(format.to_owned(), codec);
        true
    })
}

/// Reads text as a [`ConfigValue`] through the codec registered for `format`.
pub fn decode(format: &str, text: &str) -> Result<ConfigValue, String> {
    let codec = with(|store| store.codecs.get(format).copied());
    let Some(codec) = codec else {
        return Err(format!("no codec is registered for '{format}'"));
    };
    codec(text)
}

/// Registers a loader for a kind: how its documents turn into registry entries.
pub fn register_loader(kind: &str, loader: Loader) -> bool {
    if kind.is_empty() {
        return false;
    }
    with(|store| {
        store.loaders.insert(kind.to_owned(), loader);
        true
    })
}

/// Registers a processor for a kind, run on each entry a loader produced.
pub fn register_processor(
    kind: &str,
    processor: fn(&str, &ConfigValue) -> Result<ConfigValue, String>,
) -> bool {
    if kind.is_empty() {
        return false;
    }
    with(|store| {
        store.processors.insert(kind.to_owned(), processor);
        true
    })
}

/// Registers a migration of a kind from one schema version to the next.
pub fn register_migration(kind: &str, from: u64, to: u64, migration: fn(&ConfigValue) -> ConfigValue) -> bool {
    if kind.is_empty() || from >= to {
        return false;
    }
    with(|store| {
        store.migrations.push((kind.to_owned(), from, to, migration));
        true
    })
}

fn prepare(store: &Store, document: &Document) -> Result<ConfigValue, DocumentError> {
    let schema = store
        .schemas
        .get(&document.kind)
        .cloned()
        .ok_or_else(|| DocumentError::UnknownSchema(document.kind.clone()))?;
    let mut content = document.content.clone();
    if document.version != schema.version {
        let migration = store
            .migrations
            .iter()
            .find(|(kind, from, to, _)| {
                kind == &document.kind && *from == document.version && *to == schema.version
            })
            .map(|(_, _, _, migration)| *migration);
        let Some(migration) = migration else {
            return Err(DocumentError::NoMigration {
                kind: document.kind.clone(),
                from: document.version,
                to: schema.version,
            });
        };
        content = migration(&content);
    }
    fill_defaults(&mut content, &schema.fields);
    let problems = validate_against(&schema, &content);
    if !problems.is_empty() {
        return Err(DocumentError::Invalid {
            id: document.id.clone(),
            problems,
        });
    }
    Ok(content)
}

fn entries_for(store: &Store, document: &Document, content: &ConfigValue) -> Result<Vec<(String, ConfigValue)>, DocumentError> {
    let loader = store.loaders.get(&document.kind).copied();
    let raw = match loader {
        Some(loader) => loader(document).map_err(|problems| DocumentError::Invalid {
            id: document.id.clone(),
            problems,
        })?,
        None => vec![(document.id.clone(), content.clone())],
    };
    let mut out = Vec::with_capacity(raw.len());
    for (id, value) in raw {
        let value = match store.processors.get(&document.kind).copied() {
            Some(processor) => processor(&id, &value).map_err(|reason| DocumentError::Rejected {
                id: id.clone(),
                reason,
            })?,
            None => value,
        };
        out.push((id, value));
    }
    Ok(out)
}

fn order_of(store: &Store, documents: &[Document]) -> Result<Vec<String>, DocumentError> {
    let batch: BTreeMap<&str, &Document> = documents
        .iter()
        .map(|document| (document.id.as_str(), document))
        .collect();
    for document in documents {
        for parent in &document.depends_on {
            if !batch.contains_key(parent.as_str()) && !store.loaded.contains_key(parent) {
                return Err(DocumentError::MissingParent {
                    id: document.id.clone(),
                    parent: parent.clone(),
                });
            }
        }
    }
    let mut marks: BTreeMap<&str, bool> = BTreeMap::new();
    let mut order = Vec::new();
    let mut stack: Vec<&str> = Vec::new();
    for id in batch.keys() {
        visit(id, &batch, &mut marks, &mut order, &mut stack)?;
    }
    Ok(order.into_iter().map(str::to_owned).collect())
}

fn visit<'a>(
    id: &'a str,
    batch: &BTreeMap<&'a str, &'a Document>,
    marks: &mut BTreeMap<&'a str, bool>,
    order: &mut Vec<&'a str>,
    stack: &mut Vec<&'a str>,
) -> Result<(), DocumentError> {
    match marks.get(id) {
        Some(true) => return Ok(()),
        Some(false) => {
            let start = stack.iter().position(|held| *held == id).unwrap_or(0);
            let mut path: Vec<String> = stack[start..].iter().map(|held| (*held).to_owned()).collect();
            path.push(id.to_owned());
            return Err(DocumentError::Cycle(path));
        }
        None => {}
    }
    marks.insert(id, false);
    stack.push(id);
    if let Some(document) = batch.get(id) {
        let mut parents: Vec<&str> = document.depends_on.iter().map(String::as_str).collect();
        parents.sort_unstable();
        for parent in parents {
            if batch.contains_key(parent) {
                visit(parent, batch, marks, order, stack)?;
            }
        }
    }
    stack.pop();
    marks.insert(id, true);
    order.push(id);
    Ok(())
}

/// Loads a batch of documents, parents first. Returns the ids in the order they loaded.
///
/// A duplicate id, a missing parent, a cycle and content the kind's schema refuses all
/// refuse the whole batch, naming what is wrong.
pub fn load(documents: &[Document]) -> Result<Vec<String>, DocumentError> {
    let mut seen = std::collections::BTreeSet::new();
    for document in documents {
        if document.id.is_empty() {
            return with(|store| {
                store.errors = vec![DocumentError::EmptyId.to_string()];
                Err(DocumentError::EmptyId)
            });
        }
        if !seen.insert(document.id.clone()) {
            return refuse(DocumentError::Duplicate(document.id.clone()));
        }
    }
    with(|store| {
        for document in documents {
            if store.loaded.contains_key(&document.id) {
                let error = DocumentError::Duplicate(document.id.clone());
                store.errors = vec![error.to_string()];
                return Err(error);
            }
        }
        let order = order_of(store, documents)?;
        let mut prepared: BTreeMap<String, (Document, ConfigValue)> = BTreeMap::new();
        for document in documents {
            let content = prepare(store, document)?;
            let mut prepared_doc = document.clone();
            prepared_doc.content = content.clone();
            prepared.insert(document.id.clone(), (prepared_doc, content));
        }
        for id in &order {
            let (document, content) = prepared.get(id).cloned().expect("ordered");
            let entries = entries_for(store, &document, &content)?;
            let mut ids = Vec::with_capacity(entries.len());
            for (entry_id, value) in &entries {
                let _ = crate::registry::register_in(&document.kind, entry_id, value.clone());
                ids.push(entry_id.clone());
            }
            let kind = document.kind.clone();
            store.loaded.insert(id.clone(), Loaded { document, entries: ids });
            notify(store, &kind, id, "load");
        }
        store.errors.clear();
        Ok(order)
    })
}

fn refuse(error: DocumentError) -> Result<Vec<String>, DocumentError> {
    with(|store| store.errors = vec![error.to_string()]);
    Err(error)
}

/// Reloads one document: validates the new content, then replaces only its own entries.
///
/// A document that no longer validates is refused and its old content stays.
pub fn reload(document: Document) -> Result<String, DocumentError> {
    with(|store| {
        if document.id.is_empty() {
            return Err(DocumentError::EmptyId);
        }
        let Some(existing) = store.loaded.get(&document.id) else {
            return Err(DocumentError::Unknown(document.id.clone()));
        };
        let old_kind = existing.document.kind.clone();
        let old_entries = existing.entries.clone();
        let content = prepare(store, &document)?;
        let entries = entries_for(store, &document, &content)?;
        for entry in &old_entries {
            let _ = crate::registry::unregister_in(&old_kind, entry);
        }
        let mut ids = Vec::with_capacity(entries.len());
        for (entry_id, value) in &entries {
            let _ = crate::registry::register_in(&document.kind, entry_id, value.clone());
            ids.push(entry_id.clone());
        }
        let mut reloaded = document.clone();
        reloaded.content = content;
        let kind = reloaded.kind.clone();
        let id = reloaded.id.clone();
        store.loaded.insert(id.clone(), Loaded { document: reloaded, entries: ids });
        notify(store, &kind, &id, "reload");
        Ok(id)
    })
}

/// Removes one document and exactly its own content. `false` when there is no such document.
pub fn unload(id: &str) -> bool {
    with(|store| {
        let Some(loaded) = store.loaded.remove(id) else {
            return false;
        };
        for entry in &loaded.entries {
            let _ = crate::registry::unregister_in(&loaded.document.kind, entry);
        }
        notify(store, &loaded.document.kind, id, "unload");
        true
    })
}

/// A document's content by kind and id, as it sits in the registry.
#[must_use]
pub fn get(kind: &str, id: &str) -> Option<ConfigValue> {
    crate::registry::get_in(kind, id)
}

/// Every loaded document, by id.
#[must_use]
pub fn get_all() -> Vec<Document> {
    with(|store| {
        store
            .loaded
            .values()
            .map(|loaded| loaded.document.clone())
            .collect()
    })
}

/// Every loaded document of one kind, by id.
#[must_use]
pub fn get_all_of(kind: &str) -> Vec<Document> {
    with(|store| {
        store
            .loaded
            .values()
            .filter(|loaded| loaded.document.kind == kind)
            .map(|loaded| loaded.document.clone())
            .collect()
    })
}

/// How many documents are loaded for a kind.
#[must_use]
pub fn count(kind: &str) -> usize {
    get_all_of(kind).len()
}

/// The loaded documents a filter accepts, by id.
#[must_use]
pub fn query(filter: impl Fn(&Document) -> bool) -> Vec<Document> {
    with(|store| {
        store
            .loaded
            .values()
            .map(|loaded| loaded.document.clone())
            .filter(|document| filter(document))
            .collect()
    })
}

/// A one-line summary: how many documents, across how many kinds, and how many each.
#[must_use]
pub fn summary() -> String {
    with(|store| {
        let mut per_kind: BTreeMap<&str, usize> = BTreeMap::new();
        for loaded in store.loaded.values() {
            *per_kind.entry(loaded.document.kind.as_str()).or_default() += 1;
        }
        let parts = per_kind
            .iter()
            .map(|(kind, count)| format!("{kind}={count}"))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "{} documents across {} kinds: {parts}",
            store.loaded.len(),
            per_kind.len()
        )
    })
}

/// Watches one kind's documents, called with `(id, action)` on load, reload and unload.
pub fn watch(kind: &str, watcher: impl FnMut(&str, &str) + Send + 'static) -> u64 {
    with(|store| {
        store.next_watch += 1;
        let id = store.next_watch;
        store.watchers.push(Watcher {
            kind: kind.to_owned(),
            id,
            body: Box::new(watcher),
        });
        id
    })
}

/// Stops watching, and answers whether that watcher existed.
pub fn unwatch(id: u64) -> bool {
    with(|store| {
        let before = store.watchers.len();
        store.watchers.retain(|watcher| watcher.id != id);
        store.watchers.len() != before
    })
}

fn notify(store: &mut Store, kind: &str, id: &str, action: &str) {
    for watcher in &mut store.watchers {
        if watcher.kind == kind {
            (watcher.body)(id, action);
        }
    }
}

/// What went wrong the last time documents were loaded.
#[must_use]
pub fn get_errors() -> Vec<String> {
    with(|store| store.errors.clone())
}

/// The ids a loaded document said it depends on.
#[must_use]
pub fn get_dependencies(id: &str) -> Vec<String> {
    with(|store| {
        store
            .loaded
            .get(id)
            .map(|loaded| loaded.document.depends_on.clone())
            .unwrap_or_default()
    })
}

/// Every reference a document's content makes, and whether the target is loaded.
#[must_use]
pub fn resolve_references(document: &Document) -> Vec<Reference> {
    let mut targets = Vec::new();
    collect_references(&document.content, &mut targets);
    let mut seen = std::collections::BTreeSet::new();
    with(|store| {
        targets
            .into_iter()
            .filter(|target| seen.insert(target.clone()))
            .map(|target| Reference {
                found: store.loaded.contains_key(&target)
                    || crate::registry::get(&target).is_some(),
                target,
            })
            .collect()
    })
}

fn collect_references(value: &ConfigValue, out: &mut Vec<String>) {
    match value {
        ConfigValue::Text(text)
        | ConfigValue::String(text)
        | ConfigValue::ResourceId(text)
        | ConfigValue::ItemId(text)
        | ConfigValue::BlockId(text)
        | ConfigValue::EntityId(text) => {
            if text.contains(':') {
                out.push(text.clone());
            }
        }
        ConfigValue::List(items) | ConfigValue::Set(items) => {
            for item in items {
                collect_references(item, out);
            }
        }
        ConfigValue::Map(fields) | ConfigValue::Object(fields) | ConfigValue::Compound(fields) => {
            for value in fields.values() {
                collect_references(value, out);
            }
        }
        _ => {}
    }
}

/// Why a data-driven question about the client has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "client_read" => {
            "the client's own data pack documents live inside the client and never cross the \
             boundary; only documents a mod declares here are readable"
                .to_owned()
        }
        "client_write" => {
            "content the client would load belongs to the client's registries, not this host's; \
             anything declared here never reaches them"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach about the client's data-driven systems.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["client_read", "client_write"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

/// The client's own data-driven state, which this host cannot read or write.
pub mod known_only_to_the_client {
    use super::why_unknown;
    use crate::config::ConfigValue;

    /// Reads one of the client's own data pack documents. `None`.
    #[must_use]
    pub fn read(_kind: &str, _id: &str) -> Option<ConfigValue> {
        crate::warn!(
            "a mod asked this host to read the client's data pack document: {}",
            why_unknown("client_read")
        );
        None
    }

    /// Writes content the client would load. `false`.
    pub fn write(_kind: &str, _id: &str, _value: ConfigValue) -> bool {
        crate::warn!(
            "a mod asked this host to write content the client would load: {}",
            why_unknown("client_write")
        );
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with(|store| {
            let codecs = std::mem::take(&mut store.codecs);
            *store = Store::default();
            store.codecs = codecs;
        });
    }

    fn object(pairs: &[(&str, ConfigValue)]) -> ConfigValue {
        ConfigValue::Object(
            pairs
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone()))
                .collect(),
        )
    }

    #[test]
    fn a_document_loads_and_is_read_back_per_kind() {
        let _serial = serial();
        fresh();
        assert!(register_schema(
            DocumentSchema::new("item")
                .required("stack", ConfigType::Integer)
                .optional("glow", ConfigType::Boolean, ConfigValue::Boolean(false))
        ));
        assert!(register_schema(DocumentSchema::new("biome")));
        let gem = Document::new("hello:gem", "item", object(&[("stack", ConfigValue::Integer(64))]));
        let ore = Document::new("hello:ore", "item", object(&[("stack", ConfigValue::Integer(1))]));
        let biome = Document::new("hello:grove", "biome", object(&[]));
        assert_eq!(load(&[gem, ore]).unwrap(), vec!["hello:gem".to_owned(), "hello:ore".to_owned()]);
        assert_eq!(load(&[biome]).unwrap(), vec!["hello:grove".to_owned()]);
        assert_eq!(
            get("item", "hello:gem"),
            Some(object(&[
                ("glow", ConfigValue::Boolean(false)),
                ("stack", ConfigValue::Integer(64)),
            ])),
            "the optional field is filled from its default"
        );
        assert_eq!(count("item"), 2);
        assert_eq!(count("biome"), 1);
        assert_eq!(get_all().len(), 3);
        assert_eq!(get_all_of("item").len(), 2);
        let filtered = query(|document| document.id.contains("ore"));
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, "hello:ore");
        assert!(summary().contains("3 documents"));
        assert!(summary().contains("item=2"));
        assert_eq!(
            load(&[Document::new("hello:gem", "item", object(&[("stack", ConfigValue::Integer(1))]))]),
            Err(DocumentError::Duplicate("hello:gem".to_owned()))
        );
        assert_eq!(get_errors(), vec!["hello:gem is already loaded".to_owned()]);
    }

    #[test]
    fn a_schema_answers_every_problem_with_its_path() {
        let _serial = serial();
        fresh();
        register_schema(
            DocumentSchema::new("loot")
                .field(
                    SchemaField::new("pools", ConfigType::List).required().element(
                        SchemaField::new("entry", ConfigType::Object)
                            .field(SchemaField::new("weight", ConfigType::Integer).required()),
                    ),
                )
                .required("rolls", ConfigType::Integer)
        );
        let content = object(&[(
            "pools",
            ConfigValue::List(vec![object(&[("weight", ConfigValue::Text("heavy".to_owned()))])]),
        )]);
        assert_eq!(
            validate("loot", &content),
            vec![
                Validation {
                    path: "loot.pools[0].weight".to_owned(),
                    reason: "expected integer, found text".to_owned(),
                },
                Validation {
                    path: "loot.rolls".to_owned(),
                    reason: "required, but missing".to_owned(),
                },
            ]
        );
        assert_eq!(validate("nothing", &content).len(), 1);
    }

    #[test]
    fn a_dependency_loads_after_its_parent_in_order() {
        let _serial = serial();
        fresh();
        register_schema(DocumentSchema::new("loot"));
        let parent = Document::new("red", "loot", object(&[]));
        let child = Document::new("green", "loot", object(&[])).depends_on("red");
        assert_eq!(
            load(&[child.clone(), parent.clone()]).unwrap(),
            vec!["red".to_owned(), "green".to_owned()]
        );
        assert_eq!(get_dependencies("green"), vec!["red".to_owned()]);
        assert_eq!(get_dependencies("red"), Vec::<String>::new());
        assert!(unload("green"));
        assert!(unload("red"));
        assert_eq!(
            load(&[Document::new("orphan", "loot", object(&[])).depends_on("absent")]),
            Err(DocumentError::MissingParent {
                id: "orphan".to_owned(),
                parent: "absent".to_owned(),
            })
        );
    }

    #[test]
    fn a_cycle_is_refused_and_named() {
        let _serial = serial();
        fresh();
        register_schema(DocumentSchema::new("loot"));
        let red = Document::new("red", "loot", object(&[])).depends_on("green");
        let green = Document::new("green", "loot", object(&[])).depends_on("red");
        assert_eq!(
            load(&[red, green]),
            Err(DocumentError::Cycle(vec![
                "green".to_owned(),
                "red".to_owned(),
                "green".to_owned(),
            ]))
        );
        assert_eq!(get_all().len(), 0);
    }

    #[test]
    fn a_reload_replaces_one_document_and_a_refused_reload_keeps_the_old() {
        let _serial = serial();
        fresh();
        register_schema(DocumentSchema::new("item").required("stack", ConfigType::Integer));
        let gem = Document::new("hello:gem", "item", object(&[("stack", ConfigValue::Integer(64))]));
        let ore = Document::new("hello:ore", "item", object(&[("stack", ConfigValue::Integer(1))]));
        load(&[gem, ore.clone()]).unwrap();
        assert_eq!(
            reload(Document::new("hello:gem", "item", object(&[("stack", ConfigValue::Integer(16))]))),
            Ok("hello:gem".to_owned())
        );
        assert_eq!(
            get("item", "hello:gem"),
            Some(object(&[("stack", ConfigValue::Integer(16))]))
        );
        assert_eq!(
            get("item", "hello:ore"),
            Some(object(&[("stack", ConfigValue::Integer(1))])),
            "the neighbour is untouched"
        );
        assert_eq!(
            reload(Document::new("hello:gem", "item", object(&[("stack", ConfigValue::Text("bad".to_owned()))]))),
            Err(DocumentError::Invalid {
                id: "hello:gem".to_owned(),
                problems: vec![Validation {
                    path: "item.stack".to_owned(),
                    reason: "expected integer, found text".to_owned(),
                }],
            })
        );
        assert_eq!(
            get("item", "hello:gem"),
            Some(object(&[("stack", ConfigValue::Integer(16))])),
            "the refused reload left the old content"
        );
    }

    #[test]
    fn an_unload_removes_exactly_one_document() {
        let _serial = serial();
        fresh();
        register_schema(DocumentSchema::new("item"));
        load(&[
            Document::new("hello:gem", "item", object(&[])),
            Document::new("hello:ore", "item", object(&[])),
        ])
        .unwrap();
        assert!(crate::registry::get_in("item", "hello:gem").is_some());
        assert!(unload("hello:gem"));
        assert!(get("item", "hello:gem").is_none());
        assert!(get("item", "hello:ore").is_some(), "the neighbour stays");
        assert_eq!(count("item"), 1);
        assert!(!unload("hello:gem"));
    }

    #[test]
    fn loaders_codecs_processors_migrations_and_watches_really_run() {
        let _serial = serial();
        fresh();
        let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
        let captured = seen.clone();
        let watch_id = watch("item", move |id, action| {
            captured.lock().unwrap().push(format!("{id}:{action}"));
        });
        assert!(register_schema(DocumentSchema::new("item").version(2).required("stack", ConfigType::Integer)));
        assert!(register_migration("item", 1, 2, |value| match value {
            ConfigValue::Object(fields) => {
                let mut fields = fields.clone();
                fields.insert("stack".to_owned(), ConfigValue::Integer(1));
                ConfigValue::Object(fields)
            }
            other => other.clone(),
        }));
        assert!(register_loader("item", |document| {
            Ok(vec![
                (format!("{}/a", document.id), ConfigValue::Boolean(true)),
                (format!("{}/b", document.id), ConfigValue::Boolean(false)),
            ])
        }));
        assert!(register_processor("item", |id, value| {
            if id.ends_with("/b") {
                Ok(ConfigValue::Boolean(true))
            } else {
                Ok(value.clone())
            }
        }));
        let document = Document::from_text("hello:gem", "item", "json", "{}").unwrap().version(1);
        load(&[document]).unwrap();
        assert_eq!(get("item", "hello:gem/a"), Some(ConfigValue::Boolean(true)));
        assert_eq!(get("item", "hello:gem/b"), Some(ConfigValue::Boolean(true)), "the processor ran");
        assert_eq!(
            get_all_of("item")[0].content,
            object(&[("stack", ConfigValue::Integer(1))]),
            "the migration ran"
        );
        assert_eq!(seen.lock().unwrap().clone(), vec!["hello:gem:load".to_owned()]);
        assert!(unwatch(watch_id));
        assert!(unload("hello:gem"));
        assert_eq!(seen.lock().unwrap().len(), 1, "the unwatched load is all that was seen");
        assert!(decode("nope", "x").is_err());
    }

    #[test]
    fn the_clients_own_documents_are_refused() {
        let _serial = serial();
        fresh();
        assert!(known_only_to_the_client::read("item", "any").is_none());
        assert!(!known_only_to_the_client::write("item", "any", ConfigValue::Boolean(true)));
        assert_eq!(refusals().len(), 2);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
        assert!(why_unknown("client_read").contains("data pack documents"));
    }
}
