//! What this mod runs against, and whether that is what it asked for.
//!
//! A manifest can require versions — `requires_loader`, `requires_api`, `requires_game` —
//! and the loader checks them before the mod is loaded, so an unsatisfied requirement is a
//! sentence in the log instead of a strange crash later. The same facts are readable here,
//! so a mod can gate a feature on them at runtime.
//!
//! ```
//! use cinnaloader::prelude::*;
//!
//! if compat::is_game_version_supported(&VersionReq::parse(">=1.26").unwrap()) {
//!     // ...
//! }
//! ```

use crate::modinfo::{Capability, ModManifest};
use crate::version::{
    CompatibilityReport, CompatibilityRule, FeatureAvailability, FeatureFlag, Version, VersionReq,
};

/// The loader's own version.
#[must_use]
pub fn loader() -> Version {
    Version::parse(&crate::host_facts().loader_version)
}

/// The ABI version this loader provides, which a mod can compare with
/// [`ABI_VERSION`](crate::ABI_VERSION).
#[must_use]
pub fn api() -> u32 {
    crate::hosted_api_version()
}

/// The game version the loader found, empty when it could not tell.
#[must_use]
pub fn game() -> Version {
    Version::parse(&crate::host_facts().game_version)
}

/// The game build the client reports, when it reports one.
#[must_use]
pub fn game_build() -> Option<String> {
    let facts = crate::host_facts();
    (!facts.game_build.is_empty()).then_some(facts.game_build)
}

/// `bedrock`: this loader attaches to a Bedrock client.
#[must_use]
pub fn game_edition() -> String {
    let facts = crate::host_facts();
    if facts.game_edition.is_empty() {
        "bedrock".to_owned()
    } else {
        facts.game_edition
    }
}

/// The platform the client runs on, as `std::env::consts::OS` spells it.
#[must_use]
pub fn game_platform() -> String {
    let facts = crate::host_facts();
    if facts.game_platform.is_empty() {
        std::env::consts::OS.to_owned()
    } else {
        facts.game_platform
    }
}

/// The game's protocol version, when the loader knows it.
#[must_use]
pub fn game_protocol() -> Option<u32> {
    crate::host_facts().game_protocol
}

/// Whether the loaded game satisfies `requirement`.
///
/// An unknown game version counts as supported, exactly as the loader's own check does:
/// refusing over something the loader could not find out would blame the mod. A mod that
/// needs to know asks [`game()`] and tests [`is_numeric`](Version::is_numeric).
#[must_use]
pub fn is_game_version_supported(requirement: &VersionReq) -> bool {
    let version = game();
    if !version.is_numeric() {
        return true;
    }
    requirement.matches(&version)
}

/// Whether this loader satisfies `requirement`.
#[must_use]
pub fn is_loader_version_supported(requirement: &VersionReq) -> bool {
    requirement.matches(&loader())
}

/// Whether the loader's ABI is at least `required`.
#[must_use]
pub fn is_api_version_supported(required: u32) -> bool {
    api() >= required
}

/// Whether the client runs on `platform`.
#[must_use]
pub fn is_platform_supported(platform: &str) -> bool {
    platform.eq_ignore_ascii_case(&game_platform())
}

/// The runtime this loader provides: a native mod host inside a Bedrock client.
#[must_use]
pub fn runtime() -> &'static str {
    "cinnabar-client"
}

/// Whether `runtime` is the one this loader provides.
#[must_use]
pub fn is_runtime_supported(runtime: &str) -> bool {
    runtime.eq_ignore_ascii_case(self::runtime())
}

/// Whether the loader can provide a capability or a declared optional feature.
#[must_use]
pub fn is_feature_available(name: &str) -> bool {
    if let Ok(capability) = name.parse::<Capability>() {
        return loader_capabilities().contains(&capability);
    }
    crate::host_facts()
        .features
        .iter()
        .any(|feature| feature == name)
}

/// What this loader can provide, so a mod can ask before offering a feature. Taken from
/// the loader when one is present, so a mod is told what the loader actually has.
#[must_use]
pub fn loader_capabilities() -> Vec<Capability> {
    let facts = crate::host_facts();
    if facts.features.is_empty() {
        // No loader answered; report what this SDK's loader provides.
        return crate::capability::ALL
            .iter()
            .copied()
            .filter(|capability| capability.is_supported())
            .collect();
    }
    facts
        .features
        .iter()
        .filter_map(|name| name.parse().ok())
        .collect()
}

/// Every optional feature this mod declared it can live without, and whether the loader
/// has it.
#[must_use]
pub fn feature_flags() -> Vec<FeatureFlag> {
    crate::metadata::manifest()
        .optional_features
        .iter()
        .map(|name| FeatureFlag {
            name: name.clone(),
            available: is_feature_available(name),
        })
        .collect()
}

/// How a declared optional feature stands.
#[must_use]
pub fn availability(name: &str) -> FeatureAvailability {
    if is_feature_available(name) {
        FeatureAvailability::Available
    } else {
        FeatureAvailability::Unavailable
    }
}

/// Checks a manifest against the loader, the way the loader itself does. A mod can call
/// this to see its own verdict.
#[must_use]
pub fn report(manifest: &ModManifest) -> CompatibilityReport {
    check(manifest, &crate::host_facts())
}

/// The same check, over facts the caller supplies. The loader uses this too, so a mod and
/// the loader never disagree about a requirement.
#[must_use]
pub fn check(manifest: &ModManifest, facts: &crate::modinfo::HostFacts) -> CompatibilityReport {
    let mut report = CompatibilityReport {
        mod_name: manifest.id.clone(),
        rules: Vec::new(),
        missing: Vec::new(),
    };
    let mut rule = |kind: &str, requirement: &VersionReq, available: Version| {
        report.rules.push(CompatibilityRule {
            kind: kind.to_owned(),
            requirement: requirement.clone(),
            satisfied: requirement.matches(&available),
            available,
        });
    };
    let loader = Version::parse(&facts.loader_version);
    if !manifest.requires_loader.is_any() && loader.is_numeric() {
        rule("loader", &manifest.requires_loader, loader);
    }
    if manifest.requires_api > 0 {
        report.rules.push(CompatibilityRule {
            kind: "api".to_owned(),
            requirement: VersionReq::parse(&format!(">={}", manifest.requires_api))
                .unwrap_or_default(),
            available: Version::parse(&facts.api_version.to_string()),
            satisfied: facts.api_version >= manifest.requires_api,
        });
    }
    if !manifest.requires_game.is_any() {
        let available = Version::parse(&facts.game_version);
        // An unknown game version cannot be checked, so it is not treated as a failure:
        // refusing over something the loader could not find out would be wrong.
        let satisfied = !available.is_numeric() || manifest.requires_game.matches(&available);
        report.rules.push(CompatibilityRule {
            kind: "game".to_owned(),
            requirement: manifest.requires_game.clone(),
            available,
            satisfied,
        });
    }
    if let Some(platform) = &manifest.requires_platform {
        if !platform.eq_ignore_ascii_case(&facts.game_platform) {
            report.missing.push(format!(
                "platform {platform} (running on {})",
                facts.game_platform
            ));
        }
    }
    let available: Vec<Capability> = facts
        .features
        .iter()
        .filter_map(|name| name.parse().ok())
        .collect();
    for feature in &manifest.required_features {
        let present = feature
            .parse::<Capability>()
            .map_or_else(|_| facts.features.contains(feature), |c| available.contains(&c));
        if !present {
            report.missing.push(feature.clone());
        }
    }
    report
}

/// Fails unless the game satisfies `requirement`.
pub fn require_game_version(requirement: &VersionReq) -> Result<(), String> {
    if is_game_version_supported(requirement) {
        return Ok(());
    }
    Err(format!(
        "needs game {requirement}, running {}",
        display_version(&game())
    ))
}

/// Fails unless the loader satisfies `requirement`.
pub fn require_loader_version(requirement: &VersionReq) -> Result<(), String> {
    if is_loader_version_supported(requirement) {
        return Ok(());
    }
    Err(format!(
        "needs loader {requirement}, have {}",
        loader()
    ))
}

/// Fails unless the ABI is at least `required`.
pub fn require_api_version(required: u32) -> Result<(), String> {
    if is_api_version_supported(required) {
        return Ok(());
    }
    Err(format!("needs ABI {required}, have {}", api()))
}

/// Fails unless the loader provides `capability`.
pub fn require_capability(capability: Capability) -> Result<(), String> {
    if is_feature_available(capability.to_string().as_str()) {
        return Ok(());
    }
    Err(format!("{capability} is not available"))
}

/// Fails unless the mod runs on `platform`.
pub fn require_platform(platform: &str) -> Result<(), String> {
    if is_platform_supported(platform) {
        return Ok(());
    }
    Err(format!(
        "needs platform {platform}, running {}",
        game_platform()
    ))
}

fn display_version(version: &Version) -> String {
    if version.is_numeric() {
        version.to_string()
    } else {
        "an unknown version".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_loader_answers_about_itself_without_a_host() {
        assert_eq!(runtime(), "cinnabar-client");
        assert!(is_runtime_supported("Cinnabar-Client"));
        assert!(!is_runtime_supported("dedicated-server"));
        assert!(is_platform_supported(std::env::consts::OS));
        assert!(!is_platform_supported("nintendo-switch"));
        assert_eq!(game_edition(), "bedrock");
    }

    #[test]
    fn capabilities_are_reported_as_features() {
        assert!(is_feature_available("files"));
        assert!(is_feature_available("filesystem_read"));
        assert!(!is_feature_available("player_read"));
        assert!(!is_feature_available("world_write"));
        assert!(!is_feature_available("oscilloscope"));
        assert_eq!(availability("files"), FeatureAvailability::Available);
        assert_eq!(
            availability("oscilloscope"),
            FeatureAvailability::Unavailable
        );
    }

    #[test]
    fn a_requirement_against_an_unknown_game_version_says_so_rather_than_refusing() {
        let manifest = ModManifest::new("probe", "Probe", "1.0").requires_game(">=1.27");
        let report = report(&manifest);
        // No host in a unit test, so the game version is unknown: the rule is recorded and
        // not treated as a failure.
        assert!(report.is_compatible());
        assert_eq!(report.rules.len(), 1);
    }

    #[test]
    fn a_missing_capability_is_a_failure() {
        let manifest = ModManifest::new("probe", "Probe", "1.0").requires_feature("world_write");
        let report = report(&manifest);
        assert!(!report.is_compatible());
        assert_eq!(report.failures(), vec!["world_write is not supported".to_owned()]);
    }

    #[test]
    fn the_require_helpers_return_the_reason() {
        assert!(require_api_version(1).is_ok());
        let error = require_platform("nintendo-switch").unwrap_err();
        assert!(error.contains("nintendo-switch"), "{error}");
        let error = require_capability(Capability::PacketModify).unwrap_err();
        assert!(error.contains("packet_modify"), "{error}");
        // With no loader answering, the version is unknown and cannot satisfy `>=`.
        let error = require_loader_version(&VersionReq::parse(">=0.0.1").unwrap()).unwrap_err();
        assert!(error.contains("needs loader"), "{error}");
        assert!(require_loader_version(&VersionReq::parse("*").unwrap()).is_ok());
        assert!(
            is_game_version_supported(&VersionReq::parse(">=99").unwrap()),
            "an unknown game version is not held against the mod"
        );
    }
}
