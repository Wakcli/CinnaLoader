//! Who needs whom, and in what order mods load.
//!
//! A manifest declares its dependencies, the order it wants relative to other mods, and
//! what it conflicts with. The loader collects every manifest *before* it loads anything,
//! resolves the graph, and only then runs `load` in the order that came out — so a mod can
//! count on its dependencies being ready.
//!
//! The engine here is pure: it takes manifests and answers an order, the cycles, and what
//! is missing. That makes it testable without a loader, and the loader and a mod see the
//! same answers.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::modinfo::ModManifest;
use crate::version::VersionReq;

/// How much a mod cares about another one being present.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DependencyKind {
    /// Refuse to load without it.
    #[default]
    Required,
    /// Load either way, and tell the mod whether it is there.
    Optional,
    /// Like optional, but the mod would like it.
    Recommended,
    /// A hint, nothing more.
    Suggested,
}

impl DependencyKind {
    #[must_use]
    fn as_str(self) -> &'static str {
        match self {
            Self::Required => "required",
            Self::Optional => "optional",
            Self::Recommended => "recommended",
            Self::Suggested => "suggested",
        }
    }

    #[must_use]
    fn from_str(text: &str) -> Option<Self> {
        Some(match text {
            "required" => Self::Required,
            "optional" => Self::Optional,
            "recommended" => Self::Recommended,
            "suggested" => Self::Suggested,
            _ => return None,
        })
    }

    /// Whether a missing dependency must stop the load.
    #[must_use]
    pub fn is_required(self) -> bool {
        matches!(self, Self::Required)
    }
}

impl fmt::Display for DependencyKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One mod this manifest needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModDependency {
    /// The dependency's id, as it declares itself.
    pub id: String,
    /// The versions that satisfy it; anything when empty.
    pub requirement: VersionReq,
    pub kind: DependencyKind,
    /// Why the mod wants it, for a log line or a refusal.
    pub reason: Option<String>,
}

impl ModDependency {
    #[must_use]
    pub fn new(id: &str) -> Self {
        Self {
            id: id.to_owned(),
            requirement: VersionReq::default(),
            kind: DependencyKind::Required,
            reason: None,
        }
    }

    /// The same dependency, versioned.
    #[must_use]
    pub fn versioned(self, requirement: &str) -> Self {
        Self {
            requirement: VersionReq::parse(requirement).unwrap_or_default(),
            ..self
        }
    }

    #[must_use]
    pub fn kind(mut self, kind: DependencyKind) -> Self {
        self.kind = kind;
        self
    }

    #[must_use]
    pub fn because(mut self, reason: &str) -> Self {
        self.reason = Some(reason.to_owned());
        self
    }

    /// The dependency as one wire-format field.
    #[must_use]
    pub fn encode(&self) -> String {
        let mut out = format!("{}|{}|{}", self.id, self.kind, self.requirement);
        if let Some(reason) = &self.reason {
            out.push('|');
            out.push_str(reason);
        }
        out
    }

    /// Reads what [`Self::encode`] wrote.
    #[must_use]
    pub fn decode(text: &str) -> Option<Self> {
        let mut parts = text.split('|');
        let id = parts.next()?.to_owned();
        if id.is_empty() {
            return None;
        }
        let kind = parts.next().and_then(DependencyKind::from_str).unwrap_or_default();
        let requirement = parts.next().map(VersionReq::parse).transpose().ok()?;
        Some(Self {
            id,
            requirement: requirement.unwrap_or_default(),
            kind,
            reason: parts.next().map(str::to_owned),
        })
    }
}

/// A dependency a mod can live without. The wishlist's name for it.
pub type OptionalDependency = ModDependency;
/// A dependency that only costs features when missing.
pub type SoftDependency = ModDependency;
/// A dependency with a version requirement.
pub type VersionedDependency = ModDependency;

/// A mod this manifest will not load beside.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModConflict {
    pub id: String,
    /// The versions it conflicts with; all of them when empty.
    pub requirement: VersionReq,
    /// Whether this is a hard conflict rather than a known-bad pairing.
    pub breaking: bool,
}

impl ModConflict {
    #[must_use]
    pub fn new(id: &str, breaking: bool) -> Self {
        Self {
            id: id.to_owned(),
            requirement: VersionReq::default(),
            breaking,
        }
    }

    /// The conflict as one wire-format field.
    #[must_use]
    pub fn encode(&self) -> String {
        format!(
            "{}|{}|{}",
            self.id,
            u8::from(self.breaking),
            self.requirement
        )
    }

    /// Reads what [`Self::encode`] wrote.
    #[must_use]
    pub fn decode(text: &str) -> Option<Self> {
        let mut parts = text.split('|');
        let id = parts.next()?.to_owned();
        if id.is_empty() {
            return None;
        }
        let breaking = parts.next() == Some("1");
        let requirement = parts
            .next()
            .map(VersionReq::parse)
            .transpose()
            .ok()?
            .unwrap_or_default();
        Some(Self {
            id,
            requirement,
            breaking,
        })
    }
}

/// One mod in the graph, as the resolver needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphNode {
    /// The loader's name for the mod, which is also what scopes its ids.
    pub name: String,
    /// The id the mod declares, which dependencies refer to.
    pub id: String,
    pub version: crate::modinfo::ModVersion,
    pub dependencies: Vec<ModDependency>,
    pub load_after: Vec<String>,
    pub load_before: Vec<String>,
    pub conflicts: Vec<ModConflict>,
    pub provides: Vec<String>,
    pub replaces: Vec<String>,
}

impl GraphNode {
    #[must_use]
    pub fn from_manifest(name: &str, manifest: &ModManifest) -> Self {
        Self {
            name: name.to_owned(),
            id: manifest.id.clone(),
            version: manifest.version.clone(),
            dependencies: manifest.dependencies.clone(),
            load_after: manifest.load_after.clone(),
            load_before: manifest.load_before.clone(),
            conflicts: manifest.conflicts.clone(),
            provides: manifest.provides.clone(),
            replaces: manifest.replaces.clone(),
        }
    }
}

/// Why a mod cannot load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolutionErrorKind {
    /// A required dependency is not present.
    Missing,
    /// A dependency is present at a version the requirement refuses.
    VersionMismatch,
    /// Two mods conflict.
    Conflict,
    /// The load order cannot be satisfied.
    Cycle,
}

/// One refusal, with enough detail to act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModResolutionError {
    pub mod_name: String,
    pub kind: ResolutionErrorKind,
    pub detail: String,
}

impl fmt::Display for ModResolutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.detail)
    }
}

/// A cycle, as the ids that form it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CircularDependencyReport {
    pub cycle: Vec<String>,
}

impl fmt::Display for CircularDependencyReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "load order cycle: {}", self.cycle.join(" -> "))
    }
}

/// An ordering constraint between two mods.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadOrderConstraint {
    pub before: String,
    pub after: String,
}

/// What the resolver worked out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DependencyGraph {
    pub nodes: Vec<GraphNode>,
    /// The order mods load in, by loader name.
    pub order: Vec<String>,
    /// One line per ordering decision, for `explain_load_order`.
    pub explanation: Vec<String>,
    pub errors: Vec<ModResolutionError>,
    pub cycles: Vec<CircularDependencyReport>,
}

impl DependencyGraph {
    /// Builds the graph from every mod's manifest.
    #[must_use]
    pub fn build(nodes: Vec<GraphNode>) -> Self {
        Self {
            nodes,
            ..Self::default()
        }
    }

    /// Resolves versions, conflicts and the load order. Errors are reported per mod: a mod
    /// whose dependency is missing does not load, and neither does a mod that is part of a
    /// cycle, but the rest still can.
    pub fn resolve(mut self) -> Self {
        let by_id: BTreeMap<String, &GraphNode> = self
            .nodes
            .iter()
            .map(|node| (node.id.clone(), node))
            .collect();
        let mut errors = Vec::new();

        for node in &self.nodes {
            for dependency in &node.dependencies {
                match by_id.get(&dependency.id) {
                    None => {
                        if dependency.kind.is_required() {
                            errors.push(ModResolutionError {
                                mod_name: node.name.clone(),
                                kind: ResolutionErrorKind::Missing,
                                detail: format!(
                                    "{} needs {}, which is not installed{}",
                                    node.name,
                                    dependency.id,
                                    dependency
                                        .reason
                                        .as_ref()
                                        .map(|reason| format!(" ({reason})"))
                                        .unwrap_or_default()
                                ),
                            });
                        }
                    }
                    Some(found) => {
                        if !dependency.requirement.is_any()
                            && !dependency.requirement.matches(&found.version)
                        {
                            errors.push(ModResolutionError {
                                mod_name: node.name.clone(),
                                kind: ResolutionErrorKind::VersionMismatch,
                                detail: format!(
                                    "{} needs {} {}, but {} is installed",
                                    node.name, dependency.id, dependency.requirement, found.version
                                ),
                            });
                        }
                    }
                }
            }
            for conflict in &node.conflicts {
                if let Some(other) = by_id.get(&conflict.id) {
                    if conflict.requirement.is_any()
                        || conflict.requirement.matches(&other.version)
                    {
                        errors.push(ModResolutionError {
                            mod_name: node.name.clone(),
                            kind: ResolutionErrorKind::Conflict,
                            detail: format!(
                                "{} {} {} {}",
                                node.name,
                                if conflict.breaking {
                                    "cannot run beside"
                                } else {
                                    "has a known problem with"
                                },
                                other.name,
                                other.version
                            ),
                        });
                    }
                }
            }
        }

        let (order, cycles, explanation) = self.topological_order(&errors);
        self.errors = errors;
        self.cycles = cycles;
        self.order = order;
        self.explanation = explanation;
        self
    }

    /// Mods that can load, in order. A mod in a cycle, or with an error, is left out.
    fn topological_order(
        &self,
        errors: &[ModResolutionError],
    ) -> (Vec<String>, Vec<CircularDependencyReport>, Vec<String>) {
        let broken: BTreeSet<String> = errors
            .iter()
            .map(|error| error.mod_name.clone())
            .collect();
        let by_name: BTreeMap<&str, &GraphNode> = self
            .nodes
            .iter()
            .map(|node| (node.name.as_str(), node))
            .collect();
        let by_id: BTreeMap<&str, &GraphNode> = self
            .nodes
            .iter()
            .map(|node| (node.id.as_str(), node))
            .collect();

        // Edges: a mod comes after each dependency and each `load_after` target.
        let mut after: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        for node in &self.nodes {
            let mut wanted: BTreeSet<&str> = BTreeSet::new();
            for dependency in &node.dependencies {
                if let Some(other) = by_id.get(dependency.id.as_str()) {
                    wanted.insert(other.name.as_str());
                }
            }
            for id in node.load_after.iter().chain(node.replaces.iter()) {
                if let Some(other) = by_id.get(id.as_str()) {
                    wanted.insert(other.name.as_str());
                }
            }
            for other in &self.nodes {
                if other.load_before.iter().any(|id| id == &node.id) {
                    wanted.insert(other.name.as_str());
                }
            }
            wanted.remove(node.name.as_str());
            after.insert(node.name.as_str(), wanted);
        }

        let mut order: Vec<String> = Vec::new();
        let mut explanation: Vec<String> = Vec::new();
        let mut done: BTreeSet<&str> = BTreeSet::new();
        let mut remaining: Vec<&str> = self
            .nodes
            .iter()
            .map(|node| node.name.as_str())
            .filter(|name| !broken.contains(*name))
            .collect();

        while !remaining.is_empty() {
            let Some((index, ready)) = remaining
                .iter()
                .enumerate()
                .find(|(_, name)| {
                    after
                        .get(**name)
                        .is_none_or(|wanted| wanted.iter().all(|want| done.contains(want)))
                })
                .map(|(index, name)| (index, *name))
            else {
                break;
            };
            let node = by_name.get(ready).expect("a remaining node is known");
            let after_ids: Vec<&str> = after
                .get(ready)
                .map(|wanted| wanted.iter().copied().collect())
                .unwrap_or_default();
            if !after_ids.is_empty() {
                explanation.push(format!("{} after {}", node.name, after_ids.join(", ")));
            }
            order.push(ready.to_owned());
            done.insert(ready);
            remaining.remove(index);
        }

        let mut cycles = Vec::new();
        if !remaining.is_empty() {
            cycles.push(CircularDependencyReport {
                cycle: remaining.iter().map(|name| (*name).to_owned()).collect(),
            });
        }
        (order, cycles, explanation)
    }

    /// What is missing, for a mod or for the whole set.
    #[must_use]
    pub fn missing(&self) -> Vec<String> {
        self.errors
            .iter()
            .filter(|error| error.kind == ResolutionErrorKind::Missing)
            .map(|error| error.detail.clone())
            .collect()
    }

    /// Every dependency of `name`, as ids.
    #[must_use]
    pub fn dependencies_of(&self, name: &str) -> Vec<String> {
        self.nodes
            .iter()
            .find(|node| node.name == name)
            .map(|node| node.dependencies.iter().map(|dep| dep.id.clone()).collect())
            .unwrap_or_default()
    }

    /// Every mod that depends on `id`.
    #[must_use]
    pub fn dependents_of(&self, id: &str) -> Vec<String> {
        self.nodes
            .iter()
            .filter(|node| node.dependencies.iter().any(|dep| dep.id == id))
            .map(|node| node.name.clone())
            .collect()
    }

    /// Whether `name` runs after `other`, following the resolved order.
    #[must_use]
    pub fn loads_after(&self, name: &str, other: &str) -> bool {
        let position = |wanted: &str| self.order.iter().position(|name| name == wanted);
        match (position(name), position(other)) {
            (Some(left), Some(right)) => left > right,
            _ => false,
        }
    }

    /// The version a mod provides, by id.
    #[must_use]
    pub fn version_of(&self, id: &str) -> Option<&crate::modinfo::ModVersion> {
        self.nodes
            .iter()
            .find(|node| node.id == id)
            .map(|node| &node.version)
    }
}

/// The resolver the loader keeps between discovery and loading.
#[derive(Debug, Clone, Default)]
pub struct DependencyResolver {
    nodes: Vec<GraphNode>,
}

impl DependencyResolver {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, node: GraphNode) {
        self.nodes.push(node);
    }

    /// Resolves the graph, and reports anything that would stop a mod loading.
    #[must_use]
    pub fn resolve(self) -> DependencyGraph {
        DependencyGraph::build(self.nodes).resolve()
    }

    /// Validates without resolving, for a mod that wants the graph before it is loaded.
    #[must_use]
    pub fn validate(&self) -> Vec<ModResolutionError> {
        DependencyGraph::build(self.nodes.clone()).resolve().errors
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modinfo::ModManifest;

    fn node(name: &str, manifest: ModManifest) -> GraphNode {
        GraphNode::from_manifest(name, &manifest)
    }

    #[test]
    fn a_dependency_loads_first() {
        let base = node("core", ModManifest::new("core", "Core", "1.0"));
        let user = node(
            "user",
            ModManifest::new("user", "User", "1.0").depends_on("core"),
        );
        // Deliberately given out of order.
        let graph = DependencyGraph::build(vec![user, base]).resolve();
        assert!(graph.errors.is_empty(), "{:?}", graph.errors);
        assert_eq!(graph.order, vec!["core", "user"]);
        assert!(graph.loads_after("user", "core"));
        assert_eq!(graph.dependents_of("core"), vec!["user".to_owned()]);
    }

    #[test]
    fn a_missing_required_dependency_stops_that_mod_only() {
        let lonely = node(
            "lonely",
            ModManifest::new("lonely", "Lonely", "1.0").depends_on("absent"),
        );
        let fine = node("fine", ModManifest::new("fine", "Fine", "1.0"));
        let graph = DependencyResolver::new()
            .tap(|resolver| {
                resolver.add(lonely);
                resolver.add(fine);
            })
            .resolve();
        assert_eq!(graph.order, vec!["fine"], "the rest still loads");
        assert_eq!(graph.missing().len(), 1);
        assert!(graph.missing()[0].contains("needs absent"));
    }

    #[test]
    fn an_optional_dependency_may_be_absent() {
        let loose = node(
            "loose",
            ModManifest::new("loose", "Loose", "1.0").depends_on_optional("absent"),
        );
        let graph = DependencyGraph::build(vec![loose]).resolve();
        assert!(graph.errors.is_empty());
        assert_eq!(graph.order, vec!["loose"]);
    }

    #[test]
    fn a_version_requirement_is_checked() {
        let core = node("core", ModManifest::new("core", "Core", "1.2.0"));
        let user = node(
            "user",
            ModManifest::new("user", "User", "1.0").depends_on_versioned("core", ">=2"),
        );
        let graph = DependencyGraph::build(vec![core, user]).resolve();
        assert_eq!(graph.errors.len(), 1);
        assert!(graph.errors[0].detail.contains("needs core >=2"));
        assert_eq!(graph.order, vec!["core"], "the bad mod is left out");
    }

    #[test]
    fn a_conflict_is_reported_from_both_sides_of_the_rule() {
        let first = node(
            "first",
            ModManifest::new("first", "First", "1.0").conflicts_with("second"),
        );
        let second = node("second", ModManifest::new("second", "Second", "1.0"));
        let graph = DependencyGraph::build(vec![first, second]).resolve();
        assert_eq!(graph.errors.len(), 1);
        assert_eq!(graph.errors[0].kind, ResolutionErrorKind::Conflict);
        assert!(graph.errors[0].detail.contains("cannot run beside"));
    }

    #[test]
    fn a_cycle_is_named_rather_than_looped_on() {
        let a = node("a", ModManifest::new("a", "A", "1.0").depends_on("b"));
        let b = node("b", ModManifest::new("b", "B", "1.0").depends_on("a"));
        let graph = DependencyGraph::build(vec![a, b]).resolve();
        assert_eq!(graph.cycles.len(), 1);
        assert_eq!(graph.cycles[0].cycle.len(), 2);
        assert!(graph.order.is_empty(), "neither can be ordered");
    }

    #[test]
    fn load_after_and_load_before_order_without_a_dependency() {
        let early = node(
            "early",
            ModManifest::new("early", "Early", "1.0").load_after("late"),
        );
        let late = node("late", ModManifest::new("late", "Late", "1.0"));
        let graph = DependencyGraph::build(vec![early, late]).resolve();
        assert_eq!(graph.order, vec!["late", "early"]);
        assert_eq!(graph.explanation, vec!["early after late".to_owned()]);
    }

    #[test]
    fn a_replacement_loads_after_what_it_replaces() {
        let old = node("old", ModManifest::new("old", "Old", "1.0"));
        let new = node(
            "new",
            ModManifest::new("new", "New", "1.0").replaces("old"),
        );
        let graph = DependencyGraph::build(vec![new, old]).resolve();
        assert_eq!(graph.order, vec!["old", "new"]);
    }

    #[test]
    fn a_dependency_line_survives_the_wire_format() {
        let dependency = ModDependency::new("core")
            .versioned(">=1.2")
            .kind(DependencyKind::Optional)
            .because("for the extra screen");
        assert_eq!(ModDependency::decode(&dependency.encode()), Some(dependency));
        let conflict = ModConflict::new("rival", true);
        assert_eq!(ModConflict::decode(&conflict.encode()), Some(conflict));
    }

    trait Tap: Sized {
        fn tap(self, f: impl FnOnce(&mut Self)) -> Self;
    }

    impl<T> Tap for T {
        fn tap(mut self, f: impl FnOnce(&mut Self)) -> Self {
            f(&mut self);
            self
        }
    }
}
