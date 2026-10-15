//! Asking the loader about this mod, and telling it who we are.
//!
//! A mod calls [`declare`] once, in `load`, and then reads back whatever it needs:
//! [`manifest`] is what it declared, and [`report`] is the loader's fuller view — where
//! the library came from, which state the mod is in, whether the user has switched it off.
//! The accessors below read both, so a mod writes `version()` or `is_enabled()` without
//! caring which side answers.

use std::path::PathBuf;
use std::sync::OnceLock;

use crate::modinfo::{
    Capability, Environment, ModContext, ModDirectory, ModEntrypoint, ModInfo, ModInstance,
    ModManifest, ModMetadata, ModNamespace, ModVersion, Permission, State,
};

static MANIFEST: OnceLock<ModManifest> = OnceLock::new();

/// Tells the loader who this mod is. Call it first in [`Mod::load`](crate::Mod::load);
/// a second call in the same mod is ignored.
///
/// ```
/// # use cinnaloader::prelude::*;
/// declare(
///     ModManifest::new("sprint", "Sprint", env!("CARGO_PKG_VERSION"))
///         .author("you")
///         .license("MIT"),
/// );
/// ```
pub fn declare(manifest: ModManifest) -> bool {
    crate::install_manifest(&manifest) && MANIFEST.set(manifest).is_ok()
}

/// What this mod declared, or an empty manifest if it has not declared yet.
#[must_use]
pub fn manifest() -> ModManifest {
    MANIFEST
        .get()
        .cloned()
        .unwrap_or_else(|| ModManifest::new("mod", "mod", "0"))
}

/// The loader's view of this mod: the declaration plus what only the loader knows.
///
/// Cheap enough to call per tick, but not free — it asks the loader and parses its
/// answer. Call it once where a tick needs several fields.
#[must_use]
pub fn report() -> ModInfo {
    if let Some(text) = crate::read_report() {
        return ModInfo::decode(&text);
    }
    // No loader (a unit test, or a call before `load`): answer from what we know locally.
    let manifest = manifest();
    let root = crate::mods_dir().map(|dir| PathBuf::from(dir).join(&manifest.id));
    ModInfo {
        mod_name: manifest.id.clone(),
        library: None,
        directory: root,
        state: State::Loaded,
        enabled: true,
        reloading: false,
        entrypoint: ModEntrypoint::default(),
        loader_version: String::new(),
        manifest,
    }
}

/// The manifest, as opposed to [`report`].
#[must_use]
pub fn metadata() -> ModMetadata {
    report()
}

/// What this mod needs the loader to allow. See [`crate::info::Capability`].
#[must_use]
pub fn capabilities() -> Vec<Capability> {
    manifest().capabilities
}

/// What this mod promises not to touch. See [`crate::info::Permission`].
#[must_use]
pub fn permissions() -> Vec<Permission> {
    manifest().permissions
}

/// The `<mod name>:<short id>` scope of every id this mod registers.
#[must_use]
pub fn namespace() -> ModNamespace {
    ModNamespace::new(report().mod_name)
}

/// This mod, as the loader holds it: its id and the library it came from.
#[must_use]
pub fn instance() -> ModInstance {
    report().instance()
}

/// Who this mod is, where it may write, and how it stands, in one value.
#[must_use]
pub fn context() -> ModContext {
    ModContext { info: report() }
}

/// Where the loader loaded this mod from, when it knows.
#[must_use]
pub fn directory() -> Option<PathBuf> {
    report().directory
}

fn subdirectory(pick: fn(&ModDirectory) -> PathBuf) -> Option<PathBuf> {
    report().directories().map(|dirs| pick(&dirs))
}

/// Where this mod may keep anything it can rebuild.
#[must_use]
pub fn cache_directory() -> Option<PathBuf> {
    subdirectory(ModDirectory::cache)
}

/// Where this mod may keep data that must survive a restart.
#[must_use]
pub fn data_directory() -> Option<PathBuf> {
    subdirectory(ModDirectory::data)
}

/// Where this mod may keep its settings.
#[must_use]
pub fn config_directory() -> Option<PathBuf> {
    subdirectory(ModDirectory::config)
}

/// Where this mod may keep client art.
#[must_use]
pub fn asset_directory() -> Option<PathBuf> {
    subdirectory(ModDirectory::assets)
}

/// Where this mod may keep everything else it ships.
#[must_use]
pub fn resource_directory() -> Option<PathBuf> {
    subdirectory(ModDirectory::resources)
}

/// This mod's short id, as declared.
#[must_use]
pub fn id() -> String {
    manifest().id
}

/// This mod's display name.
#[must_use]
pub fn name() -> String {
    manifest().name
}

/// This mod's version.
#[must_use]
pub fn version() -> ModVersion {
    manifest().version
}

/// One line about what this mod does.
#[must_use]
pub fn description() -> String {
    manifest().description
}

#[must_use]
pub fn authors() -> Vec<String> {
    manifest().authors
}

#[must_use]
pub fn contributors() -> Vec<String> {
    manifest().contributors
}

#[must_use]
pub fn website() -> Option<String> {
    manifest().website
}

#[must_use]
pub fn source_url() -> Option<String> {
    manifest().source
}

#[must_use]
pub fn license() -> Option<String> {
    manifest().license
}

/// The mod's icon, as a path relative to its assets directory.
#[must_use]
pub fn icon() -> Option<String> {
    manifest().icon
}

#[must_use]
pub fn credits() -> Vec<String> {
    manifest().credits
}

#[must_use]
pub fn tags() -> Vec<String> {
    manifest().tags
}

/// The menu tab this mod's settings are grouped under, when it names one.
#[must_use]
pub fn category() -> Option<String> {
    manifest().category
}

/// Where this mod expects to run.
#[must_use]
pub fn environment() -> Environment {
    manifest().environment
}

/// The symbols the loader resolved to load this mod.
#[must_use]
pub fn entrypoint() -> ModEntrypoint {
    report().entrypoint
}

/// The manifest's wire format version, so a mod can refuse a loader that is too old.
#[must_use]
pub fn manifest_version() -> u32 {
    manifest().format
}

#[must_use]
pub fn build_number() -> Option<u64> {
    manifest().build.number
}

#[must_use]
pub fn git_commit() -> Option<String> {
    manifest().build.git_commit
}

#[must_use]
pub fn build_timestamp() -> Option<u64> {
    manifest().build.timestamp
}

/// Whether this mod was built with debug assertions, the usual stand-in for "a dev build".
#[must_use]
pub fn is_development() -> bool {
    manifest().build.development
}

/// Whether the loader is reloading mods. Always false: this loader has no hot reload yet.
#[must_use]
pub fn is_reloading() -> bool {
    report().reloading
}

/// Whether the user has left this mod switched on.
#[must_use]
pub fn is_enabled() -> bool {
    report().enabled
}

/// How far along this mod is.
#[must_use]
pub fn state() -> State {
    report().state
}

/// The loader's own version, empty when no loader is present.
#[must_use]
pub fn loader_version() -> ModVersion {
    ModVersion::parse(&report().loader_version)
}

/// The mods directory the loader used, which is where this mod's own directory lives.
#[must_use]
pub fn plugin_directory() -> Option<PathBuf> {
    crate::mods_dir().map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_undeclared_mod_still_answers() {
        // Tests share a process, so this only asserts the fallback never panics.
        let manifest = manifest();
        assert!(!manifest.id.is_empty());
        let report = report();
        assert_eq!(report.manifest.id, manifest.id);
        assert!(!is_reloading());
    }

    #[test]
    fn a_declared_manifest_is_readable_without_a_loader() {
        assert!(declare(
            ModManifest::new("probe", "Probe", "1.0")
                .author("someone")
                .tag("test")
        ));
        assert_eq!(id(), "probe");
        assert_eq!(name(), "Probe");
        assert_eq!(version().major(), Some(1));
        assert_eq!(authors(), vec!["someone".to_owned()]);
        assert_eq!(tags(), vec!["test".to_owned()]);
        assert_eq!(namespace().scope("speed"), "probe:speed");
    }
}
