//! The other mods: what is installed, what needs what, and the order they loaded in.
//!
//! The loader resolves dependencies before it loads anything, so by the time a mod runs the
//! answers are fixed. This reads the loader's registry rather than guessing, so a mod and
//! the loader always agree.
//!
//! ```
//! use cinnaloader::prelude::*;
//!
//! for entry in mods::all() {
//!     info!("{} {} is {}", entry.id, entry.version, entry.state);
//! }
//! if mods::is_loaded("core_lib") {
//!     // ...
//! }
//! ```

use std::sync::Mutex;

use crate::deps::{CircularDependencyReport, DependencyKind, ModDependency};
use crate::modinfo::{ModVersion, State};
use crate::version::VersionReq;

/// One installed mod, as the loader knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModEntry {
    /// The loader's name for it, which is what scopes its ids.
    pub name: String,
    /// The id it declares, which dependencies refer to.
    pub id: String,
    pub version: ModVersion,
    pub state: State,
    pub enabled: bool,
}

impl ModEntry {
    /// Whether the mod is loaded and switched on.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.enabled && self.state.is_active()
    }
}

/// Everything the loader worked out before loading: the mods, their order, and the
/// problems it found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Registry {
    pub mods: Vec<ModEntry>,
    /// Load order, by loader name.
    pub order: Vec<String>,
    /// `(mod, dependency)` pairs.
    pub dependencies: Vec<(String, ModDependency)>,
    /// `(mod, id it provides)` pairs.
    pub provides: Vec<(String, String)>,
    /// `(mod, capability it declared)` pairs.
    pub capabilities: Vec<(String, String)>,
    /// `(capability, whether the host can provide it)` pairs.
    pub host_capabilities: Vec<(String, bool)>,
    pub errors: Vec<String>,
    pub missing: Vec<String>,
    pub conflicts: Vec<String>,
    pub cycles: Vec<CircularDependencyReport>,
}

/// Verbs the loader accepts, matching `host_mods_command`.
const ENABLE: u32 = 0;
const DISABLE: u32 = 1;
const IS_ENABLED: u32 = 2;
const IS_LOADED: u32 = 3;

/// The loader's registry, re-read on every call so it stays current as mods are switched on
/// and off.
#[must_use]
pub fn registry() -> Registry {
    let Some(text) = crate::read_graph() else {
        return Registry::default();
    };
    let mut registry = Registry::default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let fields: Vec<&str> = value.split('|').collect();
        match key {
            "mod" if fields.len() >= 5 => registry.mods.push(ModEntry {
                name: fields[0].to_owned(),
                id: fields[1].to_owned(),
                version: ModVersion::parse(fields[2]),
                state: fields[3].parse().unwrap_or(State::Loaded),
                enabled: fields[4] == "1",
            }),
            "order" if !fields.is_empty() => {
                if let Ok(index) = fields.get(1).unwrap_or(&"0").parse::<usize>() {
                    registry.order.resize(index + 1, String::new());
                    registry.order[index] = fields[0].to_owned();
                }
            }
            "depends" if fields.len() >= 4 => {
                let requirement = VersionReq::parse(fields[2]).unwrap_or_default();
                let kind = match fields[3] {
                    "optional" => DependencyKind::Optional,
                    "recommended" => DependencyKind::Recommended,
                    "suggested" => DependencyKind::Suggested,
                    _ => DependencyKind::Required,
                };
                registry.dependencies.push((
                    fields[0].to_owned(),
                    ModDependency {
                        id: fields[1].to_owned(),
                        requirement,
                        kind,
                        reason: None,
                    },
                ));
            }
            "provides" if fields.len() >= 2 => {
                registry
                    .provides
                    .push((fields[0].to_owned(), fields[1].to_owned()));
            }
            "modcap" if fields.len() >= 2 => {
                registry
                    .capabilities
                    .push((fields[0].to_owned(), fields[1].to_owned()));
            }
            "capability" if fields.len() >= 2 => {
                registry
                    .host_capabilities
                    .push((fields[0].to_owned(), fields[1] == "1"));
            }
            "error" => registry.errors.push(value.to_owned()),
            "missing" => registry.missing.push(value.replace('|', ": ")),
            "conflict" => registry.conflicts.push(value.to_owned()),
            "cycle" => registry.cycles.push(CircularDependencyReport {
                cycle: value.split(',').map(str::to_owned).collect(),
            }),
            _ => {}
        }
    }
    registry
}

/// Every installed mod.
#[must_use]
pub fn all() -> Vec<ModEntry> {
    registry().mods
}

/// The mod with this loader name.
#[must_use]
pub fn get(name: &str) -> Option<ModEntry> {
    registry().mods.into_iter().find(|entry| entry.name == name)
}

/// The mod with this declared id.
#[must_use]
pub fn get_by_id(id: &str) -> Option<ModEntry> {
    registry().mods.into_iter().find(|entry| entry.id == id)
}

/// The mod with this display name.
#[must_use]
pub fn get_by_name(name: &str) -> Option<ModEntry> {
    registry()
        .mods
        .into_iter()
        .find(|entry| entry.name == name || entry.id == name)
}

/// The mods that are switched on.
#[must_use]
pub fn enabled() -> Vec<ModEntry> {
    registry()
        .mods
        .into_iter()
        .filter(|entry| entry.enabled)
        .collect()
}

/// The mods the user switched off.
#[must_use]
pub fn disabled() -> Vec<ModEntry> {
    registry()
        .mods
        .into_iter()
        .filter(|entry| !entry.enabled)
        .collect()
}

/// The mods that were refused or stopped after a panic.
#[must_use]
pub fn failed() -> Vec<ModEntry> {
    registry()
        .mods
        .into_iter()
        .filter(|entry| entry.state == State::Failed)
        .collect()
}

/// Whether a mod is installed at all.
#[must_use]
pub fn is_loaded(name: &str) -> bool {
    command(IS_LOADED, name) > 0
}

/// Whether a mod is installed and switched on.
#[must_use]
pub fn is_enabled(name: &str) -> bool {
    command(IS_ENABLED, name) > 0
}

/// Switches a mod on, as the user would.
pub fn enable(name: &str) {
    command(ENABLE, name);
}

/// Switches a mod off, as the user would.
pub fn disable(name: &str) {
    command(DISABLE, name);
}

/// Switches a mod on, and everything it depends on, dependencies first.
pub fn enable_dependency_tree(name: &str) {
    for dependency in get_dependencies(name).into_iter().rev() {
        enable_dependency_tree(&dependency);
    }
    enable(name);
}

/// Switches a mod off, and everything that depends on it.
pub fn disable_dependency_tree(name: &str) {
    for dependent in get_dependents(name) {
        disable_dependency_tree(&dependent);
    }
    disable(name);
}

/// The dependency ids of a mod, as it declared them.
#[must_use]
pub fn get_dependencies(name: &str) -> Vec<String> {
    registry()
        .dependencies
        .into_iter()
        .filter(|(owner, _)| owner == name)
        .map(|(_, dependency)| dependency.id)
        .collect()
}

/// The mods that depend on `id`.
#[must_use]
pub fn get_dependents(id: &str) -> Vec<String> {
    registry()
        .dependencies
        .into_iter()
        .filter(|(_, dependency)| dependency.id == id)
        .map(|(owner, _)| owner)
        .collect()
}

/// The order the loader used.
#[must_use]
pub fn get_load_order() -> Vec<String> {
    registry().order
}

/// The capabilities a mod declared, by loader name or id.
#[must_use]
pub fn capabilities_of(name: &str) -> Vec<String> {
    let registry = registry();
    let id = registry
        .mods
        .iter()
        .find(|entry| entry.name == name || entry.id == name)
        .map(|entry| entry.name.clone())
        .unwrap_or_else(|| name.to_owned());
    registry
        .capabilities
        .into_iter()
        .filter(|(owner, _)| owner == &id)
        .map(|(_, capability)| capability)
        .collect()
}

/// One line per mod, saying what it had to wait for.
#[must_use]
pub fn explain_load_order() -> Vec<String> {
    let registry = registry();
    let id_of = |name: &str| {
        registry
            .mods
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.id.as_str())
    };
    let position_of_id = |id: &str| {
        registry
            .order
            .iter()
            .position(|name| id_of(name) == Some(id))
    };
    let mut lines = Vec::new();
    for (index, name) in registry.order.iter().enumerate() {
        let waited: Vec<String> = registry
            .dependencies
            .iter()
            .filter(|(owner, _)| owner == name)
            .map(|(_, dependency)| dependency.id.clone())
            .filter(|id| position_of_id(id).is_some_and(|position| position < index))
            .collect();
        lines.push(match waited.is_empty() {
            true => format!("{name} first"),
            false => format!("{name} after {}", waited.join(", ")),
        });
    }
    lines
}

/// What stopped a mod from loading, as the loader reported it.
#[must_use]
pub fn get_load_errors() -> Vec<String> {
    registry().errors
}

/// Dependencies that are not installed.
#[must_use]
pub fn get_missing_dependencies() -> Vec<String> {
    registry().missing
}

/// Mods the loader refused, with the reason.
#[must_use]
pub fn get_incompatible_mods() -> Vec<String> {
    registry().errors
}

/// Pairs the loader found in conflict.
#[must_use]
pub fn get_version_conflicts() -> Vec<String> {
    registry().conflicts
}

/// Any cycle the loader had to break.
#[must_use]
pub fn find_cycles() -> Vec<CircularDependencyReport> {
    registry().cycles
}

/// The whole graph, for a mod that wants to walk it.
#[must_use]
pub fn get_dependency_graph() -> Registry {
    registry()
}

/// Checks the graph the loader already resolved, and reports what it found.
#[must_use]
pub fn validate_dependency_graph() -> Vec<String> {
    let registry = registry();
    let mut problems = registry.errors.clone();
    problems.extend(registry.missing.iter().cloned());
    problems.extend(registry.conflicts.iter().cloned());
    problems.extend(registry.cycles.iter().map(ToString::to_string));
    problems
}

/// Resolves the graph again, from the loader's report.
#[must_use]
pub fn resolve_dependencies() -> Registry {
    registry()
}

/// The resolved load order.
#[must_use]
pub fn resolve_load_order() -> Vec<String> {
    registry().order
}

fn command(verb: u32, id: &str) -> i32 {
    let Some(command) = crate::mods_command() else {
        return -1;
    };
    let Ok(id) = std::ffi::CString::new(id) else {
        return -1;
    };
    // SAFETY: `command` is a loader-owned function pointer and `id` is NUL-terminated.
    unsafe { command(verb, id.as_ptr()) }
}

/// Cached registry for tests that need a stable view.
static CACHE: Mutex<Option<Registry>> = Mutex::new(None);

/// The registry, cached. Only for a caller that knows mods are not being switched on or off.
#[must_use]
pub fn cached() -> Registry {
    let mut cache = CACHE.lock().unwrap_or_else(|error| error.into_inner());
    cache.get_or_insert_with(registry).clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_registry_without_a_loader_is_empty_rather_than_wrong() {
        assert!(all().is_empty());
        assert!(!is_loaded("anything"));
        assert!(get_load_order().is_empty());
        assert!(find_cycles().is_empty());
        assert!(validate_dependency_graph().is_empty());
    }
}
