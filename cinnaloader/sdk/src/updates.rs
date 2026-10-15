//! Update checks and release channels: what a newer release would be, what to do about it,
//! and the plan for fetching one — with the network and the client's own updater refused.
//!
//! A [`Channel`] picks candidate releases by a stated rule ([`Channel::rule`]): `Stable`
//! takes versions with no prerelease, `Beta` takes any release, and `Pinned` keeps exactly
//! the version a caller is pinned to. [`compare`] answers the step between two versions as
//! an [`UpdateKind`], and a pinned caller is never told to move: [`Channel::select`] hands
//! back the pin itself, so there is nothing newer to compare against.
//!
//! A [`Policy`] turns that into a [`Decision`]: `Auto` fetches, `Notify` prompts, `Manual`
//! waits to be asked and `Pinned` never moves. A check runs at most once per interval
//! ([`Throttle`]); a [`check`] before the interval answers [`Error::Throttled`].
//!
//! A [`Manifest`] is a version, a URL, a size and a hash a caller supplies; [`plan_update`]
//! builds the [`Plan`] for it and [`verify_update`] hashes fetched bytes with
//! [`crate::serialization`], answering [`Verification::Verified`] or naming both hashes. A
//! [`Changelog`] answers the [`Entry`]s between two versions.
//!
//! This host does not fetch: [`download_update`] is refused, and so is the client's own
//! updater, each with a sentence from [`why_unknown`] and listed by [`refusals`].
//!
//! ```
//! use cinnaloader::updates::{self, Channel, Policy};
//! use cinnaloader::version::Version;
//!
//! let current = Version::parse("1.2.0");
//! let latest = Version::parse("1.3.0");
//! assert_eq!(updates::compare(&current, &latest).as_str(), "minor");
//! assert!(!updates::compare(&current, &current).is_update());
//!
//! let stable = Channel::Stable;
//! assert!(stable.accepts(&Version::parse("1.3.0")));
//! assert!(!stable.accepts(&Version::parse("1.3.0-beta.1")));
//!
//! let decision = updates::decide(Policy::Notify, &current, &latest);
//! assert!(decision.prompt && !decision.fetch);
//! ```

use std::cmp::Ordering;
use std::fmt;
use std::sync::{Mutex, OnceLock};

pub use crate::version::Version;

/// How long a check waits before it will run again, in ticks.
pub const DEFAULT_INTERVAL: u64 = 3600;

fn has_prerelease(version: &Version) -> bool {
    version.to_string().contains('-')
}

/// The step between two versions, or that there is nothing to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateKind {
    UpToDate,
    Patch,
    Minor,
    Major,
}

impl UpdateKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UpToDate => "up_to_date",
            Self::Patch => "patch",
            Self::Minor => "minor",
            Self::Major => "major",
        }
    }

    #[must_use]
    pub fn is_update(self) -> bool {
        self != Self::UpToDate
    }
}

/// The step from `current` to `latest`, exactly, by the first component that differs.
#[must_use]
pub fn compare(current: &Version, latest: &Version) -> UpdateKind {
    if !is_newer(current, latest) {
        return UpdateKind::UpToDate;
    }
    if latest.major() != current.major() {
        UpdateKind::Major
    } else if latest.minor() != current.minor() {
        UpdateKind::Minor
    } else {
        UpdateKind::Patch
    }
}

/// Whether `latest` is a numeric version strictly above `current`.
#[must_use]
pub fn is_newer(current: &Version, latest: &Version) -> bool {
    latest.is_numeric() && latest.cmp_parts(current) == Ordering::Greater
}

/// Whether a release above `current` exists, the wishlist's spelling.
#[must_use]
pub fn is_update_available(current: &Version, latest: &Version) -> bool {
    is_newer(current, latest)
}

/// A release channel: which versions a caller is willing to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Channel {
    Stable,
    Beta,
    Pinned(Version),
}

impl Channel {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Beta => "beta",
            Self::Pinned(_) => "pinned",
        }
    }

    /// The rule the channel selects versions by, for a caller to read back.
    #[must_use]
    pub fn rule(&self) -> String {
        match self {
            Self::Stable => "releases with no prerelease".to_owned(),
            Self::Beta => "any release, prerelease included".to_owned(),
            Self::Pinned(version) => format!("exactly the pinned version {version}"),
        }
    }

    #[must_use]
    pub fn accepts(&self, version: &Version) -> bool {
        match self {
            Self::Stable => version.is_numeric() && !has_prerelease(version),
            Self::Beta => version.is_numeric(),
            Self::Pinned(want) => version == want,
        }
    }

    /// Every release the channel accepts, in the order they were given.
    #[must_use]
    pub fn candidates<'a>(&self, releases: &'a [Release]) -> Vec<&'a Release> {
        releases
            .iter()
            .filter(|release| self.accepts(&release.manifest.version))
            .collect()
    }

    /// The highest accepted release, or `None` when the channel accepts nothing.
    #[must_use]
    pub fn select<'a>(&self, releases: &'a [Release]) -> Option<&'a Release> {
        self.candidates(releases)
            .into_iter()
            .max_by(|left, right| left.manifest.version.cmp_parts(&right.manifest.version))
    }
}

/// What a caller wants done about an available update.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Policy {
    /// Fetch and stage without asking.
    Auto,
    /// Tell the caller an update exists and wait for it to act.
    #[default]
    Notify,
    /// Do nothing until asked.
    Manual,
    /// Never move off the running version.
    Pinned,
}

impl Policy {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Notify => "notify",
            Self::Manual => "manual",
            Self::Pinned => "pinned",
        }
    }
}

/// What a policy decided about one update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decision {
    pub kind: UpdateKind,
    /// Whether the caller should be told and asked to act.
    pub prompt: bool,
    /// Whether the caller should fetch the release.
    pub fetch: bool,
}

impl Decision {
    #[must_use]
    pub fn of(kind: UpdateKind) -> Self {
        Self {
            kind,
            prompt: false,
            fetch: false,
        }
    }
}

/// The decision a policy makes between two versions.
#[must_use]
pub fn decide(policy: Policy, current: &Version, latest: &Version) -> Decision {
    if policy == Policy::Pinned {
        return Decision::of(UpdateKind::UpToDate);
    }
    let kind = compare(current, latest);
    if !kind.is_update() {
        return Decision::of(kind);
    }
    match policy {
        Policy::Auto => Decision {
            kind,
            prompt: false,
            fetch: true,
        },
        Policy::Notify => Decision {
            kind,
            prompt: true,
            fetch: false,
        },
        Policy::Manual => Decision::of(kind),
        Policy::Pinned => Decision::of(UpdateKind::UpToDate),
    }
}

/// A gate that lets a check run at most once per interval, on a caller's own clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Throttle {
    interval: u64,
    last: Option<u64>,
}

impl Throttle {
    #[must_use]
    pub fn new(interval: u64) -> Self {
        Self {
            interval,
            last: None,
        }
    }

    #[must_use]
    pub fn interval(&self) -> u64 {
        self.interval
    }

    pub fn set_interval(&mut self, interval: u64) {
        self.interval = interval;
        self.last = None;
    }

    pub fn reset(&mut self) {
        self.last = None;
    }

    /// Whether a check at `now` may run. A refusal does not move the last run.
    #[must_use]
    pub fn allows(&mut self, now: u64) -> bool {
        match self.last {
            Some(last) if now.saturating_sub(last) < self.interval => false,
            _ => {
                self.last = Some(now);
                true
            }
        }
    }
}

/// A release a provider offers: how to fetch it, how big it is and its hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub version: Version,
    pub url: String,
    pub size: u64,
    pub hash: String,
}

impl Manifest {
    #[must_use]
    pub fn new(version: Version, url: impl Into<String>, size: u64, hash: impl Into<String>) -> Self {
        Self {
            version,
            url: url.into(),
            size,
            hash: hash.into(),
        }
    }

    /// The file name this manifest's URL names, or a version-based default.
    #[must_use]
    pub fn file_name(&self) -> String {
        self.url
            .rsplit('/')
            .next()
            .filter(|name| !name.is_empty())
            .map_or_else(|| format!("mod-{}.jar", self.version), ToString::to_string)
    }
}

/// A manifest and the changelog entries that came with that version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub manifest: Manifest,
    pub notes: Vec<String>,
}

impl Release {
    #[must_use]
    pub fn new(manifest: Manifest) -> Self {
        Self {
            manifest,
            notes: Vec::new(),
        }
    }

    #[must_use]
    pub fn note(mut self, text: impl Into<String>) -> Self {
        self.notes.push(text.into());
        self
    }

    #[must_use]
    pub fn version(&self) -> &Version {
        &self.manifest.version
    }
}

/// One update source: a name, the version running now, and the releases it offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provider {
    pub name: String,
    pub current: Version,
    pub releases: Vec<Release>,
}

impl Provider {
    #[must_use]
    pub fn new(name: impl Into<String>, current: Version) -> Self {
        Self {
            name: name.into(),
            current,
            releases: Vec::new(),
        }
    }

    #[must_use]
    pub fn release(mut self, release: Release) -> Self {
        self.releases.push(release);
        self
    }
}

/// Where the update lifecycle stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    #[default]
    Idle,
    Available,
    Staged,
    Applied,
    Cancelled,
}

impl Status {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Available => "available",
            Self::Staged => "staged",
            Self::Applied => "applied",
            Self::Cancelled => "cancelled",
        }
    }
}

/// One version the lifecycle reached, for [`get_history`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    pub version: Version,
    pub status: Status,
}

/// The plan for one update: what to fetch, where to put it and what it should hash to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub version: Version,
    pub url: String,
    pub size: u64,
    pub hash: String,
    pub destination: String,
    pub kind: UpdateKind,
}

/// Builds the plan for a release against the running version, into `dir`.
#[must_use]
pub fn plan_update(current: &Version, release: &Release, dir: &str) -> Plan {
    let manifest = &release.manifest;
    Plan {
        version: manifest.version.clone(),
        url: manifest.url.clone(),
        size: manifest.size,
        hash: manifest.hash.clone(),
        destination: format!("{}/{}", dir.trim_end_matches('/'), manifest.file_name()),
        kind: compare(current, &manifest.version),
    }
}

/// What a fetched artifact's hash said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verification {
    Verified,
    Mismatch { expected: String, found: String },
}

impl Verification {
    #[must_use]
    pub fn is_verified(&self) -> bool {
        matches!(self, Self::Verified)
    }
}

/// The hash this host compares a fetched artifact by, through [`crate::serialization`].
#[must_use]
pub fn hash_of(bytes: &[u8]) -> String {
    crate::serialization::encode_hex(&crate::serialization::hash(bytes).to_le_bytes())
}

/// Checks fetched bytes against an expected hash, naming both on a mismatch.
#[must_use]
pub fn verify_update(bytes: &[u8], expected: &str) -> Verification {
    let found = hash_of(bytes);
    if found.eq_ignore_ascii_case(expected) {
        Verification::Verified
    } else {
        Verification::Mismatch {
            expected: expected.to_owned(),
            found,
        }
    }
}

/// One changed line, with the version it arrived in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub version: Version,
    pub text: String,
}

/// A changelog: releases, and the entries between two versions.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changelog {
    releases: Vec<Release>,
}

impl Changelog {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a release, replacing any earlier one of the same version.
    pub fn add(&mut self, release: Release) {
        self.releases
            .retain(|held| held.manifest.version != release.manifest.version);
        self.releases.push(release);
    }

    #[must_use]
    pub fn versions(&self) -> Vec<Version> {
        self.releases
            .iter()
            .map(|release| release.manifest.version.clone())
            .collect()
    }

    /// The entries of one version.
    #[must_use]
    pub fn notes(&self, version: &Version) -> Vec<String> {
        self.releases
            .iter()
            .find(|release| &release.manifest.version == version)
            .map_or_else(Vec::new, |release| release.notes.clone())
    }

    /// Every entry above `from`, so "what changed since X".
    #[must_use]
    pub fn since(&self, from: &Version) -> Vec<Entry> {
        let mut out = Vec::new();
        for release in &self.releases {
            if release.manifest.version.cmp_parts(from) == Ordering::Greater {
                for text in &release.notes {
                    out.push(Entry {
                        version: release.manifest.version.clone(),
                        text: text.clone(),
                    });
                }
            }
        }
        out
    }

    /// Every entry in `(from, to]`, the entries between two versions.
    #[must_use]
    pub fn between(&self, from: &Version, to: &Version) -> Vec<Entry> {
        let mut out = Vec::new();
        for release in &self.releases {
            let version = &release.manifest.version;
            if version.cmp_parts(from) == Ordering::Greater
                && version.cmp_parts(to) != Ordering::Greater
            {
                for text in &release.notes {
                    out.push(Entry {
                        version: version.clone(),
                        text: text.clone(),
                    });
                }
            }
        }
        out
    }
}

/// The running version against the one a channel would pick for a provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compatibility {
    UpToDate,
    Patch,
    Minor,
    Major,
    Unknown,
}

impl Compatibility {
    #[must_use]
    pub fn of(kind: UpdateKind) -> Self {
        match kind {
            UpdateKind::UpToDate => Self::UpToDate,
            UpdateKind::Patch => Self::Patch,
            UpdateKind::Minor => Self::Minor,
            UpdateKind::Major => Self::Major,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UpToDate => "up_to_date",
            Self::Patch => "patch",
            Self::Minor => "minor",
            Self::Major => "major",
            Self::Unknown => "unknown",
        }
    }
}

/// One provider's answer in a check report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateInfo {
    pub name: String,
    pub current: Version,
    pub latest: Version,
    pub kind: UpdateKind,
    pub decision: Decision,
}

/// Every provider's answer for one check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckReport {
    pub channel: Channel,
    pub policy: Policy,
    pub updates: Vec<UpdateInfo>,
}

impl CheckReport {
    #[must_use]
    pub fn has_update(&self) -> bool {
        self.updates.iter().any(|info| info.kind.is_update())
    }

    #[must_use]
    pub fn newest(&self) -> Option<&UpdateInfo> {
        self.updates
            .iter()
            .max_by(|left, right| left.latest.cmp_parts(&right.latest))
    }
}

/// Why an update question has no answer here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// A check ran inside the interval.
    Throttled { interval: u64 },
    /// No provider is registered under that name.
    NoProvider(String),
    /// Nothing is staged to apply.
    NothingStaged,
    /// The channel accepts no release for this provider.
    NoUpdate,
    /// The network or the client's own updater, with [`why_unknown`] as the reason.
    Refused(String),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Throttled { interval } => {
                write!(formatter, "an update check runs at most once every {interval} ticks")
            }
            Self::NoProvider(name) => write!(formatter, "no update provider named {name}"),
            Self::NothingStaged => formatter.write_str("no update is staged to apply"),
            Self::NoUpdate => formatter.write_str("the channel selects no release"),
            Self::Refused(why) => formatter.write_str(why),
        }
    }
}

impl std::error::Error for Error {}

struct Registry {
    providers: Vec<Provider>,
    channel: Channel,
    policy: Policy,
    throttle: Throttle,
    now: u64,
    status: Status,
    progress: f32,
    report: Option<CheckReport>,
    staged: Option<Plan>,
    applied: Option<Plan>,
    history: Vec<HistoryEntry>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            providers: Vec::new(),
            channel: Channel::Stable,
            policy: Policy::Notify,
            throttle: Throttle::new(DEFAULT_INTERVAL),
            now: 0,
            status: Status::Idle,
            progress: 0.0,
            report: None,
            staged: None,
            applied: None,
            history: Vec::new(),
        }
    }
}

fn held() -> &'static Mutex<Registry> {
    static HELD: OnceLock<Mutex<Registry>> = OnceLock::new();
    HELD.get_or_init(|| Mutex::new(Registry::default()))
}

fn with<T>(body: impl FnOnce(&mut Registry) -> T) -> T {
    let mut guard = held().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Registers a provider, refusing a blank or taken name.
pub fn register_provider(provider: Provider) -> bool {
    with(|registry| {
        if provider.name.trim().is_empty() {
            crate::warn!("an update provider needs a name");
            return false;
        }
        if registry
            .providers
            .iter()
            .any(|held| held.name == provider.name)
        {
            crate::warn!("an update provider named {} is already registered", provider.name);
            return false;
        }
        registry.providers.push(provider);
        true
    })
}

/// Removes a provider by name, and answers whether it was there.
pub fn unregister_provider(name: &str) -> bool {
    with(|registry| {
        let before = registry.providers.len();
        registry.providers.retain(|provider| provider.name != name);
        registry.providers.len() != before
    })
}

/// Every registered provider, in registration order.
#[must_use]
pub fn get_providers() -> Vec<Provider> {
    with(|registry| registry.providers.clone())
}

/// The channels this host offers; a pin is a caller's own version.
#[must_use]
pub fn get_available_channels() -> Vec<Channel> {
    vec![Channel::Stable, Channel::Beta]
}

/// The channel checks select by.
#[must_use]
pub fn get_current_channel() -> Channel {
    with(|registry| registry.channel.clone())
}

/// Sets the channel, refusing a pin that is not a numeric version.
pub fn set_channel(channel: Channel) -> bool {
    if let Channel::Pinned(version) = &channel {
        if !version.is_numeric() {
            crate::warn!("a pinned channel needs a numeric version, not {version}");
            return false;
        }
    }
    with(|registry| registry.channel = channel);
    true
}

/// The update policy in force.
#[must_use]
pub fn get_policy() -> Policy {
    with(|registry| registry.policy)
}

/// Sets the update policy.
pub fn set_policy(policy: Policy) -> bool {
    with(|registry| registry.policy = policy);
    true
}

/// Moves the registry's clock, which the throttle measures against.
pub fn tick(now: u64) {
    with(|registry| registry.now = now);
}

/// The throttle interval, in ticks.
#[must_use]
pub fn get_interval() -> u64 {
    with(|registry| registry.throttle.interval())
}

/// Sets the throttle interval, clearing the last run.
pub fn set_interval(interval: u64) {
    with(|registry| registry.throttle.set_interval(interval));
}

fn run_check() -> CheckReport {
    let (providers, channel, policy) =
        with(|registry| (registry.providers.clone(), registry.channel.clone(), registry.policy));
    let mut updates = Vec::new();
    for provider in &providers {
        let Some(release) = channel.select(&provider.releases) else {
            continue;
        };
        let latest = release.manifest.version.clone();
        updates.push(UpdateInfo {
            name: provider.name.clone(),
            current: provider.current.clone(),
            kind: compare(&provider.current, &latest),
            decision: decide(policy, &provider.current, &latest),
            latest,
        });
    }
    let report = CheckReport {
        channel,
        policy,
        updates,
    };
    with(|registry| {
        registry.status = if report.has_update() {
            Status::Available
        } else {
            Status::Idle
        };
        registry.report = Some(report.clone());
    });
    report
}

/// Runs every provider's check through the current channel, throttled.
pub fn check() -> Result<CheckReport, Error> {
    let interval = with(|registry| registry.throttle.interval());
    let now = with(|registry| registry.now);
    if !with(|registry| registry.throttle.allows(now)) {
        crate::warn!("an update check was refused: at most one every {interval} ticks");
        return Err(Error::Throttled { interval });
    }
    Ok(run_check())
}

/// Runs one provider's check, unthrottled.
pub fn check_mod(name: &str) -> Result<UpdateInfo, Error> {
    let (channel, policy) = with(|registry| (registry.channel.clone(), registry.policy));
    let provider = with(|registry| {
        registry
            .providers
            .iter()
            .find(|provider| provider.name == name)
            .cloned()
    })
    .ok_or_else(|| Error::NoProvider(name.to_owned()))?;
    let release = channel.select(&provider.releases).ok_or(Error::NoUpdate)?;
    let latest = release.manifest.version.clone();
    let kind = compare(&provider.current, &latest);
    let decision = decide(policy, &provider.current, &latest);
    Ok(UpdateInfo {
        name: provider.name,
        current: provider.current,
        latest,
        kind,
        decision,
    })
}

/// Runs the check for the provider named `loader`.
pub fn check_loader() -> Result<UpdateInfo, Error> {
    check_mod("loader")
}

/// The version the current channel would pick for a provider.
#[must_use]
pub fn get_latest_version(name: &str) -> Option<Version> {
    with(|registry| {
        let provider = registry
            .providers
            .iter()
            .find(|provider| provider.name == name)?;
        let release = registry.channel.select(&provider.releases)?;
        Some(release.manifest.version.clone())
    })
}

/// The changelog entries that arrived with a version, across every provider.
#[must_use]
pub fn get_release_notes(version: &Version) -> Vec<String> {
    with(|registry| {
        registry
            .providers
            .iter()
            .flat_map(|provider| provider.releases.iter())
            .find(|release| &release.manifest.version == version)
            .map_or_else(Vec::new, |release| release.notes.clone())
    })
}

/// The running version against the one the channel would pick for a provider.
#[must_use]
pub fn get_compatibility(name: &str) -> Compatibility {
    with(|registry| {
        let Some(provider) = registry
            .providers
            .iter()
            .find(|provider| provider.name == name)
        else {
            return Compatibility::Unknown;
        };
        let Some(release) = registry.channel.select(&provider.releases) else {
            return Compatibility::Unknown;
        };
        Compatibility::of(compare(&provider.current, &release.manifest.version))
    })
}

/// The last check's report, if a check has run.
#[must_use]
pub fn get_report() -> Option<CheckReport> {
    with(|registry| registry.report.clone())
}

/// Where the update lifecycle stands.
#[must_use]
pub fn get_status() -> Status {
    with(|registry| registry.status)
}

/// How far the lifecycle has come, 0 before staging and 1 once staged.
#[must_use]
pub fn get_progress() -> f32 {
    with(|registry| registry.progress)
}

/// Every version the lifecycle reached, oldest first.
#[must_use]
pub fn get_history() -> Vec<HistoryEntry> {
    with(|registry| registry.history.clone())
}

/// Records a plan as staged, refusing one with no hash to verify against.
pub fn stage_update(plan: Plan) -> bool {
    if plan.hash.trim().is_empty() {
        crate::warn!("an update plan for {} has no hash to verify", plan.version);
        return false;
    }
    with(|registry| {
        registry.staged = Some(plan);
        registry.status = Status::Staged;
        registry.progress = 1.0;
    });
    true
}

/// Applies the staged update and answers the plan it applied.
pub fn apply_update() -> Result<Plan, Error> {
    let staged = with(|registry| registry.staged.clone()).ok_or(Error::NothingStaged)?;
    with(|registry| {
        registry.staged = None;
        registry.applied = Some(staged.clone());
        registry.status = Status::Applied;
        registry.history.push(HistoryEntry {
            version: staged.version.clone(),
            status: Status::Applied,
        });
    });
    Ok(staged)
}

/// Drops the staged update, and answers whether there was one.
pub fn cancel_update() -> bool {
    match with(|registry| registry.staged.take()) {
        Some(plan) => {
            with(|registry| {
                registry.status = Status::Cancelled;
                registry.progress = 0.0;
                registry.history.push(HistoryEntry {
                    version: plan.version,
                    status: Status::Cancelled,
                });
            });
            true
        }
        None => false,
    }
}

/// Reverts the applied update, and answers whether there was one.
pub fn rollback_update() -> bool {
    match with(|registry| registry.applied.take()) {
        Some(plan) => {
            with(|registry| {
                registry.status = Status::Idle;
                registry.progress = 0.0;
                registry.history.push(HistoryEntry {
                    version: plan.version,
                    status: Status::Idle,
                });
            });
            true
        }
        None => false,
    }
}

/// Clears every provider, report and lifecycle trace. The channel and policy stay.
pub fn clear() {
    with(|registry| {
        let channel = registry.channel.clone();
        let policy = registry.policy;
        let interval = registry.throttle.interval();
        *registry = Registry::default();
        registry.channel = channel;
        registry.policy = policy;
        registry.throttle.set_interval(interval);
    });
}

/// Why a question about the network or the client's own updater has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "download" => {
            "fetching a release is a network request this host does not make; the caller \
             fetches the URL in the plan itself"
                .to_owned()
        }
        "client_updater" => {
            "the client's own updater is private to the client and has no entry point this \
             host can reach"
                .to_owned()
        }
        _ => format!("{what} is not an update question this host can answer"),
    }
}

/// What this host cannot reach about fetching a release.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["download", "client_updater"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

/// Fetching a release. [`Error::Refused`], with [`why_unknown`] and a warning.
pub fn download_update(manifest: &Manifest) -> Result<Vec<u8>, Error> {
    let why = why_unknown("download");
    crate::warn!("a mod asked this host to fetch {}: {why}", manifest.url);
    Err(Error::Refused(why))
}

/// Driving the client's own updater. [`Error::Refused`], with [`why_unknown`] and a warning.
pub fn update_client() -> Result<(), Error> {
    let why = why_unknown("client_updater");
    crate::warn!("a mod asked this host to update the client: {why}");
    Err(Error::Refused(why))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn version(text: &str) -> Version {
        Version::parse(text)
    }

    fn release(text: &str, url: &str, size: u64, hash: &str, notes: &[&str]) -> Release {
        let mut release = Release::new(Manifest::new(version(text), url, size, hash));
        for note in notes {
            release = release.note(*note);
        }
        release
    }

    #[test]
    fn a_kind_is_patch_minor_or_major_and_never_for_an_equal_or_older_version() {
        assert_eq!(compare(&version("1.2.0"), &version("1.2.1")), UpdateKind::Patch);
        assert_eq!(compare(&version("1.2.0"), &version("1.3.0")), UpdateKind::Minor);
        assert_eq!(compare(&version("1.2.0"), &version("2.0.0")), UpdateKind::Major);
        assert_eq!(compare(&version("1.2.0"), &version("1.2.0")), UpdateKind::UpToDate);
        assert_eq!(compare(&version("1.2.0"), &version("1.1.9")), UpdateKind::UpToDate);
        assert_eq!(compare(&version("1.2.0"), &version("nightly")), UpdateKind::UpToDate);
        assert!(is_newer(&version("1.2.0"), &version("1.2.1")));
        assert!(!is_newer(&version("1.2.0"), &version("1.2.0")));
        assert!(!is_update_available(&version("1.2.0"), &version("1.1.0")));
        assert!(UpdateKind::Patch.is_update());
        assert!(UpdateKind::Major.is_update());
    }

    #[test]
    fn a_channel_selects_candidates_by_its_stated_rule() {
        let releases = vec![
            release("1.2.0", "u", 1, "h", &[]),
            release("1.3.0", "u", 1, "h", &[]),
            release("1.4.0-beta.1", "u", 1, "h", &[]),
        ];
        let stable = Channel::Stable;
        assert_eq!(stable.rule(), "releases with no prerelease");
        assert!(stable.accepts(&version("1.3.0")));
        assert!(!stable.accepts(&version("1.4.0-beta.1")));
        assert_eq!(stable.select(&releases).map(|rel| rel.manifest.version.clone()), Some(version("1.3.0")));

        let beta = Channel::Beta;
        assert_eq!(beta.candidates(&releases).len(), 3);
        assert_eq!(beta.select(&releases).map(|rel| rel.manifest.version.clone()), Some(version("1.4.0-beta.1")));
    }

    #[test]
    fn a_pinned_channel_hands_back_the_pin_so_it_is_never_told_to_move() {
        let _serial = serial();
        clear();
        set_channel(Channel::Pinned(version("1.2.0")));
        assert_eq!(get_current_channel(), Channel::Pinned(version("1.2.0")));

        let provider = Provider::new("sprint", version("1.2.0"))
            .release(release("1.2.0", "u", 1, "h", &[]))
            .release(release("1.3.0", "u", 1, "h", &[]));
        assert!(register_provider(provider));

        let info = check_mod("sprint").expect("a registered provider");
        assert_eq!(info.latest, version("1.2.0"), "the pin is the latest candidate");
        assert_eq!(info.kind, UpdateKind::UpToDate);
        assert!(!info.decision.prompt && !info.decision.fetch);
        assert_eq!(get_compatibility("sprint"), Compatibility::UpToDate);
        clear();
    }

    #[test]
    fn a_policy_decides_per_policy_and_prompts_only_when_it_notifies() {
        let current = version("1.2.0");
        let latest = version("1.3.0");
        assert_eq!(
            decide(Policy::Auto, &current, &latest),
            Decision { kind: UpdateKind::Minor, prompt: false, fetch: true }
        );
        assert_eq!(
            decide(Policy::Notify, &current, &latest),
            Decision { kind: UpdateKind::Minor, prompt: true, fetch: false }
        );
        assert_eq!(decide(Policy::Manual, &current, &latest), Decision::of(UpdateKind::Minor));
        assert_eq!(decide(Policy::Pinned, &current, &latest), Decision::of(UpdateKind::UpToDate));
        assert_eq!(decide(Policy::Auto, &current, &current), Decision::of(UpdateKind::UpToDate));
    }

    #[test]
    fn a_throttled_check_refuses_one_tick_before_the_interval_and_runs_at_it() {
        let _serial = serial();
        let mut throttle = Throttle::new(30);
        assert!(throttle.allows(0));
        assert!(!throttle.allows(29), "one tick before the interval is refused");
        assert!(throttle.allows(30), "at the interval it runs");
        throttle.reset();
        assert!(throttle.allows(100));

        clear();
        set_interval(30);
        tick(0);
        assert!(check().is_ok());
        assert_eq!(check(), Err(Error::Throttled { interval: 30 }));
        tick(29);
        assert_eq!(check(), Err(Error::Throttled { interval: 30 }));
        tick(30);
        assert!(check().is_ok());
        clear();
        set_interval(DEFAULT_INTERVAL);
    }

    #[test]
    fn a_plan_names_the_url_the_size_the_hash_and_the_destination() {
        let update = release("1.3.0", "https://mods.example/sprint-1.3.0.jar", 4096, "abcdef", &[]);
        let plan = plan_update(&version("1.2.0"), &update, "/tmp/cl-mods/");
        assert_eq!(plan.url, "https://mods.example/sprint-1.3.0.jar");
        assert_eq!(plan.size, 4096);
        assert_eq!(plan.hash, "abcdef");
        assert_eq!(plan.destination, "/tmp/cl-mods/sprint-1.3.0.jar");
        assert_eq!(plan.kind, UpdateKind::Minor);
    }

    #[test]
    fn a_fetched_artifact_verifies_by_hash_and_a_mismatch_names_both() {
        let body = b"a fetched jar";
        let good = hash_of(body);
        assert_eq!(good.len(), 16, "FNV-1a 64 written as hex");
        assert!(verify_update(body, &good).is_verified());
        assert_eq!(
            verify_update(body, "0000000000000000"),
            Verification::Mismatch {
                expected: "0000000000000000".to_owned(),
                found: good.clone()
            }
        );
        assert_eq!(
            verify_update(body, &good.to_uppercase()),
            Verification::Verified,
            "hex case does not matter"
        );
    }

    #[test]
    fn the_changelog_answers_the_entries_between_two_versions() {
        let mut log = Changelog::new();
        log.add(release("1.2.1", "u", 1, "h", &["fix crash", "faster ticks"]));
        log.add(release("1.3.0", "u", 1, "h", &["new sprint key"]));
        log.add(release("1.4.0", "u", 1, "h", &["beyond the range"]));

        let between = log.between(&version("1.2.0"), &version("1.3.0"));
        assert_eq!(
            between,
            vec![
                Entry { version: version("1.2.1"), text: "fix crash".to_owned() },
                Entry { version: version("1.2.1"), text: "faster ticks".to_owned() },
                Entry { version: version("1.3.0"), text: "new sprint key".to_owned() },
            ]
        );
        assert_eq!(log.since(&version("1.2.1")).len(), 2);
        assert_eq!(log.notes(&version("1.2.1")), vec!["fix crash".to_owned(), "faster ticks".to_owned()]);
        assert_eq!(log.versions(), vec![version("1.2.1"), version("1.3.0"), version("1.4.0")]);
    }

    #[test]
    fn a_provider_check_reports_the_latest_and_the_network_is_refused() {
        let _serial = serial();
        clear();
        set_channel(Channel::Stable);
        set_policy(Policy::Notify);
        let provider = Provider::new("loader", version("1.2.0"))
            .release(release("1.2.0", "u", 1, "h", &[]))
            .release(release("1.3.0", "https://mods.example/loader-1.3.0.jar", 10, "h", &["a note"]));
        assert!(register_provider(provider));
        assert!(!register_provider(Provider::new("loader", version("1.2.0"))), "a taken name is refused");

        tick(0);
        let report = check().expect("the first check runs");
        assert!(report.has_update());
        let info = report.newest().expect("one provider");
        assert_eq!(info.latest, version("1.3.0"));
        assert_eq!(info.kind, UpdateKind::Minor);
        assert_eq!(get_latest_version("loader"), Some(version("1.3.0")));
        assert_eq!(get_release_notes(&version("1.3.0")), vec!["a note".to_owned()]);
        assert_eq!(get_compatibility("loader"), Compatibility::Minor);
        assert_eq!(check_loader().expect("the provider").kind, UpdateKind::Minor);

        assert_eq!(refusals().len(), 2);
        assert!(why_unknown("download").contains("network request"));
        assert!(why_unknown("client_updater").contains("client's own updater"));
        let manifest = Manifest::new(version("1.3.0"), "https://mods.example/x.jar", 10, "h");
        assert!(matches!(download_update(&manifest), Err(Error::Refused(_))));
        assert!(matches!(update_client(), Err(Error::Refused(_))));

        assert!(unregister_provider("loader"));
        assert!(!unregister_provider("loader"));
        assert_eq!(check_mod("loader"), Err(Error::NoProvider("loader".to_owned())));
        clear();
    }

    #[test]
    fn the_lifecycle_stages_applies_cancels_and_rolls_back_a_plan() {
        let _serial = serial();
        clear();
        let update = release("1.3.0", "https://mods.example/sprint-1.3.0.jar", 4096, "abcdef", &[]);
        let plan = plan_update(&version("1.2.0"), &update, "/mods");

        assert!(!stage_update(Plan { hash: String::new(), ..plan.clone() }));
        assert!(stage_update(plan.clone()));
        assert_eq!(get_status(), Status::Staged);
        assert_eq!(get_progress(), 1.0);

        let applied = apply_update().expect("a plan is staged");
        assert_eq!(applied, plan);
        assert_eq!(get_status(), Status::Applied);
        assert_eq!(
            get_history(),
            vec![HistoryEntry { version: version("1.3.0"), status: Status::Applied }]
        );

        assert!(rollback_update());
        assert_eq!(get_status(), Status::Idle);
        assert_eq!(get_progress(), 0.0);
        assert!(!rollback_update());
        assert_eq!(apply_update(), Err(Error::NothingStaged));

        assert!(stage_update(plan));
        assert!(cancel_update());
        assert_eq!(get_status(), Status::Cancelled);
        assert!(!cancel_update());
        clear();
    }
}
