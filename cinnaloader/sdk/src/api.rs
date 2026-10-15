//! The API surface: what this loader exposes, the version it appeared in, how stable it is, and
//! what changed between two versions.
//!
//! An [`ApiItem`] has a `path` like `cinnaloader::blocks::get`, an [`ApiKind`], its signature as
//! text, the [`Version`] it appeared in and a [`Stability`] level. [`register`] refuses the same
//! path twice; [`get`] reads one back and [`get_exports`] lists them all.
//!
//! A [`Change`] records one path at one version. [`diff`] answers what was added, removed,
//! changed and deprecated between two versions, ordered by [`Version`], and [`get_change_log`]
//! renders the same list as lines.
//!
//! ```
//! use cinnaloader::api::{self, ChangeKind, Stability};
//!
//! api::clear();
//! assert!(api::register_function("cinnaloader::blocks::get", "fn get(id: &str) -> Option<Block>"));
//! assert!(api::has_function("cinnaloader::blocks::get"));
//! assert_eq!(api::get_stability("cinnaloader::blocks::get"), Some(Stability::Stable));
//! api::record_change(ChangeKind::Added, "0.2.0", "cinnaloader::blocks::get", "");
//! assert_eq!(api::diff("0.1.0", "0.2.0").added, vec!["cinnaloader::blocks::get".to_owned()]);
//! api::clear();
//! ```

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use crate::version::Version;

/// The version of the API surface this SDK describes.
pub const API_VERSION: &str = "1.0.0";

/// What an API item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ApiKind {
    Module,
    Type,
    Function,
    Constant,
}

impl ApiKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Module => "module",
            Self::Type => "type",
            Self::Function => "function",
            Self::Constant => "constant",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::all().into_iter().find(|kind| kind.as_str() == name)
    }

    #[must_use]
    pub fn all() -> Vec<Self> {
        vec![Self::Module, Self::Type, Self::Function, Self::Constant]
    }
}

/// How much a mod may depend on an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Stability {
    Stable,
    Beta,
    Experimental,
    Deprecated,
}

impl Stability {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Beta => "beta",
            Self::Experimental => "experimental",
            Self::Deprecated => "deprecated",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::all().into_iter().find(|level| level.as_str() == name)
    }

    #[must_use]
    pub fn all() -> Vec<Self> {
        vec![
            Self::Stable,
            Self::Beta,
            Self::Experimental,
            Self::Deprecated,
        ]
    }
}

/// One item on the API surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiItem {
    pub path: String,
    pub kind: ApiKind,
    pub signature: String,
    pub since: Version,
    pub stability: Stability,
    pub deprecated: Option<String>,
}

impl ApiItem {
    #[must_use]
    pub fn new(path: &str, kind: ApiKind, signature: &str) -> Self {
        Self {
            path: path.to_owned(),
            kind,
            signature: signature.to_owned(),
            since: Version::parse("0.1.0"),
            stability: Stability::Stable,
            deprecated: None,
        }
    }

    #[must_use]
    pub fn since(mut self, version: &str) -> Self {
        self.since = Version::parse(version);
        self
    }

    #[must_use]
    pub fn stability(mut self, stability: Stability) -> Self {
        self.stability = stability;
        self
    }

    /// Marks the item deprecated: the level becomes [`Stability::Deprecated`] with a message.
    #[must_use]
    pub fn deprecated(mut self, message: &str) -> Self {
        self.stability = Stability::Deprecated;
        self.deprecated = Some(message.to_owned());
        self
    }
}

/// What a change did to a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ChangeKind {
    Added,
    Removed,
    Changed,
    Deprecated,
}

impl ChangeKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Removed => "removed",
            Self::Changed => "changed",
            Self::Deprecated => "deprecated",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::all().into_iter().find(|kind| kind.as_str() == name)
    }

    #[must_use]
    pub fn all() -> Vec<Self> {
        vec![
            Self::Added,
            Self::Removed,
            Self::Changed,
            Self::Deprecated,
        ]
    }
}

/// One path touched at one version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub version: Version,
    pub path: String,
    pub kind: ChangeKind,
    pub detail: String,
}

impl Change {
    #[must_use]
    pub fn new(version: &str, path: &str, kind: ChangeKind, detail: &str) -> Self {
        Self {
            version: Version::parse(version),
            path: path.to_owned(),
            kind,
            detail: detail.to_owned(),
        }
    }
}

/// What changed between two versions, one path list per kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diff {
    pub from: Version,
    pub to: Version,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<String>,
    pub deprecated: Vec<String>,
}

impl Diff {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.added.is_empty()
            && self.removed.is_empty()
            && self.changed.is_empty()
            && self.deprecated.is_empty()
    }
}

/// How well this host's API serves a requested version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Support {
    Full,
    Partial,
    Unsupported,
}

impl Support {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Partial => "partial",
            Self::Unsupported => "unsupported",
        }
    }
}

fn items() -> &'static Mutex<BTreeMap<String, ApiItem>> {
    static ITEMS: OnceLock<Mutex<BTreeMap<String, ApiItem>>> = OnceLock::new();
    ITEMS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn changes() -> &'static Mutex<Vec<Change>> {
    static CHANGES: OnceLock<Mutex<Vec<Change>>> = OnceLock::new();
    CHANGES.get_or_init(|| Mutex::new(Vec::new()))
}

fn with_items<T>(body: impl FnOnce(&mut BTreeMap<String, ApiItem>) -> T) -> T {
    let mut guard = items().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn with_changes<T>(body: impl FnOnce(&mut Vec<Change>) -> T) -> T {
    let mut guard = changes().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Registers one item. `false` when the path is already taken.
pub fn register(item: ApiItem) -> bool {
    with_items(|items| {
        if items.contains_key(&item.path) {
            return false;
        }
        let path = item.path.clone();
        items.insert(path, item);
        true
    })
}

/// Registers a module path.
pub fn register_module(path: &str) -> bool {
    register(ApiItem::new(path, ApiKind::Module, ""))
}

/// Registers a type and its signature.
pub fn register_type(path: &str, signature: &str) -> bool {
    register(ApiItem::new(path, ApiKind::Type, signature))
}

/// Registers a function and its signature.
pub fn register_function(path: &str, signature: &str) -> bool {
    register(ApiItem::new(path, ApiKind::Function, signature))
}

/// Forgets one path.
pub fn unregister(path: &str) -> bool {
    with_items(|items| items.remove(path).is_some())
}

/// Forgets every item and every change.
pub fn clear() {
    with_items(|items| items.clear());
    with_changes(|changes| changes.clear());
}

/// The API version this host describes.
#[must_use]
pub fn get_version() -> Version {
    Version::parse(API_VERSION)
}

/// One item by path.
#[must_use]
pub fn get(path: &str) -> Option<ApiItem> {
    with_items(|items| items.get(path).cloned())
}

fn of_kind(kind: ApiKind) -> Vec<ApiItem> {
    with_items(|items| {
        items
            .values()
            .filter(|item| item.kind == kind)
            .cloned()
            .collect()
    })
}

/// Every module, by path.
#[must_use]
pub fn get_modules() -> Vec<ApiItem> {
    of_kind(ApiKind::Module)
}

/// Every type, by path.
#[must_use]
pub fn get_types() -> Vec<ApiItem> {
    of_kind(ApiKind::Type)
}

/// Every item on the surface, by path.
#[must_use]
pub fn get_exports() -> Vec<ApiItem> {
    with_items(|items| items.values().cloned().collect())
}

/// Every `(path, signature)` pair, by path.
#[must_use]
pub fn get_signatures() -> Vec<(String, String)> {
    with_items(|items| {
        items
            .values()
            .map(|item| (item.path.clone(), item.signature.clone()))
            .collect()
    })
}

/// Every capability a mod may use, with whether this host supports it.
#[must_use]
pub fn get_feature_flags() -> Vec<(String, bool)> {
    crate::capability::ALL
        .iter()
        .map(|capability| (capability.as_str().to_owned(), capability.is_supported()))
        .collect()
}

/// Every capability this host knows, with its description.
#[must_use]
pub fn get_capabilities() -> Vec<(String, String)> {
    crate::capability::ALL
        .iter()
        .map(|capability| (capability.as_str().to_owned(), capability.describe().to_owned()))
        .collect()
}

/// Every deprecated item, as `(path, message)`.
#[must_use]
pub fn get_deprecations() -> Vec<(String, String)> {
    with_items(|items| {
        items
            .values()
            .filter_map(|item| {
                item.deprecated
                    .as_ref()
                    .map(|message| (item.path.clone(), message.clone()))
            })
            .collect()
    })
}

/// Every path a change removed, sorted and deduplicated.
#[must_use]
pub fn get_removed_symbols() -> Vec<String> {
    let mut removed: Vec<String> = all_changes()
        .into_iter()
        .filter(|change| change.kind == ChangeKind::Removed)
        .map(|change| change.path)
        .collect();
    removed.sort();
    removed.dedup();
    removed
}

/// The stability level of one path.
#[must_use]
pub fn get_stability(path: &str) -> Option<Stability> {
    get(path).map(|item| item.stability)
}

/// Every change, one line each, in version then path then kind order.
#[must_use]
pub fn get_change_log() -> Vec<String> {
    all_changes()
        .iter()
        .map(|change| format!("{} {} {}", change.version, change.kind.as_str(), change.path))
        .collect()
}

/// Whether the surface has a function at this path.
#[must_use]
pub fn has_function(path: &str) -> bool {
    get(path).is_some_and(|item| item.kind == ApiKind::Function)
}

/// Whether the surface has a type at this path.
#[must_use]
pub fn has_type(path: &str) -> bool {
    get(path).is_some_and(|item| item.kind == ApiKind::Type)
}

/// Whether the surface has a module at this path.
#[must_use]
pub fn has_module(path: &str) -> bool {
    get(path).is_some_and(|item| item.kind == ApiKind::Module)
}

/// Whether a path is on the surface at all.
#[must_use]
pub fn is_available(path: &str) -> bool {
    get(path).is_some()
}

/// How well this host serves a requested version.
#[must_use]
pub fn get_compatibility(version: &str) -> Support {
    let wanted = Version::parse(version);
    if !wanted.is_numeric() {
        return Support::Unsupported;
    }
    let have = get_version();
    match wanted.cmp(&have) {
        Ordering::Less | Ordering::Equal => Support::Full,
        Ordering::Greater if wanted.major() == have.major() => Support::Partial,
        Ordering::Greater => Support::Unsupported,
    }
}

/// Records one change. `false` when that kind is already recorded for the path at the version.
pub fn record(change: Change) -> bool {
    with_changes(|changes| {
        if changes.iter().any(|held| {
            held.version == change.version && held.path == change.path && held.kind == change.kind
        }) {
            return false;
        }
        changes.push(change);
        true
    })
}

/// Records one change from its parts.
pub fn record_change(kind: ChangeKind, version: &str, path: &str, detail: &str) -> bool {
    record(Change::new(version, path, kind, detail))
}

fn all_changes() -> Vec<Change> {
    let mut changes = with_changes(|changes| changes.clone());
    changes.sort_by(|left, right| {
        left.version
            .cmp(&right.version)
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| left.kind.cmp(&right.kind))
    });
    changes
}

/// What changed between `from` (exclusive) and `to` (inclusive), by [`Version`] order.
#[must_use]
pub fn diff(from: &str, to: &str) -> Diff {
    let from = Version::parse(from);
    let to = Version::parse(to);
    let mut out = Diff {
        from: from.clone(),
        to: to.clone(),
        added: Vec::new(),
        removed: Vec::new(),
        changed: Vec::new(),
        deprecated: Vec::new(),
    };
    for change in all_changes() {
        if change.version <= from || change.version > to {
            continue;
        }
        let bucket = match change.kind {
            ChangeKind::Added => &mut out.added,
            ChangeKind::Removed => &mut out.removed,
            ChangeKind::Changed => &mut out.changed,
            ChangeKind::Deprecated => &mut out.deprecated,
        };
        if !bucket.contains(&change.path) {
            bucket.push(change.path.clone());
        }
    }
    out.added.sort();
    out.removed.sort();
    out.changed.sort();
    out.deprecated.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        clear();
    }

    #[test]
    fn a_duplicate_path_is_refused_and_kinds_are_kept() {
        let _serial = serial();
        fresh();
        assert!(register_module("cinnaloader::blocks"));
        assert!(!register_module("cinnaloader::blocks"), "per path, once");
        assert!(register_type("cinnaloader::blocks::Block", "struct Block"));
        assert!(register_function("cinnaloader::blocks::get", "fn get(id: &str) -> Option<Block>"));
        assert!(!register_function("cinnaloader::blocks::get", "again"));
        assert!(register(ApiItem::new("cinnaloader::blocks::COAL", ApiKind::Constant, "const COAL")));
        assert_eq!(get_exports().len(), 4);
    }

    #[test]
    fn a_lookup_by_path_returns_kind_signature_version_and_stability() {
        let _serial = serial();
        fresh();
        assert!(register(
            ApiItem::new(
                "cinnaloader::blocks::get",
                ApiKind::Function,
                "fn get(id: &str) -> Option<Block>"
            )
            .since("0.2.0")
            .stability(Stability::Beta)
        ));
        let item = get("cinnaloader::blocks::get").expect("declared");
        assert_eq!(item.kind, ApiKind::Function);
        assert_eq!(item.signature, "fn get(id: &str) -> Option<Block>");
        assert_eq!(item.since, Version::parse("0.2.0"));
        assert_eq!(item.stability, Stability::Beta);
        assert_eq!(get_stability("cinnaloader::blocks::get"), Some(Stability::Beta));
        assert!(get("cinnaloader::blocks::nothing").is_none());
    }

    #[test]
    fn the_listings_split_by_kind() {
        let _serial = serial();
        fresh();
        assert!(register_module("cinnaloader::blocks"));
        assert!(register_module("cinnaloader::items"));
        assert!(register_type("cinnaloader::blocks::Block", "struct Block"));
        assert!(register_function("cinnaloader::blocks::get", "fn get(id: &str) -> Option<Block>"));
        assert_eq!(get_modules().len(), 2);
        assert_eq!(get_types().len(), 1);
        assert_eq!(get_exports().len(), 4);
        assert!(has_module("cinnaloader::blocks"));
        assert!(has_type("cinnaloader::blocks::Block"));
        assert!(has_function("cinnaloader::blocks::get"));
        assert!(!has_function("cinnaloader::blocks::Block"), "a type is not a function");
        assert!(is_available("cinnaloader::blocks::get"));
        assert!(!is_available("cinnaloader::blocks::missing"));
        assert_eq!(
            get_signatures(),
            vec![
                ("cinnaloader::blocks".to_owned(), String::new()),
                ("cinnaloader::blocks::Block".to_owned(), "struct Block".to_owned()),
                (
                    "cinnaloader::blocks::get".to_owned(),
                    "fn get(id: &str) -> Option<Block>".to_owned()
                ),
                ("cinnaloader::items".to_owned(), String::new()),
            ]
        );
    }

    #[test]
    fn an_exact_diff_lists_added_removed_changed_and_deprecated() {
        let _serial = serial();
        fresh();
        assert!(register_function("cinnaloader::blocks::get", "fn get(id: &str) -> Option<Block>"));
        assert!(record_change(ChangeKind::Added, "0.2.0", "cinnaloader::blocks::get", ""));
        assert!(record_change(ChangeKind::Removed, "0.2.0", "cinnaloader::blocks::old_get", ""));
        assert!(record_change(
            ChangeKind::Changed,
            "0.2.0",
            "cinnaloader::blocks::get_by_id",
            "now takes a BlockId"
        ));
        assert!(record_change(
            ChangeKind::Deprecated,
            "0.2.0",
            "cinnaloader::blocks::legacy",
            "use cinnaloader::blocks::get"
        ));
        assert!(record_change(ChangeKind::Added, "0.3.0", "cinnaloader::chunks::load", ""));

        let window = diff("0.1.0", "0.2.0");
        assert_eq!(window.added, vec!["cinnaloader::blocks::get".to_owned()]);
        assert_eq!(window.removed, vec!["cinnaloader::blocks::old_get".to_owned()]);
        assert_eq!(window.changed, vec!["cinnaloader::blocks::get_by_id".to_owned()]);
        assert_eq!(window.deprecated, vec!["cinnaloader::blocks::legacy".to_owned()]);
        assert!(!window.is_empty());

        let later = diff("0.2.0", "0.3.0");
        assert_eq!(later.added, vec!["cinnaloader::chunks::load".to_owned()]);
        assert!(later.removed.is_empty());
        assert!(diff("0.3.0", "0.3.0").is_empty());
    }

    #[test]
    fn the_change_log_and_removed_symbols_read_the_same_record() {
        let _serial = serial();
        fresh();
        assert!(record_change(ChangeKind::Added, "0.2.0", "cinnaloader::blocks::get", ""));
        assert!(record_change(ChangeKind::Removed, "0.2.0", "cinnaloader::blocks::old_get", ""));
        assert!(!record_change(
            ChangeKind::Removed,
            "0.2.0",
            "cinnaloader::blocks::old_get",
            "duplicate"
        ));
        assert_eq!(
            get_change_log(),
            vec![
                "0.2.0 added cinnaloader::blocks::get".to_owned(),
                "0.2.0 removed cinnaloader::blocks::old_get".to_owned(),
            ]
        );
        assert_eq!(get_removed_symbols(), vec!["cinnaloader::blocks::old_get".to_owned()]);
    }

    #[test]
    fn stability_the_deprecations_and_compatibility() {
        let _serial = serial();
        fresh();
        assert!(register(
            ApiItem::new("cinnaloader::blocks::legacy", ApiKind::Function, "fn legacy()")
                .deprecated("use cinnaloader::blocks::get")
        ));
        assert_eq!(
            get_stability("cinnaloader::blocks::legacy"),
            Some(Stability::Deprecated)
        );
        assert_eq!(
            get_deprecations(),
            vec![(
                "cinnaloader::blocks::legacy".to_owned(),
                "use cinnaloader::blocks::get".to_owned()
            )]
        );
        assert_eq!(Stability::from_name("experimental"), Some(Stability::Experimental));
        assert_eq!(ApiKind::from_name("constant"), Some(ApiKind::Constant));

        assert_eq!(get_version(), Version::parse(API_VERSION));
        assert_eq!(get_compatibility("1.0.0"), Support::Full);
        assert_eq!(get_compatibility("0.9.0"), Support::Full);
        assert_eq!(get_compatibility("1.4.0"), Support::Partial);
        assert_eq!(get_compatibility("2.0.0"), Support::Unsupported);
        assert_eq!(get_compatibility("banana"), Support::Unsupported);
        assert_eq!(Support::Partial.as_str(), "partial");
    }

    #[test]
    fn the_feature_flags_and_capabilities_come_from_the_capability_list() {
        assert_eq!(get_feature_flags().len(), crate::capability::ALL.len());
        assert_eq!(get_capabilities().len(), crate::capability::ALL.len());
        assert_eq!(get_feature_flags().len(), 38);
        assert!(get_feature_flags().iter().any(|(name, _)| name == "render"));
        assert!(get_capabilities()
            .iter()
            .any(|(name, description)| name == "render" && !description.is_empty()));
        assert_eq!(ChangeKind::from_name("changed"), Some(ChangeKind::Changed));
    }
}
