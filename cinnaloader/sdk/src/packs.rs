//! Resource packs and data packs: a mod's own packs, over a virtual file tree, resolved in
//! priority order.
//!
//! A pack is a declaration — an id, a name, a description, a format version, a kind and a
//! priority — over a real file tree. Every path in it is a [`crate::resources::ResourceId`]
//! under the pack's own namespace, and the tree itself is a
//! [`crate::resources::ResourcePack`], the same type section 12 uses: this module does not
//! define a second resource type, so the section's names (`create`, `insert`, `resolve`, ...)
//! are the pack-level view over those two. `resolve(path)` answers which enabled pack wins:
//! highest priority first, and a tie broken by the lexicographically smaller pack id. A file
//! that several packs offer merges through [`merge`], later (higher-priority) entries winning
//! a duplicate key. A pack whose format is newer than [`SUPPORTED_FORMAT`] is refused by name
//! rather than loaded.
//!
//! What this host cannot do is the client's side of packs: its own pack loading and mounted
//! pack list, its resource reload, and reading a pack it has mounted. Those live in
//! [`known_only_to_the_client`], refused with their reasons and a warning.
//!
//! ```
//! use cinnaloader::packs::{self, PackManifest};
//!
//! assert!(packs::create(PackManifest::new("low", "Low").priority(0)));
//! assert!(packs::create(PackManifest::new("high", "High").priority(10)));
//! assert!(packs::insert_text("low", "data/thing.txt", "low"));
//! assert!(packs::insert_text("high", "data/thing.txt", "high"));
//! assert_eq!(
//!     packs::resolve("data/thing.txt").map(|(winner, _)| winner),
//!     Some("high".to_owned())
//! );
//! assert!(packs::disable("high"));
//! assert_eq!(
//!     packs::resolve("data/thing.txt").map(|(winner, _)| winner),
//!     Some("low".to_owned())
//! );
//! # packs::uninstall("low");
//! # packs::uninstall("high");
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

use crate::resources::ResourcePack;

/// The newest pack format this build understands. A pack that asks for more is refused.
pub const SUPPORTED_FORMAT: u32 = 34;

/// What a pack carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PackKind {
    /// Assets: textures, models, sounds, language.
    Resource,
    /// Game data: loot tables, recipes, tags.
    Data,
    #[default]
    Both,
}

impl PackKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Resource => "resource",
            Self::Data => "data",
            Self::Both => "both",
        }
    }

    /// Reads the name a manifest writes.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        match text.trim().to_ascii_lowercase().as_str() {
            "resource" | "resources" => Self::Resource,
            "data" => Self::Data,
            _ => Self::Both,
        }
    }
}

impl fmt::Display for PackKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// What a pack declares about itself, before any of its files are read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackManifest {
    pub id: String,
    pub name: String,
    pub description: String,
    pub format: u32,
    pub kind: PackKind,
    pub priority: i32,
    /// The ids of other packs this one wants loaded.
    pub dependencies: Vec<String>,
}

impl PackManifest {
    #[must_use]
    pub fn new(id: &str, name: &str) -> Self {
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            description: String::new(),
            format: SUPPORTED_FORMAT,
            kind: PackKind::Both,
            priority: 0,
            dependencies: Vec::new(),
        }
    }

    #[must_use]
    pub fn description(mut self, text: &str) -> Self {
        self.description = text.to_owned();
        self
    }

    #[must_use]
    pub fn format(mut self, format: u32) -> Self {
        self.format = format;
        self
    }

    #[must_use]
    pub fn kind(mut self, kind: PackKind) -> Self {
        self.kind = kind;
        self
    }

    #[must_use]
    pub fn priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    #[must_use]
    pub fn depends_on(mut self, id: &str) -> Self {
        self.dependencies.push(id.to_owned());
        self
    }
}

/// A declared pack: its manifest, whether it is enabled, and its file tree.
#[derive(Debug, Clone)]
pub struct Pack {
    pub manifest: PackManifest,
    pub enabled: bool,
    tree: ResourcePack,
}

impl Pack {
    fn new(manifest: PackManifest) -> Self {
        let tree = ResourcePack::memory(&manifest.id, manifest.priority);
        Self {
            manifest,
            enabled: true,
            tree,
        }
    }

    /// Every path the pack holds, in name order.
    #[must_use]
    pub fn files(&self) -> Vec<String> {
        self.tree.paths()
    }

    /// One file's bytes, from this pack alone.
    #[must_use]
    pub fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.tree.get(path)
    }
}

/// What the loader knows about a pack without handing over its files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackMetadata {
    pub id: String,
    pub name: String,
    pub description: String,
    pub format: u32,
    pub kind: PackKind,
    pub priority: i32,
    pub enabled: bool,
    pub files: usize,
    pub bytes: usize,
    pub hash: u64,
}

/// One path more than one enabled pack offers, and who won.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackOverride {
    pub path: String,
    pub winner: String,
    pub losers: Vec<String>,
}

/// One key of a merged data file, and the pack it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeEntry {
    pub key: String,
    pub value: String,
    pub source: String,
}

/// A data file several packs contributed to, merged in resolution order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergedFile {
    pub path: String,
    pub entries: BTreeMap<String, String>,
    pub sources: BTreeMap<String, String>,
}

impl MergedFile {
    /// Every entry, in key order.
    #[must_use]
    pub fn rows(&self) -> Vec<MergeEntry> {
        self.entries
            .iter()
            .map(|(key, value)| MergeEntry {
                key: key.clone(),
                value: value.clone(),
                source: self.sources.get(key).cloned().unwrap_or_default(),
            })
            .collect()
    }
}

#[derive(Default)]
struct Registry {
    packs: BTreeMap<String, Pack>,
    discovered: Vec<String>,
    errors: Vec<String>,
}

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Registry::default()))
}

fn with<T>(body: impl FnOnce(&mut Registry) -> T) -> T {
    let mut guard = registry().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// The enabled packs in resolution order: highest priority first, ties broken by pack id.
fn active_order(registry: &Registry) -> Vec<&Pack> {
    let mut active: Vec<&Pack> = registry
        .packs
        .values()
        .filter(|pack| pack.enabled)
        .collect();
    active.sort_by(|left, right| {
        right
            .manifest
            .priority
            .cmp(&left.manifest.priority)
            .then(left.manifest.id.cmp(&right.manifest.id))
    });
    active
}

fn usable_id(id: &str) -> bool {
    !id.is_empty()
        && !id.contains("..")
        && id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
}

/// Everything wrong with a manifest on its own, before it is used.
#[must_use]
pub fn validate_manifest(manifest: &PackManifest) -> Vec<String> {
    let mut problems = Vec::new();
    if !usable_id(&manifest.id) {
        problems.push(format!("'{}' is not a usable pack id", manifest.id));
    }
    if manifest.format > SUPPORTED_FORMAT {
        problems.push(format!(
            "pack '{}' wants format {} and this build supports {SUPPORTED_FORMAT}",
            manifest.id, manifest.format
        ));
    }
    problems
}

/// Declares a pack, with an empty tree. `false` for a bad manifest or a taken id.
pub fn create(manifest: PackManifest) -> bool {
    let problems = validate_manifest(&manifest);
    if !problems.is_empty() {
        crate::warn!(
            "a mod declared a pack this host cannot take: {}",
            problems.join("; ")
        );
        with(|registry| registry.errors = problems);
        return false;
    }
    with(|registry| {
        let id = manifest.id.clone();
        match registry.packs.entry(id) {
            std::collections::btree_map::Entry::Occupied(_) => false,
            std::collections::btree_map::Entry::Vacant(slot) => {
                slot.insert(Pack::new(manifest));
                true
            }
        }
    })
}

fn parse_manifest(text: &str) -> Option<PackManifest> {
    let mut id = None;
    let mut name = None;
    let mut description = String::new();
    let mut format = SUPPORTED_FORMAT;
    let mut kind = PackKind::Both;
    let mut priority = 0i32;
    let mut dependencies = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "id" => id = Some(value.to_owned()),
            "name" => name = Some(value.to_owned()),
            "description" => description = value.to_owned(),
            "format" => format = value.parse().ok()?,
            "kind" => kind = PackKind::parse(value),
            "priority" => priority = value.parse().ok()?,
            "depends" => dependencies.push(value.to_owned()),
            _ => {}
        }
    }
    let id = id?;
    Some(PackManifest {
        name: name.unwrap_or_else(|| id.clone()),
        id,
        description,
        format,
        kind,
        priority,
        dependencies,
    })
}

fn manifest_text(manifest: &PackManifest) -> String {
    let mut text = format!(
        "id={}\nname={}\ndescription={}\nformat={}\nkind={}\npriority={}\n",
        manifest.id, manifest.name, manifest.description, manifest.format, manifest.kind,
        manifest.priority
    );
    for dependency in &manifest.dependencies {
        text.push_str(&format!("depends={dependency}\n"));
    }
    text
}

fn read_into(pack: &mut Pack, base: &str, skip: &str) {
    let Ok(root) = crate::fs::resolve(base) else {
        return;
    };
    for path in crate::fs::walk(base).unwrap_or_default() {
        if !path.is_file() {
            continue;
        }
        let Ok(relative) = path.strip_prefix(&root) else {
            continue;
        };
        let relative = relative.to_string_lossy().replace('\\', "/");
        if relative == skip || relative == "pack.txt" {
            continue;
        }
        if let Ok(bytes) = std::fs::read(&path) {
            pack.tree.insert(&relative, bytes);
        }
    }
}

/// Loads a pack from the mod's own `packs/<id>` directory and enables it.
pub fn load(id: &str) -> bool {
    let directory = format!("packs/{id}");
    if !crate::fs::is_directory(&directory) {
        return false;
    }
    let text = match crate::fs::read_text(&format!("{directory}/pack.txt")) {
        Ok(text) => text,
        Err(error) => {
            crate::warn!("a mod asked this host to load pack '{id}' with no manifest: {error}");
            return false;
        }
    };
    let Some(manifest) = parse_manifest(&text) else {
        crate::warn!("a mod asked this host to load pack '{id}' and its manifest does not parse");
        return false;
    };
    if manifest.id != id {
        crate::warn!(
            "a mod asked this host to load pack '{id}' whose manifest calls itself '{}'",
            manifest.id
        );
        return false;
    }
    let problems = validate_manifest(&manifest);
    if !problems.is_empty() {
        crate::warn!("a mod asked this host to load pack '{id}': {}", problems.join("; "));
        with(|registry| registry.errors = problems);
        return false;
    }
    let mut pack = Pack::new(manifest);
    read_into(&mut pack, &directory, "pack.txt");
    with(|registry| {
        registry.packs.insert(id.to_owned(), pack);
        registry.errors.clear();
    });
    true
}

/// Empties a loaded pack's files from memory and disables it.
pub fn unload(id: &str) -> bool {
    let priority = with(|registry| registry.packs.get(id).map(|pack| pack.manifest.priority));
    let Some(priority) = priority else {
        return false;
    };
    with(|registry| match registry.packs.get_mut(id) {
        Some(pack) => {
            pack.tree = ResourcePack::memory(id, priority);
            pack.enabled = false;
            true
        }
        None => false,
    })
}

/// Re-reads a pack from its directory, keeping whether it was enabled.
pub fn reload(id: &str) -> bool {
    let was_enabled = with(|registry| registry.packs.get(id).map(|pack| pack.enabled));
    if !crate::fs::is_directory(&format!("packs/{id}")) {
        return with(|registry| registry.packs.contains_key(id));
    }
    let loaded = load(id);
    if loaded {
        if let Some(was_enabled) = was_enabled {
            with(|registry| {
                if let Some(pack) = registry.packs.get_mut(id) {
                    pack.enabled = was_enabled;
                }
            });
        }
    }
    loaded
}

/// Turns a pack on.
pub fn enable(id: &str) -> bool {
    with(|registry| match registry.packs.get_mut(id) {
        Some(pack) => {
            pack.enabled = true;
            true
        }
        None => false,
    })
}

/// Turns a pack off, so resolution skips it.
pub fn disable(id: &str) -> bool {
    with(|registry| match registry.packs.get_mut(id) {
        Some(pack) => {
            pack.enabled = false;
            true
        }
        None => false,
    })
}

/// Removes a pack from the registry and deletes its own `packs/<id>` directory.
pub fn uninstall(id: &str) -> bool {
    let removed = with(|registry| registry.packs.remove(id).is_some());
    let _ = crate::fs::delete(&format!("packs/{id}"));
    removed
}

/// Adds bytes to a pack's virtual file tree.
pub fn insert(id: &str, path: &str, bytes: Vec<u8>) -> bool {
    with(|registry| match registry.packs.get_mut(id) {
        Some(pack) => pack.tree.insert(path, bytes),
        None => false,
    })
}

/// Adds text to a pack's virtual file tree.
pub fn insert_text(id: &str, path: &str, text: &str) -> bool {
    insert(id, path, text.as_bytes().to_vec())
}

/// One file's bytes, read from the pack named, alone.
#[must_use]
pub fn read(id: &str, path: &str) -> Option<Vec<u8>> {
    with(|registry| registry.packs.get(id).and_then(|pack| pack.read(path)))
}

/// The same, as text.
#[must_use]
pub fn read_text(id: &str, path: &str) -> Option<String> {
    read(id, path).map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

/// Whether a pack holds a path.
#[must_use]
pub fn path_exists(id: &str, path: &str) -> bool {
    read(id, path).is_some()
}

/// What is directly inside a directory of a pack, in name order.
#[must_use]
pub fn list_directory(id: &str, directory: &str) -> Vec<String> {
    let prefix = directory.trim_matches('/');
    with(|registry| {
        let Some(pack) = registry.packs.get(id) else {
            return Vec::new();
        };
        let mut names = std::collections::BTreeSet::new();
        for path in pack.tree.paths() {
            let rest = if prefix.is_empty() {
                path.as_str()
            } else {
                match path.strip_prefix(&format!("{prefix}/")) {
                    Some(rest) => rest,
                    None => continue,
                }
            };
            if let Some(name) = rest.split('/').next() {
                if !name.is_empty() {
                    names.insert(name.to_owned());
                }
            }
        }
        names.into_iter().collect()
    })
}

/// Every path a pack holds.
#[must_use]
pub fn get_content(id: &str) -> Vec<String> {
    with(|registry| {
        registry
            .packs
            .get(id)
            .map(Pack::files)
            .unwrap_or_default()
    })
}

/// A pack by id.
#[must_use]
pub fn get(id: &str) -> Option<Pack> {
    with(|registry| registry.packs.get(id).cloned())
}

/// Every pack, by id.
#[must_use]
pub fn get_all() -> Vec<Pack> {
    with(|registry| registry.packs.values().cloned().collect())
}

/// The enabled packs, by id.
#[must_use]
pub fn get_enabled() -> Vec<Pack> {
    with(|registry| {
        registry
            .packs
            .values()
            .filter(|pack| pack.enabled)
            .cloned()
            .collect()
    })
}

/// The disabled packs, by id.
#[must_use]
pub fn get_disabled() -> Vec<Pack> {
    with(|registry| {
        registry
            .packs
            .values()
            .filter(|pack| !pack.enabled)
            .cloned()
            .collect()
    })
}

/// A pack's priority, which is what decides who wins a shared path.
#[must_use]
pub fn get_priority(id: &str) -> Option<i32> {
    with(|registry| registry.packs.get(id).map(|pack| pack.manifest.priority))
}

/// Sets a pack's priority, and answers whether there was such a pack.
pub fn set_priority(id: &str, priority: i32) -> bool {
    with(|registry| match registry.packs.get_mut(id) {
        Some(pack) => {
            pack.manifest.priority = priority;
            pack.tree.priority = priority;
            true
        }
        None => false,
    })
}

/// The ids a pack says it needs.
#[must_use]
pub fn get_dependencies(id: &str) -> Vec<String> {
    with(|registry| {
        registry
            .packs
            .get(id)
            .map(|pack| pack.manifest.dependencies.clone())
            .unwrap_or_default()
    })
}

/// What the loader knows about a pack without handing over its files.
#[must_use]
pub fn get_metadata(id: &str) -> Option<PackMetadata> {
    with(|registry| {
        let pack = registry.packs.get(id)?;
        let mut all = Vec::new();
        let mut bytes = 0;
        for path in pack.tree.paths() {
            all.extend_from_slice(path.as_bytes());
            if let Some(file) = pack.tree.get(&path) {
                bytes += file.len();
                all.extend_from_slice(&file);
            }
        }
        Some(PackMetadata {
            id: pack.manifest.id.clone(),
            name: pack.manifest.name.clone(),
            description: pack.manifest.description.clone(),
            format: pack.manifest.format,
            kind: pack.manifest.kind,
            priority: pack.manifest.priority,
            enabled: pack.enabled,
            files: pack.tree.paths().len(),
            bytes,
            hash: crate::fs::hash_bytes(&all),
        })
    })
}

/// Everything wrong with one pack as it stands: bad manifest, no files, missing dependency.
#[must_use]
pub fn validate(id: &str) -> Vec<String> {
    with(|registry| {
        let Some(pack) = registry.packs.get(id) else {
            return vec![format!("no pack '{id}'")];
        };
        let mut problems = validate_manifest(&pack.manifest);
        if pack.tree.is_empty() {
            problems.push(format!("pack '{id}' holds no files"));
        }
        for dependency in &pack.manifest.dependencies {
            if !registry.packs.contains_key(dependency) {
                problems.push(format!(
                    "pack '{id}' depends on '{dependency}', which is not loaded"
                ));
            }
        }
        problems
    })
}

/// The enabled pack that wins a path, and its bytes.
#[must_use]
pub fn resolve(path: &str) -> Option<(String, Vec<u8>)> {
    let path = path.trim_start_matches('/').to_owned();
    with(|registry| {
        active_order(registry).into_iter().find_map(|pack| {
            pack.tree
                .get(&path)
                .map(|bytes| (pack.manifest.id.clone(), bytes))
        })
    })
}

/// Every enabled pack that offers a path, in resolution order. The first one wins.
#[must_use]
pub fn providers(path: &str) -> Vec<String> {
    let path = path.trim_start_matches('/').to_owned();
    with(|registry| {
        active_order(registry)
            .into_iter()
            .filter(|pack| pack.tree.get(&path).is_some())
            .map(|pack| pack.manifest.id.clone())
            .collect()
    })
}

/// Merges a `key=value` data file across the enabled packs. Later in resolution order wins a
/// duplicate key, which is the same rule [`resolve`] uses for a whole file.
#[must_use]
pub fn merge(path: &str) -> Option<MergedFile> {
    let path = path.trim_start_matches('/').to_owned();
    with(|registry| {
        let active = active_order(registry);
        let mut merged = MergedFile {
            path: path.clone(),
            entries: BTreeMap::new(),
            sources: BTreeMap::new(),
        };
        let mut found = false;
        // Lowest priority first, so the winner is written last and takes the duplicate key.
        for pack in active.into_iter().rev() {
            let Some(bytes) = pack.tree.get(&path) else {
                continue;
            };
            found = true;
            for line in String::from_utf8_lossy(&bytes).lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let (key, value) = line.split_once('=').unwrap_or((line, ""));
                let key = key.trim().to_owned();
                merged.entries.insert(key.clone(), value.trim().to_owned());
                merged.sources.insert(key, pack.manifest.id.clone());
            }
        }
        found.then_some(merged)
    })
}

/// Every path more than one enabled pack offers, with the winner and the losers.
#[must_use]
pub fn get_overrides() -> Vec<PackOverride> {
    with(|registry| {
        let active = active_order(registry);
        let mut by_path: BTreeMap<String, Vec<&Pack>> = BTreeMap::new();
        for pack in &active {
            for path in pack.tree.paths() {
                by_path.entry(path).or_default().push(pack);
            }
        }
        by_path
            .into_iter()
            .filter(|(_, packs)| packs.len() > 1)
            .map(|(path, packs)| PackOverride {
                path,
                winner: packs[0].manifest.id.clone(),
                losers: packs[1..]
                    .iter()
                    .map(|pack| pack.manifest.id.clone())
                    .collect(),
            })
            .collect()
    })
}

/// The contested paths, as sentences.
#[must_use]
pub fn check_conflicts() -> Vec<String> {
    get_overrides()
        .into_iter()
        .map(|over| {
            format!(
                "{} is offered by {} and {}",
                over.path,
                over.winner,
                over.losers.join(", ")
            )
        })
        .collect()
}

/// Lowers each loser below the winner so the documented order is explicit, and answers how
/// many packs it moved.
pub fn resolve_conflicts() -> usize {
    let overrides = get_overrides();
    let mut moved = 0;
    with(|registry| {
        for over in overrides {
            let Some(winner_priority) = registry
                .packs
                .get(&over.winner)
                .map(|pack| pack.manifest.priority)
            else {
                continue;
            };
            for (index, loser) in over.losers.iter().enumerate() {
                let target = winner_priority - 1 - index as i32;
                let Some(pack) = registry.packs.get_mut(loser) else {
                    continue;
                };
                if pack.manifest.priority > target {
                    pack.manifest.priority = target;
                    pack.tree.priority = target;
                    moved += 1;
                }
            }
        }
    });
    moved
}

/// Writes a pack to a directory as `pack.txt` and its files, and answers how many bytes went.
pub fn export(id: &str, directory: &str) -> Result<usize, String> {
    let Some(pack) = get(id) else {
        return Err(format!("no pack '{id}'"));
    };
    let mut written = crate::fs::write_text(
        &format!("{directory}/pack.txt"),
        &manifest_text(&pack.manifest),
    )
    .map_err(|error| error.to_string())?;
    for path in pack.files() {
        if path == "pack.txt" {
            continue;
        }
        let Some(bytes) = pack.read(&path) else {
            continue;
        };
        written += crate::fs::write(&format!("{directory}/{path}"), &bytes)
            .map_err(|error| error.to_string())?;
    }
    Ok(written)
}

/// Reads a directory an [`export`] wrote, and answers the id of the pack it declared.
pub fn import(directory: &str) -> Result<String, String> {
    let text = crate::fs::read_text(&format!("{directory}/pack.txt"))
        .map_err(|error| error.to_string())?;
    let Some(manifest) = parse_manifest(&text) else {
        return Err(format!("{directory}/pack.txt does not parse"));
    };
    let problems = validate_manifest(&manifest);
    if !problems.is_empty() {
        crate::warn!(
            "a mod asked this host to import a pack it cannot take: {}",
            problems.join("; ")
        );
        return Err(problems.join("; "));
    }
    let id = manifest.id.clone();
    let mut pack = Pack::new(manifest);
    read_into(&mut pack, directory, "pack.txt");
    with(|registry| registry.packs.insert(id.clone(), pack));
    Ok(id)
}

fn copy_tree(from: &str, to: &str) -> Result<(), String> {
    let root = crate::fs::resolve(from).map_err(|error| error.to_string())?;
    for path in crate::fs::walk(from).map_err(|error| error.to_string())? {
        if !path.is_file() {
            continue;
        }
        let Ok(relative) = path.strip_prefix(&root) else {
            continue;
        };
        let relative = relative.to_string_lossy().replace('\\', "/");
        let bytes = std::fs::read(&path).map_err(|error| error.to_string())?;
        crate::fs::write(&format!("{to}/{relative}"), &bytes).map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Copies an exported pack directory into the mod's own `packs/<id>` and loads it.
pub fn install(source: &str) -> bool {
    let text = match crate::fs::read_text(&format!("{source}/pack.txt")) {
        Ok(text) => text,
        Err(error) => {
            crate::warn!("a mod asked this host to install a pack from '{source}': {error}");
            return false;
        }
    };
    let Some(manifest) = parse_manifest(&text) else {
        crate::warn!("a mod asked this host to install a pack from '{source}' with no manifest");
        return false;
    };
    let problems = validate_manifest(&manifest);
    if !problems.is_empty() {
        crate::warn!(
            "a mod asked this host to install a pack it cannot take: {}",
            problems.join("; ")
        );
        with(|registry| registry.errors = problems);
        return false;
    }
    let id = manifest.id.clone();
    if copy_tree(source, &format!("packs/{id}")).is_err() {
        return false;
    }
    load(&id)
}

/// The packs a `discover` found under the mod's own `packs` directory.
#[must_use]
pub fn discovered() -> Vec<String> {
    with(|registry| registry.discovered.clone())
}

/// Finds the packs under the mod's own `packs` directory, by their manifests.
pub fn discover() -> Vec<String> {
    let mut found = Vec::new();
    for path in crate::fs::walk("packs").unwrap_or_default() {
        if path.file_name().and_then(|name| name.to_str()) != Some("pack.txt") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some(manifest) = parse_manifest(&text) else {
            continue;
        };
        if validate_manifest(&manifest).is_empty() {
            found.push(manifest.id);
        }
    }
    found.sort();
    found.dedup();
    with(|registry| registry.discovered = found.clone());
    found
}

/// Watches the mod's own `packs` directory, re-running [`discover`] on a change.
pub fn watch_directory() -> Result<u64, String> {
    crate::fs::watch("packs", |_, _| {
        discover();
    })
    .map_err(|error| error.to_string())
}

/// Enables exactly the packs named, disables the rest, and answers how many are enabled.
pub fn set_active(ids: &[&str]) -> usize {
    with(|registry| {
        let wanted: std::collections::BTreeSet<String> =
            ids.iter().map(|id| (*id).to_owned()).collect();
        registry.errors = wanted
            .iter()
            .filter(|id| !registry.packs.contains_key(*id))
            .map(|id| format!("no pack '{id}'"))
            .collect();
        let mut active = 0;
        for (id, pack) in registry.packs.iter_mut() {
            pack.enabled = wanted.contains(id);
            if pack.enabled {
                active += 1;
            }
        }
        active
    })
}

/// The enabled packs, in resolution order, by id.
#[must_use]
pub fn get_active_stack() -> Vec<String> {
    with(|registry| {
        active_order(registry)
            .into_iter()
            .map(|pack| pack.manifest.id.clone())
            .collect()
    })
}

/// What went wrong the last time a pack was activated or resolved against.
#[must_use]
pub fn get_activation_errors() -> Vec<String> {
    with(|registry| registry.errors.clone())
}

/// Why a pack question about the client has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "client_load" => {
            "loading a pack into the client's own pack list is the client's, not this host's; \
             the packs a mod declares here never reach it"
                .to_owned()
        }
        "client_list" => {
            "the client's mounted pack list is inside the client and never crosses the boundary"
                .to_owned()
        }
        "client_reload" => {
            "the client reloads resources over its own pack list; this host can only reload the \
             packs a mod declared here"
                .to_owned()
        }
        "client_read" => {
            "reading a pack the client has mounted needs the client's file system, which this \
             host has no handle on"
                .to_owned()
        }
        "format" => {
            "a pack newer than this build is refused by the format gate, so nothing beyond \
             the names it declares is read"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach about packs.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["client_load", "client_list", "client_reload", "client_read"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

/// The client's own pack state, which this host cannot read or drive.
pub mod known_only_to_the_client {
    use super::why_unknown;

    /// Loads a pack into the client. `false`.
    pub fn load(_id: &str) -> bool {
        crate::warn!(
            "a mod asked this host to load a pack into the client: {}",
            why_unknown("client_load")
        );
        false
    }

    /// The client's mounted packs. Empty.
    #[must_use]
    pub fn get_mounted() -> Vec<String> {
        crate::warn!(
            "a mod asked this host for the client's pack list: {}",
            why_unknown("client_list")
        );
        Vec::new()
    }

    /// Reloads the client's resources. `0`.
    pub fn reload() -> usize {
        crate::warn!(
            "a mod asked this host to reload the client's resources: {}",
            why_unknown("client_reload")
        );
        0
    }

    /// Reads a pack the client has mounted. `None`.
    #[must_use]
    pub fn read(_id: &str, _path: &str) -> Option<Vec<u8>> {
        crate::warn!(
            "a mod asked this host to read a mounted pack: {}",
            why_unknown("client_read")
        );
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with(|registry| *registry = Registry::default());
    }

    fn root(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "cinnaloader-packs-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        crate::fs::set_root(root.clone());
        root
    }

    #[test]
    fn a_pack_declares_its_metadata_and_reads_back() {
        let _serial = serial();
        fresh();
        assert!(create(
            PackManifest::new("alpha", "Alpha")
                .description("first")
                .format(17)
                .kind(PackKind::Data)
                .priority(4)
                .depends_on("beta")
        ));
        assert!(create(PackManifest::new("beta", "Beta")));
        assert!(!create(PackManifest::new("alpha", "Again")), "the id is taken");
        insert_text("alpha", "data/a.txt", "a");
        let pack = get("alpha").expect("declared");
        assert_eq!(pack.manifest.name, "Alpha");
        assert_eq!(pack.manifest.description, "first");
        assert_eq!(pack.manifest.format, 17);
        assert_eq!(pack.manifest.kind, PackKind::Data);
        assert_eq!(pack.manifest.priority, 4);
        assert_eq!(get_dependencies("alpha"), vec!["beta".to_owned()]);
        assert_eq!(get_priority("alpha"), Some(4));
        assert!(set_priority("alpha", 9));
        assert_eq!(get_priority("alpha"), Some(9));
        assert_eq!(get_all().len(), 2);
        assert_eq!(get_enabled().len(), 2);
        assert!(disable("alpha"));
        assert_eq!(get_disabled().len(), 1);
        assert!(enable("alpha"));
        let meta = get_metadata("alpha").expect("declared");
        assert_eq!(meta.id, "alpha");
        assert_eq!(meta.files, 1);
        assert_eq!(meta.bytes, 1);
        assert!(meta.enabled);
        assert_eq!(validate("alpha"), Vec::<String>::new());
        assert!(uninstall("beta"));
        assert_eq!(validate("alpha").len(), 1, "beta is gone");
    }

    #[test]
    fn a_virtual_tree_holds_bytes_text_and_lists_a_directory() {
        let _serial = serial();
        fresh();
        assert!(create(PackManifest::new("tree", "Tree")));
        assert!(insert("tree", "textures/gui/menu.png", b"png bytes".to_vec()));
        assert!(insert_text("tree", "lang/en_us.txt", "hello=hi"));
        assert!(!insert("missing", "x", vec![1]));
        assert!(insert_text("tree", "lang/keys/en_us.txt", "k=v"));
        assert_eq!(read("tree", "textures/gui/menu.png").map(|bytes| bytes.len()), Some(9));
        assert_eq!(read_text("tree", "lang/en_us.txt"), Some("hello=hi".to_owned()));
        assert!(path_exists("tree", "lang/en_us.txt"));
        assert!(!path_exists("tree", "lang/de_de.txt"));
        assert_eq!(
            get_content("tree"),
            vec![
                "lang/en_us.txt".to_owned(),
                "lang/keys/en_us.txt".to_owned(),
                "textures/gui/menu.png".to_owned()
            ]
        );
        assert_eq!(list_directory("tree", "lang"), vec!["en_us.txt".to_owned(), "keys".to_owned()]);
        assert_eq!(list_directory("tree", ""), vec!["lang".to_owned(), "textures".to_owned()]);
        assert_eq!(list_directory("tree", "nothing"), Vec::<String>::new());
    }

    #[test]
    fn a_higher_priority_pack_wins_and_disabling_it_promotes_the_next() {
        let _serial = serial();
        fresh();
        assert!(create(PackManifest::new("low", "Low").priority(0)));
        assert!(create(PackManifest::new("high", "High").priority(10)));
        assert!(insert_text("low", "data/thing.txt", "low"));
        assert!(insert_text("high", "data/thing.txt", "high"));
        assert_eq!(get_active_stack(), vec!["high".to_owned(), "low".to_owned()]);
        assert_eq!(
            resolve("data/thing.txt").map(|(id, bytes)| (id, bytes.len())),
            Some(("high".to_owned(), 4))
        );
        assert_eq!(providers("data/thing.txt"), vec!["high".to_owned(), "low".to_owned()]);
        assert!(disable("high"));
        assert_eq!(
            resolve("data/thing.txt").map(|(id, bytes)| (id, bytes.len())),
            Some(("low".to_owned(), 3))
        );
        assert_eq!(providers("data/thing.txt"), vec!["low".to_owned()]);
    }

    #[test]
    fn a_shared_data_file_merges_and_the_later_key_wins() {
        let _serial = serial();
        fresh();
        assert!(create(PackManifest::new("base", "Base").priority(0)));
        assert!(create(PackManifest::new("over", "Overlay").priority(10)));
        assert!(insert_text("base", "data/tags.txt", "smooth=base\nunique=base\n"));
        assert!(insert_text("over", "data/tags.txt", "smooth=over\n"));
        let merged = merge("data/tags.txt").expect("two packs offer it");
        assert_eq!(merged.entries.len(), 2);
        assert_eq!(merged.entries.get("smooth"), Some(&"over".to_owned()));
        assert_eq!(merged.entries.get("unique"), Some(&"base".to_owned()));
        assert_eq!(merged.sources.get("smooth"), Some(&"over".to_owned()));
        assert_eq!(merged.sources.get("unique"), Some(&"base".to_owned()));
        assert_eq!(merge("data/missing.txt"), None);
    }

    #[test]
    fn a_newer_format_is_refused_by_name() {
        let _serial = serial();
        fresh();
        let future = PackManifest::new("future", "Future").format(SUPPORTED_FORMAT + 1);
        assert!(!create(future.clone()));
        assert!(get("future").is_none());
        let errors = get_activation_errors();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("future"), "{errors:?}");
        assert!(errors[0].contains("format"), "{errors:?}");
        assert_eq!(validate_manifest(&future).len(), 1);
        assert!(validate_manifest(&PackManifest::new("ok", "Ok")).is_empty());
        assert!(why_unknown("format").contains("format gate"));
    }

    #[test]
    fn overrides_name_the_winner_and_resolve_conflicts_settles_them() {
        let _serial = serial();
        fresh();
        assert!(create(PackManifest::new("aaa", "A").priority(5)));
        assert!(create(PackManifest::new("zzz", "Z").priority(5)));
        assert!(insert_text("aaa", "shared.txt", "a"));
        assert!(insert_text("zzz", "shared.txt", "z"));
        assert_eq!(
            resolve("shared.txt").map(|(id, _)| id),
            Some("aaa".to_owned()),
            "equal priority is broken by the smaller id"
        );
        let overrides = get_overrides();
        assert_eq!(overrides.len(), 1);
        assert_eq!(overrides[0].path, "shared.txt");
        assert_eq!(overrides[0].winner, "aaa");
        assert_eq!(overrides[0].losers, vec!["zzz".to_owned()]);
        assert_eq!(check_conflicts().len(), 1);
        assert_eq!(resolve_conflicts(), 1);
        assert_eq!(get_priority("zzz"), Some(4), "moved below the winner");
        assert_eq!(resolve("shared.txt").map(|(id, _)| id), Some("aaa".to_owned()));
        assert_eq!(get_overrides()[0].winner, "aaa");
    }

    #[test]
    fn a_pack_round_trips_through_export_import_and_install() {
        let _serial = serial();
        fresh();
        let root = root("disk");
        assert!(create(PackManifest::new("alpha", "Alpha").description("disk").priority(3)));
        assert!(insert_text("alpha", "data/a.txt", "a"));
        assert!(insert_text("alpha", "lang/en_us.txt", "hello=hi"));
        assert!(export("alpha", "held/alpha").unwrap() > 0);
        assert_eq!(discover(), Vec::<String>::new(), "nothing is installed yet");
        assert!(uninstall("alpha"));
        assert!(get("alpha").is_none());
        assert_eq!(import("held/alpha").unwrap(), "alpha");
        assert_eq!(read_text("alpha", "lang/en_us.txt"), Some("hello=hi".to_owned()));
        assert!(uninstall("alpha"));
        assert!(install("held/alpha"));
        assert!(crate::fs::is_file("packs/alpha/pack.txt"));
        assert_eq!(discover(), vec!["alpha".to_owned()]);
        assert!(uninstall("alpha"));
        assert!(!crate::fs::is_file("packs/alpha/pack.txt"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_active_set_can_be_chosen_and_read_back() {
        let _serial = serial();
        fresh();
        assert!(create(PackManifest::new("alpha", "Alpha")));
        assert!(create(PackManifest::new("beta", "Beta")));
        assert!(create(PackManifest::new("gamma", "Gamma")));
        assert_eq!(get_active_stack().len(), 3);
        assert_eq!(set_active(&["alpha", "gamma"]), 2);
        assert_eq!(get_active_stack(), vec!["alpha".to_owned(), "gamma".to_owned()]);
        assert_eq!(get_disabled().len(), 1);
        assert_eq!(set_active(&["nope"]), 0);
        assert_eq!(get_activation_errors(), vec!["no pack 'nope'".to_owned()]);
    }

    #[test]
    fn the_clients_own_pack_state_is_refused() {
        let _serial = serial();
        fresh();
        assert!(!known_only_to_the_client::load("any"));
        assert_eq!(known_only_to_the_client::get_mounted(), Vec::<String>::new());
        assert_eq!(known_only_to_the_client::reload(), 0);
        assert!(known_only_to_the_client::read("any", "a").is_none());
        assert_eq!(refusals().len(), 4);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
        assert!(why_unknown("client_list").contains("mounted pack list"));
    }
}
