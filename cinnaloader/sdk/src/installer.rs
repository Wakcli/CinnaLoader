//! Mod installation and updates: a real install pipeline over the mod's own directory.
//!
//! A package is a directory tree with a `manifest.txt`. [`scan`] fingerprints it — each
//! file's size and a content hash — and [`plan`] says what installing it would do: [`Add`] a
//! path the target does not have, [`Replace`] one that differs, [`Keep`] one that is
//! byte-identical or only in the target. [`install`] stages the desired tree into a temporary
//! directory and swaps it into place, so a failure leaves the target as it was rather than half
//! installed. It returns a [`Journal`], an ordered record of its steps; [`uninstall`] removes
//! only what the journal says it added and restores what it replaced, and [`rollback`] puts the
//! previous bytes back exactly. [`verify_checksum`] re-hashes the installed files against the
//! package's manifest and names the paths that no longer match.
//!
//! [`update`] compares versions the way [`crate::version`] orders them and refuses to downgrade
//! unless the caller asks for it by name; [`update_all`] runs that over everything installed.
//! [`stage`] and [`commit`] are the two halves of [`install`], exposed for a caller that wants
//! to drive them itself.
//!
//! What this host cannot do is the client's side of installation: writing into the client's own
//! install directory or its mods folder, as the client would. Those are refused with their
//! reasons by [`why_unknown`] and [`refusals`].
//!
//! [`Add`]: PlanAction::Add
//! [`Replace`]: PlanAction::Replace
//! [`Keep`]: PlanAction::Keep
//!
//! ```
//! use cinnaloader::{fs, installer};
//!
//! # let root = std::env::temp_dir().join(format!("cinnaloader-doc-install-{}", std::process::id()));
//! # let _ = std::fs::remove_dir_all(&root);
//! # std::fs::create_dir_all(&root).unwrap();
//! # fs::set_root(root);
//! fs::write_text("pkg/a.txt", "one").unwrap();
//! fs::write_text("pkg/manifest.txt", "id=alpha\nversion=1.2.0\n").unwrap();
//! fs::write_text("game/a.txt", "old").unwrap();
//! let plan = installer::plan("pkg", "game").expect("a plan over both trees");
//! assert!(plan.replaced().contains(&"a.txt"));
//! let journal = installer::install("pkg", "game", "alpha").expect("a real install");
//! assert_eq!(journal.version.to_string(), "1.2.0");
//! assert_eq!(fs::read_text("game/a.txt").unwrap(), "one");
//! assert_eq!(installer::verify_checksum("alpha").unwrap(), Vec::<String>::new());
//! # let _ = installer::uninstall("alpha");
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::fs::{self, FsError};
use crate::modinfo::ModVersion as Version;

/// What went wrong while installing, with the path or package it went wrong for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallError {
    /// There is no such file or directory.
    Missing(String),
    /// No package is installed under that name.
    NotInstalled(String),
    /// A package name is not one this host will use.
    Invalid(String),
    /// The caller cancelled the install part-way.
    Cancelled(String),
    /// The update would move a package backwards and was not asked to.
    Downgrade { from: String, to: String },
    /// The operating system refused, with its message.
    Io(String),
}

impl fmt::Display for InstallError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(path) => write!(formatter, "no such file or directory: {path}"),
            Self::NotInstalled(name) => write!(formatter, "'{name}' is not installed"),
            Self::Invalid(name) => write!(formatter, "'{name}' is not a usable package name"),
            Self::Cancelled(name) => write!(formatter, "the install of '{name}' was cancelled"),
            Self::Downgrade { from, to } => {
                write!(formatter, "refusing to downgrade from {from} to {to}")
            }
            Self::Io(message) => write!(formatter, "{message}"),
        }
    }
}

impl std::error::Error for InstallError {}

impl From<FsError> for InstallError {
    fn from(error: FsError) -> Self {
        match error {
            FsError::Missing(path) => Self::Missing(path),
            other => Self::Io(other.to_string()),
        }
    }
}

/// What installing one path would do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlanAction {
    /// The target does not have it.
    Add,
    /// The target has a different file there, so it would be replaced.
    Replace,
    /// Byte-identical to the target, or only in the target and left alone.
    Keep,
}

impl PlanAction {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Add => "added",
            Self::Replace => "replaced",
            Self::Keep => "kept",
        }
    }
}

impl fmt::Display for PlanAction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One file's size and content hash, taken by [`scan`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint {
    pub path: String,
    pub size: u64,
    pub hash: u64,
}

/// One path an [`InstallPlan`] would touch, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanEntry {
    pub path: String,
    pub action: PlanAction,
    pub reason: String,
}

/// What installing a source over a target would do, path by path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstallPlan {
    pub source: String,
    pub target: String,
    pub entries: Vec<PlanEntry>,
}

impl InstallPlan {
    fn pick(&self, action: PlanAction) -> Vec<&str> {
        self.entries
            .iter()
            .filter(|entry| entry.action == action)
            .map(|entry| entry.path.as_str())
            .collect()
    }

    /// The paths the install would create.
    #[must_use]
    pub fn added(&self) -> Vec<&str> {
        self.pick(PlanAction::Add)
    }

    /// The paths the install would overwrite.
    #[must_use]
    pub fn replaced(&self) -> Vec<&str> {
        self.pick(PlanAction::Replace)
    }

    /// The paths the install would leave as they are.
    #[must_use]
    pub fn kept(&self) -> Vec<&str> {
        self.pick(PlanAction::Keep)
    }
}

/// How one step of an install ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StepOutcome {
    Ok,
    Failed,
    Undone,
}

/// One ordered step of an install: what it touched and how it ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalStep {
    pub path: String,
    pub action: PlanAction,
    pub outcome: StepOutcome,
    pub reason: String,
}

/// The record of an install, in the order it happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Journal {
    pub name: String,
    pub source: String,
    pub target: String,
    pub version: Version,
    pub steps: Vec<JournalStep>,
    /// Where the previous target tree was kept, when there was one, for a rollback.
    pub backup: Option<String>,
    /// Every file the install left in the target, fingerprinted for a verify.
    pub manifest: Vec<Fingerprint>,
}

impl Journal {
    /// The steps that ended well, in order.
    #[must_use]
    pub fn completed(&self) -> Vec<&JournalStep> {
        self.steps
            .iter()
            .filter(|step| step.outcome == StepOutcome::Ok)
            .collect()
    }
}

/// What a package's `manifest.txt` declares about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: Version,
    pub dependencies: Vec<String>,
    /// A fingerprint string, compared by [`verify_signature`]; not cryptography.
    pub signature: String,
    pub changelog: String,
}

impl Manifest {
    /// A manifest for a source with no file of its own, named after the directory.
    #[must_use]
    pub fn named(source: &str) -> Self {
        let id = source
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or(source)
            .to_owned();
        Self {
            name: id.clone(),
            id,
            version: Version::parse("0.0.0"),
            dependencies: Vec::new(),
            signature: String::new(),
            changelog: String::new(),
        }
    }
}

/// A package offered by a repository: what a search or a version listing answers with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageInfo {
    pub id: String,
    pub name: String,
    pub version: Version,
    pub dependencies: Vec<String>,
    pub changelog: String,
}

/// A repository of packages this host can read, rooted at a directory of the mod's own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repository {
    pub id: String,
    pub name: String,
    pub url: String,
    /// A directory relative to the mod's own folder holding `<id>/<version>/manifest.txt`.
    pub root: String,
}

impl Repository {
    #[must_use]
    pub fn new(id: &str, name: &str, root: &str) -> Self {
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            url: String::new(),
            root: root.to_owned(),
        }
    }

    #[must_use]
    pub fn url(mut self, url: &str) -> Self {
        self.url = url.to_owned();
        self
    }
}

/// A package this host has installed, with the journal that put it there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub name: String,
    pub id: String,
    pub version: Version,
    pub source: String,
    pub target: String,
    pub journal: Journal,
}

/// What the installer is doing or last did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstallationStatus {
    pub installed: usize,
    pub backups: usize,
    pub busy: bool,
    pub cancelled: bool,
    pub last: Option<String>,
}

#[derive(Clone)]
struct Backup {
    directory: String,
    target: String,
}

/// A package directory found under a repository's root.
struct Found {
    info: PackageInfo,
    directory: String,
}

#[derive(Default)]
struct Registry {
    repositories: BTreeMap<String, Repository>,
    installed: BTreeMap<String, Installed>,
    backups: BTreeMap<String, Backup>,
    journal: Option<Journal>,
    cancel: bool,
    busy: bool,
    last: Option<String>,
}

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Registry::default()))
}

fn with<T>(body: impl FnOnce(&mut Registry) -> T) -> T {
    let mut guard = registry().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn next_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn usable_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains("..")
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
}

/// The path of `path` relative to the mod's own directory, with `/` separators.
fn to_rel(path: &Path) -> String {
    match fs::root() {
        Some(root) => path
            .strip_prefix(&root)
            .map(|relative| relative.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| path.to_string_lossy().into_owned()),
        None => path.to_string_lossy().into_owned(),
    }
}

/// Walks a source directory and fingerprints every file under it, in name order.
pub fn scan(source: &str) -> Result<Vec<Fingerprint>, InstallError> {
    let root = fs::resolve(source)?;
    if !root.is_dir() {
        return Err(InstallError::Missing(source.to_owned()));
    }
    let mut files = Vec::new();
    for path in fs::walk(source)? {
        if !path.is_file() {
            continue;
        }
        let Ok(relative) = path.strip_prefix(&root) else {
            continue;
        };
        let bytes = std::fs::read(&path)
            .map_err(|error| InstallError::Io(format!("{}: {error}", path.display())))?;
        files.push(Fingerprint {
            path: relative.to_string_lossy().replace('\\', "/"),
            size: bytes.len() as u64,
            hash: fs::hash_bytes(&bytes),
        });
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

/// The same as [`scan`], but an empty list when there is no such directory.
#[must_use]
pub fn scan_optional(source: &str) -> Vec<Fingerprint> {
    scan(source).unwrap_or_default()
}

/// Says what installing `source` over `target` would do, path by path, with a reason each.
pub fn plan(source: &str, target: &str) -> Result<InstallPlan, InstallError> {
    let files = scan(source)?;
    let existing = scan_optional(target);
    let by_path: BTreeMap<&str, &Fingerprint> = existing
        .iter()
        .map(|fingerprint| (fingerprint.path.as_str(), fingerprint))
        .collect();
    let mut entries = Vec::new();
    for fingerprint in &files {
        let entry = match by_path.get(fingerprint.path.as_str()) {
            Some(old) if old.hash == fingerprint.hash && old.size == fingerprint.size => PlanEntry {
                path: fingerprint.path.clone(),
                action: PlanAction::Keep,
                reason: "byte-identical to the target".to_owned(),
            },
            Some(_) => PlanEntry {
                path: fingerprint.path.clone(),
                action: PlanAction::Replace,
                reason: "the target differs and would be replaced".to_owned(),
            },
            None => PlanEntry {
                path: fingerprint.path.clone(),
                action: PlanAction::Add,
                reason: "not in the target yet".to_owned(),
            },
        };
        entries.push(entry);
    }
    let offered: BTreeSet<&str> = files.iter().map(|file| file.path.as_str()).collect();
    for fingerprint in &existing {
        if !offered.contains(fingerprint.path.as_str()) {
            entries.push(PlanEntry {
                path: fingerprint.path.clone(),
                action: PlanAction::Keep,
                reason: "only in the target, left untouched".to_owned(),
            });
        }
    }
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(InstallPlan {
        source: source.to_owned(),
        target: target.to_owned(),
        entries,
    })
}

/// Copies every file under `from` into `to`, skipping anything under `skip`.
fn copy_tree(from: &str, to: &str, skip: Option<&Path>) -> Result<usize, InstallError> {
    let root = fs::resolve(from)?;
    if !root.is_dir() {
        return Err(InstallError::Missing(from.to_owned()));
    }
    let mut written = 0;
    for path in fs::walk(from)? {
        if !path.is_file() || skip.is_some_and(|skip| path.starts_with(skip)) {
            continue;
        }
        let Ok(relative) = path.strip_prefix(&root) else {
            continue;
        };
        let relative = relative.to_string_lossy().replace('\\', "/");
        let bytes = std::fs::read(&path)
            .map_err(|error| InstallError::Io(format!("{}: {error}", path.display())))?;
        written += bytes.len();
        fs::write(&format!("{to}/{relative}"), &bytes)?;
    }
    Ok(written)
}

/// Builds the desired target tree in a temporary directory and returns its path. The target's
/// own files are laid down first, so a source file overwrites the one it replaces.
pub fn stage(source: &str, target: &str) -> Result<String, InstallError> {
    let staging = fs::temporary_directory(Some(".installer/staging"))?;
    let staging_rel = to_rel(&staging);
    let skip = fs::resolve(&staging_rel).ok();
    if fs::is_directory(target) {
        copy_tree(target, &staging_rel, skip.as_deref())?;
    }
    copy_tree(source, &staging_rel, skip.as_deref())?;
    Ok(staging_rel)
}

fn parse_manifest(text: &str, source: &str) -> Manifest {
    let mut manifest = Manifest::named(source);
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
            "id" => manifest.id = value.to_owned(),
            "name" => manifest.name = value.to_owned(),
            "version" => manifest.version = Version::parse(value),
            "depends" => manifest.dependencies.push(value.to_owned()),
            "signature" => manifest.signature = value.to_owned(),
            "changelog" => manifest.changelog = value.to_owned(),
            _ => {}
        }
    }
    manifest
}

/// Reads a package's `manifest.txt`, or [`InstallError::Missing`] when there is none.
pub fn get_manifest(source: &str) -> Result<Manifest, InstallError> {
    let text = fs::read_text(&format!("{source}/manifest.txt"))?;
    Ok(parse_manifest(&text, source))
}

/// A checksum over a package's fingerprints, for [`verify_signature`] to compare against.
pub fn package_checksum(source: &str) -> Result<u64, InstallError> {
    let files = scan(source)?;
    let mut all = Vec::new();
    for fingerprint in &files {
        all.extend_from_slice(fingerprint.path.as_bytes());
        all.extend_from_slice(&fingerprint.size.to_le_bytes());
        all.extend_from_slice(&fingerprint.hash.to_le_bytes());
    }
    Ok(fs::hash_bytes(&all))
}

/// Whether `signature` matches the package's checksum. A fingerprint comparison, not
/// cryptography: it notices a changed package, it does not prove who built one.
#[must_use]
pub fn verify_signature(source: &str, signature: &str) -> bool {
    match package_checksum(source) {
        Ok(hash) => signature.trim() == format!("fnv1a:{hash:016x}"),
        Err(_) => false,
    }
}

/// Everything wrong with a package as it stands: no manifest, a bad id, no version, no files,
/// or a dependency nothing here provides.
#[must_use]
pub fn validate_package(source: &str) -> Vec<String> {
    let manifest = match get_manifest(source) {
        Ok(manifest) => manifest,
        Err(error) => return vec![error.to_string()],
    };
    let mut problems = Vec::new();
    if !usable_name(&manifest.id) {
        problems.push(format!("'{}' is not a usable package id", manifest.id));
    }
    if !manifest.version.is_numeric() {
        problems.push(format!("'{}' declares no version", manifest.id));
    }
    let content = scan(source)
        .unwrap_or_default()
        .into_iter()
        .filter(|file| file.path != "manifest.txt")
        .count();
    if content == 0 {
        problems.push(format!("'{}' holds no files", manifest.id));
    }
    for dependency in &manifest.dependencies {
        if !dependency_known(dependency) {
            problems.push(format!(
                "'{}' depends on '{dependency}', which is not installed or available",
                manifest.id
            ));
        }
    }
    problems
}

fn dependency_known(id: &str) -> bool {
    with(|registry| registry.installed.values().any(|record| record.id == id))
        || !get_versions(id).is_empty()
}

fn repositories() -> Vec<Repository> {
    with(|registry| registry.repositories.values().cloned().collect())
}

fn discover_repo(repository: &Repository) -> Vec<Found> {
    let mut found = Vec::new();
    for path in fs::walk(&repository.root).unwrap_or_default() {
        if path.file_name().and_then(|name| name.to_str()) != Some("manifest.txt") {
            continue;
        }
        let Some(directory) = path.parent() else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let manifest = parse_manifest(&text, &repository.id);
        found.push(Found {
            info: PackageInfo {
                id: manifest.id,
                name: manifest.name,
                version: manifest.version,
                dependencies: manifest.dependencies,
                changelog: manifest.changelog,
            },
            directory: to_rel(directory),
        });
    }
    found
}

/// Registers a repository, refusing a bad id or an id already taken.
pub fn register_repository(repository: Repository) -> bool {
    if !usable_name(&repository.id) {
        crate::warn!(
            "a mod registered a repository named '{}', which is not usable",
            repository.id
        );
        return false;
    }
    with(|registry| match registry.repositories.entry(repository.id.clone()) {
        std::collections::btree_map::Entry::Occupied(_) => false,
        std::collections::btree_map::Entry::Vacant(slot) => {
            slot.insert(repository);
            true
        }
    })
}

/// Forgets a repository, answering whether there was one.
pub fn unregister_repository(id: &str) -> bool {
    with(|registry| registry.repositories.remove(id).is_some())
}

/// Every package every repository offers, by id then version.
#[must_use]
pub fn get_available() -> Vec<PackageInfo> {
    let mut all: Vec<PackageInfo> = repositories()
        .iter()
        .flat_map(discover_repo)
        .map(|found| found.info)
        .collect();
    all.sort_by(|left, right| left.id.cmp(&right.id).then(left.version.cmp(&right.version)));
    all.dedup_by(|left, right| left.id == right.id && left.version == right.version);
    all
}

/// The packages whose id or name contains `query`, or every package for an empty query.
#[must_use]
pub fn search_repository(query: &str) -> Vec<PackageInfo> {
    let needle = query.trim().to_ascii_lowercase();
    get_available()
        .into_iter()
        .filter(|package| {
            needle.is_empty()
                || package.id.to_ascii_lowercase().contains(&needle)
                || package.name.to_ascii_lowercase().contains(&needle)
        })
        .collect()
}

/// Every version of a package any repository offers, oldest first.
#[must_use]
pub fn get_versions(id: &str) -> Vec<Version> {
    let mut versions: Vec<Version> = repositories()
        .iter()
        .flat_map(discover_repo)
        .filter(|found| found.info.id == id)
        .map(|found| found.info.version)
        .collect();
    versions.sort();
    versions.dedup();
    versions
}

/// A package version's changelog, when a repository offers it.
#[must_use]
pub fn get_changelog(id: &str, version: &str) -> Option<String> {
    let wanted = Version::parse(version);
    get_available()
        .into_iter()
        .find(|package| package.id == id && package.version == wanted)
        .map(|package| package.changelog)
}

/// Copies a package out of a repository into `to`, and answers how many bytes went.
pub fn download(id: &str, version: &str, to: &str) -> Result<u64, InstallError> {
    let wanted = Version::parse(version);
    for repository in repositories() {
        for found in discover_repo(&repository) {
            if found.info.id == id && found.info.version == wanted {
                return copy_tree(&found.directory, to, None).map(|bytes| bytes as u64);
            }
        }
    }
    Err(InstallError::Missing(format!("no repository has {id} {version}")))
}

/// Expands a downloaded package directory into `target`, and answers how many bytes went.
pub fn extract(source: &str, target: &str) -> Result<usize, InstallError> {
    fs::create_directories(target)?;
    copy_tree(source, target, None)
}

/// The direct dependencies a package declares, read from its source.
#[must_use]
pub fn get_dependencies(name: &str) -> Vec<String> {
    let installed = with(|registry| registry.installed.get(name).cloned());
    if let Some(installed) = installed {
        if let Ok(manifest) = get_manifest(&installed.source) {
            return manifest.dependencies;
        }
    }
    for repository in repositories() {
        for found in discover_repo(&repository) {
            if found.info.id == name {
                return found.info.dependencies;
            }
        }
    }
    Vec::new()
}

fn visit(id: &str, order: &mut Vec<String>) {
    if order.iter().any(|seen| seen == id) {
        return;
    }
    order.push(id.to_owned());
    for dependency in get_dependencies(id) {
        visit(&dependency, order);
    }
}

/// A package's dependencies in install order, each dependency before what needs it. A
/// dependency this host cannot find is named too, so the caller can see it is missing.
#[must_use]
pub fn resolve_dependencies(source: &str) -> Vec<String> {
    let mut order = Vec::new();
    if let Ok(manifest) = get_manifest(source) {
        for dependency in manifest.dependencies {
            visit(&dependency, &mut order);
        }
    }
    order
}

fn journal_from(
    staging: &str,
    target: &str,
    name: &str,
    source: &str,
) -> Result<(Vec<JournalStep>, Vec<Fingerprint>), InstallError> {
    let staged = scan(staging)?;
    let existing = scan_optional(target);
    let by_path: BTreeMap<&str, &Fingerprint> = existing
        .iter()
        .map(|fingerprint| (fingerprint.path.as_str(), fingerprint))
        .collect();
    let mut steps: Vec<JournalStep> = staged
        .iter()
        .map(|fingerprint| JournalStep {
            path: fingerprint.path.clone(),
            action: match by_path.get(fingerprint.path.as_str()) {
                Some(old) if old.hash == fingerprint.hash && old.size == fingerprint.size => {
                    PlanAction::Keep
                }
                Some(_) => PlanAction::Replace,
                None => PlanAction::Add,
            },
            outcome: StepOutcome::Ok,
            reason: format!("written into {target} by '{name}'"),
        })
        .collect();
    let offered: BTreeSet<&str> = staged.iter().map(|file| file.path.as_str()).collect();
    for fingerprint in &existing {
        if !offered.contains(fingerprint.path.as_str()) {
            steps.push(JournalStep {
                path: fingerprint.path.clone(),
                action: PlanAction::Keep,
                outcome: StepOutcome::Ok,
                reason: format!("already in {target} and not in '{source}'"),
            });
        }
    }
    steps.sort_by(|left, right| left.path.cmp(&right.path));
    Ok((steps, staged))
}

fn commit_staged(
    staging: &str,
    target: &str,
    name: &str,
    source: &str,
    manifest: &Manifest,
) -> Result<Journal, InstallError> {
    let (steps, manifest_files) = journal_from(staging, target, name, source)?;
    let backup = format!(".installer/backups/{name}-{}", next_id());
    let had_target = fs::is_directory(target);
    if had_target {
        fs::create_directories(".installer/backups")?;
        fs::move_path(target, &backup)?;
    }
    if let Err(error) = fs::move_path(staging, target) {
        if had_target {
            let _ = fs::move_path(&backup, target);
        }
        let _ = fs::delete(staging);
        return Err(error.into());
    }
    Ok(Journal {
        name: name.to_owned(),
        source: source.to_owned(),
        target: target.to_owned(),
        version: manifest.version.clone(),
        steps,
        backup: had_target.then_some(backup),
        manifest: manifest_files,
    })
}

/// Moves a staged tree into place, recording each step. A failure before the swap leaves the
/// target exactly as it was.
pub fn commit(staging: &str, target: &str, name: &str) -> Result<Journal, InstallError> {
    let manifest = get_manifest(staging).unwrap_or_else(|_| Manifest::named(name));
    let journal = commit_staged(staging, target, name, staging, &manifest)?;
    with(|registry| registry.journal = Some(journal.clone()));
    Ok(journal)
}

fn take_cancel() -> bool {
    with(|registry| std::mem::take(&mut registry.cancel))
}

/// Asks the next install to stop, answering whether one was running.
pub fn cancel() -> bool {
    with(|registry| {
        let busy = registry.busy;
        registry.cancel = true;
        busy
    })
}

/// The journal of the last install, if any.
#[must_use]
pub fn journal() -> Option<Journal> {
    with(|registry| registry.journal.clone())
}

fn install_body(source: &str, target: &str, name: &str) -> Result<Journal, InstallError> {
    if !usable_name(name) {
        crate::warn!("a mod asked this host to install a package named '{name}', which is not usable");
        return Err(InstallError::Invalid(name.to_owned()));
    }
    let manifest = get_manifest(source)?;
    let staging = stage(source, target)?;
    if take_cancel() {
        let _ = fs::delete(&staging);
        crate::warn!(
            "a mod cancelled the install of '{name}' after staging it; {target} is untouched"
        );
        return Err(InstallError::Cancelled(name.to_owned()));
    }
    let journal = commit_staged(&staging, target, name, source, &manifest)?;
    with(|registry| {
        registry.journal = Some(journal.clone());
        registry.last = Some(format!("installed {name} {}", journal.version));
        registry.installed.insert(
            name.to_owned(),
            Installed {
                name: name.to_owned(),
                id: manifest.id.clone(),
                version: manifest.version.clone(),
                source: source.to_owned(),
                target: target.to_owned(),
                journal: journal.clone(),
            },
        );
    });
    Ok(journal)
}

/// Installs a package over a target, atomically, and answers its journal.
pub fn install(source: &str, target: &str, name: &str) -> Result<Journal, InstallError> {
    with(|registry| registry.busy = true);
    let result = install_body(source, target, name);
    with(|registry| {
        registry.busy = false;
        registry.cancel = false;
    });
    result
}

/// Removes what an install added and restores what it replaced, leaving a pre-existing file
/// that the package did not touch alone. Answers whether anything was removed.
pub fn uninstall(name: &str) -> bool {
    let Some(installed) = with(|registry| registry.installed.remove(name)) else {
        crate::warn!("a mod asked this host to uninstall '{name}', which is not installed");
        return false;
    };
    let mut touched = false;
    for step in &installed.journal.steps {
        match step.action {
            PlanAction::Add if step.outcome == StepOutcome::Ok => {
                if fs::delete(&format!("{}/{}", installed.target, step.path)).is_ok() {
                    touched = true;
                }
            }
            PlanAction::Replace => {
                if let Some(backup) = &installed.journal.backup {
                    let from = format!("{backup}/{}", step.path);
                    if fs::is_file(&from)
                        && fs::copy(&from, &format!("{}/{}", installed.target, step.path), true)
                            .is_ok()
                    {
                        touched = true;
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(backup) = &installed.journal.backup {
        let _ = fs::delete(backup);
    }
    with(|registry| registry.last = Some(format!("uninstalled {name}")));
    touched
}

/// Puts the bytes that were in the target before an install back, exactly, by swapping the
/// install out for the tree it kept.
pub fn rollback(name: &str) -> bool {
    let Some(installed) = with(|registry| registry.installed.remove(name)) else {
        return false;
    };
    let Some(backup) = installed.journal.backup.clone() else {
        let _ = fs::delete(&installed.target);
        return true;
    };
    let aside = format!(".installer/staging/rollback-{}", next_id());
    if fs::is_directory(&installed.target) {
        let _ = fs::create_directories(".installer/staging");
        let _ = fs::move_path(&installed.target, &aside);
    }
    let restored = fs::move_path(&backup, &installed.target).is_ok();
    if !restored && fs::is_directory(&aside) {
        let _ = fs::move_path(&aside, &installed.target);
    }
    let _ = fs::delete(&aside);
    with(|registry| registry.last = Some(format!("rolled {name} back")));
    restored
}

/// Copies an installed package's target aside, and answers the backup's id.
pub fn create_backup(name: &str) -> Result<String, InstallError> {
    let installed = with(|registry| registry.installed.get(name).cloned())
        .ok_or_else(|| InstallError::NotInstalled(name.to_owned()))?;
    let directory = format!(".installer/backups/{name}-manual-{}", next_id());
    fs::create_directories(".installer/backups")?;
    copy_tree(&installed.target, &directory, None)?;
    with(|registry| {
        registry.backups.insert(
            directory.clone(),
            Backup {
                directory: directory.clone(),
                target: installed.target.clone(),
            },
        );
    });
    Ok(directory)
}

/// Puts a backup made by [`create_backup`] back over the target it came from.
pub fn restore_backup(id: &str) -> Result<(), InstallError> {
    let backup = with(|registry| registry.backups.get(id).cloned())
        .ok_or_else(|| InstallError::Missing(id.to_owned()))?;
    let staged = format!(".installer/staging/restore-{}", next_id());
    copy_tree(&backup.directory, &staged, None)?;
    if fs::is_directory(&backup.target) {
        let _ = fs::delete(&backup.target);
    }
    fs::move_path(&staged, &backup.target)?;
    Ok(())
}

/// Re-hashes the installed files against the package's manifest and names the paths that no
/// longer match, so a caller can see what changed on disk.
pub fn verify_checksum(name: &str) -> Result<Vec<String>, InstallError> {
    let installed = with(|registry| registry.installed.get(name).cloned())
        .ok_or_else(|| InstallError::NotInstalled(name.to_owned()))?;
    let mut mismatches = Vec::new();
    for expected in &installed.journal.manifest {
        let path = format!("{}/{}", installed.target, expected.path);
        match fs::read(&path) {
            Ok(bytes)
                if bytes.len() as u64 == expected.size
                    && fs::hash_bytes(&bytes) == expected.hash => {}
            Ok(_) => mismatches.push(expected.path.clone()),
            Err(_) => mismatches.push(expected.path.clone()),
        }
    }
    Ok(mismatches)
}

/// Upgrades an installed package from a newer source, refusing a downgrade unless the caller
/// asks for one. A version equal to the installed one is reinstalled.
pub fn update(source: &str, name: &str, allow_downgrade: bool) -> Result<Journal, InstallError> {
    let installed = with(|registry| registry.installed.get(name).cloned())
        .ok_or_else(|| InstallError::NotInstalled(name.to_owned()))?;
    let manifest = get_manifest(source)?;
    if manifest.version < installed.version && !allow_downgrade {
        crate::warn!(
            "a mod asked this host to downgrade '{name}' from {} to {}; this host refuses unless \
             the caller asks for it",
            installed.version,
            manifest.version
        );
        return Err(InstallError::Downgrade {
            from: installed.version.to_string(),
            to: manifest.version.to_string(),
        });
    }
    install(source, &installed.target, name)
}

/// Runs [`update`] over everything installed, and answers each name with whether it moved.
pub fn update_all(allow_downgrade: bool) -> Vec<(String, bool)> {
    let names: Vec<String> = with(|registry| registry.installed.keys().cloned().collect());
    let mut results = Vec::new();
    for name in names {
        let source = with(|registry| registry.installed.get(&name).map(|record| record.source.clone()));
        let Some(source) = source else {
            continue;
        };
        let moved = update(&source, &name, allow_downgrade).is_ok();
        results.push((name, moved));
    }
    results
}

/// Every package this host has installed.
#[must_use]
pub fn get_installed() -> Vec<Installed> {
    with(|registry| registry.installed.values().cloned().collect())
}

/// The installed packages a repository now offers a newer version of, with both versions.
#[must_use]
pub fn get_updates() -> Vec<(String, Version, Version)> {
    let mut updates = Vec::new();
    for record in get_installed() {
        if let Some(newest) = get_versions(&record.id).into_iter().max() {
            if newest > record.version {
                updates.push((record.name, record.version, newest));
            }
        }
    }
    updates
}

/// What the installer is doing or last did.
#[must_use]
pub fn get_installation_status() -> InstallationStatus {
    with(|registry| InstallationStatus {
        installed: registry.installed.len(),
        backups: registry.backups.len(),
        busy: registry.busy,
        cancelled: registry.cancel,
        last: registry.last.clone(),
    })
}

/// Why a question about the client's own installation has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "client_install" => {
            "installing into the client's own install directory is the client's job, not this \
             host's; nothing a mod stages here reaches it"
                .to_owned()
        }
        "client_update" => {
            "updating the client's installed mods would mean writing the client's own files, \
             which this host has no handle on"
                .to_owned()
        }
        "client_remove" => {
            "removing a mod from the client's mods folder is the client's own file operation and \
             does not cross the boundary"
                .to_owned()
        }
        "mods_directory" => {
            "the client's mods folder sits outside every mod's own directory, so this host \
             neither reads nor writes it"
                .to_owned()
        }
        _ => format!("{what} is not an installer question this host can answer"),
    }
}

/// What this host cannot reach about the client's own installation.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["client_install", "client_update", "client_remove", "mods_directory"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

/// The client's own installation, which this host cannot drive. Each call says why.
pub mod known_only_to_the_client {
    use super::why_unknown;

    /// Installs a package into the client. `false`.
    pub fn install_package(name: &str) -> bool {
        crate::warn!(
            "a mod asked this host to install '{name}' into the client: {}",
            why_unknown("client_install")
        );
        false
    }

    /// Updates a package in the client. `false`.
    pub fn update_package(name: &str) -> bool {
        crate::warn!(
            "a mod asked this host to update '{name}' in the client: {}",
            why_unknown("client_update")
        );
        false
    }

    /// Removes a package from the client. `false`.
    pub fn remove_package(name: &str) -> bool {
        crate::warn!(
            "a mod asked this host to remove '{name}' from the client: {}",
            why_unknown("client_remove")
        );
        false
    }

    /// The client's mods folder. `None`.
    #[must_use]
    pub fn mods_directory() -> Option<std::path::PathBuf> {
        crate::warn!(
            "a mod asked this host for the client's mods folder: {}",
            why_unknown("mods_directory")
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
            "cinnaloader-installer-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        fs::set_root(root.clone());
        root
    }

    fn package(source: &str, id: &str, version: &str) {
        fs::write_text(&format!("{source}/manifest.txt"), &format!("id={id}\nversion={version}\n"))
            .unwrap();
    }

    #[test]
    fn a_scan_fingerprints_every_file_in_name_order() {
        let _serial = serial();
        fresh();
        let root = root("scan");
        fs::write_text("src/a.txt", "one").unwrap();
        fs::write_text("src/deep/b.txt", "two!").unwrap();
        let files = scan("src").unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "a.txt");
        assert_eq!(files[1].path, "deep/b.txt");
        assert_eq!(files[0].size, 3);
        assert_eq!(files[0].hash, fs::hash_bytes(b"one"));
        assert_eq!(scan("nothing").unwrap_err(), InstallError::Missing("nothing".to_owned()));
        assert_eq!(scan_optional("nothing"), Vec::<Fingerprint>::new());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_plan_names_an_added_a_replaced_and_a_kept_file_with_reasons() {
        let _serial = serial();
        fresh();
        let root = root("plan");
        fs::write_text("src/new.txt", "new").unwrap();
        fs::write_text("src/change.txt", "after").unwrap();
        fs::write_text("src/same.txt", "same").unwrap();
        fs::write_text("game/change.txt", "before").unwrap();
        fs::write_text("game/same.txt", "same").unwrap();
        fs::write_text("game/extra.txt", "extra").unwrap();
        let install_plan = plan("src", "game").unwrap();
        assert_eq!(install_plan.added(), vec!["new.txt"]);
        assert_eq!(install_plan.replaced(), vec!["change.txt"]);
        let mut kept = install_plan.kept();
        kept.sort_unstable();
        assert_eq!(kept, vec!["extra.txt", "same.txt"]);
        for entry in &install_plan.entries {
            assert!(!entry.reason.is_empty(), "{} has no reason", entry.path);
        }
        assert_eq!(plan("nothing", "game").unwrap_err(), InstallError::Missing("nothing".to_owned()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_cancelled_install_leaves_the_target_unchanged() {
        let _serial = serial();
        fresh();
        let root = root("atomic");
        package("src", "alpha", "1.0.0");
        fs::write_text("src/a.txt", "a").unwrap();
        fs::write_text("game/existing.txt", "old").unwrap();
        assert!(!cancel(), "nothing was running");
        let refused = install("src", "game", "alpha").unwrap_err();
        assert_eq!(refused, InstallError::Cancelled("alpha".to_owned()));
        assert_eq!(fs::read_text("game/existing.txt").unwrap(), "old");
        assert!(!fs::exists("game/a.txt"), "nothing is half installed");
        assert_eq!(fs::list("game").unwrap().len(), 1);
        let journal = install("src", "game", "alpha").expect("a retry succeeds");
        assert_eq!(fs::read_text("game/a.txt").unwrap(), "a");
        assert!(journal.completed().len() >= 2);
        assert!(!get_installation_status().busy);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_journal_is_ordered_and_uninstall_leaves_a_preexisting_file() {
        let _serial = serial();
        fresh();
        let root = root("journal");
        package("src", "alpha", "1.0.0");
        fs::write_text("src/new.txt", "n").unwrap();
        fs::write_text("src/shared.txt", "new").unwrap();
        fs::write_text("game/shared.txt", "old").unwrap();
        fs::write_text("game/preexisting.txt", "keep").unwrap();
        let journal = install("src", "game", "alpha").unwrap();
        assert!(journal
            .steps
            .iter()
            .any(|step| step.path == "new.txt" && step.action == PlanAction::Add));
        assert!(journal
            .steps
            .iter()
            .any(|step| step.path == "shared.txt" && step.action == PlanAction::Replace));
        assert!(journal.completed().iter().all(|step| step.outcome == StepOutcome::Ok));
        assert_eq!(fs::read_text("game/shared.txt").unwrap(), "new");
        assert!(uninstall("alpha"));
        assert!(!fs::exists("game/new.txt"));
        assert_eq!(
            fs::read_text("game/shared.txt").unwrap(),
            "old",
            "a replaced file is restored from the backup"
        );
        assert!(fs::exists("game/preexisting.txt"), "a pre-existing file survives");
        assert!(!uninstall("alpha"), "it is gone now");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_rollback_restores_the_previous_bytes_exactly() {
        let _serial = serial();
        fresh();
        let root = root("rollback");
        package("src", "alpha", "2.0.0");
        fs::write_text("src/f.txt", "after").unwrap();
        fs::write_text("game/f.txt", "before").unwrap();
        let before = fs::read_text("game/f.txt").unwrap();
        install("src", "game", "alpha").unwrap();
        assert_eq!(fs::read_text("game/f.txt").unwrap(), "after");
        assert!(rollback("alpha"));
        assert_eq!(fs::read_text("game/f.txt").unwrap(), before);
        assert!(get_installed().is_empty(), "a rollback forgets the install");
        assert!(!rollback("alpha"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_verify_names_the_mismatched_path() {
        let _serial = serial();
        fresh();
        let root = root("verify");
        package("src", "alpha", "1.0.0");
        fs::write_text("src/f.txt", "one").unwrap();
        fs::write_text("src/g.txt", "two").unwrap();
        install("src", "game", "alpha").unwrap();
        assert_eq!(verify_checksum("alpha").unwrap(), Vec::<String>::new());
        fs::write_text("game/f.txt", "tampered").unwrap();
        assert_eq!(verify_checksum("alpha").unwrap(), vec!["f.txt".to_owned()]);
        assert_eq!(
            verify_checksum("nothing").unwrap_err(),
            InstallError::NotInstalled("nothing".to_owned())
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_update_refuses_a_downgrade_unless_the_caller_asks() {
        let _serial = serial();
        fresh();
        let root = root("update");
        package("newer", "alpha", "2.0.0");
        fs::write_text("newer/g.txt", "v2").unwrap();
        install("newer", "game", "alpha").unwrap();
        assert_eq!(get_installed()[0].version, Version::parse("2.0.0"));
        package("older", "alpha", "1.0.0");
        fs::write_text("older/g.txt", "v1").unwrap();
        let refused = update("older", "alpha", false).unwrap_err();
        assert_eq!(
            refused,
            InstallError::Downgrade {
                from: "2.0.0".to_owned(),
                to: "1.0.0".to_owned(),
            }
        );
        assert_eq!(fs::read_text("game/g.txt").unwrap(), "v2", "nothing moved");
        assert!(update("older", "alpha", true).is_ok());
        assert_eq!(fs::read_text("game/g.txt").unwrap(), "v1");
        assert_eq!(get_installed()[0].version, Version::parse("1.0.0"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_clients_own_installation_is_refused() {
        let _serial = serial();
        fresh();
        assert!(!known_only_to_the_client::install_package("alpha"));
        assert!(!known_only_to_the_client::update_package("alpha"));
        assert!(!known_only_to_the_client::remove_package("alpha"));
        assert!(known_only_to_the_client::mods_directory().is_none());
        let rows = refusals();
        assert_eq!(rows.len(), 4);
        for (what, why) in rows {
            assert!(why.len() > 20, "{what} has no real reason: {why}");
            assert_ne!(why, format!("{what} is not an installer question this host can answer"));
        }
    }
}
