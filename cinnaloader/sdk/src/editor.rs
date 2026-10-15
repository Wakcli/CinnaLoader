//! The visual mod editor's document model: a project, its node tree, a selection, properties,
//! an undo stack and a preview.
//!
//! Everything a mod can hold is real data. A [`Project`] has an id, a name and a tree of
//! [`Node`]s — each with an id, a parent, a [`NodeKind`] and properties as [`ConfigValue`]s —
//! where [`Project::add_node`] refuses a duplicate id and a missing parent by name,
//! [`Project::remove_node`] drops a whole subtree and answers how many nodes went,
//! [`Project::move_node`] refuses a cycle and [`Project::rename_node`] refuses a duplicate.
//! A [`Selection`] adds, drops, toggles and lists in tree order. Properties are read and
//! written by a path like `on_start.spawn.size`, a missing path is refused with the path and
//! a value of the wrong kind with both kinds. Every mutation snapshots the project, so
//! [`Project::undo`] and [`Project::redo`] really restore the previous and next state and a
//! new mutation clears the redo stack. [`save_project`] writes the whole document as JSON
//! through [`crate::serialization`] and [`import_project`] rebuilds an identical project,
//! byte for byte on the way back out. [`get_preview`] summarises the tree with a count per
//! kind.
//!
//! What is refused is a **real editor window**. Opening a project or a document in the
//! client's editor and hot-reloading a live one would drive the client's own UI, which this
//! host has no handle on. Those entry points answer with [`why_unknown`] and are listed by
//! [`refusals`].
//!
//! ```
//! use cinnaloader::config::ConfigValue;
//! use cinnaloader::editor::{self, NodeKind};
//!
//! let mut project = editor::create_project("hello:overworld", "Overworld");
//! project.add_node("root", "on_start", NodeKind::Event).expect("a new node");
//! project.add_node("on_start", "spawn", NodeKind::Action).expect("a child");
//! assert!(project.add_node("on_start", "on_start", NodeKind::Action).is_err(), "duplicate");
//! assert!(project.add_node("missing", "x", NodeKind::Action).is_err(), "missing parent");
//! project
//!     .set_property("on_start.spawn.speed", ConfigValue::Float(2.5))
//!     .expect("a float speed");
//! assert_eq!(
//!     project.get_property("on_start.spawn.speed"),
//!     Ok(ConfigValue::Float(2.5))
//! );
//! let text = editor::save_project(&project);
//! assert_eq!(
//!     editor::save_project(&editor::import_project(&text).expect("valid")),
//!     text
//! );
//! assert!(editor::open_project("hello:overworld").is_err(), "no editor window here");
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use crate::config::{ConfigType, ConfigValue};
use crate::serialization::{self, Map, Value};

/// The document version [`save_project`] writes and [`import_project`] accepts.
pub const FORMAT: u64 = 1;

/// What a node is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum NodeKind {
    #[default]
    Root,
    Event,
    Getter,
    Action,
    Flow,
    Group,
}

impl NodeKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Root => "root",
            Self::Event => "event",
            Self::Getter => "getter",
            Self::Action => "action",
            Self::Flow => "flow",
            Self::Group => "group",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::all().into_iter().find(|kind| kind.as_str() == name)
    }

    #[must_use]
    pub fn all() -> Vec<Self> {
        vec![
            Self::Root,
            Self::Event,
            Self::Getter,
            Self::Action,
            Self::Flow,
            Self::Group,
        ]
    }
}

impl fmt::Display for NodeKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One node of the project's tree.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub id: String,
    pub parent: Option<String>,
    pub kind: NodeKind,
    pub properties: BTreeMap<String, ConfigValue>,
}

impl fmt::Display for Node {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} [{}] {} prop(s){}",
            self.id,
            self.kind.as_str(),
            self.properties.len(),
            match &self.parent {
                Some(parent) => format!(" under {parent}"),
                None => String::new(),
            },
        )
    }
}

/// The state a mutation snapshots, so [`Project::undo`] can put it back.
#[derive(Debug, Clone, PartialEq)]
struct Snapshot {
    id: String,
    name: String,
    root: String,
    nodes: BTreeMap<String, Node>,
}

/// An editor project: an id, a name and a tree of nodes, with an undo and redo stack.
#[derive(Debug, Clone)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub root: String,
    pub nodes: BTreeMap<String, Node>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
}

impl PartialEq for Project {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.name == other.name
            && self.root == other.root
            && self.nodes == other.nodes
    }
}

impl fmt::Display for Project {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} '{}' with {} node(s)",
            self.id,
            self.name,
            self.nodes.len()
        )
    }
}

impl Project {
    /// A project with a root node and nothing under it yet.
    #[must_use]
    pub fn new(id: &str, name: &str) -> Self {
        let root = "root".to_owned();
        let mut nodes = BTreeMap::new();
        nodes.insert(
            root.clone(),
            Node {
                id: root.clone(),
                parent: None,
                kind: NodeKind::Root,
                properties: BTreeMap::new(),
            },
        );
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            root,
            nodes,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    /// A node by id.
    #[must_use]
    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.get(id)
    }

    /// Whether a node is there.
    #[must_use]
    pub fn has_node(&self, id: &str) -> bool {
        self.nodes.contains_key(id)
    }

    /// How many nodes the tree holds.
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// A node's children, in id order.
    #[must_use]
    pub fn children_of(&self, id: &str) -> Vec<String> {
        self.nodes
            .values()
            .filter(|node| node.parent.as_deref() == Some(id))
            .map(|node| node.id.clone())
            .collect()
    }

    /// Every node id, root first, depth first, children in id order.
    #[must_use]
    pub fn tree_order(&self) -> Vec<String> {
        let mut order = Vec::new();
        self.walk(&self.root, &mut order);
        order
    }

    /// How many nodes there are of each kind, in kind order.
    #[must_use]
    pub fn kind_counts(&self) -> Vec<(NodeKind, usize)> {
        let mut counts: BTreeMap<NodeKind, usize> = BTreeMap::new();
        for node in self.nodes.values() {
            *counts.entry(node.kind).or_insert(0) += 1;
        }
        counts.into_iter().collect()
    }

    /// Adds a node under a parent, refusing a duplicate id and a missing parent by name.
    pub fn add_node(&mut self, parent: &str, id: &str, kind: NodeKind) -> Result<(), Error> {
        if id.trim().is_empty() {
            return Err(Error::EmptyId);
        }
        if self.nodes.contains_key(id) {
            return Err(Error::DuplicateId {
                id: id.to_owned(),
            });
        }
        if !self.nodes.contains_key(parent) {
            return Err(Error::MissingParent {
                parent: parent.to_owned(),
            });
        }
        self.push_snapshot();
        self.nodes.insert(
            id.to_owned(),
            Node {
                id: id.to_owned(),
                parent: Some(parent.to_owned()),
                kind,
                properties: BTreeMap::new(),
            },
        );
        Ok(())
    }

    /// Removes a node and its whole subtree, and answers how many nodes went.
    pub fn remove_node(&mut self, id: &str) -> Result<usize, Error> {
        if id == self.root {
            return Err(Error::RootRemoval);
        }
        if !self.nodes.contains_key(id) {
            return Err(Error::MissingNode { id: id.to_owned() });
        }
        let mut going = Vec::new();
        self.walk(id, &mut going);
        self.push_snapshot();
        for held in &going {
            self.nodes.remove(held);
        }
        Ok(going.len())
    }

    /// Moves a node under a new parent, refusing a cycle.
    pub fn move_node(&mut self, id: &str, parent: &str) -> Result<(), Error> {
        if !self.nodes.contains_key(id) {
            return Err(Error::MissingNode { id: id.to_owned() });
        }
        if !self.nodes.contains_key(parent) {
            return Err(Error::MissingParent {
                parent: parent.to_owned(),
            });
        }
        let mut below = Vec::new();
        self.walk(id, &mut below);
        if below.iter().any(|held| held == parent) {
            return Err(Error::Cycle {
                id: id.to_owned(),
                parent: parent.to_owned(),
            });
        }
        self.push_snapshot();
        if let Some(node) = self.nodes.get_mut(id) {
            node.parent = Some(parent.to_owned());
        }
        Ok(())
    }

    /// Renames a node, refusing a missing node and a duplicate id.
    pub fn rename_node(&mut self, old: &str, new: &str) -> Result<(), Error> {
        if !self.nodes.contains_key(old) {
            return Err(Error::MissingNode { id: old.to_owned() });
        }
        if new.trim().is_empty() {
            return Err(Error::EmptyId);
        }
        if old == new {
            return Ok(());
        }
        if self.nodes.contains_key(new) {
            return Err(Error::DuplicateId { id: new.to_owned() });
        }
        self.push_snapshot();
        let mut node = self.nodes.remove(old).expect("checked above");
        node.id = new.to_owned();
        self.nodes.insert(new.to_owned(), node);
        for held in self.nodes.values_mut() {
            if held.parent.as_deref() == Some(old) {
                held.parent = Some(new.to_owned());
            }
        }
        if self.root == old {
            self.root = new.to_owned();
        }
        Ok(())
    }

    /// Reads a property by path, refusing a missing path with the path.
    pub fn get_property(&self, path: &str) -> Result<ConfigValue, Error> {
        let (node_id, name) = self.locate(path)?;
        self.nodes
            .get(&node_id)
            .and_then(|node| node.properties.get(&name))
            .cloned()
            .ok_or_else(|| Error::MissingPath {
                path: path.to_owned(),
            })
    }

    /// Writes a property by path, refusing a missing path and a wrong kind with both kinds.
    pub fn set_property(&mut self, path: &str, value: ConfigValue) -> Result<(), Error> {
        let (node_id, name) = self.locate(path)?;
        if let Some(expected) = property_kind(&name) {
            if !expected.accepts(value.kind()) {
                return Err(Error::WrongKind {
                    path: path.to_owned(),
                    expected: expected.as_str().to_owned(),
                    found: value.kind().as_str().to_owned(),
                });
            }
        }
        self.push_snapshot();
        if let Some(node) = self.nodes.get_mut(&node_id) {
            node.properties.insert(name, value);
        }
        Ok(())
    }

    /// A node's properties, in name order.
    pub fn list_properties(&self, node: &str) -> Result<Vec<(String, ConfigValue)>, Error> {
        let segments: Vec<&str> = if node.is_empty() {
            Vec::new()
        } else {
            node.split('.').collect()
        };
        let held = self.node_at(&segments).ok_or_else(|| Error::MissingPath {
            path: node.to_owned(),
        })?;
        Ok(held
            .properties
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect())
    }

    /// Puts the project back the way it was before the last mutation.
    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.undo.pop() else {
            return false;
        };
        let current = self.snapshot();
        self.redo.push(current);
        self.restore(previous);
        true
    }

    /// Reapplies the mutation the last `undo` took back.
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        let current = self.snapshot();
        self.undo.push(current);
        self.restore(next);
        true
    }

    /// How many undo steps are waiting.
    #[must_use]
    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    /// How many redo steps are waiting.
    #[must_use]
    pub fn redo_depth(&self) -> usize {
        self.redo.len()
    }

    /// A text summary of the tree, with a count per kind.
    #[must_use]
    pub fn preview(&self) -> String {
        let counts = self
            .kind_counts()
            .iter()
            .map(|(kind, count)| format!("{} {}", kind.as_str(), count))
            .collect::<Vec<_>>()
            .join(", ");
        let mut lines = vec![format!(
            "{} '{}': {} node(s), {}",
            self.id,
            self.name,
            self.nodes.len(),
            counts
        )];
        self.preview_node(&self.root, 0, &mut lines);
        lines.join("\n")
    }

    fn preview_node(&self, id: &str, depth: usize, lines: &mut Vec<String>) {
        if let Some(node) = self.nodes.get(id) {
            lines.push(format!(
                "{}{} [{}] {} prop(s)",
                "  ".repeat(depth),
                node.id,
                node.kind.as_str(),
                node.properties.len()
            ));
            for child in self.children_of(id) {
                self.preview_node(&child, depth + 1, lines);
            }
        }
    }

    fn locate(&self, path: &str) -> Result<(String, String), Error> {
        let segments: Vec<&str> = path.split('.').collect();
        if segments.len() < 2 || segments.iter().any(|segment| segment.is_empty()) {
            return Err(Error::MissingPath {
                path: path.to_owned(),
            });
        }
        let (node_path, tail) = segments.split_at(segments.len() - 1);
        let node = self.node_at(node_path).ok_or_else(|| Error::MissingPath {
            path: path.to_owned(),
        })?;
        Ok((node.id.clone(), tail[0].to_owned()))
    }

    fn node_at(&self, segments: &[&str]) -> Option<&Node> {
        let mut current = self.nodes.get(&self.root)?;
        for segment in segments {
            if *segment == current.id {
                continue;
            }
            current = self
                .nodes
                .values()
                .find(|node| node.id == *segment && node.parent.as_deref() == Some(current.id.as_str()))?;
        }
        Some(current)
    }

    fn walk(&self, id: &str, out: &mut Vec<String>) {
        out.push(id.to_owned());
        for child in self.children_of(id) {
            self.walk(&child, out);
        }
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            id: self.id.clone(),
            name: self.name.clone(),
            root: self.root.clone(),
            nodes: self.nodes.clone(),
        }
    }

    fn restore(&mut self, snapshot: Snapshot) {
        self.id = snapshot.id;
        self.name = snapshot.name;
        self.root = snapshot.root;
        self.nodes = snapshot.nodes;
    }

    fn push_snapshot(&mut self) {
        let snapshot = self.snapshot();
        self.undo.push(snapshot);
        self.redo.clear();
    }
}

/// The host's own selection of node ids.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    ids: Vec<String>,
}

impl Selection {
    /// Adds a node id, and answers whether it was new.
    pub fn select(&mut self, id: &str) -> bool {
        if self.ids.iter().any(|held| held == id) {
            return false;
        }
        self.ids.push(id.to_owned());
        true
    }

    /// Drops a node id, and answers whether it was there.
    pub fn deselect(&mut self, id: &str) -> bool {
        let before = self.ids.len();
        self.ids.retain(|held| held != id);
        self.ids.len() != before
    }

    /// Adds a node id if it is absent and drops it if it is present, answering whether it
    /// is now selected.
    pub fn toggle(&mut self, id: &str) -> bool {
        if self.deselect(id) {
            false
        } else {
            self.select(id);
            true
        }
    }

    /// Empties the selection, and answers how many it dropped.
    pub fn clear(&mut self) -> usize {
        let had = self.ids.len();
        self.ids.clear();
        had
    }

    /// How many node ids are selected.
    #[must_use]
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    /// Whether nothing is selected.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// Whether a node id is selected.
    #[must_use]
    pub fn contains(&self, id: &str) -> bool {
        self.ids.iter().any(|held| held == id)
    }

    /// The selection in the project's tree order.
    #[must_use]
    pub fn list(&self, project: &Project) -> Vec<String> {
        project
            .tree_order()
            .into_iter()
            .filter(|id| self.contains(id))
            .collect()
    }

    /// The selection in the order it was built.
    #[must_use]
    pub fn ids(&self) -> Vec<String> {
        self.ids.clone()
    }
}

/// One problem a validation pass found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub problems: Vec<String>,
}

impl Report {
    /// How many problems there are.
    #[must_use]
    pub fn count(&self) -> usize {
        self.problems.len()
    }

    /// Whether the pass found nothing.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.problems.is_empty()
    }
}

/// What a build produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Build {
    pub artifact: String,
    pub nodes: usize,
    pub ok: bool,
}

/// Whether anything has been built, and how it went.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    #[default]
    Idle,
    Built,
    Failed,
}

impl Status {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Built => "built",
            Self::Failed => "failed",
        }
    }
}

/// What went wrong at an editor entry point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// An id that was empty or all whitespace.
    EmptyId,
    /// A node id already in the tree.
    DuplicateId { id: String },
    /// A parent named by a misspelled or absent id.
    MissingParent { parent: String },
    /// A node id that is not in the tree.
    MissingNode { id: String },
    /// A move that would put a node inside itself.
    Cycle { id: String, parent: String },
    /// An attempt to remove the root, which everything hangs from.
    RootRemoval,
    /// A property path that names no node and no property.
    MissingPath { path: String },
    /// A value whose kind is not the one the property declares.
    WrongKind {
        path: String,
        expected: String,
        found: String,
    },
    /// A document that is not an editor project.
    Malformed(String),
    /// The client's own state, with [`why_unknown`] as the reason.
    Refused(String),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyId => formatter.write_str("a node id cannot be empty"),
            Self::DuplicateId { id } => write!(formatter, "a node named '{id}' is already here"),
            Self::MissingParent { parent } => {
                write!(formatter, "there is no parent node named '{parent}'")
            }
            Self::MissingNode { id } => write!(formatter, "there is no node named '{id}'"),
            Self::Cycle { id, parent } => {
                write!(formatter, "moving '{id}' under '{parent}' would put it inside itself")
            }
            Self::RootRemoval => formatter.write_str("the root node cannot be removed"),
            Self::MissingPath { path } => write!(formatter, "no property at '{path}'"),
            Self::WrongKind {
                path,
                expected,
                found,
            } => write!(
                formatter,
                "'{path}' is a {expected}, not a {found}"
            ),
            Self::Malformed(msg) => write!(formatter, "not an editor project: {msg}"),
            Self::Refused(why) => formatter.write_str(why),
        }
    }
}

impl std::error::Error for Error {}

fn custom_nodes() -> &'static Mutex<BTreeMap<String, NodeKind>> {
    static NODES: OnceLock<Mutex<BTreeMap<String, NodeKind>>> = OnceLock::new();
    NODES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn properties() -> &'static Mutex<BTreeMap<String, ConfigType>> {
    static PROPERTIES: OnceLock<Mutex<BTreeMap<String, ConfigType>>> = OnceLock::new();
    PROPERTIES.get_or_init(|| {
        let mut kinds = BTreeMap::new();
        kinds.insert("name".to_owned(), ConfigType::String);
        kinds.insert("enabled".to_owned(), ConfigType::Boolean);
        kinds.insert("size".to_owned(), ConfigType::Float);
        kinds.insert("speed".to_owned(), ConfigType::Float);
        kinds.insert("position".to_owned(), ConfigType::Position);
        kinds.insert("color".to_owned(), ConfigType::Color);
        kinds.insert("key".to_owned(), ConfigType::Keybind);
        Mutex::new(kinds)
    })
}

fn panels() -> &'static Mutex<BTreeMap<String, String>> {
    static PANELS: OnceLock<Mutex<BTreeMap<String, String>>> = OnceLock::new();
    PANELS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn inspectors() -> &'static Mutex<BTreeMap<String, String>> {
    static INSPECTORS: OnceLock<Mutex<BTreeMap<String, String>>> = OnceLock::new();
    INSPECTORS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn generators() -> &'static Mutex<BTreeMap<String, String>> {
    static GENERATORS: OnceLock<Mutex<BTreeMap<String, String>>> = OnceLock::new();
    GENERATORS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn active() -> &'static Mutex<Option<Project>> {
    static ACTIVE: OnceLock<Mutex<Option<Project>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(None))
}

fn build_state() -> &'static Mutex<(Status, Vec<String>)> {
    static STATE: OnceLock<Mutex<(Status, Vec<String>)>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new((Status::Idle, Vec::new())))
}

fn with<T, R>(registry: &'static Mutex<T>, body: impl FnOnce(&mut T) -> R) -> R {
    let mut guard = registry.lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// The kind a property declares, when one is registered under that name.
#[must_use]
fn property_kind(name: &str) -> Option<ConfigType> {
    with(properties(), |kinds| kinds.get(name).copied())
}

fn remember<V>(registry: &'static Mutex<BTreeMap<String, V>>, key: &str, value: V) -> bool {
    if key.trim().is_empty() {
        crate::warn!("a mod tried to register an editor entry with an empty name");
        return false;
    }
    let mut guard = registry.lock().unwrap_or_else(|error| error.into_inner());
    if guard.contains_key(key) {
        return false;
    }
    guard.insert(key.to_owned(), value);
    true
}

/// Forgets every registration and the active project, so a test starts clean.
pub fn clear() {
    with(custom_nodes(), |nodes| nodes.clear());
    with(panels(), |panels| panels.clear());
    with(inspectors(), |inspectors| inspectors.clear());
    with(generators(), |generators| generators.clear());
    with(properties(), |kinds| {
        kinds.retain(|name, _| {
            matches!(
                name.as_str(),
                "name" | "enabled" | "size" | "speed" | "position" | "color" | "key"
            )
        });
    });
    with(active(), |held| *held = None);
    with(build_state(), |state| *state = (Status::Idle, Vec::new()));
}

fn activate(project: &Project) {
    let mut held = active().lock().unwrap_or_else(|error| error.into_inner());
    *held = Some(project.clone());
}

/// Starts a project with an id and a name, and makes it the active one.
#[must_use]
pub fn create_project(id: &str, name: &str) -> Project {
    let project = Project::new(id, name);
    activate(&project);
    project
}

/// Starts an editor document, which is a project under the wishlist's other name.
#[must_use]
pub fn create_document(id: &str, name: &str) -> Project {
    create_project(id, name)
}

/// Writes the whole project as JSON, in a fixed key order so two runs compare byte for byte.
#[must_use]
pub fn save_project(project: &Project) -> String {
    serialization::to_json(&project_value(project)).expect("a project holds no non-finite float")
}

/// The wishlist's name for [`save_project`].
#[must_use]
pub fn save_document(project: &Project) -> String {
    save_project(project)
}

/// Reads a project [`save_project`] wrote, and rebuilds it exactly.
pub fn import_project(text: &str) -> Result<Project, Error> {
    let value =
        serialization::from_json(text).map_err(|error| Error::Malformed(error.to_string()))?;
    let project = read_project(&value)?;
    activate(&project);
    Ok(project)
}

/// The wishlist's name for [`import_project`].
pub fn import_document(text: &str) -> Result<Project, Error> {
    import_project(text)
}

/// Forgets the active project, and answers whether one was open.
pub fn close_project() -> bool {
    let mut held = active().lock().unwrap_or_else(|error| error.into_inner());
    held.take().is_some()
}

/// The active project, when a caller wants to look at it.
#[must_use]
pub fn get_active_project() -> Option<Project> {
    let held = active().lock().unwrap_or_else(|error| error.into_inner());
    held.clone()
}

/// Every problem in a project: a missing root, a node whose parent is gone, a node the root
/// cannot reach, an empty name.
#[must_use]
pub fn validate_document(project: &Project) -> Report {
    let mut problems = Vec::new();
    if project.name.trim().is_empty() {
        problems.push("the project has an empty name".to_owned());
    }
    if !project.nodes.contains_key(&project.root) {
        problems.push(format!("the root node '{}' is missing", project.root));
    }
    for node in project.nodes.values() {
        if node.id.trim().is_empty() {
            problems.push("a node has an empty id".to_owned());
        }
        if node.id != project.root {
            match &node.parent {
                None => problems.push(format!("node '{}' has no parent", node.id)),
                Some(parent) if !project.nodes.contains_key(parent) => {
                    problems.push(format!("node '{}' names a missing parent '{parent}'", node.id));
                }
                Some(_) => {}
            }
        }
    }
    let reachable = project.tree_order().len();
    if reachable != project.nodes.len() {
        problems.push(format!(
            "the tree reaches {reachable} of {} nodes",
            project.nodes.len()
        ));
    }
    Report { problems }
}

/// The active project's problems, one line each.
#[must_use]
pub fn get_diagnostics() -> Vec<String> {
    match get_active_project() {
        Some(project) => validate_document(&project).problems,
        None => Vec::new(),
    }
}

/// Compiles a project, which is the document pass that finds what is wrong with it.
#[must_use]
pub fn compile(project: &Project) -> Report {
    validate_document(project)
}

/// Builds a project and records the result, naming the artifact the package would be.
pub fn build(project: &Project) -> Build {
    let report = validate_document(project);
    let ok = report.is_clean();
    let artifact = format!("{}.mod.json", project.id);
    let status = if ok { Status::Built } else { Status::Failed };
    let mut state = build_state().lock().unwrap_or_else(|error| error.into_inner());
    state.0 = status;
    state.1.push(format!(
        "{} {}: {} node(s), {} problem(s)",
        status.as_str(),
        artifact,
        project.nodes.len(),
        report.count()
    ));
    Build {
        artifact,
        nodes: project.nodes.len(),
        ok,
    }
}

/// Packages a built project as a JSON document with the artifact header and the project.
#[must_use]
pub fn package(project: &Project) -> String {
    let mut map = Map::new();
    map.insert("format", Value::Int(FORMAT as i64));
    map.insert("kind", Value::str("editor_package"));
    map.insert("artifact", Value::str(format!("{}.mod.json", project.id)));
    map.insert("project", project_value(project));
    serialization::to_json(&Value::Map(map)).expect("a project holds no non-finite float")
}

/// How the last [`build`] went.
#[must_use]
pub fn get_build_status() -> Status {
    let state = build_state().lock().unwrap_or_else(|error| error.into_inner());
    state.0
}

/// Every line the build has logged.
#[must_use]
pub fn get_build_logs() -> Vec<String> {
    let state = build_state().lock().unwrap_or_else(|error| error.into_inner());
    state.1.clone()
}

/// The document schema, as JSON.
#[must_use]
pub fn get_schema() -> String {
    let fields: Vec<Value> = ["id", "kind", "parent", "properties"]
        .iter()
        .map(|name| Value::str(*name))
        .collect();
    let kinds: Vec<Value> = NodeKind::all()
        .iter()
        .map(|kind| Value::str(kind.as_str()))
        .collect();
    let mut node = Map::new();
    node.insert("fields", Value::List(fields));
    let mut root = Map::new();
    root.insert("format", Value::Int(FORMAT as i64));
    root.insert("node", Value::Map(node));
    root.insert("kinds", Value::List(kinds));
    serialization::to_json(&Value::Map(root)).expect("no non-finite float")
}

/// The asset kinds an editor can import.
#[must_use]
pub fn get_asset_types() -> Vec<String> {
    [
        "texture",
        "model",
        "sound",
        "font",
        "shader",
        "structure",
        "lang",
        "data",
        "animation",
    ]
    .iter()
    .map(|name| (*name).to_owned())
    .collect()
}

/// Metadata for one asset kind, or `None` when the kind is not importable.
#[must_use]
pub fn get_asset_metadata(name: &str) -> Option<String> {
    let (category, importable) = match name {
        "texture" => ("image", true),
        "model" => ("mesh", true),
        "sound" => ("audio", true),
        "font" => ("font", true),
        "shader" => ("shader", false),
        "structure" => ("structure", true),
        "lang" => ("text", true),
        "data" => ("data", true),
        "animation" => ("animation", true),
        _ => return None,
    };
    let mut map = Map::new();
    map.insert("type", Value::str(name));
    map.insert("category", Value::str(category));
    map.insert("importable", Value::Bool(importable));
    Some(serialization::to_json(&Value::Map(map)).expect("no non-finite float"))
}

/// What the editor can do, on this host and in general.
#[must_use]
pub fn get_supported_features() -> Vec<String> {
    [
        "node_tree",
        "properties",
        "selection",
        "undo_redo",
        "save_load",
        "preview",
        "templates",
        "generators",
        "asset_browser",
    ]
    .iter()
    .map(|name| (*name).to_owned())
    .collect()
}

/// The fields a component carries, or `None` for an unknown component.
#[must_use]
pub fn get_component_schema(name: &str) -> Option<String> {
    let fields: Vec<Value> = match name {
        "transform" => vec![
            field("x", "float"),
            field("y", "float"),
            field("z", "float"),
            field("rotation", "vector3"),
        ],
        "health" => vec![field("max", "float"), field("current", "float")],
        "inventory" => vec![field("size", "integer")],
        "render" => vec![field("model", "string"), field("texture", "string")],
        _ => return None,
    };
    let mut map = Map::new();
    map.insert("component", Value::str(name));
    map.insert("fields", Value::List(fields));
    Some(serialization::to_json(&Value::Map(map)).expect("no non-finite float"))
}

/// The kind a property declares, or `None` for an unknown property.
#[must_use]
pub fn get_property_schema(name: &str) -> Option<String> {
    let kind = property_kind(name)?;
    let mut map = Map::new();
    map.insert("name", Value::str(name));
    map.insert("kind", Value::str(kind.as_str()));
    Some(serialization::to_json(&Value::Map(map)).expect("no non-finite float"))
}

/// Every node kind the editor knows: the built-in kinds and the custom ones.
#[must_use]
pub fn get_node_types() -> Vec<String> {
    let mut names: Vec<String> = NodeKind::all()
        .iter()
        .map(|kind| kind.as_str().to_owned())
        .collect();
    names.extend(with(custom_nodes(), |nodes| {
        nodes.keys().cloned().collect::<Vec<_>>()
    }));
    names
}

/// The graph kinds a document may be.
#[must_use]
pub fn get_graph_types() -> Vec<String> {
    ["event", "logic", "data", "render"]
        .iter()
        .map(|name| (*name).to_owned())
        .collect()
}

/// The built-in project templates.
#[must_use]
pub fn get_templates() -> Vec<String> {
    ["blank", "event_chain", "timer", "player_action"]
        .iter()
        .map(|name| (*name).to_owned())
        .collect()
}

/// Every generator a mod registered.
#[must_use]
pub fn get_generators() -> Vec<String> {
    with(generators(), |generators| {
        generators.keys().cloned().collect::<Vec<_>>()
    })
}

/// Every custom node a mod registered, by name.
#[must_use]
pub fn get_custom_nodes() -> Vec<(String, NodeKind)> {
    with(custom_nodes(), |nodes| {
        nodes
            .iter()
            .map(|(name, kind)| (name.clone(), *kind))
            .collect::<Vec<_>>()
    })
}

/// Every custom panel a mod registered, by id.
#[must_use]
pub fn get_panels() -> Vec<(String, String)> {
    with(panels(), |panels| {
        panels
            .iter()
            .map(|(id, title)| (id.clone(), title.clone()))
            .collect::<Vec<_>>()
    })
}

/// Every custom inspector a mod registered, by id.
#[must_use]
pub fn get_inspectors() -> Vec<(String, String)> {
    with(inspectors(), |inspectors| {
        inspectors
            .iter()
            .map(|(id, target)| (id.clone(), target.clone()))
            .collect::<Vec<_>>()
    })
}

/// Registers a node kind of the mod's own. `false` when the name is taken.
pub fn register_custom_node(name: &str, kind: NodeKind) -> bool {
    remember(custom_nodes(), name, kind)
}

/// Registers a property kind of the mod's own. `false` when the name is taken.
pub fn register_custom_property(name: &str, kind: ConfigType) -> bool {
    remember(properties(), name, kind)
}

/// Registers a panel of the mod's own. `false` when the id is taken.
pub fn register_custom_panel(id: &str, title: &str) -> bool {
    remember(panels(), id, title.to_owned())
}

/// Registers an inspector for a component. `false` when the id is taken.
pub fn register_custom_inspector(id: &str, target: &str) -> bool {
    remember(inspectors(), id, target.to_owned())
}

/// Registers a generator. `false` when the id is taken.
pub fn register_custom_generator(id: &str, output: &str) -> bool {
    remember(generators(), id, output.to_owned())
}

/// Opens a project in the client's editor. `Err`: this host has no editor window.
pub fn open_project(_id: &str) -> Result<Project, Error> {
    Err(refused("editor_window"))
}

/// Opens a document in the client's editor. `Err`: this host has no editor window.
pub fn open_document(_id: &str) -> Result<Project, Error> {
    Err(refused("editor_window"))
}

/// Reloads a live document inside the client's editor. `Err`: there is no window to reload.
pub fn hot_reload() -> Result<(), Error> {
    Err(refused("live_reload"))
}

fn refused(what: &str) -> Error {
    let why = why_unknown(what);
    crate::warn!("a mod asked this host to drive the client's editor: {why}");
    Error::Refused(why)
}

/// Why a question about the client's editor has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "editor_window" => {
            "opening a project or document in the client's editor, and building or packaging \
             it there, drives the client's own editor UI; this host has no handle on that \
             window, and the client's editor is the client's"
                .to_owned()
        }
        "live_reload" => {
            "a hot reload swaps the running editor's document in place; there is no running \
             editor window here for this host to reload"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach about the client's editor.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["editor_window", "live_reload"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

fn field(name: &str, kind: &str) -> Value {
    let mut map = Map::new();
    map.insert("name", Value::str(name));
    map.insert("kind", Value::str(kind));
    Value::Map(map)
}

fn project_value(project: &Project) -> Value {
    let mut map = Map::new();
    map.insert("format", Value::Int(FORMAT as i64));
    map.insert("id", Value::str(project.id.as_str()));
    map.insert("name", Value::str(project.name.as_str()));
    map.insert("root", Value::str(project.root.as_str()));
    let nodes: Vec<Value> = project.nodes.values().map(node_value).collect();
    map.insert("nodes", Value::List(nodes));
    Value::Map(map)
}

fn node_value(node: &Node) -> Value {
    let mut map = Map::new();
    map.insert("id", Value::str(node.id.as_str()));
    map.insert("kind", Value::str(node.kind.as_str()));
    map.insert(
        "parent",
        match &node.parent {
            Some(parent) => Value::str(parent.as_str()),
            None => Value::Null,
        },
    );
    let mut props = Map::new();
    for (name, value) in &node.properties {
        props.insert(name.as_str(), encode(value));
    }
    map.insert("properties", Value::Map(props));
    Value::Map(map)
}

fn read_project(value: &Value) -> Result<Project, Error> {
    let map = value
        .as_map()
        .ok_or_else(|| Error::Malformed("a project must be an object".to_owned()))?;
    let text = |key: &str| -> Result<String, Error> {
        map.get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| Error::Malformed(format!("a project needs a {key}")))
    };
    let id = text("id")?;
    let name = text("name")?;
    let root = text("root")?;
    let list = map
        .get("nodes")
        .and_then(Value::as_list)
        .ok_or_else(|| Error::Malformed("a project needs a nodes list".to_owned()))?;
    let mut nodes = BTreeMap::new();
    for entry in list {
        let node = read_node(entry)?;
        nodes.insert(node.id.clone(), node);
    }
    Ok(Project {
        id,
        name,
        root,
        nodes,
        undo: Vec::new(),
        redo: Vec::new(),
    })
}

fn read_node(value: &Value) -> Result<Node, Error> {
    let map = value
        .as_map()
        .ok_or_else(|| Error::Malformed("a node must be an object".to_owned()))?;
    let id = map
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Malformed("a node needs an id".to_owned()))?
        .to_owned();
    let kind = map
        .get("kind")
        .and_then(Value::as_str)
        .and_then(NodeKind::from_name)
        .ok_or_else(|| Error::Malformed(format!("node '{id}' has an unknown kind")))?;
    let parent = map.get("parent").and_then(Value::as_str).map(str::to_owned);
    let mut properties = BTreeMap::new();
    if let Some(props) = map.get("properties").and_then(Value::as_map) {
        for (name, value) in props.iter() {
            let decoded = decode(value).ok_or_else(|| {
                Error::Malformed(format!("node '{id}' property '{name}' is malformed"))
            })?;
            properties.insert(name.to_owned(), decoded);
        }
    }
    Ok(Node {
        id,
        parent,
        kind,
        properties,
    })
}

fn encode(value: &ConfigValue) -> Value {
    let mut map = Map::new();
    map.insert("kind", Value::str(value.kind().as_str()));
    map.insert("value", encode_inner(value));
    Value::Map(map)
}

fn encode_inner(value: &ConfigValue) -> Value {
    match value {
        ConfigValue::Boolean(flag) => Value::Bool(*flag),
        ConfigValue::Integer(number) => Value::Int(*number),
        ConfigValue::Duration(millis) => Value::Int(*millis as i64),
        ConfigValue::Float(number) => Value::Float(*number),
        ConfigValue::Percentage(number) => Value::Float(*number),
        ConfigValue::Color(colour) => Value::Int(i64::from(*colour)),
        ConfigValue::String(text)
        | ConfigValue::Text(text)
        | ConfigValue::Enum(text)
        | ConfigValue::Keybind(text)
        | ConfigValue::ResourceId(text)
        | ConfigValue::ItemId(text)
        | ConfigValue::BlockId(text)
        | ConfigValue::EntityId(text) => Value::str(text.as_str()),
        ConfigValue::FilePath(path) | ConfigValue::DirectoryPath(path) => {
            Value::str(path.display().to_string())
        }
        ConfigValue::Position(x, y, z) => triple(*x, *y, *z),
        ConfigValue::Vector2(x, y) => Value::List(vec![Value::Float(*x), Value::Float(*y)]),
        ConfigValue::Vector3(x, y, z) => triple(*x, *y, *z),
        ConfigValue::Range(from, to) => Value::List(vec![Value::Int(*from), Value::Int(*to)]),
        ConfigValue::List(items) | ConfigValue::Set(items) => {
            Value::List(items.iter().map(encode).collect())
        }
        ConfigValue::Map(fields)
        | ConfigValue::Object(fields)
        | ConfigValue::Compound(fields) => {
            let mut map = Map::new();
            for (name, held) in fields {
                map.insert(name.as_str(), encode(held));
            }
            Value::Map(map)
        }
    }
}

fn triple(x: f64, y: f64, z: f64) -> Value {
    Value::List(vec![Value::Float(x), Value::Float(y), Value::Float(z)])
}

fn decode(value: &Value) -> Option<ConfigValue> {
    let map = value.as_map()?;
    let kind = map.get("kind").and_then(Value::as_str).and_then(kind_from_name)?;
    decode_inner(kind, map.get("value")?)
}

fn decode_inner(kind: ConfigType, value: &Value) -> Option<ConfigValue> {
    Some(match kind {
        ConfigType::Boolean => ConfigValue::Boolean(value.as_bool()?),
        ConfigType::Integer => ConfigValue::Integer(value.as_int()?),
        ConfigType::Float => ConfigValue::Float(value.as_float()?),
        ConfigType::Percentage => ConfigValue::Percentage(value.as_float()?),
        ConfigType::Duration => ConfigValue::Duration(u64::try_from(value.as_int()?).ok()?),
        ConfigType::Color => ConfigValue::Color(u32::try_from(value.as_int()?).ok()?),
        ConfigType::String => ConfigValue::String(text_of(value)?),
        ConfigType::Text => ConfigValue::Text(text_of(value)?),
        ConfigType::Enum => ConfigValue::Enum(text_of(value)?),
        ConfigType::Keybind => ConfigValue::Keybind(text_of(value)?),
        ConfigType::ResourceId => ConfigValue::ResourceId(text_of(value)?),
        ConfigType::ItemId => ConfigValue::ItemId(text_of(value)?),
        ConfigType::BlockId => ConfigValue::BlockId(text_of(value)?),
        ConfigType::EntityId => ConfigValue::EntityId(text_of(value)?),
        ConfigType::FilePath => ConfigValue::FilePath(PathBuf::from(text_of(value)?)),
        ConfigType::DirectoryPath => ConfigValue::DirectoryPath(PathBuf::from(text_of(value)?)),
        ConfigType::Position => {
            let items = value.as_list()?;
            ConfigValue::Position(floats(items, 0)?, floats(items, 1)?, floats(items, 2)?)
        }
        ConfigType::Vector2 => {
            let items = value.as_list()?;
            ConfigValue::Vector2(floats(items, 0)?, floats(items, 1)?)
        }
        ConfigType::Vector3 => {
            let items = value.as_list()?;
            ConfigValue::Vector3(floats(items, 0)?, floats(items, 1)?, floats(items, 2)?)
        }
        ConfigType::Range => {
            let items = value.as_list()?;
            ConfigValue::Range(items.first()?.as_int()?, items.get(1)?.as_int()?)
        }
        ConfigType::List => ConfigValue::List(decode_list(value)?),
        ConfigType::Set => ConfigValue::Set(decode_list(value)?),
        ConfigType::Map => ConfigValue::Map(decode_map(value)?),
        ConfigType::Object => ConfigValue::Object(decode_map(value)?),
        ConfigType::Compound => ConfigValue::Compound(decode_map(value)?),
    })
}

fn decode_list(value: &Value) -> Option<Vec<ConfigValue>> {
    value
        .as_list()?
        .iter()
        .map(decode)
        .collect::<Option<Vec<_>>>()
}

fn decode_map(value: &Value) -> Option<BTreeMap<String, ConfigValue>> {
    let map = value.as_map()?;
    let mut fields = BTreeMap::new();
    for (name, held) in map.iter() {
        fields.insert(name.to_owned(), decode(held)?);
    }
    Some(fields)
}

fn text_of(value: &Value) -> Option<String> {
    value.as_str().map(str::to_owned)
}

fn floats(items: &[Value], at: usize) -> Option<f64> {
    items.get(at)?.as_float()
}

fn kind_from_name(name: &str) -> Option<ConfigType> {
    Some(match name {
        "boolean" => ConfigType::Boolean,
        "integer" => ConfigType::Integer,
        "float" => ConfigType::Float,
        "string" => ConfigType::String,
        "text" => ConfigType::Text,
        "enum" => ConfigType::Enum,
        "color" => ConfigType::Color,
        "keybind" => ConfigType::Keybind,
        "file_path" => ConfigType::FilePath,
        "directory_path" => ConfigType::DirectoryPath,
        "resource_id" => ConfigType::ResourceId,
        "item_id" => ConfigType::ItemId,
        "block_id" => ConfigType::BlockId,
        "entity_id" => ConfigType::EntityId,
        "position" => ConfigType::Position,
        "vector2" => ConfigType::Vector2,
        "vector3" => ConfigType::Vector3,
        "range" => ConfigType::Range,
        "list" => ConfigType::List,
        "set" => ConfigType::Set,
        "map" => ConfigType::Map,
        "object" => ConfigType::Object,
        "compound" => ConfigType::Compound,
        "duration" => ConfigType::Duration,
        "percentage" => ConfigType::Percentage,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn tree() -> Project {
        let mut project = Project::new("s89:p", "Demo");
        project.add_node("root", "on_start", NodeKind::Event).expect("a node");
        project.add_node("on_start", "spawn", NodeKind::Action).expect("a child");
        project.add_node("root", "tick", NodeKind::Flow).expect("a node");
        project
    }

    #[test]
    fn a_project_is_an_id_a_name_and_a_tree_of_nodes() {
        let _serial = serial();
        let mut project = tree();
        assert_eq!(project.id, "s89:p");
        assert_eq!(project.name, "Demo");
        assert_eq!(project.root, "root");
        assert_eq!(project.node_count(), 4);
        assert_eq!(
            project.tree_order(),
            vec!["root", "on_start", "spawn", "tick"]
        );
        assert_eq!(project.children_of("root"), vec!["on_start", "tick"]);
        assert_eq!(project.children_of("spawn"), Vec::<String>::new());
        assert_eq!(
            project.kind_counts(),
            vec![
                (NodeKind::Root, 1),
                (NodeKind::Event, 1),
                (NodeKind::Action, 1),
                (NodeKind::Flow, 1),
            ]
        );
        assert_eq!(
            project.node("on_start").map(|node| node.kind),
            Some(NodeKind::Event)
        );

        assert_eq!(
            project.add_node("root", "on_start", NodeKind::Action).unwrap_err(),
            Error::DuplicateId {
                id: "on_start".to_owned()
            }
        );
        assert_eq!(
            project.add_node("nope", "x", NodeKind::Action).unwrap_err(),
            Error::MissingParent {
                parent: "nope".to_owned()
            }
        );
        assert_eq!(
            project.add_node("root", "  ", NodeKind::Action).unwrap_err(),
            Error::EmptyId
        );

        assert_eq!(project.remove_node("on_start").expect("present"), 2, "it went with its child");
        assert_eq!(project.node_count(), 2);
        assert_eq!(project.tree_order(), vec!["root", "tick"]);
        assert_eq!(
            project.remove_node("on_start").unwrap_err(),
            Error::MissingNode {
                id: "on_start".to_owned()
            }
        );
        assert_eq!(project.remove_node("root").unwrap_err(), Error::RootRemoval);
    }

    #[test]
    fn moving_and_renaming_refuse_a_cycle_and_a_duplicate() {
        let _serial = serial();
        let mut project = tree();
        assert_eq!(
            project.move_node("on_start", "spawn").unwrap_err(),
            Error::Cycle {
                id: "on_start".to_owned(),
                parent: "spawn".to_owned()
            }
        );
        assert_eq!(
            project.move_node("on_start", "on_start").unwrap_err(),
            Error::Cycle {
                id: "on_start".to_owned(),
                parent: "on_start".to_owned()
            }
        );
        assert_eq!(
            project.move_node("spawn", "ghost").unwrap_err(),
            Error::MissingParent {
                parent: "ghost".to_owned()
            }
        );
        project.move_node("spawn", "tick").expect("a real parent");
        assert_eq!(project.children_of("tick"), vec!["spawn"]);
        assert_eq!(project.children_of("on_start"), Vec::<String>::new());

        assert_eq!(
            project.rename_node("spawn", "tick").unwrap_err(),
            Error::DuplicateId {
                id: "tick".to_owned()
            }
        );
        assert_eq!(
            project.rename_node("ghost", "x").unwrap_err(),
            Error::MissingNode {
                id: "ghost".to_owned()
            }
        );
        project.rename_node("spawn", "respawn").expect("a fresh name");
        assert_eq!(project.children_of("tick"), vec!["respawn"]);
        assert!(project.has_node("respawn"));
    }

    #[test]
    fn properties_read_and_write_by_path_and_refuse_missing_and_wrong_kind() {
        let _serial = serial();
        let mut project = tree();
        project
            .set_property("on_start.spawn.size", ConfigValue::Float(1.5))
            .expect("a float size");
        assert_eq!(
            project.get_property("on_start.spawn.size"),
            Ok(ConfigValue::Float(1.5))
        );
        assert_eq!(
            project
                .list_properties("on_start.spawn")
                .expect("a node"),
            vec![("size".to_owned(), ConfigValue::Float(1.5))]
        );
        assert_eq!(
            project.get_property("on_start.spawn.nope").unwrap_err(),
            Error::MissingPath {
                path: "on_start.spawn.nope".to_owned()
            }
        );
        assert_eq!(
            project.get_property("ghost.spawn.size").unwrap_err(),
            Error::MissingPath {
                path: "ghost.spawn.size".to_owned()
            }
        );
        let wrong = project
            .set_property("on_start.spawn.size", ConfigValue::String("big".to_owned()))
            .unwrap_err();
        assert_eq!(
            wrong,
            Error::WrongKind {
                path: "on_start.spawn.size".to_owned(),
                expected: "float".to_owned(),
                found: "string".to_owned()
            }
        );
        assert_eq!(wrong.to_string(), "'on_start.spawn.size' is a float, not a string");
    }

    #[test]
    fn undo_and_redo_restore_exact_values_and_a_new_mutation_clears_redo() {
        let _serial = serial();
        let mut project = tree();
        let base = project.undo_depth();
        project
            .set_property("on_start.spawn.size", ConfigValue::Float(0.5))
            .expect("a float");
        project
            .set_property("on_start.spawn.size", ConfigValue::Float(1.5))
            .expect("a float");
        project
            .set_property("on_start.spawn.size", ConfigValue::Float(2.5))
            .expect("a float");
        assert_eq!(project.undo_depth(), base + 3);
        assert_eq!(project.redo_depth(), 0);
        assert_eq!(
            project.get_property("on_start.spawn.size"),
            Ok(ConfigValue::Float(2.5))
        );

        assert!(project.undo());
        assert_eq!(
            project.get_property("on_start.spawn.size"),
            Ok(ConfigValue::Float(1.5))
        );
        assert!(project.undo());
        assert_eq!(
            project.get_property("on_start.spawn.size"),
            Ok(ConfigValue::Float(0.5)),
            "two steps back is the first value"
        );
        assert_eq!(project.undo_depth(), base + 1);
        assert_eq!(project.redo_depth(), 2);

        assert!(project.redo());
        assert_eq!(
            project.get_property("on_start.spawn.size"),
            Ok(ConfigValue::Float(1.5))
        );
        assert!(project.redo());
        assert_eq!(
            project.get_property("on_start.spawn.size"),
            Ok(ConfigValue::Float(2.5))
        );
        assert!(!project.redo());

        assert!(project.undo());
        assert_eq!(project.redo_depth(), 1);
        project
            .set_property("on_start.spawn.size", ConfigValue::Float(9.0))
            .expect("a float");
        assert_eq!(project.redo_depth(), 0, "a new mutation drops the redo stack");
        assert!(!project.redo());
    }

    #[test]
    fn a_project_saves_and_loads_byte_stable_and_import_rebuilds_it() {
        let _serial = serial();
        let mut project = tree();
        project
            .set_property("on_start.spawn.size", ConfigValue::Float(1.5))
            .expect("a float");
        project
            .set_property("tick.name", ConfigValue::String("Tick".to_owned()))
            .expect("a string");

        let text = save_project(&project);
        assert!(text.starts_with(r#"{"format":1,"id":"s89:p","name":"Demo","root":"root""#));
        assert_eq!(save_project(&project), text, "two saves compare byte for byte");

        let loaded = import_project(&text).expect("well formed");
        assert_eq!(loaded, project, "the same ids, name, root and nodes");
        assert_eq!(save_project(&loaded), text, "a byte-stable round trip");
        assert_eq!(loaded.undo_depth(), 0, "the undo stack is not part of the document");

        assert_eq!(save_document(&project), text);
        assert_eq!(import_document(&text).expect("the same"), project);
        assert!(matches!(import_project("not json"), Err(Error::Malformed(_))));
        assert!(import_project(r#"{"id":"x"}"#).is_err());
    }

    #[test]
    fn a_preview_summarises_the_tree_with_counts_per_kind() {
        let _serial = serial();
        let project = tree();
        let preview = project.preview();
        assert!(preview.contains("'Demo': 4 node(s)"));
        assert!(preview.contains("event 1"));
        assert!(preview.contains("action 1"));
        assert!(preview.contains("flow 1"));
        assert_eq!(preview.lines().count(), 5, "a summary line and one per node");
        assert!(preview.lines().any(|line| line == "    spawn [action] 0 prop(s)"));
    }

    #[test]
    fn the_registries_and_schemas_are_real() {
        let _serial = serial();
        clear();
        assert!(register_custom_node("s89:custom", NodeKind::Getter));
        assert!(!register_custom_node("s89:custom", NodeKind::Action), "once");
        assert!(get_node_types().iter().any(|name| name == "s89:custom"));
        assert_eq!(get_custom_nodes().len(), 1);

        assert!(register_custom_property("s89:weight", ConfigType::Integer));
        assert_eq!(property_kind("s89:weight"), Some(ConfigType::Integer));
        assert!(get_property_schema("s89:weight").expect("declared").contains("\"kind\":\"integer\""));
        assert_eq!(get_property_schema("s89:absent"), None);

        assert!(register_custom_panel("s89:panel", "Panel"));
        assert!(register_custom_inspector("s89:inspector", "transform"));
        assert!(register_custom_generator("s89:gen", "rust"));
        assert_eq!(get_panels(), vec![("s89:panel".to_owned(), "Panel".to_owned())]);
        assert_eq!(get_inspectors().len(), 1);
        assert_eq!(get_generators(), vec!["s89:gen".to_owned()]);
        assert_eq!(get_templates().len(), 4);
        assert_eq!(get_graph_types().len(), 4);
        assert!(!get_supported_features().is_empty());
        assert_eq!(get_asset_types().len(), 9);
        assert!(get_asset_metadata("texture").expect("known").contains("\"importable\":true"));
        assert_eq!(get_asset_metadata("nope"), None);
        assert!(get_component_schema("transform").expect("known").contains("\"name\":\"rotation\""));
        assert_eq!(get_component_schema("nope"), None);
        assert!(get_schema().contains("\"kinds\""));
        clear();
        assert!(get_custom_nodes().is_empty());
        assert_eq!(property_kind("s89:weight"), None);
    }

    #[test]
    fn compile_build_and_package_report_the_project() {
        let _serial = serial();
        clear();
        let project = tree();
        assert!(compile(&project).is_clean());
        assert_eq!(validate_document(&project).count(), 0);

        let built = build(&project);
        assert_eq!(built.artifact, "s89:p.mod.json");
        assert_eq!(built.nodes, 4);
        assert!(built.ok);
        assert_eq!(get_build_status(), Status::Built);
        assert_eq!(get_build_logs().len(), 1);
        assert!(get_build_logs()[0].starts_with("built s89:p.mod.json: 4 node(s)"));
        assert!(package(&project).contains("\"kind\":\"editor_package\""));
    }

    #[test]
    fn the_clients_editor_window_is_refused_with_a_sentence() {
        let _serial = serial();
        assert_eq!(
            open_project("s89:p").unwrap_err(),
            Error::Refused(why_unknown("editor_window"))
        );
        assert!(open_document("s89:p").is_err());
        assert!(hot_reload().is_err());
        assert_eq!(refusals().len(), 2);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
        assert!(why_unknown("editor_window").contains("editor UI"));
        assert_eq!(
            why_unknown("nothing"),
            "nothing is not something this host can see"
        );
    }

    #[test]
    fn the_selection_lists_in_tree_order_and_close_clears_the_active_project() {
        let _serial = serial();
        clear();
        let mut project = create_project("s89:active", "Active");
        assert_eq!(get_active_project().map(|held| held.id), Some("s89:active".to_owned()));
        project.add_node("root", "a", NodeKind::Action).expect("a node");
        project.add_node("root", "b", NodeKind::Action).expect("a node");

        let mut selection = Selection::default();
        assert!(selection.select("b"));
        assert!(!selection.select("b"), "once");
        assert!(selection.select("a"));
        assert_eq!(selection.len(), 2);
        assert!(selection.contains("a"));
        assert_eq!(selection.list(&project), vec!["a", "b"], "tree order, not pick order");
        assert!(!selection.toggle("a"), "was on, so off");
        assert!(selection.toggle("a"), "was off, so on");
        assert!(selection.deselect("b"));
        assert_eq!(selection.len(), 1);
        assert_eq!(selection.clear(), 1);
        assert!(selection.is_empty());

        assert!(close_project());
        assert!(!close_project());
        assert_eq!(get_active_project(), None);
        assert!(get_diagnostics().is_empty());
    }
}
