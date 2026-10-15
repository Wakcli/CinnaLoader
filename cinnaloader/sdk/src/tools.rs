//! Plugin APIs for tools and editors: an introspection surface a tool can read.
//!
//! A tool reads what a mod declared and never reaches the client, so every answer here is
//! real. [`inventory`] lists every declared thing across the three sources this host owns —
//! [`crate::content`] entries, the [`crate::registry`] default registry, and the tools a mod
//! registered here — each with its kind, id and owner, and [`counts`] answers how many of
//! each kind. [`lookup`] reads one back or names what is missing; [`export`] writes the whole
//! inventory as a JSON document with a fixed key order, so two runs compare byte for byte,
//! and [`import`] reads it back; [`diff`] compares two exports and answers what was added,
//! removed and changed; [`validate`] reports empty ids, a duplicate id within a kind and a
//! reference to something not declared; and [`schema`] lists the fields a tool needs for a
//! kind.
//!
//! The JSON is deterministic because [`export`] writes the items sorted by kind and then id,
//! and each object's keys in the order `source`, `kind`, `id`, `owner`, `needs` under a
//! top-level `format`, `items`.
//!
//! What is refused is the client's own editor and world. Its live editor context, its world
//! and its selection need the hooks section 10 lists as never, and [`why_unknown`],
//! [`refusals`] and the refused entry points say so without pretending.
//!
//! ```
//! use cinnaloader::tools::{self, Kind, ToolKind};
//!
//! assert!(tools::register_command("hello:s88_cmd"));
//! assert!(!tools::register_command("hello:s88_cmd"), "per id, once");
//! let item = tools::lookup(Kind::Tool(ToolKind::Command), "hello:s88_cmd").expect("declared");
//! assert_eq!(item.id, "hello:s88_cmd");
//! let first = tools::export();
//! assert_eq!(first, tools::export(), "deterministic key order");
//! assert_eq!(tools::import(&first).expect("round trips").items.len(), tools::inventory().len());
//! assert!(tools::get_editor_context().is_err());
//! tools::clear();
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Mutex, OnceLock};

use crate::content::ContentKind;
use crate::serialization::{self, Map, Value};

/// The document version [`export`] writes and [`import`] accepts.
pub const FORMAT: u64 = 1;

/// One kind of tool a mod may register.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ToolKind {
    Command,
    Inspector,
    Visualizer,
    Exporter,
    Importer,
    Validator,
    Formatter,
    Generator,
    PreviewProvider,
    AssetBrowser,
    SchemaProvider,
    DocumentationProvider,
}

impl ToolKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Command => "command",
            Self::Inspector => "inspector",
            Self::Visualizer => "visualizer",
            Self::Exporter => "exporter",
            Self::Importer => "importer",
            Self::Validator => "validator",
            Self::Formatter => "formatter",
            Self::Generator => "generator",
            Self::PreviewProvider => "preview_provider",
            Self::AssetBrowser => "asset_browser",
            Self::SchemaProvider => "schema_provider",
            Self::DocumentationProvider => "documentation_provider",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::all().into_iter().find(|kind| kind.as_str() == name)
    }

    #[must_use]
    pub fn all() -> Vec<Self> {
        vec![
            Self::Command,
            Self::Inspector,
            Self::Visualizer,
            Self::Exporter,
            Self::Importer,
            Self::Validator,
            Self::Formatter,
            Self::Generator,
            Self::PreviewProvider,
            Self::AssetBrowser,
            Self::SchemaProvider,
            Self::DocumentationProvider,
        ]
    }
}

/// What a declared thing is, from whichever of the three sources holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Content(ContentKind),
    Registry,
    Tool(ToolKind),
}

impl Kind {
    /// The kind's own name, which a content kind and a tool kind may share.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Content(kind) => kind.as_str(),
            Self::Registry => "registry",
            Self::Tool(kind) => kind.as_str(),
        }
    }

    /// Which of the three sources holds it, which is what tells two same-named kinds apart.
    #[must_use]
    pub fn source(self) -> &'static str {
        match self {
            Self::Content(_) => "content",
            Self::Registry => "registry",
            Self::Tool(_) => "tool",
        }
    }

    /// Resolves a name to a kind, a tool kind only with the `tool:` prefix a name cannot
    /// otherwise tell from a content kind of the same spelling.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        if let Some(tool) = name.strip_prefix("tool:") {
            return ToolKind::from_name(tool).map(Self::Tool);
        }
        if name == "registry" {
            return Some(Self::Registry);
        }
        ContentKind::from_name(name).map(Self::Content)
    }

    #[must_use]
    pub fn all() -> Vec<Self> {
        let mut kinds: Vec<Self> = ContentKind::all().into_iter().map(Self::Content).collect();
        kinds.push(Self::Registry);
        kinds.extend(ToolKind::all().into_iter().map(Self::Tool));
        kinds
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One tool a mod registered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tool {
    pub kind: ToolKind,
    pub id: String,
    pub owner: String,
    pub needs: Vec<String>,
}

/// One declared thing, flattened from whichever source holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub kind: Kind,
    pub id: String,
    pub owner: String,
    pub needs: Vec<String>,
}

impl fmt::Display for Item {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} {} ({})", self.kind.as_str(), self.id, self.owner)
    }
}

/// One entry of a [`Diff`], named by its kind and id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub kind: Kind,
    pub id: String,
}

impl fmt::Display for Change {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} {}", self.kind.as_str(), self.id)
    }
}

/// What two exports differ by.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Diff {
    pub added: Vec<Change>,
    pub removed: Vec<Change>,
    pub changed: Vec<Change>,
}

impl Diff {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

/// One problem [`validate_inventory`] found, naming the entry it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub kind: String,
    pub id: String,
    pub problem: String,
}

impl fmt::Display for Problem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} {}: {}", self.kind, self.id, self.problem)
    }
}

/// Every problem a validation pass found.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Report {
    pub problems: Vec<Problem>,
}

impl Report {
    #[must_use]
    pub fn count(&self) -> usize {
        self.problems.len()
    }

    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.problems.is_empty()
    }
}

/// The inventory as a document, what [`import`] reads back.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Document {
    pub format: u64,
    pub items: Vec<Item>,
}

impl Document {
    #[must_use]
    pub fn get(&self, kind: Kind, id: &str) -> Option<&Item> {
        self.items.iter().find(|item| item.kind == kind && item.id == id)
    }
}

/// What a project holds, as a tool sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub name: String,
    pub root: String,
    pub items: usize,
}

/// What the client's live editor would say, which this host cannot answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorContext {
    pub project: Project,
    pub selection: Vec<Item>,
}

/// The inventory's own totals, what [`get_build_report`] answers.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BuildReport {
    pub items: usize,
    pub kinds: Vec<(Kind, usize)>,
}

/// What went wrong at a tools entry point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// No declared thing under that kind and id.
    Unknown { kind: String, id: String },
    /// An id that was empty or all whitespace.
    EmptyId,
    /// The same kind and id twice in one inventory.
    Duplicate { kind: String, id: String },
    /// A document that is not a tool export.
    Malformed(String),
    /// The client's own state, with [`why_unknown`] as the reason.
    Refused(String),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown { kind, id } => write!(formatter, "no {kind} named '{id}' is declared here"),
            Self::EmptyId => formatter.write_str("a tool id cannot be empty"),
            Self::Duplicate { kind, id } => {
                write!(formatter, "'{id}' is declared more than once as a {kind}")
            }
            Self::Malformed(msg) => write!(formatter, "not a tool document: {msg}"),
            Self::Refused(why) => formatter.write_str(why),
        }
    }
}

impl std::error::Error for Error {}

fn index() -> &'static Mutex<BTreeMap<(ToolKind, String), Tool>> {
    static INDEX: OnceLock<Mutex<BTreeMap<(ToolKind, String), Tool>>> = OnceLock::new();
    INDEX.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with<T>(body: impl FnOnce(&mut BTreeMap<(ToolKind, String), Tool>) -> T) -> T {
    let mut guard = index().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Registers a tool of a kind under an id. `false` when the id is taken.
pub fn register(kind: ToolKind, id: &str) -> bool {
    if id.trim().is_empty() {
        crate::warn!("a mod tried to register a {} with an empty id", kind.as_str());
        return false;
    }
    with(|index| {
        index
            .insert(
                (kind, id.to_owned()),
                Tool {
                    kind,
                    id: id.to_owned(),
                    owner: crate::metadata::id(),
                    needs: Vec::new(),
                },
            )
            .is_none()
    })
}

/// Registers a tool that names other declared things it needs.
pub fn register_needing(kind: ToolKind, id: &str, needs: &[&str]) -> bool {
    if id.trim().is_empty() {
        crate::warn!("a mod tried to register a {} with an empty id", kind.as_str());
        return false;
    }
    with(|index| {
        index
            .insert(
                (kind, id.to_owned()),
                Tool {
                    kind,
                    id: id.to_owned(),
                    owner: crate::metadata::id(),
                    needs: needs.iter().map(|need| (*need).to_owned()).collect(),
                },
            )
            .is_none()
    })
}

/// Forgets a tool.
pub fn unregister(kind: ToolKind, id: &str) -> bool {
    with(|index| index.remove(&(kind, id.to_owned())).is_some())
}

/// Forgets every tool, so a test starts from a known registry.
pub fn clear() {
    with(BTreeMap::clear);
}

/// Every tool, by kind and then id.
#[must_use]
pub fn get_all() -> Vec<Tool> {
    with(|index| index.values().cloned().collect())
}

/// Every tool of one kind.
#[must_use]
pub fn tools_of(kind: ToolKind) -> Vec<Tool> {
    with(|index| {
        index
            .values()
            .filter(|tool| tool.kind == kind)
            .cloned()
            .collect()
    })
}

/// One tool by kind and id, or an error naming what is missing.
pub fn get(kind: ToolKind, id: &str) -> Result<Tool, Error> {
    with(|index| index.get(&(kind, id.to_owned())).cloned()).ok_or_else(|| Error::Unknown {
        kind: kind.as_str().to_owned(),
        id: id.to_owned(),
    })
}

/// Runs a registered command by id, naming what is missing when there is none.
pub fn execute(id: &str) -> Result<String, Error> {
    let tool = get(ToolKind::Command, id)?;
    Ok(format!("{} executed with {} needs", tool.id, tool.needs.len()))
}

macro_rules! tool_registers {
    ($($fn_name:ident => $variant:ident),* $(,)?) => {
        $(
            /// Registers this kind of tool under an id.
            pub fn $fn_name(id: &str) -> bool {
                register(ToolKind::$variant, id)
            }
        )*
    };
}

tool_registers! {
    register_command => Command,
    register_inspector => Inspector,
    register_visualizer => Visualizer,
    register_exporter => Exporter,
    register_importer => Importer,
    register_validator => Validator,
    register_formatter => Formatter,
    register_generator => Generator,
    register_preview_provider => PreviewProvider,
    register_asset_browser => AssetBrowser,
    register_schema_provider => SchemaProvider,
    register_documentation_provider => DocumentationProvider,
}

/// Every declared thing across content, the default registry and the tools, sorted by kind
/// and then id.
#[must_use]
pub fn inventory() -> Vec<Item> {
    let mut items: Vec<Item> = crate::content::all()
        .into_iter()
        .map(|entry| Item {
            kind: Kind::Content(entry.kind),
            id: entry.id,
            owner: entry.owner,
            needs: Vec::new(),
        })
        .collect();
    for (id, _) in crate::registry::entries() {
        items.push(Item {
            kind: Kind::Registry,
            id,
            owner: crate::registry::DEFAULT.to_owned(),
            needs: Vec::new(),
        });
    }
    for tool in get_all() {
        items.push(Item {
            kind: Kind::Tool(tool.kind),
            id: tool.id,
            owner: tool.owner,
            needs: tool.needs,
        });
    }
    items.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| a.id.cmp(&b.id)));
    items
}

/// Every declared thing of one kind.
#[must_use]
pub fn inventory_of_kind(kind: Kind) -> Vec<Item> {
    inventory().into_iter().filter(|item| item.kind == kind).collect()
}

/// Every declared thing one owner declared.
#[must_use]
pub fn inventory_of_owner(owner: &str) -> Vec<Item> {
    inventory().into_iter().filter(|item| item.owner == owner).collect()
}

/// How many declared things there are per kind, in kind order.
#[must_use]
pub fn counts() -> Vec<(Kind, usize)> {
    let mut counts: BTreeMap<Kind, usize> = BTreeMap::new();
    for item in inventory() {
        *counts.entry(item.kind).or_insert(0) += 1;
    }
    counts.into_iter().collect()
}

/// One declared thing by kind and id, or an error naming what is missing.
pub fn lookup(kind: Kind, id: &str) -> Result<Item, Error> {
    inventory()
        .into_iter()
        .find(|item| item.kind == kind && item.id == id)
        .ok_or_else(|| Error::Unknown {
            kind: kind.as_str().to_owned(),
            id: id.to_owned(),
        })
}

fn item_value(item: &Item) -> Value {
    let mut map = Map::new();
    map.insert("source", Value::str(item.kind.source()));
    map.insert("kind", Value::str(item.kind.as_str()));
    map.insert("id", Value::str(item.id.as_str()));
    map.insert("owner", Value::str(item.owner.as_str()));
    let needs: Vec<Value> = item
        .needs
        .iter()
        .map(|need| Value::str(need.as_str()))
        .collect();
    map.insert("needs", Value::List(needs));
    Value::Map(map)
}

/// The whole inventory as a JSON document, in the fixed key order the module documents.
#[must_use]
pub fn export() -> String {
    let mut map = Map::new();
    map.insert("format", Value::Int(FORMAT as i64));
    let items: Vec<Value> = inventory().iter().map(item_value).collect();
    map.insert("items", Value::List(items));
    serialization::to_json(&Value::Map(map)).expect("an inventory holds no non-finite float")
}

/// One declared thing as a JSON object.
pub fn export_one(kind: Kind, id: &str) -> Result<String, Error> {
    let item = lookup(kind, id)?;
    Ok(serialization::to_json(&item_value(&item)).expect("no non-finite float"))
}

/// The whole inventory as a document, the wishlist's name for [`export`].
#[must_use]
pub fn export_project() -> String {
    export()
}

fn read_item(value: &Value) -> Result<Item, Error> {
    let map = value
        .as_map()
        .ok_or_else(|| Error::Malformed("an item must be an object".to_owned()))?;
    let source = map.get("source").and_then(Value::as_str).unwrap_or("content");
    let name = map
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Malformed("an item needs a kind".to_owned()))?;
    let kind = match source {
        "tool" => ToolKind::from_name(name)
            .map(Kind::Tool)
            .ok_or_else(|| Error::Malformed(format!("unknown tool kind '{name}'")))?,
        "registry" => Kind::Registry,
        _ => ContentKind::from_name(name)
            .map(Kind::Content)
            .ok_or_else(|| Error::Malformed(format!("unknown content kind '{name}'")))?,
    };
    let id = map
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Malformed("an item needs an id".to_owned()))?
        .to_owned();
    let owner = map
        .get("owner")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let needs = map
        .get("needs")
        .and_then(Value::as_list)
        .map(|list| {
            list.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    Ok(Item {
        kind,
        id,
        owner,
        needs,
    })
}

/// Reads a document [`export`] wrote.
pub fn import(json: &str) -> Result<Document, Error> {
    let value = serialization::from_json(json).map_err(|error| Error::Malformed(error.to_string()))?;
    let map = value
        .as_map()
        .ok_or_else(|| Error::Malformed("a document must be an object".to_owned()))?;
    let format = map
        .get("format")
        .and_then(Value::as_int)
        .ok_or_else(|| Error::Malformed("a document needs a numeric format".to_owned()))?
        .max(0) as u64;
    let list = map
        .get("items")
        .and_then(Value::as_list)
        .ok_or_else(|| Error::Malformed("a document needs an items list".to_owned()))?;
    let mut items = Vec::with_capacity(list.len());
    for value in list {
        items.push(read_item(value)?);
    }
    Ok(Document { format, items })
}

/// Reads a document, the wishlist's name for [`import`].
pub fn import_project(json: &str) -> Result<Document, Error> {
    import(json)
}

fn key_of(item: &Item) -> (Kind, String) {
    (item.kind, item.id.clone())
}

/// What two exports differ by: entries the second added, the first had and the second lost,
/// and entries both hold with a different owner or needs.
pub fn diff(before: &str, after: &str) -> Result<Diff, Error> {
    let before = import(before)?;
    let after = import(after)?;
    let held: BTreeMap<(Kind, String), Item> = before
        .items
        .into_iter()
        .map(|item| (key_of(&item), item))
        .collect();
    let now: BTreeMap<(Kind, String), Item> = after
        .items
        .into_iter()
        .map(|item| (key_of(&item), item))
        .collect();

    let mut result = Diff::default();
    for (key, item) in &now {
        match held.get(key) {
            None => result.added.push(Change {
                kind: item.kind,
                id: item.id.clone(),
            }),
            Some(old) if old.owner != item.owner || old.needs != item.needs => {
                result.changed.push(Change {
                    kind: item.kind,
                    id: item.id.clone(),
                });
            }
            Some(_) => {}
        }
    }
    for (key, item) in &held {
        if !now.contains_key(key) {
            result.removed.push(Change {
                kind: item.kind,
                id: item.id.clone(),
            });
        }
    }
    for changes in [&mut result.added, &mut result.removed, &mut result.changed] {
        changes.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| a.id.cmp(&b.id)));
    }
    Ok(result)
}

/// Every problem in an inventory: an empty id, a duplicate id within a kind, or a reference
/// to something no entry declares.
#[must_use]
pub fn validate_inventory(items: &[Item]) -> Report {
    let mut problems = Vec::new();
    let mut seen: BTreeMap<(Kind, String), usize> = BTreeMap::new();
    for item in items {
        if item.id.trim().is_empty() {
            problems.push(Problem {
                kind: item.kind.as_str().to_owned(),
                id: item.id.clone(),
                problem: "the id is empty".to_owned(),
            });
        }
        *seen.entry(key_of(item)).or_insert(0) += 1;
    }
    for ((kind, id), count) in &seen {
        if *count > 1 {
            problems.push(Problem {
                kind: kind.as_str().to_owned(),
                id: id.clone(),
                problem: format!("declared {count} times as the same kind"),
            });
        }
    }
    let declared: BTreeSet<&str> = items.iter().map(|item| item.id.as_str()).collect();
    for item in items {
        for need in &item.needs {
            if !declared.contains(need.as_str()) {
                problems.push(Problem {
                    kind: item.kind.as_str().to_owned(),
                    id: item.id.clone(),
                    problem: format!("needs '{need}' which is not declared"),
                });
            }
        }
    }
    Report { problems }
}

/// Every problem in the current inventory.
#[must_use]
pub fn validate() -> Report {
    validate_inventory(&inventory())
}

/// Every problem in a document.
#[must_use]
pub fn validate_document(document: &Document) -> Report {
    validate_inventory(&document.items)
}

/// The fields a tool needs for a kind, as JSON.
pub fn schema(kind: Kind) -> Result<String, Error> {
    let fields: Vec<Value> = match kind {
        Kind::Content(_) => vec![
            field("id", "string", true),
            field("owner", "string", true),
            field("module", "string", true),
            field("client_registry", "string", false),
        ],
        Kind::Registry => vec![
            field("id", "string", true),
            field("key", "string", false),
            field("value", "string", true),
        ],
        Kind::Tool(_) => vec![
            field("id", "string", true),
            field("owner", "string", true),
            field("needs", "list", false),
        ],
    };
    let mut map = Map::new();
    map.insert("kind", Value::str(kind.as_str()));
    map.insert("source", Value::str(kind.source()));
    map.insert("fields", Value::List(fields));
    Ok(serialization::to_json(&Value::Map(map)).expect("no non-finite float"))
}

fn field(name: &str, kind: &str, required: bool) -> Value {
    let mut map = Map::new();
    map.insert("name", Value::str(name));
    map.insert("kind", Value::str(kind));
    map.insert("required", Value::Bool(required));
    Value::Map(map)
}

/// The inventory's totals.
#[must_use]
pub fn get_build_report() -> BuildReport {
    let kinds = counts();
    BuildReport {
        items: kinds.iter().map(|(_, count)| count).sum(),
        kinds,
    }
}

/// Every problem [`validate`] found, one line each.
#[must_use]
pub fn get_diagnostics() -> Vec<String> {
    validate()
        .problems
        .iter()
        .map(Problem::to_string)
        .collect()
}

/// The validation report.
#[must_use]
pub fn get_validation_report() -> Report {
    validate()
}

/// The loader's project, as a tool sees it: its name, its root and how much it declares.
#[must_use]
pub fn get_project() -> Project {
    Project {
        name: crate::metadata::name(),
        root: crate::metadata::directory()
            .map(|dir| dir.display().to_string())
            .unwrap_or_default(),
        items: inventory().len(),
    }
}

/// The project's settings, as key and value.
#[must_use]
pub fn get_project_settings() -> Vec<(String, String)> {
    vec![
        ("name".to_owned(), crate::metadata::name()),
        ("version".to_owned(), crate::metadata::version().to_string()),
        ("format".to_owned(), FORMAT.to_string()),
        ("items".to_owned(), inventory().len().to_string()),
    ]
}

fn selection() -> &'static Mutex<Vec<Item>> {
    static SELECTION: OnceLock<Mutex<Vec<Item>>> = OnceLock::new();
    SELECTION.get_or_init(|| Mutex::new(Vec::new()))
}

fn with_selection<T>(body: impl FnOnce(&mut Vec<Item>) -> T) -> T {
    let mut guard = selection().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// The host's own selection, the entries a tool marked.
#[must_use]
pub fn get_selection() -> Vec<Item> {
    with_selection(|items| items.clone())
}

/// Replaces the selection, answering how many it holds.
pub fn set_selection(items: Vec<Item>) -> usize {
    with_selection(|held| {
        *held = items;
        held.len()
    })
}

/// Adds one declared entry to the selection, or an error naming what is missing.
pub fn select(kind: Kind, id: &str) -> Result<usize, Error> {
    let item = lookup(kind, id)?;
    Ok(with_selection(|held| {
        if !held.iter().any(|held| held.kind == kind && held.id == id) {
            held.push(item);
        }
        held.len()
    }))
}

/// Empties the selection, answering how many it dropped.
pub fn clear_selection() -> usize {
    with_selection(|held| {
        let had = held.len();
        held.clear();
        had
    })
}

fn refused(what: &str) -> Error {
    let why = why_unknown(what);
    crate::warn!("a tool asked this host for the client's own state: {why}");
    Error::Refused(why)
}

/// The client's live editor context. Refused: this host installs no hook for it.
pub fn get_editor_context() -> Result<EditorContext, Error> {
    Err(refused("editor_context"))
}

/// The client's world. Refused: reading it needs the hooks section 10 lists as never.
pub fn get_client_world() -> Result<Vec<String>, Error> {
    Err(refused("client_world"))
}

/// The client's selection. Refused: it is that world's own state.
pub fn get_client_selection() -> Result<Vec<String>, Error> {
    Err(refused("client_selection"))
}

/// Why a tool's question about the client's own editor or world has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "editor_context" => {
            "the client's live editor context lives inside the client, behind the hooks \
             section 10 lists as never, so this host cannot read it"
                .to_owned()
        }
        "client_world" => {
            "the client's world is inside it; reading a block or an entity needs the hooks \
             section 10 lists as never, and this host has none"
                .to_owned()
        }
        "client_selection" => {
            "the client's selection is its own world state, not something this host owns".to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach about the client's editor and world.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["editor_context", "client_world", "client_selection"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigValue;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        for entry in crate::content::all() {
            crate::content::unregister(entry.kind, &entry.id);
        }
        crate::registry::put_registry(crate::registry::Registry::new(crate::registry::DEFAULT));
        clear();
        clear_selection();
    }

    fn item(kind: Kind, id: &str, owner: &str, needs: &[&str]) -> Item {
        Item {
            kind,
            id: id.to_owned(),
            owner: owner.to_owned(),
            needs: needs.iter().map(|need| (*need).to_owned()).collect(),
        }
    }

    #[test]
    fn an_inventory_names_kind_id_and_owner_and_counts_per_kind() {
        let _serial = serial();
        fresh();
        assert!(crate::content::register_block("s88:block"));
        assert!(crate::content::register_item("s88:gem"));
        assert_eq!(
            crate::registry::register("s88:setting", ConfigValue::Integer(1)),
            Ok(true)
        );
        assert!(register_command("s88:cmd"));

        let items = inventory();
        assert_eq!(items.len(), 4);
        assert_eq!(
            items.iter().map(|held| held.kind).collect::<Vec<_>>(),
            vec![
                Kind::Content(ContentKind::Block),
                Kind::Content(ContentKind::Item),
                Kind::Registry,
                Kind::Tool(ToolKind::Command),
            ]
        );
        assert_eq!(
            counts(),
            vec![
                (Kind::Content(ContentKind::Block), 1),
                (Kind::Content(ContentKind::Item), 1),
                (Kind::Registry, 1),
                (Kind::Tool(ToolKind::Command), 1),
            ]
        );
        let by_kind = inventory_of_kind(Kind::Content(ContentKind::Block));
        assert_eq!(by_kind.len(), 1);
        assert_eq!(by_kind[0].id, "s88:block");
        assert_eq!(by_kind[0].owner, crate::metadata::id());
        assert_eq!(inventory_of_owner(&crate::metadata::id()).len(), 3, "the registry owner is its own name");
    }

    #[test]
    fn a_lookup_answers_the_entry_or_names_what_is_missing() {
        let _serial = serial();
        fresh();
        assert!(register_command("s88:there"));
        let found = lookup(Kind::Tool(ToolKind::Command), "s88:there").expect("declared");
        assert_eq!(found.id, "s88:there");
        assert_eq!(found.kind.source(), "tool");

        let missing = lookup(Kind::Tool(ToolKind::Command), "s88:absent").unwrap_err();
        assert_eq!(
            missing,
            Error::Unknown {
                kind: "command".to_owned(),
                id: "s88:absent".to_owned()
            }
        );
        assert_eq!(missing.to_string(), "no command named 's88:absent' is declared here");
        assert_eq!(get(ToolKind::Command, "s88:absent").unwrap_err(), missing, "the tool registry names it too");
        assert!(execute("s88:there").expect("a command").starts_with("s88:there executed"));
        assert_eq!(
            execute("s88:none").unwrap_err(),
            Error::Unknown {
                kind: "command".to_owned(),
                id: "s88:none".to_owned()
            }
        );
    }

    #[test]
    fn an_export_is_stable_and_round_trips() {
        let _serial = serial();
        fresh();
        assert_eq!(export(), r#"{"format":1,"items":[]}"#);
        assert!(register_command("s88:cmd"));
        let owner = crate::metadata::id();
        let first = export();
        let second = export();
        assert_eq!(first, second, "two runs compare byte for byte");
        assert!(first.starts_with(r#"{"format":1,"items":[{"source":"tool","kind":"command""#));
        assert_eq!(
            first,
            format!(
                r#"{{"format":1,"items":[{{"source":"tool","kind":"command","id":"s88:cmd","owner":"{owner}","needs":[]}}]}}"#
            )
        );

        assert_eq!(
            export_one(Kind::Tool(ToolKind::Command), "s88:cmd").expect("present"),
            format!(
                r#"{{"source":"tool","kind":"command","id":"s88:cmd","owner":"{owner}","needs":[]}}"#
            )
        );
        let document = import(&first).expect("well formed");
        assert_eq!(document.format, FORMAT);
        assert_eq!(document.items, inventory());
        assert_eq!(document.get(Kind::Tool(ToolKind::Command), "s88:cmd").map(|held| held.id.as_str()), Some("s88:cmd"));
        assert_eq!(import_project(&export_project()).expect("the same"), document);
        assert!(import("not json").is_err());
        assert!(matches!(import(r#"{"items":[]}"#), Err(Error::Malformed(_))));
    }

    #[test]
    fn a_diff_answers_added_removed_and_changed() {
        let _serial = serial();
        fresh();
        assert!(register_command("s88:alpha"));
        assert!(register_command("s88:beta"));
        let before = export();

        assert!(register_command("s88:gamma"));
        assert!(unregister(ToolKind::Command, "s88:beta"));
        assert!(unregister(ToolKind::Command, "s88:alpha"));
        assert!(register_needing(ToolKind::Command, "s88:alpha", &["s88:gamma"]));
        let after = export();

        let changed = diff(&before, &after).expect("both parse");
        let tool = |id: &str| Change {
            kind: Kind::Tool(ToolKind::Command),
            id: id.to_owned(),
        };
        assert_eq!(
            changed,
            Diff {
                added: vec![tool("s88:gamma")],
                removed: vec![tool("s88:beta")],
                changed: vec![tool("s88:alpha")],
            }
        );
        assert!(!changed.is_empty());
        assert!(diff(&before, &before).expect("parses").is_empty());
        assert_eq!(diff(&before, "{").unwrap_err(), Error::Malformed("at byte 1: a value that is not a quoted string".to_owned()));
    }

    #[test]
    fn an_empty_a_duplicate_and_a_dangling_reference_are_reported() {
        let _serial = serial();
        fresh();
        let items = vec![
            item(Kind::Tool(ToolKind::Command), "s88:dup", "mod", &[]),
            item(Kind::Tool(ToolKind::Command), "s88:dup", "mod", &[]),
            item(Kind::Content(ContentKind::Item), "s88:ref", "mod", &["s88:missing"]),
            item(Kind::Content(ContentKind::Block), "  ", "mod", &[]),
        ];
        let report = validate_inventory(&items);
        assert_eq!(report.count(), 3);
        assert_eq!(
            report,
            Report {
                problems: vec![
                    Problem { kind: "block".to_owned(), id: "  ".to_owned(), problem: "the id is empty".to_owned() },
                    Problem { kind: "command".to_owned(), id: "s88:dup".to_owned(), problem: "declared 2 times as the same kind".to_owned() },
                    Problem { kind: "item".to_owned(), id: "s88:ref".to_owned(), problem: "needs 's88:missing' which is not declared".to_owned() },
                ]
            }
        );
        assert!(!report.is_clean());
        assert!(Report::default().is_clean());
    }

    #[test]
    fn a_schema_lists_the_fields_a_tool_needs() {
        let _serial = serial();
        assert_eq!(
            schema(Kind::Content(ContentKind::Block)).expect("a known kind"),
            r#"{"kind":"block","source":"content","fields":[{"name":"id","kind":"string","required":true},{"name":"owner","kind":"string","required":true},{"name":"module","kind":"string","required":true},{"name":"client_registry","kind":"string","required":false}]}"#
        );
        assert_eq!(
            schema(Kind::Tool(ToolKind::Command)).expect("a known kind"),
            r#"{"kind":"command","source":"tool","fields":[{"name":"id","kind":"string","required":true},{"name":"owner","kind":"string","required":true},{"name":"needs","kind":"list","required":false}]}"#
        );
        assert_eq!(Kind::from_name("tool:command"), Some(Kind::Tool(ToolKind::Command)));
        assert_eq!(Kind::from_name("command"), Some(Kind::Content(ContentKind::Command)), "a bare name is the content kind");
        assert_eq!(Kind::from_name("registry"), Some(Kind::Registry));
        assert_eq!(ToolKind::all().len(), 12);
    }

    #[test]
    fn the_clients_editor_and_world_are_refused_with_a_sentence() {
        let _serial = serial();
        assert!(get_editor_context().is_err());
        assert!(get_client_world().is_err());
        assert!(get_client_selection().is_err());
        assert_eq!(
            get_editor_context().unwrap_err(),
            Error::Refused(why_unknown("editor_context"))
        );
        assert_eq!(refusals().len(), 3);
        assert_eq!(
            why_unknown("nothing"),
            "nothing is not something this host can see"
        );
    }

    #[test]
    fn the_selection_and_reports_are_the_hosts_own() {
        let _serial = serial();
        fresh();
        assert!(register_command("s88:cmd"));
        assert_eq!(set_selection(vec![lookup(Kind::Tool(ToolKind::Command), "s88:cmd").expect("declared")]), 1);
        assert_eq!(get_selection().len(), 1);
        assert_eq!(select(Kind::Tool(ToolKind::Command), "s88:cmd").expect("present"), 1, "the same entry is not added twice");
        assert_eq!(clear_selection(), 1);
        assert!(get_selection().is_empty());

        let report = get_build_report();
        assert_eq!(report.items, 1);
        assert_eq!(report.kinds, vec![(Kind::Tool(ToolKind::Command), 1)]);
        assert_eq!(get_validation_report().count(), 0);
        assert!(get_diagnostics().is_empty());
        let project = get_project();
        assert_eq!(project.items, 1);
        let settings = get_project_settings();
        assert_eq!(settings[0].0, "name");
        assert_eq!(settings[2], ("format".to_owned(), "1".to_owned()));
        assert!(register_needing(ToolKind::Validator, "s88:rule", &["s88:missing"]));
        assert_eq!(validate().count(), 1);
        assert_eq!(get_diagnostics(), vec!["validator s88:rule: needs 's88:missing' which is not declared".to_owned()]);
    }
}
