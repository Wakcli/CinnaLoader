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

use crate::deps::{DependencyKind, ModConflict, ModDependency};

pub use crate::capability::Capability;
use crate::version::{VersionRange, VersionReq};

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

/// Something a mod may not do. The inverse of [`Capability`], for mods that spell out
/// what they will not touch. The set is the wishlist's, and the seven this loader used
/// before it are still read from a manifest that spells them the old way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Permission {
    // game state
    ReadGameState,
    ModifyGameState,
    ReadWorld,
    ModifyWorld,
    ReadPlayerData,
    ModifyPlayerData,
    ReadInventory,
    ModifyInventory,
    // networking
    SendNetworkMessages,
    InterceptPackets,
    ModifyPackets,
    UseSockets,
    // files
    ReadFiles,
    WriteFiles,
    AccessExternalFiles,
    // the process
    LaunchProcesses,
    LoadNativeLibraries,
    AccessMemory,
    AccessSystemInformation,
    // the display and input
    ModifyRendering,
    // commands
    RegisterCommands,
    ExecuteCommands,
    // the desktop
    AccessClipboard,
    OpenUrls,
    // what this loader used to call the same things
    Network,
    PlayerState,
    WorldState,
    Render,
    Input,
}

/// Every permission this loader knows about.
pub const ALL_PERMISSIONS: [Permission; 29] = [
    Permission::ReadGameState,
    Permission::ModifyGameState,
    Permission::ReadWorld,
    Permission::ModifyWorld,
    Permission::ReadPlayerData,
    Permission::ModifyPlayerData,
    Permission::ReadInventory,
    Permission::ModifyInventory,
    Permission::SendNetworkMessages,
    Permission::InterceptPackets,
    Permission::ModifyPackets,
    Permission::UseSockets,
    Permission::ReadFiles,
    Permission::WriteFiles,
    Permission::AccessExternalFiles,
    Permission::LaunchProcesses,
    Permission::LoadNativeLibraries,
    Permission::AccessMemory,
    Permission::AccessSystemInformation,
    Permission::ModifyRendering,
    Permission::RegisterCommands,
    Permission::ExecuteCommands,
    Permission::AccessClipboard,
    Permission::OpenUrls,
    Permission::Network,
    Permission::PlayerState,
    Permission::WorldState,
    Permission::Render,
    Permission::Input,
];

impl fmt::Display for Permission {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Permission {
    /// The name the wire format uses.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadGameState => "read_game_state",
            Self::ModifyGameState => "modify_game_state",
            Self::ReadWorld => "read_world",
            Self::ModifyWorld => "modify_world",
            Self::ReadPlayerData => "read_player_data",
            Self::ModifyPlayerData => "modify_player_data",
            Self::ReadInventory => "read_inventory",
            Self::ModifyInventory => "modify_inventory",
            Self::SendNetworkMessages => "send_network_messages",
            Self::InterceptPackets => "intercept_packets",
            Self::ModifyPackets => "modify_packets",
            Self::UseSockets => "use_sockets",
            Self::ReadFiles => "read_files",
            Self::WriteFiles => "write_files",
            Self::AccessExternalFiles => "access_external_files",
            Self::LaunchProcesses => "launch_processes",
            Self::LoadNativeLibraries => "load_native_libraries",
            Self::AccessMemory => "access_memory",
            Self::AccessSystemInformation => "access_system_information",
            Self::ModifyRendering => "modify_rendering",
            Self::RegisterCommands => "register_commands",
            Self::ExecuteCommands => "execute_commands",
            Self::AccessClipboard => "access_clipboard",
            Self::OpenUrls => "open_urls",
            Self::Network => "network",
            Self::PlayerState => "player_state",
            Self::WorldState => "world_state",
            Self::Render => "render",
            Self::Input => "input",
        }
    }

    /// What it lets a mod do, in one line, for a panel that asks the user.
    #[must_use]
    pub fn describe(self) -> &'static str {
        match self {
            Self::ReadGameState => "read the game's state: time, weather, mode",
            Self::ModifyGameState => "change the game's state",
            Self::ReadWorld => "read the world",
            Self::ModifyWorld => "change the world",
            Self::ReadPlayerData => "read the local player's data",
            Self::ModifyPlayerData => "change the local player's data",
            Self::ReadInventory => "read inventories",
            Self::ModifyInventory => "change inventories",
            Self::SendNetworkMessages => "send network messages",
            Self::InterceptPackets => "watch packets",
            Self::ModifyPackets => "rewrite packets",
            Self::UseSockets => "open sockets",
            Self::ReadFiles => "read files",
            Self::WriteFiles => "write files",
            Self::AccessExternalFiles => "reach outside its own directory",
            Self::LaunchProcesses => "start other programs",
            Self::LoadNativeLibraries => "load more native code",
            Self::AccessMemory => "read and write the process's memory",
            Self::AccessSystemInformation => "read system information",
            Self::ModifyRendering => "change how the game draws",
            Self::RegisterCommands => "register commands",
            Self::ExecuteCommands => "run commands",
            Self::AccessClipboard => "read and write the clipboard",
            Self::OpenUrls => "open links",
            Self::Network => "send and receive network traffic",
            Self::PlayerState => "read the local player's state",
            Self::WorldState => "read the world's state",
            Self::Render => "draw into the HUD",
            Self::Input => "read input",
        }
    }

    /// Whether it is the same as the permission a mod spelled an older way.
    #[must_use]
    pub fn canonical(self) -> Self {
        match self {
            Self::Network => Self::UseSockets,
            Self::PlayerState => Self::ReadPlayerData,
            Self::WorldState => Self::ReadGameState,
            Self::Render => Self::ModifyRendering,
            Self::Input => Self::ReadGameState,
            other => other,
        }
    }
}

impl FromStr for Permission {
    type Err = ();
    fn from_str(text: &str) -> Result<Self, ()> {
        let text = text.trim().to_ascii_lowercase();
        ALL_PERMISSIONS
            .iter()
            .copied()
            .find(|permission| permission.as_str() == text)
            .ok_or(())
    }
}

/// A version, compared the way a mod author expects: `1.2` == `1.2.0`, `1.2` < `1.2.1`, and
/// `1.2.1` < `1.10`. Equality is numeric, so the same version written two ways matches.
#[derive(Debug, Clone)]
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

    /// The numeric comparison two versions are ordered and equated by.
    #[must_use]
    pub fn cmp_parts(&self, other: &Self) -> std::cmp::Ordering {
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

impl PartialEq for ModVersion {
    fn eq(&self, other: &Self) -> bool {
        self.cmp_parts(other) == std::cmp::Ordering::Equal
    }
}

impl Eq for ModVersion {}

impl PartialOrd for ModVersion {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ModVersion {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.cmp_parts(other)
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
    /// The trust level the mod asks for. The loader clamps it: see [`crate::permissions`].
    pub trust: crate::permissions::TrustLevel,
    pub build: BuildInfo,
    /// Loader versions this mod accepts.
    pub requires_loader: VersionReq,
    /// The lowest ABI version this mod needs.
    pub requires_api: u32,
    /// Game versions this mod accepts.
    pub requires_game: VersionReq,
    /// The game versions this mod was tried against, for a report.
    pub supported_game: VersionRange,
    /// A platform this mod needs, when it only runs on one.
    pub requires_platform: Option<String>,
    /// Capabilities or features the mod cannot run without.
    pub required_features: Vec<String>,
    /// Capabilities or features the mod can live without.
    pub optional_features: Vec<String>,
    /// Mods this one needs, optionally, or would like.
    pub dependencies: Vec<ModDependency>,
    /// Ids this mod must be loaded after.
    pub load_after: Vec<String>,
    /// Ids this mod must be loaded before.
    pub load_before: Vec<String>,
    /// Mods this one will not run beside.
    pub conflicts: Vec<ModConflict>,
    /// Ids this mod satisfies, such as a library it embeds.
    pub provides: Vec<String>,
    /// Ids this mod stands in for.
    pub replaces: Vec<String>,
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
            trust: crate::permissions::TrustLevel::default(),
            build: BuildInfo {
                development: cfg!(debug_assertions),
                ..BuildInfo::default()
            },
            requires_loader: VersionReq::default(),
            requires_api: 0,
            requires_game: VersionReq::default(),
            supported_game: VersionRange::default(),
            requires_platform: None,
            required_features: Vec::new(),
            optional_features: Vec::new(),
            dependencies: Vec::new(),
            load_after: Vec::new(),
            load_before: Vec::new(),
            conflicts: Vec::new(),
            provides: Vec::new(),
            replaces: Vec::new(),
        }
    }

    /// A mod this one cannot run without.
    #[must_use]
    pub fn depends_on(mut self, id: &str) -> Self {
        self.dependencies.push(ModDependency::new(id));
        self
    }

    /// A mod this one needs at a version it names, as `>=1.2`.
    #[must_use]
    pub fn depends_on_versioned(mut self, id: &str, requirement: &str) -> Self {
        self.dependencies
            .push(ModDependency::new(id).versioned(requirement));
        self
    }

    /// A mod this one works without, and would rather not.
    #[must_use]
    pub fn depends_on_optional(mut self, id: &str) -> Self {
        self.dependencies
            .push(ModDependency::new(id).kind(DependencyKind::Optional));
        self
    }

    /// A mod this one would like, and can do without.
    #[must_use]
    pub fn recommends(mut self, id: &str) -> Self {
        self.dependencies
            .push(ModDependency::new(id).kind(DependencyKind::Recommended));
        self
    }

    /// A mod this one mentions, nothing more.
    #[must_use]
    pub fn suggests(mut self, id: &str) -> Self {
        self.dependencies
            .push(ModDependency::new(id).kind(DependencyKind::Suggested));
        self
    }

    /// Load after `id`, without depending on it.
    #[must_use]
    pub fn load_after(mut self, id: &str) -> Self {
        self.load_after.push(id.to_owned());
        self
    }

    /// Load before `id`, without depending on it.
    #[must_use]
    pub fn load_before(mut self, id: &str) -> Self {
        self.load_before.push(id.to_owned());
        self
    }

    /// Refuse to run beside `id`.
    #[must_use]
    pub fn conflicts_with(mut self, id: &str) -> Self {
        self.conflicts.push(ModConflict::new(id, true));
        self
    }

    /// Known to misbehave beside `id`, but not fatal.
    #[must_use]
    pub fn breaks_with(mut self, id: &str) -> Self {
        self.conflicts.push(ModConflict::new(id, false));
        self
    }

    /// Satisfies `id`, such as a library this mod ships.
    #[must_use]
    pub fn provides(mut self, id: &str) -> Self {
        self.provides.push(id.to_owned());
        self
    }

    /// Stands in for `id`.
    #[must_use]
    pub fn replaces(mut self, id: &str) -> Self {
        self.replaces.push(id.to_owned());
        self
    }

    /// Ships `id` inside itself, so a dependency on it is satisfied.
    #[must_use]
    pub fn bundles(mut self, id: &str) -> Self {
        self.provides.push(id.to_owned());
        self
    }

    /// Compiles `id` into itself, so a dependency on it is satisfied.
    #[must_use]
    pub fn embeds(mut self, id: &str) -> Self {
        self.provides.push(id.to_owned());
        self
    }

    /// The loader versions this mod accepts, as `>=0.1, <1`.
    #[must_use]
    pub fn requires_loader(mut self, requirement: &str) -> Self {
        self.requires_loader = VersionReq::parse(requirement).unwrap_or_default();
        self
    }

    /// The lowest ABI version this mod needs.
    #[must_use]
    pub fn requires_api(mut self, api: u32) -> Self {
        self.requires_api = api;
        self
    }

    /// The game versions this mod accepts, as `>=1.26, <1.27`.
    #[must_use]
    pub fn requires_game(mut self, requirement: &str) -> Self {
        self.requires_game = VersionReq::parse(requirement).unwrap_or_default();
        self
    }

    /// The game versions this mod was tried against, as a range.
    #[must_use]
    pub fn supports_game(mut self, minimum: &str, maximum: &str) -> Self {
        self.supported_game = VersionRange::new(
            Some(ModVersion::parse(minimum)),
            Some(ModVersion::parse(maximum)),
        );
        self
    }

    /// Only run on `platform`, as `std::env::consts::OS` spells it.
    #[must_use]
    pub fn requires_platform(mut self, platform: &str) -> Self {
        self.requires_platform = Some(platform.to_owned());
        self
    }

    /// A capability or feature the mod cannot run without.
    #[must_use]
    pub fn requires_feature(mut self, feature: &str) -> Self {
        self.required_features.push(feature.to_owned());
        self
    }

    /// A capability or feature the mod can live without.
    #[must_use]
    /// The same as [`ModManifest::requires_mod`]: this mod needs `id` before it.
    pub fn requires_mod(self, id: &str) -> Self {
        self.depends_on(id)
    }

    /// A capability this mod cannot run without, such as `"files"`. The loader checks it
    /// against what the host supports and refuses the mod when it is missing.
    pub fn requires_capability(self, capability: &str) -> Self {
        self.requires_feature(capability)
    }

    pub fn optional_feature(mut self, feature: &str) -> Self {
        self.optional_features.push(feature.to_owned());
        self
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

    /// Asks for a trust level. The loader clamps anything above [`TrustLevel::Normal`],
    /// because a mod asking to be trusted is exactly the thing not to trust.
    pub fn trust(mut self, trust: crate::permissions::TrustLevel) -> Self {
        self.trust = trust;
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

    /// Folds a second declaration into this one: what the overlay states wins, and lists
    /// are joined rather than replaced, so a `load` that adds one dependency keeps
    /// everything the manifest declared at build time.
    #[must_use]
    pub fn amended_with(&self, overlay: &Self) -> Self {
        fn join<T: PartialEq + Clone>(base: &[T], extra: &[T]) -> Vec<T> {
            let mut joined = base.to_vec();
            for item in extra {
                if !joined.contains(item) {
                    joined.push(item.clone());
                }
            }
            joined
        }
        fn prefer(base: &str, overlay: &str) -> String {
            if overlay.is_empty() {
                base.to_owned()
            } else {
                overlay.to_owned()
            }
        }
        Self {
            format: self.format.max(overlay.format),
            id: overlay.id.clone(),
            name: prefer(&self.name, &overlay.name),
            version: overlay.version.clone(),
            description: prefer(&self.description, &overlay.description),
            authors: join(&self.authors, &overlay.authors),
            contributors: join(&self.contributors, &overlay.contributors),
            website: overlay.website.clone().or_else(|| self.website.clone()),
            source: overlay.source.clone().or_else(|| self.source.clone()),
            license: overlay.license.clone().or_else(|| self.license.clone()),
            icon: overlay.icon.clone().or_else(|| self.icon.clone()),
            credits: join(&self.credits, &overlay.credits),
            tags: join(&self.tags, &overlay.tags),
            category: overlay.category.clone().or_else(|| self.category.clone()),
            environment: overlay.environment,
            capabilities: join(&self.capabilities, &overlay.capabilities),
            permissions: join(&self.permissions, &overlay.permissions),
            trust: self.trust.max(overlay.trust),
            build: overlay.build.clone(),
            requires_loader: overlay.requires_loader.clone(),
            requires_api: self.requires_api.max(overlay.requires_api),
            requires_game: overlay.requires_game.clone(),
            supported_game: overlay.supported_game.clone(),
            requires_platform: overlay
                .requires_platform
                .clone()
                .or_else(|| self.requires_platform.clone()),
            required_features: join(&self.required_features, &overlay.required_features),
            optional_features: join(&self.optional_features, &overlay.optional_features),
            dependencies: join(&self.dependencies, &overlay.dependencies),
            load_after: join(&self.load_after, &overlay.load_after),
            load_before: join(&self.load_before, &overlay.load_before),
            conflicts: join(&self.conflicts, &overlay.conflicts),
            provides: join(&self.provides, &overlay.provides),
            replaces: join(&self.replaces, &overlay.replaces),
        }
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
        push("trust", self.trust.as_str());
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
        if !self.requires_loader.is_any() {
            push("requires_loader", &self.requires_loader.to_string());
        }
        if self.requires_api > 0 {
            push("requires_api", &self.requires_api.to_string());
        }
        if !self.requires_game.is_any() {
            push("requires_game", &self.requires_game.to_string());
        }
        if let Some(min) = &self.supported_game.min {
            push("supports_game_min", &min.to_string());
        }
        if let Some(max) = &self.supported_game.max {
            push("supports_game_max", &max.to_string());
        }
        if let Some(platform) = &self.requires_platform {
            push("requires_platform", platform);
        }
        for feature in &self.required_features {
            push("requires_feature", feature);
        }
        for feature in &self.optional_features {
            push("optional_feature", feature);
        }
        for dependency in &self.dependencies {
            push("depends", &dependency.encode());
        }
        for id in &self.load_after {
            push("load_after", id);
        }
        for id in &self.load_before {
            push("load_before", id);
        }
        for conflict in &self.conflicts {
            push("conflict", &conflict.encode());
        }
        for id in &self.provides {
            push("provides", id);
        }
        for id in &self.replaces {
            push("replaces", id);
        }
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
                "trust" => {
                    if let Ok(trust) = text.parse() {
                        manifest.trust = trust;
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
                "requires_loader" => {
                    manifest.requires_loader = VersionReq::parse(text).unwrap_or_default();
                }
                "requires_api" => manifest.requires_api = text.parse().unwrap_or(0),
                "requires_game" => {
                    manifest.requires_game = VersionReq::parse(text).unwrap_or_default();
                }
                "supports_game_min" => {
                    manifest.supported_game.min = Some(ModVersion::parse(text));
                }
                "supports_game_max" => {
                    manifest.supported_game.max = Some(ModVersion::parse(text));
                }
                "requires_platform" => manifest.requires_platform = Some(value),
                "requires_feature" => manifest.required_features.push(value),
                "optional_feature" => manifest.optional_features.push(value),
                "depends" => {
                    if let Some(dependency) = ModDependency::decode(text) {
                        manifest.dependencies.push(dependency);
                    }
                }
                "load_after" => manifest.load_after.push(value),
                "load_before" => manifest.load_before.push(value),
                "conflict" => {
                    if let Some(conflict) = ModConflict::decode(text) {
                        manifest.conflicts.push(conflict);
                    }
                }
                "provides" => manifest.provides.push(value),
                "replaces" => manifest.replaces.push(value),
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
    /// The trust level the loader enforces, which is what a mod is held to.
    pub trust: crate::permissions::TrustLevel,
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
        push("trust", self.trust.as_str());
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
            trust: crate::permissions::TrustLevel::default(),
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
                "trust" => {
                    if let Ok(trust) = value.parse() {
                        info.trust = trust;
                    }
                }
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

/// What the host is: the loader, the game, and what the process can provide. A mod reads
/// this through [`crate::compat`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HostFacts {
    /// The game version, empty when the loader could not tell.
    pub game_version: String,
    /// The client's build, when it reports one.
    pub game_build: String,
    /// `bedrock`.
    pub game_edition: String,
    /// The platform, as `std::env::consts::OS` spells it.
    pub game_platform: String,
    /// The game's protocol number, when it is known.
    pub game_protocol: Option<u32>,
    /// The ABI version the loader provides.
    pub api_version: u32,
    /// The loader's own version.
    pub loader_version: String,
    /// Extra features the loader can provide, beyond [`crate::compat::loader_capabilities`].
    pub features: Vec<String>,
    /// The address the client was launched with, when there was one.
    pub launch_address: Option<String>,
    /// The window the client is drawing into, in pixels.
    pub window: Option<(u32, u32)>,
}

impl HostFacts {
    /// The facts as the loader reports them, one `key=value` per line.
    #[must_use]
    pub fn encode(&self) -> String {
        let mut out = String::new();
        let mut push = |key: &str, value: &str| {
            out.push_str(key);
            out.push('=');
            out.push_str(&escape(value));
            out.push('\n');
        };
        push("host_game_version", &self.game_version);
        push("host_game_build", &self.game_build);
        push("host_game_edition", &self.game_edition);
        push("host_game_platform", &self.game_platform);
        if let Some(protocol) = self.game_protocol {
            push("host_game_protocol", &protocol.to_string());
        }
        push("host_api_version", &self.api_version.to_string());
        push("host_loader_version", &self.loader_version);
        for feature in &self.features {
            push("host_feature", feature);
        }
        if let Some(address) = &self.launch_address {
            push("host_launch_address", address);
        }
        if let Some((width, height)) = self.window {
            push("host_window", &format!("{width}x{height}"));
        }
        out
    }

    /// Reads what [`Self::encode`] wrote, ignoring everything else in the text.
    #[must_use]
    pub fn decode(text: &str) -> Self {
        let mut facts = Self {
            game_edition: "bedrock".to_owned(),
            game_platform: std::env::consts::OS.to_owned(),
            ..Self::default()
        };
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = unescape(value);
            match key {
                "host_game_version" => facts.game_version = value,
                "host_game_build" => facts.game_build = value,
                "host_game_edition" => facts.game_edition = value,
                "host_game_platform" => facts.game_platform = value,
                "host_game_protocol" => facts.game_protocol = value.parse().ok(),
                "host_api_version" => facts.api_version = value.parse().unwrap_or(0),
                "host_loader_version" => facts.loader_version = value,
                "host_feature" => facts.features.push(value),
                "host_launch_address" => facts.launch_address = Some(value),
                "host_window" => {
                    if let Some((width, height)) = value.split_once('x') {
                        facts.window = Some((width.parse().unwrap_or(0), height.parse().unwrap_or(0)));
                    }
                }
                _ => {}
            }
        }
        facts
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
            .capability(Capability::FilesystemRead)
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
            trust: crate::permissions::TrustLevel::Restricted,
        };
        let decoded = ModInfo::decode(&info.encode());
        assert_eq!(decoded.trust, crate::permissions::TrustLevel::Restricted);
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
