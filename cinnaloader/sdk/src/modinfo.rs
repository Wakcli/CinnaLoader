//! What a mod declares about itself, and what the loader reports back.
//!
//! A mod describes itself once, in [`Mod::load`](crate::Mod::load):
//!
//! ```
//! use cinnaloader::prelude::*;
//!
//! declare(
//!     ModManifest::new("sprint", "Sprint", env!("CARGO_PKG_VERSION"))
//!         .description("Sprint when you hold the key")
//!         .author("you")
//!         .license("MIT")
//!         .tag("movement")
//!         .category("Movement")
//!         .capability(Capability::PlayerRead),
//! );
//! ```
//!
//! The loader keeps that, adds the facts only it knows (where the library came from, its
//! state, its namespace) and hands the whole thing back as [`ModInfo`], which the accessors
//! at the bottom of this module read. Nothing here needs the client.

use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// The wire format the SDK and the loader agree on for a manifest.
pub const FORMAT: u32 = 1;
/// Longest manifest the loader accepts, in bytes.
pub const MAX_BYTES: usize = 4096;

/// Where a mod runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Environment {
    /// Only where the client hosts mods — this loader.
    #[default]
    Client,
    /// A dedicated server.
    Server,
    /// Both.
    Both,
}

impl Environment {
    /// The name the loader logs and the wire format uses.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Client => "client",
            Self::Server => "server",
            Self::Both => "both",
        }
    }
}

impl fmt::Display for Environment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for Environment {
    type Err = ();
    fn from_str(text: &str) -> Result<Self, ()> {
        match text {
            "client" => Ok(Self::Client),
            "server" => Ok(Self::Server),
            "both" => Ok(Self::Both),
            _ => Err(()),
        }
    }
}

/// How far along a mod is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum State {
    /// Discovered, not yet loaded.
    Discovered,
    /// Its `load` is running.
    Loading,
    /// Loaded and ticking.
    #[default]
    Loaded,
    /// Loaded but switched off, so its `tick` is skipped.
    Disabled,
    /// Refused, for instance on an ABI mismatch.
    Failed,
}

impl State {
    #[must_use]
    fn as_str(self) -> &'static str {
        match self {
            Self::Discovered => "discovered",
            Self::Loading => "loading",
            Self::Loaded => "loaded",
            Self::Disabled => "disabled",
            Self::Failed => "failed",
        }
    }

    /// Whether the mod is in play.
    #[must_use]
    pub fn is_active(self) -> bool {
        matches!(self, Self::Loaded)
    }
}

impl fmt::Display for State {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for State {
    type Err = ();
    fn from_str(text: &str) -> Result<Self, ()> {
        match text {
            "discovered" => Ok(Self::Discovered),
            "loading" => Ok(Self::Loading),
            "loaded" => Ok(Self::Loaded),
            "disabled" => Ok(Self::Disabled),
            "failed" => Ok(Self::Failed),
            _ => Err(()),
        }
    }
}

/// Something a mod needs the loader to allow. Declared, reported, and enforced by the
/// capability system as that lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    /// Read and write under its own directory.
    Files,
    /// Open sockets of its own.
    Network,
    /// Read local player state.
    PlayerRead,
    /// Read world and entity state.
    WorldRead,
    /// Draw into the HUD.
    Render,
    /// Observe keyboard and mouse beyond its declared keybinds.
    Input,
    /// Send or rewrite packets.
    NetworkIntercept,
    /// Change the world.
    WorldWrite,
}

impl Capability {
    #[must_use]
    fn as_str(self) -> &'static str {
        match self {
            Self::Files => "files",
            Self::Network => "network",
            Self::PlayerRead => "player_read",
            Self::WorldRead => "world_read",
            Self::Render => "render",
            Self::Input => "input",
            Self::NetworkIntercept => "network_intercept",
            Self::WorldWrite => "world_write",
        }
    }
}

impl FromStr for Capability {
    type Err = ();
    fn from_str(text: &str) -> Result<Self, ()> {
        match text {
            "files" => Ok(Self::Files),
            "network" => Ok(Self::Network),
            "player_read" => Ok(Self::PlayerRead),
            "world_read" => Ok(Self::WorldRead),
            "render" => Ok(Self::Render),
            "input" => Ok(Self::Input),
            "network_intercept" => Ok(Self::NetworkIntercept),
            "world_write" => Ok(Self::WorldWrite),
            _ => Err(()),
        }
    }
}

/// Something a mod may not do. The inverse of [`Capability`], for mods that spell out
/// what they will not touch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Permission {
    ReadFiles,
    WriteFiles,
    Network,
    PlayerState,
    WorldState,
    Render,
    Input,
}

impl Permission {
    #[must_use]
    fn as_str(self) -> &'static str {
        match self {
            Self::ReadFiles => "read_files",
            Self::WriteFiles => "write_files",
            Self::Network => "network",
            Self::PlayerState => "player_state",
            Self::WorldState => "world_state",
            Self::Render => "render",
            Self::Input => "input",
        }
    }
}

impl FromStr for Permission {
    type Err = ();
    fn from_str(text: &str) -> Result<Self, ()> {
        match text {
            "read_files" => Ok(Self::ReadFiles),
            "write_files" => Ok(Self::WriteFiles),
            "network" => Ok(Self::Network),
            "player_state" => Ok(Self::PlayerState),
            "world_state" => Ok(Self::WorldState),
            "render" => Ok(Self::Render),
            "input" => Ok(Self::Input),
            _ => Err(()),
        }
    }
}

/// A version, compared the way a mod author expects: `1.2` < `1.2.1` < `1.10`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModVersion {
    raw: String,
    parts: Vec<u64>,
    prerelease: Option<String>,
}

impl ModVersion {
    /// Parses `major.minor.patch[-pre]`; anything else keeps only its raw text.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let (numbers, prerelease) = match text.split_once('-') {
            Some((numbers, pre)) => (numbers, Some(pre.to_owned())),
            None => (text, None),
        };
        let parts: Option<Vec<u64>> = numbers
            .split('.')
            .map(|part| part.trim().parse::<u64>().ok())
            .collect();
        match parts {
            Some(parts) if !parts.is_empty() => Self {
                raw: text.to_owned(),
                parts,
                prerelease,
            },
            _ => Self {
                raw: text.to_owned(),
                parts: Vec::new(),
                prerelease,
            },
        }
    }

    #[must_use]
    pub fn major(&self) -> Option<u64> {
        self.parts.first().copied()
    }

    #[must_use]
    pub fn minor(&self) -> Option<u64> {
        self.parts.get(1).copied()
    }

    #[must_use]
    pub fn patch(&self) -> Option<u64> {
        self.parts.get(2).copied()
    }

    /// Whether the text looked like a version at all.
    #[must_use]
    pub fn is_numeric(&self) -> bool {
        !self.parts.is_empty()
    }

    /// Whether this version is at least `other`, so a mod can gate on a loader or a peer.
    #[must_use]
    pub fn at_least(&self, other: &Self) -> bool {
        self.cmp_parts(other) != std::cmp::Ordering::Less
    }

    fn cmp_parts(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        for index in 0..self.parts.len().max(other.parts.len()) {
            let left = self.parts.get(index).copied().unwrap_or(0);
            let right = other.parts.get(index).copied().unwrap_or(0);
            match left.cmp(&right) {
                Ordering::Equal => {}
                order => return order,
            }
        }
        match (&self.prerelease, &other.prerelease) {
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (Some(left), Some(right)) => left.cmp(right),
            (None, None) => Ordering::Equal,
        }
    }
}

impl fmt::Display for ModVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.raw)
    }
}

impl PartialOrd for ModVersion {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp_parts(other))
    }
}

/// Which build produced the mod, filled in by the crate that built it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BuildInfo {
    pub number: Option<u64>,
    pub git_commit: Option<String>,
    pub timestamp: Option<u64>,
    pub development: bool,
}

/// Where a mod's own files live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModDirectory {
    root: PathBuf,
}

impl ModDirectory {
    /// The mods directory the loader loaded this mod from.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where the mod may keep data that must survive a restart.
    #[must_use]
    pub fn data(&self) -> PathBuf {
        self.root.join("data")
    }

    /// Where the mod may keep anything it can rebuild.
    #[must_use]
    pub fn cache(&self) -> PathBuf {
        self.root.join("cache")
    }

    /// Where the mod may keep its settings.
    #[must_use]
    pub fn config(&self) -> PathBuf {
        self.root.clone()
    }

    /// Where the mod may keep art and other client assets.
    #[must_use]
    pub fn assets(&self) -> PathBuf {
        self.root.join("assets")
    }

    /// Where the mod may keep everything else it ships.
    #[must_use]
    pub fn resources(&self) -> PathBuf {
        self.root.join("resources")
    }

    /// Creates the directory and the subdirectories a mod may use.
    pub fn create(&self) -> std::io::Result<()> {
        for path in [
            self.data(),
            self.cache(),
            self.assets(),
            self.resources(),
        ] {
            std::fs::create_dir_all(path)?;
        }
        Ok(())
    }
}

/// The `<mod name>:<short id>` scope every setting and command id lives in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModNamespace {
    mod_name: String,
}

impl ModNamespace {
    #[must_use]
    pub fn new(mod_name: impl Into<String>) -> Self {
        Self {
            mod_name: mod_name.into(),
        }
    }

    /// The mod's own name.
    #[must_use]
    pub fn mod_name(&self) -> &str {
        &self.mod_name
    }

    /// Scopes a short id, the form the loader registers and reports.
    #[must_use]
    pub fn scope(&self, id: &str) -> String {
        format!("{}:{}", self.mod_name, id)
    }

    /// Removes the scope from an id this namespace owns, or returns it unchanged.
    #[must_use]
    pub fn unscope<'a>(&self, id: &'a str) -> &'a str {
        match id.split_once(':') {
            Some((prefix, rest)) if prefix == self.mod_name => rest,
            _ => id,
        }
    }
}

/// The symbols a mod exports, as the loader resolves them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModEntrypoint {
    pub abi_version: String,
    pub init: String,
    pub frame: String,
    pub shutdown: String,
}

impl Default for ModEntrypoint {
    fn default() -> Self {
        Self {
            abi_version: "cinnaloader_abi_version".to_owned(),
            init: "cinnaloader_init".to_owned(),
            frame: "cinnaloader_frame".to_owned(),
            shutdown: "cinnaloader_shutdown".to_owned(),
        }
    }
}

/// A loaded mod, as the loader sees it: its id and the library it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModInstance {
    pub id: String,
    pub library: Option<PathBuf>,
}

/// What a mod declares about itself. Built in `load` and handed to [`declare`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModManifest {
    pub format: u32,
    pub id: String,
    pub name: String,
    pub version: ModVersion,
    pub description: String,
    pub authors: Vec<String>,
    pub contributors: Vec<String>,
    pub website: Option<String>,
    pub source: Option<String>,
    pub license: Option<String>,
    pub icon: Option<String>,
    pub credits: Vec<String>,
    pub tags: Vec<String>,
    pub category: Option<String>,
    pub environment: Environment,
    pub capabilities: Vec<Capability>,
    pub permissions: Vec<Permission>,
    pub build: BuildInfo,
}

impl ModManifest {
    /// Starts a manifest. `id` is the short name a mod uses in its own ids; `name` is what
    /// the user sees.
    #[must_use]
    pub fn new(id: &str, name: &str, version: &str) -> Self {
        Self {
            format: FORMAT,
            id: id.to_owned(),
            name: name.to_owned(),
            version: ModVersion::parse(version),
            description: String::new(),
            authors: Vec::new(),
            contributors: Vec::new(),
            website: None,
            source: None,
            license: None,
            icon: None,
            credits: Vec::new(),
            tags: Vec::new(),
            category: None,
            environment: Environment::Client,
            capabilities: Vec::new(),
            permissions: Vec::new(),
            build: BuildInfo {
                development: cfg!(debug_assertions),
                ..BuildInfo::default()
            },
        }
    }

    #[must_use]
    pub fn description(mut self, description: &str) -> Self {
        self.description = description.to_owned();
        self
    }

    #[must_use]
    pub fn author(mut self, author: &str) -> Self {
        self.authors.push(author.to_owned());
        self
    }

    #[must_use]
    pub fn contributors(mut self, contributors: &[&str]) -> Self {
        self.contributors
            .extend(contributors.iter().map(|name| (*name).to_owned()));
        self
    }

    #[must_use]
    pub fn website(mut self, url: &str) -> Self {
        self.website = Some(url.to_owned());
        self
    }

    #[must_use]
    pub fn source(mut self, url: &str) -> Self {
        self.source = Some(url.to_owned());
        self
    }

    #[must_use]
    pub fn license(mut self, license: &str) -> Self {
        self.license = Some(license.to_owned());
        self
    }

    #[must_use]
    pub fn icon(mut self, icon: &str) -> Self {
        self.icon = Some(icon.to_owned());
        self
    }

    #[must_use]
    pub fn credit(mut self, credit: &str) -> Self {
        self.credits.push(credit.to_owned());
        self
    }

    #[must_use]
    pub fn tag(mut self, tag: &str) -> Self {
        self.tags.push(tag.to_owned());
        self
    }

    #[must_use]
    pub fn category(mut self, category: &str) -> Self {
        self.category = Some(category.to_owned());
        self
    }

    #[must_use]
    pub fn environment(mut self, environment: Environment) -> Self {
        self.environment = environment;
        self
    }

    #[must_use]
    pub fn capability(mut self, capability: Capability) -> Self {
        self.capabilities.push(capability);
        self
    }

    #[must_use]
    pub fn permission(mut self, permission: Permission) -> Self {
        self.permissions.push(permission);
        self
    }

    #[must_use]
    pub fn build(mut self, build: BuildInfo) -> Self {
        self.build = build;
        self
    }

    /// Where this mod expects to run, as text.
    #[must_use]
    pub fn environment_name(&self) -> &'static str {
        self.environment.as_str()
    }

    /// The manifest as the loader reads it.
    #[must_use]
    pub fn encode(&self) -> String {
        let mut out = String::new();
        let mut push = |key: &str, value: &str| {
            out.push_str(key);
            out.push('=');
            out.push_str(&escape(value));
            out.push('\n');
        };
        push("v", &self.format.to_string());
        push("id", &self.id);
        push("name", &self.name);
        push("version", &self.version.to_string());
        push("description", &self.description);
        for author in &self.authors {
            push("author", author);
        }
        for contributor in &self.contributors {
            push("contributor", contributor);
        }
        for credit in &self.credits {
            push("credits", credit);
        }
        for tag in &self.tags {
            push("tag", tag);
        }
        if let Some(website) = &self.website {
            push("website", website);
        }
        if let Some(source) = &self.source {
            push("source", source);
        }
        if let Some(license) = &self.license {
            push("license", license);
        }
        if let Some(icon) = &self.icon {
            push("icon", icon);
        }
        if let Some(category) = &self.category {
            push("category", category);
        }
        push("environment", self.environment.as_str());
        for capability in &self.capabilities {
            push("capability", capability.as_str());
        }
        for permission in &self.permissions {
            push("permission", permission.as_str());
        }
        if let Some(number) = self.build.number {
            push("build_number", &number.to_string());
        }
        if let Some(commit) = &self.build.git_commit {
            push("git_commit", commit);
        }
        if let Some(timestamp) = self.build.timestamp {
            push("build_timestamp", &timestamp.to_string());
        }
        push("dev", u8::from(self.build.development).to_string().as_str());
        out
    }

    /// Parses what [`Self::encode`] writes. Unknown keys are kept out of the way rather
    /// than rejected, so a newer SDK still loads on an older loader.
    #[must_use]
    pub fn decode(text: &str) -> Self {
        let mut manifest = Self::new("mod", "mod", "0");
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = unescape(value);
            let text = value.as_str();
            match key {
                "v" => manifest.format = text.parse().unwrap_or(FORMAT),
                "id" => manifest.id = value,
                "name" => manifest.name = value,
                "version" => manifest.version = ModVersion::parse(text),
                "description" => manifest.description = value,
                "author" => manifest.authors.push(value),
                "contributor" => manifest.contributors.push(value),
                "credits" => manifest.credits.push(value),
                "tag" => manifest.tags.push(value),
                "website" => manifest.website = Some(value),
                "source" => manifest.source = Some(value),
                "license" => manifest.license = Some(value),
                "icon" => manifest.icon = Some(value),
                "category" => manifest.category = Some(value),
                "environment" => {
                    manifest.environment = text.parse().unwrap_or_default();
                }
                "capability" => {
                    if let Ok(capability) = text.parse() {
                        manifest.capabilities.push(capability);
                    }
                }
                "permission" => {
                    if let Ok(permission) = text.parse() {
                        manifest.permissions.push(permission);
                    }
                }
                "build_number" => manifest.build.number = text.parse().ok(),
                "git_commit" => manifest.build.git_commit = Some(value),
                "build_timestamp" => manifest.build.timestamp = text.parse().ok(),
                "dev" => manifest.build.development = text == "1" || text == "true",
                _ => {}
            }
        }
        manifest
    }
}

/// The loader's answer for one mod: what it declared, plus what only the loader knows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModInfo {
    pub manifest: ModManifest,
    /// The directory the loader loaded this mod from.
    pub directory: Option<PathBuf>,
    /// The `mod_name` the loader knows this mod by, which scopes its ids.
    pub mod_name: String,
    pub state: State,
    pub enabled: bool,
    pub reloading: bool,
    pub library: Option<PathBuf>,
    pub entrypoint: ModEntrypoint,
    /// The loader's own version, for a mod that needs to gate on it.
    pub loader_version: String,
}

/// The loader's answer, as one line per field. See [`ModInfo::decode`].
impl ModInfo {
    /// The namespace every one of this mod's ids lives in.
    #[must_use]
    pub fn namespace(&self) -> ModNamespace {
        ModNamespace::new(self.mod_name.clone())
    }

    /// The mod's own directories.
    #[must_use]
    pub fn directories(&self) -> Option<ModDirectory> {
        self.directory.clone().map(ModDirectory::new)
    }

    /// The mod, as the loader holds it.
    #[must_use]
    pub fn instance(&self) -> ModInstance {
        ModInstance {
            id: self.manifest.id.clone(),
            library: self.library.clone(),
        }
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.manifest.id
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.manifest.name
    }

    #[must_use]
    pub fn version(&self) -> &ModVersion {
        &self.manifest.version
    }

    pub fn encode(&self) -> String {
        let mut out = self.manifest.encode();
        let mut push = |key: &str, value: &str| {
            out.push_str(key);
            out.push('=');
            out.push_str(&escape(value));
            out.push('\n');
        };
        if let Some(directory) = &self.directory {
            push("directory", &directory.to_string_lossy());
        }
        push("mod_name", &self.mod_name);
        push("state", self.state.as_str());
        push("enabled", u8::from(self.enabled).to_string().as_str());
        push("reloading", u8::from(self.reloading).to_string().as_str());
        if let Some(library) = &self.library {
            push("library", &library.to_string_lossy());
        }
        push("entrypoint", &self.entrypoint.init);
        push("loader_version", &self.loader_version);
        out
    }

    #[must_use]
    pub fn decode(text: &str) -> Self {
        let manifest = ModManifest::decode(text);
        let mut info = Self {
            manifest,
            directory: None,
            mod_name: String::new(),
            state: State::Loaded,
            enabled: true,
            reloading: false,
            library: None,
            entrypoint: ModEntrypoint::default(),
            loader_version: String::new(),
        };
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = unescape(value);
            match key {
                "directory" => info.directory = Some(PathBuf::from(&value)),
                "mod_name" => info.mod_name = value,
                "state" => info.state = value.parse().unwrap_or(State::Loaded),
                "enabled" => info.enabled = value == "1" || value == "true",
                "reloading" => info.reloading = value == "1" || value == "true",
                "library" => info.library = Some(PathBuf::from(&value)),
                "loader_version" => info.loader_version = value,
                _ => {}
            }
        }
        if info.mod_name.is_empty() {
            info.mod_name = info.manifest.id.clone();
        }
        info
    }
}

/// Everything the loader knows, in one type. An alias so both names in the wishlist exist.
pub type ModMetadata = ModInfo;

/// What a mod needs while it runs: who it is, where it may write, how it stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModContext {
    pub info: ModInfo,
}

impl ModContext {
    #[must_use]
    pub fn id(&self) -> &str {
        self.info.id()
    }

    #[must_use]
    pub fn namespace(&self) -> ModNamespace {
        self.info.namespace()
    }

    #[must_use]
    pub fn directories(&self) -> Option<ModDirectory> {
        self.info.directories()
    }

    #[must_use]
    pub fn state(&self) -> State {
        self.info.state
    }

    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.info.enabled
    }
}

/// Escapes the characters that would break the one-value-per-line form.
fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            other => out.push(other),
        }
    }
    out
}

fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut characters = value.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            out.push(character);
            continue;
        }
        match characters.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_manifest_survives_a_round_trip() {
        let manifest = ModManifest::new("sprint", "Sprint", "1.2.0")
            .description("Sprint when you hold the key")
            .author("you")
            .author("someone else")
            .contributors(&["a", "b"])
            .credit("inspired by a mod")
            .website("https://example.com")
            .source("https://example.com/src")
            .license("MIT")
            .icon("icon.png")
            .tag("movement")
            .tag("client")
            .category("Movement")
            .environment(Environment::Client)
            .capability(Capability::PlayerRead)
            .capability(Capability::Files)
            .permission(Permission::WorldState)
            .build(BuildInfo {
                number: Some(42),
                git_commit: Some("abc123".to_owned()),
                timestamp: Some(1_700_000_000),
                development: true,
            });
        let decoded = ModManifest::decode(&manifest.encode());
        assert_eq!(decoded, manifest);
    }

    #[test]
    fn a_value_may_contain_the_separators() {
        let manifest = ModManifest::new("odd", "Odd", "0.1")
            .description("line one\nline two\ttabbed\\slash");
        let decoded = ModManifest::decode(&manifest.encode());
        assert_eq!(
            decoded.description,
            "line one\nline two\ttabbed\\slash",
            "newlines and backslashes survive"
        );
    }

    #[test]
    fn an_unknown_key_is_ignored_rather_than_fatal() {
        let decoded = ModManifest::decode("v=1\nid=x\nname=X\nversion=1.0\nfuture_thing=1\n");
        assert_eq!(decoded.id, "x");
        assert_eq!(decoded.name, "X");
    }

    #[test]
    fn versions_compare_numerically() {
        let one_two = ModVersion::parse("1.2");
        let one_two_one = ModVersion::parse("1.2.1");
        let one_ten = ModVersion::parse("1.10");
        assert!(one_two < one_two_one);
        assert!(one_two_one < one_ten, "1.2.1 < 1.10, not string order");
        assert!(one_ten.at_least(&one_two));
        assert!(!one_two.at_least(&one_ten));
        assert_eq!(ModVersion::parse("2.3.4").major(), Some(2));
        assert_eq!(ModVersion::parse("2.3.4").minor(), Some(3));
        assert_eq!(ModVersion::parse("2.3.4").patch(), Some(4));
        assert!(!ModVersion::parse("nightly").is_numeric());
    }

    #[test]
    fn a_prerelease_sorts_before_its_release() {
        assert!(ModVersion::parse("1.0.0-beta") < ModVersion::parse("1.0.0"));
    }

    #[test]
    fn the_loader_view_carries_the_facts_the_mod_cannot_know() {
        let info = ModInfo {
            manifest: ModManifest::new("sprint", "Sprint", "1.0"),
            directory: Some(PathBuf::from("/mods/sprint")),
            mod_name: "sprint".to_owned(),
            state: State::Disabled,
            enabled: false,
            reloading: false,
            library: Some(PathBuf::from("/mods/libsprint.so")),
            entrypoint: ModEntrypoint::default(),
            loader_version: "0.1.0".to_owned(),
        };
        let decoded = ModInfo::decode(&info.encode());
        assert_eq!(decoded.state, State::Disabled);
        assert!(!decoded.enabled);
        assert_eq!(decoded.namespace().scope("speed"), "sprint:speed");
        assert_eq!(decoded.namespace().unscope("sprint:speed"), "speed");
        assert_eq!(
            decoded.directories().unwrap().data(),
            PathBuf::from("/mods/sprint/data")
        );
        assert_eq!(decoded.instance().id, "sprint");
        assert_eq!(decoded.loader_version, "0.1.0");
    }
}
