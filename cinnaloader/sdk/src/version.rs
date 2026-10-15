//! Version requirements and compatibility rules.
//!
//! A mod says what it needs — a loader version, an ABI version, a game version — and the
//! loader checks it before the mod is loaded, so a mismatch is a sentence in the log
//! rather than a crash later.
//!
//! ```
//! use cinnaloader::prelude::*;
//!
//! let requirement = VersionReq::parse(">=1.26, <1.27").unwrap();
//! assert!(requirement.matches(&Version::parse("1.26.50")));
//! assert!(!requirement.matches(&Version::parse("1.27.0")));
//! ```

use std::fmt;

pub use crate::modinfo::ModVersion as Version;

/// One comparison in a requirement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionConstraint {
    /// `=1.2.3`, or a bare `1.2.3`.
    Exact,
    Greater,
    GreaterOrEqual,
    Less,
    LessOrEqual,
    /// `^1.2.3`: at least this, below the next breaking version.
    Caret,
    /// `~1.2.3`: at least this, below the next minor version.
    Tilde,
    /// `1.2.*`: any patch of that minor.
    Wildcard,
}

impl VersionConstraint {
    /// Whether `version` satisfies this constraint against `bound`.
    #[must_use]
    pub fn satisfied_by(self, version: &Version, bound: &Version) -> bool {
        match self {
            Self::Exact => version == bound,
            Self::Greater => version > bound,
            Self::GreaterOrEqual => version >= bound,
            Self::Less => version < bound,
            Self::LessOrEqual => version <= bound,
            Self::Caret => version >= bound && version < &caret_ceiling(bound),
            Self::Tilde => version >= bound && version < &tilde_ceiling(bound),
            Self::Wildcard => version >= bound && version < &wildcard_ceiling(bound),
        }
    }
}

/// The version a caret requirement stays below: `^1.2.3` keeps `1.x`, `^0.2.3` keeps `0.2.x`.
fn caret_ceiling(bound: &Version) -> Version {
    match bound.major() {
        Some(major) if major > 0 => Version::parse(&format!("{}", major + 1)),
        Some(_) => match bound.minor() {
            Some(minor) if minor > 0 => Version::parse(&format!("0.{}", minor + 1)),
            _ => Version::parse(&format!("0.0.{}", bound.patch().unwrap_or(0) + 1)),
        },
        None => bound.clone(),
    }
}

/// The version a tilde requirement stays below: `~1.2.3` keeps `1.2.x`.
fn tilde_ceiling(bound: &Version) -> Version {
    match (bound.major(), bound.minor()) {
        (Some(major), Some(minor)) => Version::parse(&format!("{major}.{}", minor + 1)),
        (Some(major), None) => Version::parse(&format!("{}", major + 1)),
        _ => bound.clone(),
    }
}

/// The version a wildcard stays below: `1.2.*` keeps `1.2.x`.
fn wildcard_ceiling(bound: &Version) -> Version {
    match (bound.major(), bound.minor()) {
        (Some(major), Some(minor)) => Version::parse(&format!("{major}.{}", minor + 1)),
        (Some(major), None) => Version::parse(&format!("{}", major + 1)),
        _ => bound.clone(),
    }
}

/// One predicate: a constraint and the version it compares against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionPredicate {
    pub constraint: VersionConstraint,
    pub bound: Version,
}

impl VersionPredicate {
    #[must_use]
    pub fn matches(&self, version: &Version) -> bool {
        self.constraint.satisfied_by(version, &self.bound)
    }
}

impl fmt::Display for VersionPredicate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let operator = match self.constraint {
            VersionConstraint::Exact => "=",
            VersionConstraint::Greater => ">",
            VersionConstraint::GreaterOrEqual => ">=",
            VersionConstraint::Less => "<",
            VersionConstraint::LessOrEqual => "<=",
            VersionConstraint::Caret => "^",
            VersionConstraint::Tilde => "~",
            VersionConstraint::Wildcard => "",
        };
        write!(formatter, "{operator}{}", self.bound)
    }
}

/// A set of predicates. Commas mean "all of", a `|` means "either side".
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VersionReq {
    /// Each inner vector is one alternative; all of its predicates must hold.
    alternatives: Vec<Vec<VersionPredicate>>,
    text: String,
}

/// Why a requirement could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionReqError {
    pub text: String,
}

impl fmt::Display for VersionReqError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "`{}` is not a version requirement", self.text)
    }
}

impl std::error::Error for VersionReqError {}

impl VersionReq {
    /// Parses `>=1.2, <1.27` or `1.2.*` or `^1.2.3` or `*`. Spaces are ignored.
    pub fn parse(text: &str) -> Result<Self, VersionReqError> {
        let trimmed = text.trim();
        if trimmed.is_empty() || trimmed == "*" {
            return Ok(Self {
                alternatives: Vec::new(),
                text: text.to_owned(),
            });
        }
        let mut alternatives = Vec::new();
        for alternative in trimmed.split('|') {
            let mut predicates = Vec::new();
            for part in alternative.split(',') {
                let part = part.trim();
                if part.is_empty() {
                    continue;
                }
                predicates.push(parse_predicate(part).ok_or_else(|| VersionReqError {
                    text: text.to_owned(),
                })?);
            }
            if predicates.is_empty() {
                return Err(VersionReqError {
                    text: text.to_owned(),
                });
            }
            alternatives.push(predicates);
        }
        Ok(Self {
            alternatives,
            text: text.to_owned(),
        })
    }

    /// Whether `version` satisfies the requirement. An empty requirement accepts anything.
    #[must_use]
    pub fn matches(&self, version: &Version) -> bool {
        if self.alternatives.is_empty() {
            return true;
        }
        self.alternatives
            .iter()
            .any(|predicates| predicates.iter().all(|predicate| predicate.matches(version)))
    }

    /// Every predicate of every alternative, for a report.
    pub fn predicates(&self) -> impl Iterator<Item = &VersionPredicate> {
        self.alternatives.iter().flatten()
    }

    /// Whether nothing at all is required.
    #[must_use]
    pub fn is_any(&self) -> bool {
        self.alternatives.is_empty()
    }
}

impl fmt::Display for VersionReq {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.alternatives.is_empty() {
            return formatter.write_str("*");
        }
        let alternatives: Vec<String> = self
            .alternatives
            .iter()
            .map(|predicates| {
                predicates
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .collect();
        formatter.write_str(&alternatives.join(" | "))
    }
}

fn parse_predicate(text: &str) -> Option<VersionPredicate> {
    let (constraint, rest) = if let Some(rest) = text.strip_prefix(">=") {
        (VersionConstraint::GreaterOrEqual, rest)
    } else if let Some(rest) = text.strip_prefix("<=") {
        (VersionConstraint::LessOrEqual, rest)
    } else if let Some(rest) = text.strip_prefix('>') {
        (VersionConstraint::Greater, rest)
    } else if let Some(rest) = text.strip_prefix('<') {
        (VersionConstraint::Less, rest)
    } else if let Some(rest) = text.strip_prefix('^') {
        (VersionConstraint::Caret, rest)
    } else if let Some(rest) = text.strip_prefix('~') {
        (VersionConstraint::Tilde, rest)
    } else if let Some(rest) = text.strip_prefix('=') {
        (VersionConstraint::Exact, rest)
    } else {
        (VersionConstraint::Exact, text)
    };
    let rest = rest.trim();
    if rest.is_empty() {
        return None;
    }
    if rest.contains('*') || rest.contains('x') || rest.contains('X') {
        let bound = Version::parse(&rest.replace(['*', 'x', 'X'], "0"));
        if !bound.is_numeric() {
            return None;
        }
        return Some(VersionPredicate {
            constraint: VersionConstraint::Wildcard,
            bound,
        });
    }
    let bound = Version::parse(rest);
    if !bound.is_numeric() {
        return None;
    }
    Some(VersionPredicate { constraint, bound })
}

/// A closed range, for a mod that thinks in minimums and maximums.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VersionRange {
    pub min: Option<Version>,
    pub max: Option<Version>,
}

impl VersionRange {
    #[must_use]
    pub fn new(min: Option<Version>, max: Option<Version>) -> Self {
        Self { min, max }
    }

    #[must_use]
    pub fn matches(&self, version: &Version) -> bool {
        self.min.as_ref().is_none_or(|min| version >= min)
            && self.max.as_ref().is_none_or(|max| version <= max)
    }

    /// The range as a requirement, so one checker serves both.
    #[must_use]
    pub fn to_requirement(&self) -> VersionReq {
        let mut parts = Vec::new();
        if let Some(min) = &self.min {
            parts.push(format!(">={min}"));
        }
        if let Some(max) = &self.max {
            parts.push(format!("<={max}"));
        }
        VersionReq::parse(&parts.join(", ")).unwrap_or_default()
    }
}

/// One thing a mod needs, and whether the loader can satisfy it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompatibilityRule {
    /// What is being required, such as `game` or `loader`.
    pub kind: String,
    /// The requirement as written.
    pub requirement: VersionReq,
    /// What the loader actually has.
    pub available: Version,
    /// Whether the requirement is met.
    pub satisfied: bool,
}

impl fmt::Display for CompatibilityRule {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} {} (have {}){}",
            self.kind,
            self.requirement,
            self.available,
            if self.satisfied { "" } else { " — not met" }
        )
    }
}

/// Everything the loader checked about one mod.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompatibilityReport {
    pub mod_name: String,
    pub rules: Vec<CompatibilityRule>,
    /// Requirements naming something the loader does not provide at all.
    pub missing: Vec<String>,
}

impl CompatibilityReport {
    #[must_use]
    pub fn is_compatible(&self) -> bool {
        self.missing.is_empty() && self.rules.iter().all(|rule| rule.satisfied)
    }

    /// The rules that failed, for a refusal message.
    #[must_use]
    pub fn failures(&self) -> Vec<String> {
        let mut failures: Vec<String> = self
            .rules
            .iter()
            .filter(|rule| !rule.satisfied)
            .map(ToString::to_string)
            .collect();
        failures.extend(self.missing.iter().map(|missing| format!("{missing} is not supported")));
        failures
    }
}

impl fmt::Display for CompatibilityReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_compatible() {
            return write!(formatter, "{} is compatible", self.mod_name);
        }
        write!(formatter, "{}: {}", self.mod_name, self.failures().join("; "))
    }
}

/// A loader or game version the mod accepts, kept for a report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionMapping {
    /// The version a mod was written against.
    pub from: Version,
    /// The version whose behavior it is treated as.
    pub to: Version,
}

/// How stable an API is, so a mod can decide whether to depend on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ApiStability {
    /// May change in any release.
    Experimental,
    /// May change between minor versions.
    #[default]
    Unstable,
    /// Only changes with a major version.
    Stable,
    /// Kept for compatibility, and going away.
    Deprecated,
}

/// Something that went away, and what to use instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeprecationInfo {
    pub since: Version,
    pub what: String,
    pub instead: String,
}

/// An optional capability a mod may ask about before using it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureFlag {
    pub name: String,
    pub available: bool,
}

/// What a feature flag says about a feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeatureAvailability {
    Available,
    Unavailable,
    /// Available, but only on some platforms or game versions.
    Conditional,
}

impl FeatureFlag {
    #[must_use]
    pub fn availability(&self) -> FeatureAvailability {
        if self.available {
            FeatureAvailability::Available
        } else {
            FeatureAvailability::Unavailable
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(text: &str) -> Version {
        Version::parse(text)
    }

    #[test]
    fn comparisons_accept_the_spellings_a_mod_author_writes() {
        assert!(VersionReq::parse(">=1.2.3").unwrap().matches(&version("1.2.3")));
        assert!(VersionReq::parse(">=1.2.3").unwrap().matches(&version("2.0")));
        assert!(!VersionReq::parse(">1.2.3").unwrap().matches(&version("1.2.3")));
        assert!(VersionReq::parse("<=2").unwrap().matches(&version("2.0.0")));
        assert!(VersionReq::parse("<2").unwrap().matches(&version("1.9")));
        assert!(VersionReq::parse("=1.2").unwrap().matches(&version("1.2.0")));
        assert!(VersionReq::parse("1.2.3").unwrap().matches(&version("1.2.3")));
        assert!(VersionReq::parse("*").unwrap().matches(&version("9.9.9")));
        assert!(VersionReq::parse("").unwrap().matches(&version("0.1")));
    }

    #[test]
    fn carets_tilde_and_wildcards_keep_the_range_authors_expect() {
        let caret = VersionReq::parse("^1.2.3").unwrap();
        assert!(caret.matches(&version("1.9.9")));
        assert!(!caret.matches(&version("2.0.0")));
        let zero_caret = VersionReq::parse("^0.2.3").unwrap();
        assert!(zero_caret.matches(&version("0.2.9")));
        assert!(!zero_caret.matches(&version("0.3.0")));

        let tilde = VersionReq::parse("~1.2.3").unwrap();
        assert!(tilde.matches(&version("1.2.9")));
        assert!(!tilde.matches(&version("1.3.0")));

        let wildcard = VersionReq::parse("1.2.*").unwrap();
        assert!(wildcard.matches(&version("1.2.7")));
        assert!(!wildcard.matches(&version("1.3.0")));
    }

    #[test]
    fn commas_are_and_and_a_pipe_is_or() {
        let both = VersionReq::parse(">=1.26, <1.27").unwrap();
        assert!(both.matches(&version("1.26.50")));
        assert!(!both.matches(&version("1.27.0")));
        // A bare version means exactly that version, so a range needs a wildcard.
        let either = VersionReq::parse("1.26 | 1.27.*").unwrap();
        assert!(either.matches(&version("1.26.0")));
        assert!(either.matches(&version("1.27.3")));
        assert!(!either.matches(&version("1.28.0")));
    }

    #[test]
    fn nonsense_is_refused_rather_than_guessed() {
        assert!(VersionReq::parse(">=").is_err());
        assert!(VersionReq::parse("banana").is_err());
        assert!(VersionReq::parse(">a.b").is_err());
    }

    #[test]
    fn a_range_and_a_requirement_agree() {
        let range = VersionRange::new(Some(version("1.26")), Some(version("1.27")));
        let requirement = range.to_requirement();
        for text in ["1.25", "1.26", "1.26.50", "1.27", "1.28"] {
            assert_eq!(
                range.matches(&version(text)),
                requirement.matches(&version(text)),
                "{text}"
            );
        }
    }

    #[test]
    fn a_report_says_what_failed() {
        let report = CompatibilityReport {
            mod_name: "sprint".to_owned(),
            rules: vec![
                CompatibilityRule {
                    kind: "game".to_owned(),
                    requirement: VersionReq::parse(">=1.27").unwrap(),
                    available: version("1.26.50"),
                    satisfied: false,
                },
                CompatibilityRule {
                    kind: "loader".to_owned(),
                    requirement: VersionReq::parse(">=0.1").unwrap(),
                    available: version("0.1.0"),
                    satisfied: true,
                },
            ],
            missing: vec!["oscilloscope".to_owned()],
        };
        assert!(!report.is_compatible());
        let failures = report.failures();
        assert_eq!(failures.len(), 2);
        assert!(failures[0].starts_with("game >=1.27"));
        assert!(failures[1].contains("oscilloscope"));
        assert!(report.to_string().contains("not met"));
    }
}
