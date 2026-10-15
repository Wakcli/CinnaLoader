//! Resources: a mod's own assets, found by id, overridable by priority, reloadable.
//!
//! The client's renderer is not reachable from here — that is the `TextureRegistration` and
//! `ModelRegistration` capability, which this host answers `never` to — so this is what a mod
//! uses for the resources it reads itself: a stack of packs, looked up by
//! `namespace:path`, with the loader reading a mod's `assets/` directory as one of them.
//!
//! ```
//! use cinnaloader::prelude::*;
//!
//! let mut pack = ResourcePack::memory("hello", 0);
//! pack.insert("textures/gui/menu.png", b"not really a png".to_vec());
//! resources::register_pack(pack);
//! assert_eq!(
//!     resources::read_text(&"hello:textures/gui/menu.png".parse::<ResourceId>().unwrap()),
//!     Some("not really a png".to_owned())
//! );
//! # resources::unregister_pack("hello");
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// Where a resource comes from and what it is called.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResourceId {
    namespace: String,
    path: String,
}

impl ResourceId {
    /// Builds one, or `None` when either half is empty or the path leaves the pack.
    #[must_use]
    pub fn new(namespace: &str, path: &str) -> Option<Self> {
        let namespace = namespace.trim().to_ascii_lowercase();
        let path = path.trim().trim_start_matches('/').to_owned();
        let usable = |text: &str| {
            !text.is_empty()
                && !text.contains("..")
                && !text.contains('\\')
                && !text.contains(':')
                && text
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || "._-/".contains(character))
        };
        match usable(&namespace) && usable(&path) && !path.contains("//") {
            true => Some(Self { namespace, path }),
            false => None,
        }
    }

    #[must_use]
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// What kind of file it looks like, from the extension.
    #[must_use]
    pub fn kind(&self) -> ResourceKind {
        ResourceKind::from_path(&self.path)
    }

    /// The same id, under another namespace.
    #[must_use]
    pub fn with_namespace(&self, namespace: &str) -> Option<Self> {
        Self::new(namespace, &self.path)
    }
}

impl fmt::Display for ResourceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.namespace, self.path)
    }
}

impl FromStr for ResourceId {
    type Err = ();

    fn from_str(text: &str) -> Result<Self, ()> {
        let (namespace, path) = text.split_once(':').ok_or(())?;
        Self::new(namespace, path).ok_or(())
    }
}

/// The same id, as a value a caller can compare and store.
pub type ResourceLocation = ResourceId;

/// What a resource is, guessed from its extension, for a caller that wants to sort them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ResourceKind {
    Texture,
    Model,
    Sound,
    Shader,
    Language,
    Animation,
    Particle,
    Font,
    Structure,
    Data,
    Unknown,
}

impl ResourceKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Texture => "texture",
            Self::Model => "model",
            Self::Sound => "sound",
            Self::Shader => "shader",
            Self::Language => "language",
            Self::Animation => "animation",
            Self::Particle => "particle",
            Self::Font => "font",
            Self::Structure => "structure",
            Self::Data => "data",
            Self::Unknown => "unknown",
        }
    }

    /// Reads it from a path: `textures/gui/menu.png` is a texture, and so on.
    #[must_use]
    pub fn from_path(path: &str) -> Self {
        let lower = path.to_ascii_lowercase();
        let leading = lower.split('/').next().unwrap_or_default();
        match leading {
            "textures" | "texture" => Self::Texture,
            "models" | "model" => Self::Model,
            "sounds" | "sound" => Self::Sound,
            "shaders" | "shader" => Self::Shader,
            "lang" | "language" => Self::Language,
            "animations" | "animation" => Self::Animation,
            "particles" | "particle" => Self::Particle,
            "fonts" | "font" => Self::Font,
            "structures" | "structure" => Self::Structure,
            "data" => Self::Data,
            _ => match lower.rsplit('.').next() {
                Some("png" | "jpg" | "jpeg" | "tga" | "ktx") => Self::Texture,
                Some("ogg" | "wav" | "fsb") => Self::Sound,
                Some("json" | "toml" | "yaml" | "yml") => Self::Data,
                _ => Self::Unknown,
            },
        }
    }
}

impl fmt::Display for ResourceKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// What the loader knows about one resource without reading it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceMetadata {
    pub id: ResourceId,
    pub kind: ResourceKind,
    pub size: usize,
    /// FNV-1a, as [`crate::fs::hash_bytes`].
    pub hash: u64,
    /// The pack it came from.
    pub source: String,
}

impl fmt::Display for ResourceMetadata {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ({} bytes, {}, from {})",
            self.id,
            self.size,
            self.kind,
            self.source
        )
    }
}

/// Where a pack keeps its resources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Provider {
    /// In memory: what a mod registers at runtime.
    Memory(BTreeMap<String, Vec<u8>>),
    /// A directory inside the mod's own folder, read on demand.
    Directory(PathBuf),
}

impl Provider {
    /// Whether the provider has a path, and what it is.
    fn get(&self, path: &str) -> Option<Vec<u8>> {
        match self {
            Self::Memory(files) => files.get(path).cloned(),
            Self::Directory(directory) => {
                // Everything goes through the sandbox, so a pack cannot read outside the
                // mod's own directory even if a path in it says `..`.
                let relative = directory.join(path);
                crate::fs::read(&relative.to_string_lossy()).ok()
            }
        }
    }

    /// Every path it holds, in name order.
    fn paths(&self) -> Vec<String> {
        match self {
            Self::Memory(files) => files.keys().cloned().collect(),
            Self::Directory(directory) => {
                let Ok(base) = crate::fs::resolve(&directory.to_string_lossy()) else {
                    return Vec::new();
                };
                let mut found = Vec::new();
                for path in crate::fs::walk(&base.to_string_lossy()).unwrap_or_default() {
                    if path.is_file() {
                        if let Ok(relative) = path.strip_prefix(&base) {
                            found.push(relative.to_string_lossy().replace('\\', "/"));
                        }
                    }
                }
                found.sort();
                found
            }
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Self::Memory(files) => files.len(),
            Self::Directory(_) => self.paths().len(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// One pack of resources, with the priority that decides who wins a shared id.
///
/// The name identifies the pack; the namespace is what its ids are spelled with, so two
/// packs can offer the same namespace and the higher priority wins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourcePack {
    pub name: String,
    /// What its resources are called: `namespace:path`.
    pub namespace: String,
    /// Higher wins. Two packs with the same priority are ordered by name.
    pub priority: i32,
    pub provider: Provider,
}

impl ResourcePack {
    /// A pack with nothing in it yet, offering its own name as the namespace.
    #[must_use]
    pub fn memory(name: &str, priority: i32) -> Self {
        Self {
            name: name.to_owned(),
            namespace: name.to_owned(),
            priority,
            provider: Provider::Memory(BTreeMap::new()),
        }
    }

    /// A pack that reads a directory inside the mod's own folder, on each lookup.
    #[must_use]
    pub fn directory(name: &str, directory: &str, priority: i32) -> Self {
        Self {
            name: name.to_owned(),
            namespace: name.to_owned(),
            priority,
            provider: Provider::Directory(PathBuf::from(directory)),
        }
    }

    /// The same pack, offering another namespace. Two packs naming one namespace is how a
    /// resource gets overridden.
    #[must_use]
    pub fn namespace(mut self, namespace: &str) -> Self {
        self.namespace = namespace.to_owned();
        self
    }

    /// Adds a resource to a pack that keeps them in memory.
    pub fn insert(&mut self, path: &str, bytes: Vec<u8>) -> bool {
        match &mut self.provider {
            Provider::Memory(files) => {
                files.insert(path.trim_start_matches('/').to_owned(), bytes);
                true
            }
            Provider::Directory(_) => false,
        }
    }

    /// Reads one resource out of this pack alone.
    #[must_use]
    pub fn get(&self, path: &str) -> Option<Vec<u8>> {
        self.provider.get(path.trim_start_matches('/'))
    }

    /// Every path this pack holds.
    #[must_use]
    pub fn paths(&self) -> Vec<String> {
        self.provider.paths()
    }

    /// What the pack offers, as full ids.
    #[must_use]
    pub fn ids(&self) -> Vec<ResourceId> {
        self.paths()
            .into_iter()
            .filter_map(|path| ResourceId::new(&self.namespace, &path))
            .collect()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.provider.is_empty()
    }
}

/// One resource two packs both offer, and who won.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceOverride {
    pub id: ResourceId,
    pub winner: String,
    pub losers: Vec<String>,
}

/// The packs, in the order a lookup should try them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResourceStack {
    packs: Vec<ResourcePack>,
}

impl ResourceStack {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a pack, or replaces one of the same name.
    pub fn push(&mut self, pack: ResourcePack) {
        self.packs.retain(|held| held.name != pack.name);
        self.packs.push(pack);
        self.sort();
    }

    /// Removes a pack by name, and answers whether it was there.
    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.packs.len();
        self.packs.retain(|pack| pack.name != name);
        self.packs.len() != before
    }

    fn sort(&mut self) {
        // Highest priority first, then by name so the order never depends on insertion.
        self.packs
            .sort_by(|left, right| right.priority.cmp(&left.priority).then(left.name.cmp(&right.name)));
    }

    /// Every pack, in lookup order.
    #[must_use]
    pub fn packs(&self) -> &[ResourcePack] {
        &self.packs
    }

    /// The pack with this name.
    #[must_use]
    pub fn pack(&self, name: &str) -> Option<&ResourcePack> {
        self.packs.iter().find(|pack| pack.name == name)
    }

    /// The pack that would win this id.
    #[must_use]
    pub fn winner(&self, id: &ResourceId) -> Option<&ResourcePack> {
        self.packs
            .iter()
            .find(|pack| pack.get(id.path()).is_some())
    }

    /// Every pack that offers this id, in lookup order.
    #[must_use]
    pub fn providers(&self, id: &ResourceId) -> Vec<&ResourcePack> {
        self.packs
            .iter()
            .filter(|pack| pack.get(id.path()).is_some())
            .collect()
    }

    /// Every id any pack offers, under `namespace`.
    #[must_use]
    pub fn ids(&self, namespace: &str) -> Vec<ResourceId> {
        let mut seen: BTreeMap<ResourceId, ()> = BTreeMap::new();
        for pack in &self.packs {
            if pack.namespace != namespace {
                continue;
            }
            for id in pack.ids() {
                seen.insert(id, ());
            }
        }
        seen.into_keys().collect()
    }

    /// Every id more than one pack offers.
    #[must_use]
    pub fn overrides(&self) -> Vec<ResourceOverride> {
        let mut by_id: BTreeMap<ResourceId, Vec<&ResourcePack>> = BTreeMap::new();
        for pack in &self.packs {
            for path in pack.paths() {
                let Some(id) = ResourceId::new(&pack.namespace, &path) else {
                    continue;
                };
                by_id.entry(id).or_default().push(pack);
            }
        }
        let mut overrides = Vec::new();
        for (id, packs) in by_id {
            if packs.len() < 2 {
                continue;
            }
            overrides.push(ResourceOverride {
                id,
                winner: packs[0].name.clone(),
                losers: packs[1..].iter().map(|pack| pack.name.clone()).collect(),
            });
        }
        overrides
    }
}

/// Something a resource needs to be there, for `get_missing_resources`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceDependency {
    pub id: ResourceId,
    /// Who needs it.
    pub required_by: String,
    pub optional: bool,
}

/// A caller that wants to be told when resources are reloaded.
pub trait ResourceReloadListener: Send {
    /// What it is called, for the report.
    fn name(&self) -> &str;

    /// Called after the stack has been reloaded.
    fn reload(&mut self);

    /// The namespaces it cares about; empty means all of them.
    fn namespaces(&self) -> Vec<String> {
        Vec::new()
    }
}

/// A listener that is a plain function, for a mod that does not want a type.
struct FnListener {
    name: String,
    namespaces: Vec<String>,
    run: Box<dyn FnMut() + Send>,
}

impl ResourceReloadListener for FnListener {
    fn name(&self) -> &str {
        &self.name
    }

    fn reload(&mut self) {
        (self.run)();
    }

    fn namespaces(&self) -> Vec<String> {
        self.namespaces.clone()
    }
}

/// The stack, the listeners and what went wrong last time.
#[derive(Default)]
pub struct ResourceManager {
    stack: ResourceStack,
    listeners: Vec<Box<dyn ResourceReloadListener>>,
    dependencies: Vec<ResourceDependency>,
    errors: Vec<String>,
    reloads: u64,
}

impl fmt::Debug for ResourceManager {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResourceManager")
            .field("packs", &self.stack.packs().len())
            .field("listeners", &self.listeners.len())
            .field("dependencies", &self.dependencies.len())
            .field("errors", &self.errors.len())
            .field("reloads", &self.reloads)
            .finish()
    }
}

impl ResourceManager {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn stack(&self) -> &ResourceStack {
        &self.stack
    }

    /// Finds an id across the stack, and answers with the bytes and where they came from.
    #[must_use]
    pub fn resolve(&self, id: &ResourceId) -> Option<(Vec<u8>, String)> {
        self.stack
            .winner(id)
            .and_then(|pack| pack.get(id.path()).map(|bytes| (bytes, pack.name.clone())))
    }

    /// Runs the listeners, then says how many ran.
    pub fn reload(&mut self, namespaces: &[String]) -> usize {
        self.reloads += 1;
        let mut ran = 0;
        for listener in &mut self.listeners {
            let wanted = listener.namespaces().is_empty()
                || namespaces.is_empty()
                || listener
                    .namespaces()
                    .iter()
                    .any(|namespace| namespaces.contains(namespace));
            if wanted {
                listener.reload();
                ran += 1;
            }
        }
        self.errors = self.check();
        ran
    }

    /// Everything wrong with the stack as it stands.
    fn check(&self) -> Vec<String> {
        let mut errors = Vec::new();
        for pack in self.stack.packs() {
            if pack.is_empty() {
                errors.push(format!("pack '{}' holds nothing", pack.name));
            }
            for path in pack.paths() {
                if ResourceId::new(&pack.name, &path).is_none() {
                    errors.push(format!("pack '{}' has an unusable path: {path}", pack.name));
                }
            }
        }
        for dependency in &self.dependencies {
            if !dependency.optional && self.stack.winner(&dependency.id).is_none() {
                errors.push(format!(
                    "{} needs {}, which no pack offers",
                    dependency.required_by, dependency.id
                ));
            }
        }
        errors
    }
}

/// The manager this process uses. One per mod, because each mod has its own copy of the SDK.
fn manager() -> &'static Mutex<ResourceManager> {
    static MANAGER: OnceLock<Mutex<ResourceManager>> = OnceLock::new();
    MANAGER.get_or_init(|| Mutex::new(ResourceManager::new()))
}

fn with<T>(body: impl FnOnce(&mut ResourceManager) -> T) -> T {
    let mut guard = manager().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Adds a pack to the stack, or replaces one with the same name.
pub fn register_pack(pack: ResourcePack) {
    with(|manager| manager.stack.push(pack));
}

/// Removes a pack by name.
pub fn unregister_pack(name: &str) -> bool {
    with(|manager| manager.stack.remove(name))
}

/// Every pack in lookup order, by name.
#[must_use]
pub fn get_active_packs() -> Vec<String> {
    with(|manager| {
        manager
            .stack
            .packs()
            .iter()
            .map(|pack| pack.name.clone())
            .collect()
    })
}

/// The stack itself: the packs, and who wins what.
#[must_use]
pub fn get_resource_stack() -> Vec<(String, String, i32, usize)> {
    with(|manager| {
        manager
            .stack
            .packs()
            .iter()
            .map(|pack| {
                (
                    pack.name.clone(),
                    pack.namespace.clone(),
                    pack.priority,
                    pack.provider.len(),
                )
            })
            .collect()
    })
}

/// Every id more than one pack offers, and who wins.
#[must_use]
pub fn get_overrides() -> Vec<ResourceOverride> {
    with(|manager| manager.stack.overrides())
}

/// Registers a resource in memory, in a pack named for the id's namespace.
pub fn register(id: &ResourceId, bytes: Vec<u8>) -> bool {
    with(|manager| {
        let name = id.namespace().to_owned();
        let mut pack = match manager.stack.pack(&name) {
            Some(pack) => pack.clone(),
            None => ResourcePack::memory(&name, 0),
        };
        let inserted = pack.insert(id.path(), bytes);
        manager.stack.push(pack);
        inserted
    })
}

/// Removes a resource from the pack it is in, and answers whether it was there.
pub fn unregister(id: &ResourceId) -> bool {
    with(|manager| {
        let names: Vec<String> = manager
            .stack
            .packs()
            .iter()
            .filter(|pack| matches!(pack.provider, Provider::Memory(_)))
            .map(|pack| pack.name.clone())
            .collect();
        let mut removed = false;
        for name in names {
            let Some(pack) = manager.stack.pack(&name).cloned() else {
                continue;
            };
            let mut pack = pack;
            if let Provider::Memory(files) = &mut pack.provider {
                removed |= files.remove(id.path()).is_some();
            }
            manager.stack.push(pack);
        }
        removed
    })
}

/// The bytes of a resource, from whichever pack wins it.
#[must_use]
pub fn get(id: &ResourceId) -> Option<Vec<u8>> {
    with(|manager| manager.resolve(id).map(|(bytes, _)| bytes))
}

/// The same, with where it came from.
#[must_use]
pub fn open(id: &ResourceId) -> Option<(Vec<u8>, String)> {
    with(|manager| manager.resolve(id))
}

/// Bytes, under the name a caller may prefer.
#[must_use]
pub fn read_bytes(id: &ResourceId) -> Option<Vec<u8>> {
    get(id)
}

/// Text, which is what a language file or a data file is.
#[must_use]
pub fn read_text(id: &ResourceId) -> Option<String> {
    get(id).map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

/// Bytes, and an error saying why not.
pub fn read(id: &ResourceId) -> Result<Vec<u8>, String> {
    get(id).ok_or_else(|| format!("no pack offers {id}"))
}

#[must_use]
pub fn exists(id: &ResourceId) -> bool {
    with(|manager| manager.stack.winner(id).is_some())
}

/// Every id in the stack, or only those in `namespace`.
#[must_use]
pub fn list(namespace: &str) -> Vec<ResourceId> {
    with(|manager| manager.stack.ids(namespace))
}

/// Every id that matches a path, with a `*` for a run of characters.
#[must_use]
pub fn find(pattern: &str) -> Vec<ResourceId> {
    let (namespace, path) = match pattern.split_once(':') {
        Some((namespace, path)) => (namespace.to_owned(), path.to_owned()),
        None => (String::new(), pattern.to_owned()),
    };
    with(|manager| {
        let namespaces: Vec<String> = match namespace.is_empty() {
            true => manager
                .stack
                .packs()
                .iter()
                .map(|pack| pack.name.clone())
                .collect(),
            false => vec![namespace],
        };
        let mut found = Vec::new();
        for namespace in namespaces {
            for id in manager.stack.ids(&namespace) {
                if matches_path(&path, id.path()) {
                    found.push(id);
                }
            }
        }
        found
    })
}

fn matches_path(pattern: &str, path: &str) -> bool {
    if !pattern.contains('*') {
        let prefix = pattern.trim_end_matches('/');
        return path == prefix || path.starts_with(&format!("{prefix}/"));
    }
    let mut rest = path;
    let mut parts = pattern.split('*').peekable();
    if let Some(first) = parts.next() {
        if !rest.starts_with(first) {
            return false;
        }
        rest = &rest[first.len()..];
    }
    while let Some(part) = parts.next() {
        match parts.peek() {
            None => return part.is_empty() || rest.ends_with(part),
            Some(_) => match rest.find(part) {
                Some(at) => rest = &rest[at + part.len()..],
                None => return false,
            },
        }
    }
    true
}

/// What the loader knows about a resource without handing over the bytes.
#[must_use]
pub fn metadata(id: &ResourceId) -> Option<ResourceMetadata> {
    with(|manager| {
        manager
            .resolve(id)
            .map(|(bytes, source)| ResourceMetadata {
                id: id.clone(),
                kind: id.kind(),
                size: bytes.len(),
                hash: crate::fs::hash_bytes(&bytes),
                source,
            })
    })
}

/// The namespace an id is in, which is the mod or pack that owns it.
#[must_use]
pub fn get_namespace(id: &ResourceId) -> String {
    id.namespace().to_owned()
}

/// The priority of a pack, which is what decides who wins a shared id.
#[must_use]
pub fn get_priority(name: &str) -> Option<i32> {
    with(|manager| manager.stack.pack(name).map(|pack| pack.priority))
}

/// Sets a pack's priority, and answers whether there was such a pack.
pub fn set_priority(name: &str, priority: i32) -> bool {
    with(|manager| {
        let Some(pack) = manager.stack.pack(name).cloned() else {
            return false;
        };
        let mut pack = pack;
        pack.priority = priority;
        manager.stack.push(pack);
        true
    })
}

/// Runs the reload listeners, and answers how many ran.
pub fn reload() -> usize {
    with(|manager| manager.reload(&[]))
}

/// The same, under the name a caller may look for.
pub fn reload_all() -> usize {
    reload()
}

/// Runs the listeners that asked for one namespace, in the given order.
pub fn reload_namespace(namespace: &str) -> usize {
    with(|manager| manager.reload(&[namespace.to_owned()]))
}

/// Adds a listener that is a function.
pub fn add_reload_listener(
    name: &str,
    namespaces: &[&str],
    run: impl FnMut() + Send + 'static,
) -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    with(|manager| {
        manager.listeners.push(Box::new(FnListener {
            name: name.to_owned(),
            namespaces: namespaces.iter().map(|name| (*name).to_owned()).collect(),
            run: Box::new(run),
        }));
    });
    id
}

/// Removes a listener by name, and answers whether there was one.
pub fn remove_reload_listener(name: &str) -> bool {
    with(|manager| {
        let before = manager.listeners.len();
        manager.listeners
            .retain(|listener| listener.name() != name);
        manager.listeners.len() != before
    })
}

/// The listeners in place, by name.
#[must_use]
pub fn get_reload_listeners() -> Vec<String> {
    with(|manager| {
        manager
            .listeners
            .iter()
            .map(|listener| listener.name().to_owned())
            .collect()
    })
}

/// Says that a resource has to be there.
pub fn depend(dependency: ResourceDependency) {
    with(|manager| manager.dependencies.push(dependency));
}

/// What is wrong with the stack: empty packs, unusable paths, unmet dependencies.
#[must_use]
pub fn validate() -> Vec<String> {
    with(|manager| {
        manager.errors = manager.check();
        manager.errors.clone()
    })
}

/// Dependencies nothing offers.
#[must_use]
pub fn get_missing_resources() -> Vec<ResourceId> {
    with(|manager| {
        manager
            .dependencies
            .iter()
            .filter(|dependency| {
                !dependency.optional && manager.stack.winner(&dependency.id).is_none()
            })
            .map(|dependency| dependency.id.clone())
            .collect()
    })
}

/// What the last check or reload found wrong.
#[must_use]
pub fn get_loading_errors() -> Vec<String> {
    with(|manager| manager.errors.clone())
}

/// How many times the stack has been reloaded.
#[must_use]
pub fn reload_count() -> u64 {
    with(|manager| manager.reloads)
}

/// Calls `watcher` when a resource is reloaded, which is what a caller that caches wants.
pub fn watch(id: &ResourceId, watcher: impl FnMut(&ResourceId) + Send + 'static) -> Option<u64> {
    if !exists(id) {
        return None;
    }
    let watched = id.clone();
    let mut watcher = watcher;
    static NEXT: AtomicU64 = AtomicU64::new(1_000);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    crate::events::subscribe_filtered(
        crate::events::kind::RESOURCE_RELOAD,
        crate::events::Priority::Normal,
        move |_| true,
        move |_| watcher(&watched),
    );
    Some(id)
}

/// The directory a mod's own assets live in, for a caller that wants to build a pack from it.
#[must_use]
pub fn assets_directory() -> Option<PathBuf> {
    crate::fs::resolve("assets").ok()
}

/// A pack over this mod's own `assets` directory, which is the one the loader would read.
#[must_use]
pub fn own_pack() -> Option<ResourcePack> {
    crate::fs::resolve("assets").ok()?;
    Some(ResourcePack::directory(&crate::metadata::id(), "assets", 0))
}

/// Every file directly inside a directory of the stack, for a caller listing what a mod ships.
#[must_use]
pub fn list_directory(id: &ResourceId) -> Vec<ResourceId> {
    find(&format!("{}:{}/", id.namespace(), id.path()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with(|manager| {
            *manager = ResourceManager::new();
        });
    }

    fn id(text: &str) -> ResourceId {
        text.parse().expect("a usable id")
    }

    #[test]
    fn an_id_needs_a_namespace_and_a_sane_path() {
        let _serial = serial();
        assert_eq!(
            id("mymod:textures/gui/menu.png").to_string(),
            "mymod:textures/gui/menu.png"
        );
        assert_eq!(id("Mymod:Textures/Gui.PNG").namespace(), "mymod");
        assert!("no-namespace".parse::<ResourceId>().is_err());
        assert!(":nothing".parse::<ResourceId>().is_err());
        assert!("mod:../escape".parse::<ResourceId>().is_err());
        assert!("mod:".parse::<ResourceId>().is_err());
        assert!("a::b".parse::<ResourceId>().is_err());
        assert_eq!(id("mod:textures/a.png").kind(), ResourceKind::Texture);
        assert_eq!(id("mod:sounds/a.ogg").kind(), ResourceKind::Sound);
        assert_eq!(id("mod:data/thing.json").kind(), ResourceKind::Data);
        assert_eq!(id("mod:weird").kind(), ResourceKind::Unknown);
    }

    #[test]
    fn a_pack_hands_out_what_was_put_in_it() {
        let _serial = serial();
        fresh();
        let mut pack = ResourcePack::memory("mymod", 0);
        assert!(pack.insert("textures/gui/menu.png", b"png bytes".to_vec()));
        register_pack(pack);
        assert!(exists(&id("mymod:textures/gui/menu.png")));
        assert_eq!(
            read_text(&id("mymod:textures/gui/menu.png")),
            Some("png bytes".to_owned())
        );
        assert_eq!(read(&id("mymod:textures/gui/menu.png")).ok(), Some(b"png bytes".to_vec()));
        assert!(read(&id("mymod:missing")).is_err());
        assert_eq!(metadata(&id("mymod:textures/gui/menu.png")).unwrap().size, 9);
        assert_eq!(list("mymod").len(), 1);
        assert_eq!(find("mymod:textures/*").len(), 1);
        assert!(unregister(&id("mymod:textures/gui/menu.png")));
        assert!(!exists(&id("mymod:textures/gui/menu.png")));
    }

    #[test]
    fn a_pack_with_a_higher_priority_wins_a_shared_id() {
        let _serial = serial();
        fresh();
        // Both packs offer the same namespace, so the priority decides.
        let mut low = ResourcePack::memory("low", 0).namespace("mymod");
        low.insert("data/thing.json", b"low".to_vec());
        let mut high = ResourcePack::memory("high", 10).namespace("mymod");
        high.insert("data/thing.json", b"high".to_vec());
        register_pack(low);
        register_pack(high);

        assert_eq!(get_active_packs(), vec!["high".to_owned(), "low".to_owned()]);
        assert_eq!(
            open(&id("mymod:data/thing.json")).map(|(_, source)| source),
            Some("high".to_owned())
        );
        assert_eq!(
            read_text(&id("mymod:data/thing.json")),
            Some("high".to_owned())
        );
        let overrides = get_overrides();
        assert_eq!(overrides.len(), 1);
        assert_eq!(overrides[0].id.to_string(), "mymod:data/thing.json");
        assert_eq!(overrides[0].winner, "high");
        assert_eq!(overrides[0].losers, vec!["low".to_owned()]);

        // Raising the other one changes who wins, without re-registering anything.
        assert!(set_priority("low", 20));
        assert_eq!(get_priority("low"), Some(20));
        assert_eq!(
            open(&id("mymod:data/thing.json")).map(|(_, source)| source),
            Some("low".to_owned())
        );
        assert_eq!(get_overrides()[0].winner, "low");
        assert_eq!(get_resource_stack().len(), 2);
    }

    #[test]
    fn a_reload_runs_the_listeners_that_asked_for_it() {
        let _serial = serial();
        fresh();
        let calls = std::sync::Arc::new(Mutex::new(Vec::new()));
        let into = std::sync::Arc::clone(&calls);
        add_reload_listener("every", &[], move || {
            into.lock().unwrap().push("every".to_owned());
        });
        let into = std::sync::Arc::clone(&calls);
        add_reload_listener("mine", &["mymod"], move || {
            into.lock().unwrap().push("mine".to_owned());
        });
        let into = std::sync::Arc::clone(&calls);
        add_reload_listener("other", &["othermod"], move || {
            into.lock().unwrap().push("other".to_owned());
        });

        assert_eq!(reload(), 3, "every listener runs when no namespace was named");
        assert_eq!(
            get_reload_listeners(),
            vec!["every".to_owned(), "mine".to_owned(), "other".to_owned()]
        );
        calls.lock().unwrap().clear();
        assert_eq!(
            reload_namespace("mymod"),
            2,
            "the one that asked for mymod and the one that asked for everything"
        );
        assert_eq!(*calls.lock().unwrap(), vec!["every".to_owned(), "mine".to_owned()]);
        assert_eq!(reload_count(), 2);
        assert!(remove_reload_listener("mine"));
        assert!(!remove_reload_listener("mine"));
    }

    #[test]
    fn a_validation_reports_an_empty_pack_and_a_missing_dependency() {
        let _serial = serial();
        fresh();
        register_pack(ResourcePack::memory("empty", 0));
        depend(ResourceDependency {
            id: id("mymod:textures/needed.png"),
            required_by: "mymod".to_owned(),
            optional: false,
        });
        depend(ResourceDependency {
            id: id("mymod:textures/optional.png"),
            required_by: "mymod".to_owned(),
            optional: true,
        });
        let errors = validate();
        assert!(errors.iter().any(|error| error.contains("holds nothing")), "{errors:?}");
        assert!(errors.iter().any(|error| error.contains("needed.png")), "{errors:?}");
        assert_eq!(get_missing_resources(), vec![id("mymod:textures/needed.png")]);
        assert_eq!(get_loading_errors(), errors);
    }

    #[test]
    fn a_directory_pack_reads_the_mods_own_assets() {
        let _serial = serial();
        fresh();
        let root =
            std::env::temp_dir().join(format!("cinnaloader-resources-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        crate::fs::set_root(root.clone());
        assert_eq!(crate::fs::write_text("assets/lang/en_us.txt", "hello=hi"), Ok(8));

        let pack = own_pack().expect("the assets directory resolves");
        register_pack(pack);
        assert_eq!(
            read_text(&id(&format!(
                "{}:lang/en_us.txt",
                crate::metadata::id()
            ))),
            Some("hello=hi".to_owned())
        );
        assert_eq!(assets_directory().map(|path| path.ends_with("assets")), Some(true));
        assert_eq!(
            list_directory(&id(&format!("{}:lang", crate::metadata::id()))).len(),
            1
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_watch_reports_a_resource_that_is_reloaded() {
        let _serial = serial();
        fresh();
        let mut pack = ResourcePack::memory("mymod", 0);
        pack.insert("data/thing.json", b"{}".to_vec());
        register_pack(pack);
        assert!(watch(&id("mymod:data/thing.json"), |id| {
            crate::info!("reloaded {id}");
        })
        .is_some());
        assert!(watch(&id("mymod:nothing.json"), |_| {}).is_none());
        assert_eq!(reload(), 0, "no listener was added, so nothing runs");
    }
}
