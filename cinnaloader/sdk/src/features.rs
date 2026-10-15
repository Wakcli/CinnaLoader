//! Feature flags: what a mod ships behind a switch, and what the client will not let it touch.
//!
//! A mod declares a feature by id with a stage, a default, an owner and the version it
//! arrived in. Declaring twice is refused by id, so a typo shows up rather than shadowing.
//! A feature can then be enabled and disabled — an id that was never declared comes back as
//! [`FeatureError::UnknownFeature`], and a query answers the declared default until something
//! is set. An experimental feature cannot be enabled until the mod records an
//! [`Acknowledgement`] of who opted in and when; a deprecated one enables with a
//! [`FeatureWarning`] naming its replacement. [`report`] says what is on, which of those are
//! experimental, and how many features sit in each stage, as text or through
//! [`crate::serialization`].
//!
//! What this host cannot reach is the *client's* own flags and experimental options: reading
//! them, changing them, or asking whether one is on. Each has a sentence in [`why_unknown`],
//! and [`refusals`] lists them.
//!
//! ```
//! use cinnaloader::features::{self, Feature, FeatureStage};
//!
//! features::register(Feature::new("render-hooks", "example", "0.7")
//!     .stage(FeatureStage::Experimental)).unwrap();
//! assert!(features::enable("render-hooks").is_err());
//! features::acknowledge("render-hooks", "ada", "2026-10-15T09:00:00Z").unwrap();
//! assert!(features::enable("render-hooks").is_ok());
//! assert_eq!(features::is_enabled("render-hooks"), Ok(true));
//! ```

use std::fmt;
use std::sync::{Mutex, MutexGuard, OnceLock};

use crate::serialization::{to_json, Error as SerializeError, Map, Value};
use crate::version::Version;

/// How settled a feature is, so a mod can decide whether to depend on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FeatureStage {
    Stable,
    Beta,
    Experimental,
    Deprecated,
}

impl FeatureStage {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Beta => "beta",
            Self::Experimental => "experimental",
            Self::Deprecated => "deprecated",
        }
    }
}

impl fmt::Display for FeatureStage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// What a feature's own state says about whether it can be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeatureAvailability {
    Available,
    /// Available only under a condition the mod set.
    Conditional,
    /// Available only after an acknowledgement.
    Experimental,
    /// Available, with a warning that it is going away.
    Deprecated,
    /// Declared, but this host cannot provide it.
    Unsupported,
    /// Declared once, now removed; kept as a tombstone so a report can name it.
    Removed,
}

/// Why a feature could not be declared or switched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeatureError {
    UnknownFeature(String),
    AlreadyDeclared(String),
    GateRequired(String),
}

impl fmt::Display for FeatureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownFeature(id) => write!(formatter, "feature {id} was never declared"),
            Self::AlreadyDeclared(id) => write!(formatter, "feature {id} is already declared"),
            Self::GateRequired(id) => write!(
                formatter,
                "feature {id} is experimental and needs an acknowledgement before it can be enabled"
            ),
        }
    }
}

impl std::error::Error for FeatureError {}

/// A record of who opted in to an experimental feature and when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Acknowledgement {
    pub by: String,
    pub at: String,
}

/// The warning a deprecated feature answers when it is enabled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureWarning {
    pub id: String,
    pub message: String,
    pub replaced_by: Option<String>,
}

impl fmt::Display for FeatureWarning {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.replaced_by {
            Some(to) => write!(
                formatter,
                "feature {} is deprecated: {} (use {to} instead)",
                self.id, self.message
            ),
            None => write!(formatter, "feature {} is deprecated: {}", self.id, self.message),
        }
    }
}

/// One feature a mod declared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Feature {
    pub id: String,
    pub stage: FeatureStage,
    pub default_enabled: bool,
    pub owner: String,
    pub since: Version,
    pub message: Option<String>,
    pub replaced_by: Option<String>,
    pub unsupported: bool,
    pub condition: Option<String>,
    pub requirements: Vec<String>,
}

impl Feature {
    #[must_use]
    pub fn new(id: &str, owner: &str, since: &str) -> Self {
        Self {
            id: id.to_owned(),
            stage: FeatureStage::Stable,
            default_enabled: false,
            owner: owner.to_owned(),
            since: Version::parse(since),
            message: None,
            replaced_by: None,
            unsupported: false,
            condition: None,
            requirements: Vec::new(),
        }
    }

    #[must_use]
    pub fn stage(mut self, stage: FeatureStage) -> Self {
        self.stage = stage;
        self
    }

    #[must_use]
    pub fn default_enabled(mut self, enabled: bool) -> Self {
        self.default_enabled = enabled;
        self
    }

    #[must_use]
    pub fn requires(mut self, what: &str) -> Self {
        self.requirements.push(what.to_owned());
        self
    }
}

/// What is on, which of those are experimental, and how many features each stage holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureReport {
    pub enabled: Vec<String>,
    pub experimental: Vec<String>,
    pub counts: Vec<(FeatureStage, usize)>,
}

#[derive(Debug, Clone, Default)]
struct State {
    explicit: Option<bool>,
    acknowledged: Option<Acknowledgement>,
}

type Watcher = Box<dyn FnMut(&str, bool) + Send>;

#[derive(Default)]
struct Registry {
    defs: Vec<Feature>,
    states: std::collections::BTreeMap<String, State>,
    removed: Vec<String>,
    watchers: Vec<(u64, Watcher)>,
    next_watcher: u64,
}

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Registry::default()))
}

fn lock() -> MutexGuard<'static, Registry> {
    registry().lock().unwrap_or_else(|error| error.into_inner())
}

fn with_def<R>(id: &str, change: impl FnOnce(&mut Feature) -> R) -> Option<R> {
    let mut reg = lock();
    reg.defs.iter_mut().find(|def| def.id == id).map(change)
}

/// Declares a feature. A second declaration of the same id is refused, not shadowed.
pub fn register(feature: Feature) -> Result<(), FeatureError> {
    let mut reg = lock();
    if reg.defs.iter().any(|def| def.id == feature.id) {
        crate::warn!("feature {} was declared twice", feature.id);
        return Err(FeatureError::AlreadyDeclared(feature.id));
    }
    reg.removed.retain(|removed| removed != &feature.id);
    reg.states.entry(feature.id.clone()).or_default();
    reg.defs.push(feature);
    Ok(())
}

/// Removes a feature and leaves a tombstone so a report can still name it.
pub fn unregister(id: &str) -> bool {
    let mut reg = lock();
    let before = reg.defs.len();
    reg.defs.retain(|def| def.id != id);
    reg.states.remove(id);
    let existed = reg.defs.len() != before;
    if existed && !reg.removed.iter().any(|removed| removed == id) {
        reg.removed.push(id.to_owned());
    }
    existed
}

/// One feature by id.
#[must_use]
pub fn get(id: &str) -> Option<Feature> {
    lock().defs.iter().find(|def| def.id == id).cloned()
}

/// Every declared feature, in the order it was declared.
#[must_use]
pub fn get_all() -> Vec<Feature> {
    lock().defs.clone()
}

/// Changes what a feature answers when nothing was set. An explicit set still wins.
pub fn set_default(id: &str, enabled: bool) -> bool {
    with_def(id, |def| def.default_enabled = enabled).is_some()
}

/// Sets the condition under which a feature applies.
pub fn set_condition(id: &str, condition: &str) -> bool {
    with_def(id, |def| def.condition = Some(condition.to_owned())).is_some()
}

/// What a feature needs before it can be used.
#[must_use]
pub fn get_requirements(id: &str) -> Vec<String> {
    match lock().defs.iter().find(|def| def.id == id) {
        Some(def) => def.requirements.clone(),
        None => Vec::new(),
    }
}

/// Whether a feature is on, answering its declared default when nothing was set.
pub fn is_enabled(id: &str) -> Result<bool, FeatureError> {
    let reg = lock();
    let Some(def) = reg.defs.iter().find(|def| def.id == id) else {
        return Err(FeatureError::UnknownFeature(id.to_owned()));
    };
    Ok(reg
        .states
        .get(id)
        .and_then(|state| state.explicit)
        .unwrap_or(def.default_enabled))
}

/// Turns a feature on. An experimental feature needs an acknowledgement first.
pub fn enable(id: &str) -> Result<Option<FeatureWarning>, FeatureError> {
    set_enabled(id, true)
}

/// Turns a feature off. The acknowledgement stays, but a stable no-gate rule still holds.
pub fn disable(id: &str) -> Result<Option<FeatureWarning>, FeatureError> {
    set_enabled(id, false)
}

fn set_enabled(id: &str, on: bool) -> Result<Option<FeatureWarning>, FeatureError> {
    let mut reg = lock();
    let Some(index) = reg.defs.iter().position(|def| def.id == id) else {
        crate::warn!("a mod changed feature {id}, which was never declared");
        return Err(FeatureError::UnknownFeature(id.to_owned()));
    };
    let stage = reg.defs[index].stage;
    let message = reg.defs[index].message.clone();
    let replaced_by = reg.defs[index].replaced_by.clone();
    let acknowledged = reg
        .states
        .get(id)
        .is_some_and(|state| state.acknowledged.is_some());
    if on && stage == FeatureStage::Experimental && !acknowledged {
        crate::warn!("feature {id} is experimental and has not been acknowledged");
        return Err(FeatureError::GateRequired(id.to_owned()));
    }
    let warning = if on && stage == FeatureStage::Deprecated {
        Some(FeatureWarning {
            id: id.to_owned(),
            message: message.unwrap_or_else(|| "this feature is deprecated".to_owned()),
            replaced_by,
        })
    } else {
        None
    };
    {
        let state = reg.states.entry(id.to_owned()).or_default();
        state.explicit = Some(on);
    }
    for (_, watcher) in &mut reg.watchers {
        watcher(id, on);
    }
    Ok(warning)
}

/// Records who opted in to a feature and when, which the experimental gate then reads.
pub fn acknowledge(id: &str, by: &str, at: &str) -> Result<(), FeatureError> {
    let mut reg = lock();
    if !reg.defs.iter().any(|def| def.id == id) {
        crate::warn!("a mod acknowledged feature {id}, which was never declared");
        return Err(FeatureError::UnknownFeature(id.to_owned()));
    }
    let state = reg.states.entry(id.to_owned()).or_default();
    state.acknowledged = Some(Acknowledgement {
        by: by.to_owned(),
        at: at.to_owned(),
    });
    Ok(())
}

/// The acknowledgement recorded for a feature, with the caller's own who and when.
#[must_use]
pub fn get_acknowledgement(id: &str) -> Option<Acknowledgement> {
    lock()
        .states
        .get(id)
        .and_then(|state| state.acknowledged.clone())
}

/// Moves a feature to the experimental stage.
pub fn mark_experimental(id: &str) -> bool {
    with_def(id, |def| def.stage = FeatureStage::Experimental).is_some()
}

/// Moves a feature to the deprecated stage with its message, version and replacement.
pub fn mark_deprecated(id: &str, message: &str, since: &str, replaced_by: Option<&str>) -> bool {
    with_def(id, |def| {
        def.stage = FeatureStage::Deprecated;
        def.message = Some(message.to_owned());
        def.since = Version::parse(since);
        def.replaced_by = replaced_by.map(str::to_owned);
    })
    .is_some()
}

/// Marks a feature as one this host cannot provide.
pub fn mark_unsupported(id: &str) -> bool {
    with_def(id, |def| def.unsupported = true).is_some()
}

/// What a feature's stage and flags say about whether it can be used.
#[must_use]
pub fn get_availability(id: &str) -> Option<FeatureAvailability> {
    let reg = lock();
    if reg.removed.iter().any(|removed| removed == id) {
        return Some(FeatureAvailability::Removed);
    }
    let def = reg.defs.iter().find(|def| def.id == id)?;
    Some(if def.unsupported {
        FeatureAvailability::Unsupported
    } else {
        match def.stage {
            FeatureStage::Deprecated => FeatureAvailability::Deprecated,
            FeatureStage::Experimental => FeatureAvailability::Experimental,
            FeatureStage::Stable | FeatureStage::Beta if def.condition.is_some() => {
                FeatureAvailability::Conditional
            }
            FeatureStage::Stable | FeatureStage::Beta => FeatureAvailability::Available,
        }
    })
}

/// The sentence that carries a deprecated feature's replacement, when it named one.
#[must_use]
pub fn get_migration_path(id: &str) -> Option<String> {
    let reg = lock();
    let def = reg.defs.iter().find(|def| def.id == id)?;
    def.replaced_by.as_ref().map(|to| format!("{id} -> {to}"))
}

/// Every feature in a stage, in declaration order.
fn ids_with_stage(stage: FeatureStage) -> Vec<String> {
    lock()
        .defs
        .iter()
        .filter(|def| def.stage == stage)
        .map(|def| def.id.clone())
        .collect()
}

#[must_use]
pub fn get_experimental() -> Vec<String> {
    ids_with_stage(FeatureStage::Experimental)
}

#[must_use]
pub fn get_deprecated() -> Vec<String> {
    ids_with_stage(FeatureStage::Deprecated)
}

/// Features that were declared and then unregistered, kept as tombstones.
#[must_use]
pub fn get_removed() -> Vec<String> {
    lock().removed.clone()
}

/// Calls `watcher` with an id and its new state whenever a feature is switched.
pub fn on_changed(watcher: impl FnMut(&str, bool) + Send + 'static) -> u64 {
    let mut reg = lock();
    reg.next_watcher += 1;
    let id = reg.next_watcher;
    reg.watchers.push((id, Box::new(watcher)));
    id
}

/// What is on now, which of those are experimental, and the count of each stage.
#[must_use]
pub fn report() -> FeatureReport {
    let reg = lock();
    let mut enabled = Vec::new();
    let mut experimental = Vec::new();
    for def in &reg.defs {
        let on = reg
            .states
            .get(&def.id)
            .and_then(|state| state.explicit)
            .unwrap_or(def.default_enabled);
        if on {
            enabled.push(def.id.clone());
            if def.stage == FeatureStage::Experimental {
                experimental.push(def.id.clone());
            }
        }
    }
    let mut counts = Vec::new();
    for stage in [
        FeatureStage::Stable,
        FeatureStage::Beta,
        FeatureStage::Experimental,
        FeatureStage::Deprecated,
    ] {
        let count = reg.defs.iter().filter(|def| def.stage == stage).count();
        counts.push((stage, count));
    }
    FeatureReport {
        enabled,
        experimental,
        counts,
    }
}

/// The report as one line, for a log or a panel.
#[must_use]
pub fn report_text() -> String {
    let report = report();
    let counts = report
        .counts
        .iter()
        .map(|(stage, count)| format!("{} {count}", stage.as_str()))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{} enabled, {} experimental | {counts}",
        report.enabled.len(),
        report.experimental.len()
    )
}

/// The report as JSON, through the crate's serializer.
pub fn report_json() -> Result<String, SerializeError> {
    let report = report();
    let mut root = Map::new();
    root.insert(
        "enabled",
        Value::List(report.enabled.iter().cloned().map(Value::str).collect()),
    );
    root.insert(
        "experimental",
        Value::List(report.experimental.iter().cloned().map(Value::str).collect()),
    );
    let mut counts = Map::new();
    for (stage, count) in &report.counts {
        counts.insert(stage.as_str(), Value::Int(*count as i64));
    }
    root.insert("counts", Value::Map(counts));
    to_json(&Value::Map(root))
}

/// Why a question about the client's own feature flags has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "flags" => "the client's feature flags are chosen in its own build and are not \
                    exposed to a mod"
            .to_owned(),
        "state" => "whether one of the client's flags is on is the client's own configuration"
            .to_owned(),
        "experimental" => "the client's experimental options are its own, chosen in its launcher"
            .to_owned(),
        "override" => "changing one of the client's flags would change the client itself, which \
                       this host cannot do"
            .to_owned(),
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach in this section.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["flags", "state", "experimental", "override"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

fn refused<T: Default>(what: &str) -> T {
    crate::warn!(
        "a mod asked this host about the client's feature flags: {}",
        why_unknown(what)
    );
    T::default()
}

/// The client's own feature flags. Empty.
#[must_use]
pub fn client_flags() -> Vec<String> {
    refused("flags")
}

/// Whether one of the client's flags is on. `false`.
#[must_use]
pub fn client_enabled(_id: &str) -> bool {
    refused("state")
}

/// The client's experimental options. Empty.
#[must_use]
pub fn client_experimental() -> Vec<String> {
    refused("experimental")
}

/// Turns one of the client's flags on or off. `false`.
pub fn set_client_flag(_id: &str, _on: bool) -> bool {
    refused("override")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        *lock() = Registry::default();
    }

    #[test]
    fn a_declaration_is_read_back_and_a_second_one_is_refused() {
        let _serial = serial();
        fresh();
        let feature = Feature::new("probe:alpha", "probe", "0.1");
        assert_eq!(register(feature.clone()), Ok(()));
        assert_eq!(get("probe:alpha"), Some(feature.clone()));
        assert_eq!(get_all().len(), 1);
        assert_eq!(
            register(feature),
            Err(FeatureError::AlreadyDeclared("probe:alpha".to_owned()))
        );
        assert_eq!(get_all().len(), 1);
    }

    #[test]
    fn an_undeclared_feature_is_refused_by_id() {
        let _serial = serial();
        fresh();
        assert_eq!(
            is_enabled("probe:ghost"),
            Err(FeatureError::UnknownFeature("probe:ghost".to_owned()))
        );
        assert_eq!(
            enable("probe:ghost"),
            Err(FeatureError::UnknownFeature("probe:ghost".to_owned()))
        );
        assert_eq!(get("probe:ghost"), None);
        assert_eq!(get_availability("probe:ghost"), None);
    }

    #[test]
    fn an_experimental_feature_needs_a_recorded_acknowledgement() {
        let _serial = serial();
        fresh();
        register(Feature::new("probe:net", "probe", "0.2").stage(FeatureStage::Experimental))
            .unwrap();
        assert_eq!(
            enable("probe:net"),
            Err(FeatureError::GateRequired("probe:net".to_owned()))
        );
        assert_eq!(is_enabled("probe:net"), Ok(false));
        assert_eq!(acknowledge("probe:net", "ada", "2026-10-15T09:00:00Z"), Ok(()));
        assert_eq!(
            get_acknowledgement("probe:net"),
            Some(Acknowledgement {
                by: "ada".to_owned(),
                at: "2026-10-15T09:00:00Z".to_owned(),
            })
        );
        assert_eq!(enable("probe:net"), Ok(None));
        assert_eq!(is_enabled("probe:net"), Ok(true));
        assert_eq!(disable("probe:net"), Ok(None));
        assert_eq!(is_enabled("probe:net"), Ok(false));
        assert_eq!(
            acknowledge("probe:ghost", "ada", "now"),
            Err(FeatureError::UnknownFeature("probe:ghost".to_owned()))
        );
    }

    #[test]
    fn a_stable_feature_answers_its_default_and_hears_a_change() {
        let _serial = serial();
        fresh();
        register(Feature::new("probe:hud", "probe", "0.1").default_enabled(true)).unwrap();
        assert_eq!(is_enabled("probe:hud"), Ok(true));
        assert_eq!(enable("probe:hud"), Ok(None));
        assert_eq!(get_availability("probe:hud"), Some(FeatureAvailability::Available));

        let heard = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = std::sync::Arc::clone(&heard);
        on_changed(move |id, on| sink.lock().unwrap().push(format!("{id}={on}")));
        enable("probe:hud").unwrap();
        assert_eq!(*heard.lock().unwrap(), vec!["probe:hud=true".to_owned()]);

        set_default("probe:hud", false);
        assert_eq!(
            is_enabled("probe:hud"),
            Ok(true),
            "an explicit set wins over the default"
        );
    }

    #[test]
    fn a_deprecated_feature_warns_and_names_its_replacement() {
        let _serial = serial();
        fresh();
        register(Feature::new("probe:old", "probe", "0.1")).unwrap();
        assert!(mark_deprecated(
            "probe:old",
            "the old path is gone",
            "0.3",
            Some("probe:new")
        ));
        assert_eq!(get_availability("probe:old"), Some(FeatureAvailability::Deprecated));
        let warning = enable("probe:old").unwrap().expect("a deprecation warning");
        assert_eq!(warning.id, "probe:old");
        assert_eq!(warning.message, "the old path is gone");
        assert_eq!(warning.replaced_by.as_deref(), Some("probe:new"));
        assert!(warning.to_string().contains("probe:new"));
        assert_eq!(
            get_migration_path("probe:old"),
            Some("probe:old -> probe:new".to_owned())
        );
        assert_eq!(get_migration_path("probe:top"), None);
    }

    #[test]
    fn the_listings_split_by_stage_and_unregister_tombstones() {
        let _serial = serial();
        fresh();
        register(Feature::new("probe:stable", "probe", "0.1")).unwrap();
        register(Feature::new("probe:beta", "probe", "0.1").stage(FeatureStage::Beta).requires("render"))
            .unwrap();
        register(Feature::new("probe:exp", "probe", "0.2").stage(FeatureStage::Experimental)).unwrap();
        register(Feature::new("probe:old", "probe", "0.1")).unwrap();
        mark_deprecated("probe:old", "gone", "0.4", None);
        mark_unsupported("probe:beta");

        assert_eq!(get_experimental(), vec!["probe:exp".to_owned()]);
        assert_eq!(get_deprecated(), vec!["probe:old".to_owned()]);
        assert_eq!(get_requirements("probe:beta"), vec!["render".to_owned()]);
        assert_eq!(get_availability("probe:beta"), Some(FeatureAvailability::Unsupported));
        assert_eq!(get_availability("probe:exp"), Some(FeatureAvailability::Experimental));
        assert_eq!(get_availability("probe:stable"), Some(FeatureAvailability::Available));

        assert!(set_condition("probe:stable", "when the render hook exists"));
        assert_eq!(get_availability("probe:stable"), Some(FeatureAvailability::Conditional));
        assert_eq!(
            get("probe:stable").unwrap().condition.as_deref(),
            Some("when the render hook exists")
        );

        assert!(unregister("probe:stable"));
        assert!(!unregister("probe:stable"));
        assert_eq!(get_removed(), vec!["probe:stable".to_owned()]);
        assert_eq!(get_availability("probe:stable"), Some(FeatureAvailability::Removed));
    }

    #[test]
    fn a_report_counts_each_stage() {
        let _serial = serial();
        fresh();
        register(Feature::new("probe:stable", "probe", "0.1")).unwrap();
        register(Feature::new("probe:stable2", "probe", "0.1").default_enabled(true)).unwrap();
        register(Feature::new("probe:exp", "probe", "0.2").stage(FeatureStage::Experimental)).unwrap();
        acknowledge("probe:exp", "ada", "2026-10-15").unwrap();
        enable("probe:exp").unwrap();
        register(Feature::new("probe:beta", "probe", "0.1").stage(FeatureStage::Beta)).unwrap();
        register(Feature::new("probe:old", "probe", "0.1")).unwrap();
        mark_deprecated("probe:old", "gone", "0.4", Some("probe:stable"));
        enable("probe:old").unwrap();

        let report = report();
        assert_eq!(
            report.enabled,
            vec![
                "probe:stable2".to_owned(),
                "probe:exp".to_owned(),
                "probe:old".to_owned(),
            ]
        );
        assert_eq!(report.experimental, vec!["probe:exp".to_owned()]);
        assert_eq!(
            report.counts,
            vec![
                (FeatureStage::Stable, 2),
                (FeatureStage::Beta, 1),
                (FeatureStage::Experimental, 1),
                (FeatureStage::Deprecated, 1),
            ]
        );
        assert!(report_text().contains("stable 2"));
        let json = report_json().unwrap();
        assert!(json.contains("\"experimental\""));
        assert!(json.contains("probe:exp"));
    }

    #[test]
    fn the_client_flags_are_refused_with_reasons() {
        assert_eq!(refusals().len(), 4);
        assert!(client_flags().is_empty());
        assert!(!client_enabled("probe"));
        assert!(client_experimental().is_empty());
        assert!(!set_client_flag("probe", true));
        for (what, why) in refusals() {
            assert!(!why.is_empty());
            assert_eq!(why, why_unknown(&what));
        }
        assert_eq!(
            why_unknown("unknown-key"),
            "unknown-key is not something this host can see"
        );
    }
}
