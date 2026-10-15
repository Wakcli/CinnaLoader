//! The loader itself: its configuration, the services its parts publish, its startup state and
//! the phase report it keeps.
//!
//! A mod may read all of this. It may write none of it: the loader's configuration, its state
//! and its safe-mode flag belong to the loader, and an attempt to set one is refused with a
//! sentence and a log line rather than a silent success. What a mod *may* do is publish a
//! service of its own and look up the ones the loader's own parts publish — a name, a version,
//! a capability list and an owner each — with a real compatibility rule for a required
//! capability and version.
//!
//! ```
//! use cinnaloader::loader::{
//!     self, LoaderConfig, LoaderState, LogLevel, PhaseReport, ServiceRequirement, MOD_MANAGER,
//! };
//! use cinnaloader::lifecycle::Phase;
//! use std::time::Duration;
//!
//! assert_eq!(loader::get_api_version(), cinnaloader::ABI_VERSION);
//! assert!(LoaderState::Loading.transition(LoaderState::Ready).is_ok());
//! assert!(LoaderState::Starting.transition(LoaderState::Ready).is_err());
//! let mut config = LoaderConfig::defaults();
//! assert_eq!(config.log_level, LogLevel::Info);
//! assert!(config.set("log_level", "trace").is_ok());
//! assert_eq!(config.log_level, LogLevel::Trace);
//! let manager = loader::get_service(MOD_MANAGER).expect("the loader publishes a mod manager");
//! assert!(manager.supports(&ServiceRequirement::new("mods", "1.0.0")).is_ok());
//! let mut report = PhaseReport::new();
//! report.record(Phase::Load, "reading manifests", Duration::from_millis(2));
//! assert_eq!(report.total(), Duration::from_millis(2));
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::fs::{self, FsError};
use crate::lifecycle::Phase;
use crate::modinfo::{Environment, ModVersion};

pub use crate::interop::{CompatibilityAdapter, ExtensionPoint};

/// The file the loader reads its configuration from, under its own directory.
pub const CONFIG_PATH: &str = "loader.cfg";

/// The keys the loader's configuration document knows.
pub const MOD_DIRECTORIES: &str = "mod_directories";
pub const DISABLED_MODS: &str = "disabled_mods";
pub const LOAD_ORDER: &str = "load_order";
pub const LOG_LEVEL: &str = "log_level";
pub const DEV: &str = "dev";

/// The names the loader's own parts are published under.
pub const MOD_MANAGER: &str = "mod_manager";
pub const REGISTRY_MANAGER: &str = "registry_manager";
pub const EVENT_BUS: &str = "event_bus";
pub const TASK_SCHEDULER: &str = "task_scheduler";
pub const RESOURCE_MANAGER: &str = "resource_manager";
pub const PERMISSION_MANAGER: &str = "permission_manager";
pub const DEPENDENCY_RESOLVER: &str = "dependency_resolver";
pub const DIAGNOSTICS: &str = "diagnostics";
pub const LOGGER: &str = "logger";
pub const RUNTIME: &str = "runtime";

/// How much the loader logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LogLevel {
    Error,
    Warn,
    #[default]
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }

    #[must_use]
    pub const fn number(self) -> u32 {
        match self {
            Self::Error => crate::level::ERROR,
            Self::Warn => crate::level::WARN,
            Self::Info | Self::Debug | Self::Trace => crate::level::INFO,
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text.trim() {
            "error" => Self::Error,
            "warn" => Self::Warn,
            "info" => Self::Info,
            "debug" => Self::Debug,
            "trace" => Self::Trace,
            _ => return None,
        })
    }
}

impl fmt::Display for LogLevel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// What kind of value a configuration entry holds. The last one is only ever a description of
/// what a wrongly-typed value looked like, never what an entry expects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FieldKind {
    List,
    LogLevel,
    Flag,
    Text,
}

impl FieldKind {
    #[must_use]
    pub const fn noun(self) -> &'static str {
        match self {
            Self::List => "a list",
            Self::LogLevel => "a log level",
            Self::Flag => "a flag",
            Self::Text => "text",
        }
    }
}

impl fmt::Display for FieldKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.noun())
    }
}

/// Why a configuration document did not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// A key the loader does not know.
    UnknownKey(String),
    /// A known key holding the wrong kind of value, with both kinds named.
    WrongType {
        key: String,
        expected: FieldKind,
        found: FieldKind,
    },
    /// The file could not be read or written.
    Io(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownKey(key) => write!(formatter, "{key} is not a key the loader knows"),
            Self::WrongType {
                key,
                expected,
                found,
            } => write!(formatter, "{key} holds {expected}, not {found}"),
            Self::Io(message) => {
                write!(
                    formatter,
                    "the loader's configuration could not be read or written: {message}"
                )
            }
        }
    }
}

impl std::error::Error for ConfigError {}

/// The loader's configuration document: typed entries with defaults, read from and written to
/// [`CONFIG_PATH`] through [`crate::fs`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoaderConfig {
    pub mod_directories: Vec<PathBuf>,
    pub disabled_mods: Vec<String>,
    pub load_order: Vec<String>,
    pub log_level: LogLevel,
    pub dev: bool,
}

impl Default for LoaderConfig {
    fn default() -> Self {
        Self {
            mod_directories: vec![PathBuf::from("mods")],
            disabled_mods: Vec::new(),
            load_order: Vec::new(),
            log_level: LogLevel::Info,
            dev: false,
        }
    }
}

impl LoaderConfig {
    /// The entries as they stand before any file is read.
    #[must_use]
    pub fn defaults() -> Self {
        Self::default()
    }

    /// Reads the file over the defaults. A missing file — or a mod with no directory to read
    /// from — answers the defaults, which is what a first run sees.
    pub fn read() -> Result<Self, ConfigError> {
        let text = match fs::read_text(CONFIG_PATH) {
            Ok(text) => text,
            Err(FsError::Missing(_) | FsError::Outside(_)) => return Ok(Self::default()),
            Err(error) => return Err(ConfigError::Io(error.to_string())),
        };
        let mut config = Self::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            config.set(key.trim(), value.trim())?;
        }
        Ok(config)
    }

    /// Writes the document to [`CONFIG_PATH`].
    pub fn write(&self) -> Result<(), ConfigError> {
        fs::write_text(CONFIG_PATH, &self.render())
            .map(|_written| ())
            .map_err(|error| ConfigError::Io(error.to_string()))
    }

    /// The document as one file, the same shape [`Self::read`] parses.
    #[must_use]
    pub fn render(&self) -> String {
        format!(
            "# written by cinnaloader; edits are read on the next start\n\
             {MOD_DIRECTORIES} = {}\n\
             {DISABLED_MODS} = {}\n\
             {LOAD_ORDER} = {}\n\
             {LOG_LEVEL} = {}\n\
             {DEV} = {}\n",
            paths_text(&self.mod_directories),
            self.disabled_mods.join(", "),
            self.load_order.join(", "),
            self.log_level,
            self.dev,
        )
    }

    /// Sets one entry by key. An unknown key is refused by name, and a value of the wrong kind
    /// is refused with both kinds.
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), ConfigError> {
        match key {
            MOD_DIRECTORIES => self.mod_directories = directories(value),
            DISABLED_MODS => self.disabled_mods = list(value),
            LOAD_ORDER => self.load_order = list(value),
            LOG_LEVEL => self.log_level = parse_level(key, value)?,
            DEV => self.dev = parse_flag(key, value)?,
            _ => return Err(ConfigError::UnknownKey(key.to_owned())),
        }
        Ok(())
    }

    /// One entry as text, or `None` for a key the loader does not know.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<String> {
        Some(match key {
            MOD_DIRECTORIES => paths_text(&self.mod_directories),
            DISABLED_MODS => self.disabled_mods.join(", "),
            LOAD_ORDER => self.load_order.join(", "),
            LOG_LEVEL => self.log_level.name().to_owned(),
            DEV => self.dev.to_string(),
            _ => return None,
        })
    }
}

fn list(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect()
}

fn directories(text: &str) -> Vec<PathBuf> {
    list(text).into_iter().map(PathBuf::from).collect()
}

fn paths_text(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

fn classify(text: &str) -> FieldKind {
    if matches!(text, "true" | "false") {
        FieldKind::Flag
    } else if LogLevel::parse(text).is_some() {
        FieldKind::LogLevel
    } else {
        FieldKind::Text
    }
}

fn wrong(key: &str, expected: FieldKind, value: &str) -> ConfigError {
    ConfigError::WrongType {
        key: key.to_owned(),
        expected,
        found: classify(value),
    }
}

fn parse_level(key: &str, value: &str) -> Result<LogLevel, ConfigError> {
    LogLevel::parse(value).ok_or_else(|| wrong(key, FieldKind::LogLevel, value))
}

fn parse_flag(key: &str, value: &str) -> Result<bool, ConfigError> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(wrong(key, FieldKind::Flag, value)),
    }
}

/// Where the loader keeps its files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoaderPaths {
    pub mod_directories: Vec<PathBuf>,
    pub config: Option<PathBuf>,
    pub data: Option<PathBuf>,
    pub cache: Option<PathBuf>,
}

/// The loader, as a value a mod can hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loader {
    pub name: String,
    pub version: ModVersion,
    pub api_version: u32,
}

impl Loader {
    #[must_use]
    pub fn new() -> Self {
        // The loader's own version is empty when no loader report is present, so a standalone
        // build falls back to this SDK's version rather than naming nothing.
        let reported = crate::metadata::loader_version();
        let version = match reported.is_numeric() {
            true => reported,
            false => ModVersion::parse(env!("CARGO_PKG_VERSION")),
        };
        Self {
            name: "cinnaloader".to_owned(),
            version,
            api_version: crate::ABI_VERSION,
        }
    }
}

impl Default for Loader {
    fn default() -> Self {
        Self::new()
    }
}

/// The running loader.
#[must_use]
pub fn get() -> Loader {
    Loader::new()
}

/// The loader's version.
#[must_use]
pub fn get_version() -> ModVersion {
    get().version
}

/// The ABI version this loader speaks.
#[must_use]
pub fn get_api_version() -> u32 {
    get().api_version
}

/// The loader's configuration document, read from its file on first use.
#[must_use]
pub fn get_config() -> LoaderConfig {
    config_held()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
}

fn config_held() -> &'static Mutex<LoaderConfig> {
    static CONFIG: OnceLock<Mutex<LoaderConfig>> = OnceLock::new();
    CONFIG.get_or_init(|| Mutex::new(LoaderConfig::read().unwrap_or_default()))
}

/// The directories the loader works in.
#[must_use]
pub fn get_paths() -> LoaderPaths {
    LoaderPaths {
        mod_directories: get_config().mod_directories,
        config: crate::metadata::config_directory(),
        data: crate::metadata::data_directory(),
        cache: crate::metadata::cache_directory(),
    }
}

/// A service a loader part or a mod publishes: a name, a version, a capability list and an
/// owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalService {
    pub name: String,
    pub version: ModVersion,
    pub capabilities: Vec<String>,
    pub owner: String,
}

impl GlobalService {
    #[must_use]
    pub fn new(name: &str, version: &str, owner: &str) -> Self {
        Self {
            name: name.to_owned(),
            version: ModVersion::parse(version),
            capabilities: Vec::new(),
            owner: owner.to_owned(),
        }
    }

    #[must_use]
    pub fn capability(mut self, capability: &str) -> Self {
        if !self.publishes(capability) {
            self.capabilities.push(capability.to_owned());
        }
        self
    }

    #[must_use]
    pub fn publishes(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|held| held == capability)
    }

    /// Whether this service satisfies a requirement: the capability first, then the version.
    pub fn supports(&self, requirement: &ServiceRequirement) -> Result<(), ServiceError> {
        if !self.publishes(&requirement.capability) {
            return Err(ServiceError::MissingCapability {
                service: self.name.clone(),
                capability: requirement.capability.clone(),
            });
        }
        if !self.version.at_least(&requirement.minimum) {
            return Err(ServiceError::VersionTooOld {
                service: self.name.clone(),
                required: requirement.minimum.to_string(),
                offered: self.version.to_string(),
            });
        }
        Ok(())
    }
}

/// What a caller needs from a service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceRequirement {
    pub capability: String,
    pub minimum: ModVersion,
}

impl ServiceRequirement {
    #[must_use]
    pub fn new(capability: &str, minimum: &str) -> Self {
        Self {
            capability: capability.to_owned(),
            minimum: ModVersion::parse(minimum),
        }
    }
}

/// Why a service could not be found or does not fit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceError {
    Unknown(String),
    MissingCapability {
        service: String,
        capability: String,
    },
    VersionTooOld {
        service: String,
        required: String,
        offered: String,
    },
}

impl fmt::Display for ServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown(name) => write!(formatter, "no service named {name} is published"),
            Self::MissingCapability {
                service,
                capability,
            } => {
                write!(
                    formatter,
                    "{service} does not offer the {capability} capability"
                )
            }
            Self::VersionTooOld {
                service,
                required,
                offered,
            } => write!(
                formatter,
                "{service} offers {offered}, below the required {required}"
            ),
        }
    }
}

impl std::error::Error for ServiceError {}

fn builtin_services() -> BTreeMap<String, GlobalService> {
    [
        GlobalService::new(MOD_MANAGER, "1.0.0", "loader")
            .capability("mods")
            .capability("quarantine"),
        GlobalService::new(REGISTRY_MANAGER, "1.0.0", "loader").capability("registries"),
        GlobalService::new(EVENT_BUS, "1.0.0", "loader").capability("events"),
        GlobalService::new(TASK_SCHEDULER, "1.0.0", "loader").capability("tasks"),
        GlobalService::new(RESOURCE_MANAGER, "1.0.0", "loader").capability("resources"),
        GlobalService::new(PERMISSION_MANAGER, "1.0.0", "loader").capability("permissions"),
        GlobalService::new(DEPENDENCY_RESOLVER, "1.0.0", "loader").capability("dependencies"),
        GlobalService::new(DIAGNOSTICS, "1.0.0", "loader").capability("diagnostics"),
        GlobalService::new(LOGGER, "1.0.0", "loader").capability("logs"),
        GlobalService::new(RUNTIME, "1.0.0", "loader").capability("runtime"),
    ]
    .into_iter()
    .map(|service| (service.name.clone(), service))
    .collect()
}

fn services_held() -> &'static Mutex<BTreeMap<String, GlobalService>> {
    static SERVICES: OnceLock<Mutex<BTreeMap<String, GlobalService>>> = OnceLock::new();
    SERVICES.get_or_init(|| Mutex::new(builtin_services()))
}

/// Publishes a service. A name is taken once; a second publisher of the same name is refused.
pub fn register_service(service: GlobalService) -> bool {
    let mut services = services_held()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if services.contains_key(&service.name) {
        return false;
    }
    services.insert(service.name.clone(), service);
    true
}

/// Takes a service back, and answers whether it was there.
pub fn unregister_service(name: &str) -> bool {
    services_held()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(name)
        .is_some()
}

/// A service by name.
#[must_use]
pub fn get_service(name: &str) -> Option<GlobalService> {
    services_held()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(name)
        .cloned()
}

/// Every published service, in name order.
#[must_use]
pub fn services() -> Vec<GlobalService> {
    services_held()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .values()
        .cloned()
        .collect()
}

/// Every published service's name, in order.
#[must_use]
pub fn service_names() -> Vec<String> {
    services_held()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .keys()
        .cloned()
        .collect()
}

/// Puts the registry back to the loader's own services.
pub fn reset_services() {
    *services_held()
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = builtin_services();
}

/// The loader's mod manager.
#[must_use]
pub fn get_mod_manager() -> Option<GlobalService> {
    get_service(MOD_MANAGER)
}

/// The loader's registry manager.
#[must_use]
pub fn get_registry_manager() -> Option<GlobalService> {
    get_service(REGISTRY_MANAGER)
}

/// The loader's event bus.
#[must_use]
pub fn get_event_bus() -> Option<GlobalService> {
    get_service(EVENT_BUS)
}

/// The loader's task scheduler.
#[must_use]
pub fn get_task_scheduler() -> Option<GlobalService> {
    get_service(TASK_SCHEDULER)
}

/// The loader's resource manager.
#[must_use]
pub fn get_resource_manager() -> Option<GlobalService> {
    get_service(RESOURCE_MANAGER)
}

/// The loader's permission manager.
#[must_use]
pub fn get_permission_manager() -> Option<GlobalService> {
    get_service(PERMISSION_MANAGER)
}

/// The loader's dependency resolver.
#[must_use]
pub fn get_dependency_resolver() -> Option<GlobalService> {
    get_service(DEPENDENCY_RESOLVER)
}

/// The loader's diagnostics.
#[must_use]
pub fn get_diagnostics() -> Option<GlobalService> {
    get_service(DIAGNOSTICS)
}

/// The loader's logger.
#[must_use]
pub fn get_logger() -> Option<GlobalService> {
    get_service(LOGGER)
}

/// The loader's runtime.
#[must_use]
pub fn get_runtime() -> Option<GlobalService> {
    get_service(RUNTIME)
}

/// Every capability the loader's own services publish, in name order.
#[must_use]
pub fn get_capabilities() -> Vec<String> {
    let mut capabilities: Vec<String> = services()
        .into_iter()
        .flat_map(|service| service.capabilities)
        .collect();
    capabilities.sort();
    capabilities.dedup();
    capabilities
}

/// Where this loader runs, which is inside the client.
#[must_use]
pub fn get_environment() -> Environment {
    Environment::Client
}

/// The loader's state: starting, loading, ready, or failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LoaderState {
    #[default]
    Starting,
    Loading,
    Ready,
    Failed,
}

impl LoaderState {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Loading => "loading",
            Self::Ready => "ready",
            Self::Failed => "failed",
        }
    }

    /// Whether the loader may move here from `self`.
    #[must_use]
    pub const fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Starting, Self::Loading)
                | (Self::Loading, Self::Ready)
                | (Self::Loading, Self::Failed)
                | (Self::Failed, Self::Loading)
                | (Self::Ready, Self::Loading)
        )
    }

    /// The state after moving here, or the refusal naming both states.
    pub fn transition(self, next: Self) -> Result<Self, TransitionError> {
        if self.can_transition_to(next) {
            Ok(next)
        } else {
            Err(TransitionError {
                from: self,
                to: next,
            })
        }
    }
}

impl fmt::Display for LoaderState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// An illegal move between two states, both named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionError {
    pub from: LoaderState,
    pub to: LoaderState,
}

impl fmt::Display for TransitionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "the loader cannot go from {} to {}",
            self.from, self.to
        )
    }
}

impl std::error::Error for TransitionError {}

/// The loader's state and how it got there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoaderStateMachine {
    current: LoaderState,
    history: Vec<LoaderState>,
}

impl Default for LoaderStateMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl LoaderStateMachine {
    #[must_use]
    pub fn new() -> Self {
        Self {
            current: LoaderState::Starting,
            history: vec![LoaderState::Starting],
        }
    }

    #[must_use]
    pub fn current(&self) -> LoaderState {
        self.current
    }

    /// Every state the machine has held, oldest first, starting with [`LoaderState::Starting`].
    #[must_use]
    pub fn history(&self) -> &[LoaderState] {
        &self.history
    }

    /// Moves to `next`, recording it, or refuses with both states named.
    pub fn advance(&mut self, next: LoaderState) -> Result<LoaderState, TransitionError> {
        self.current = self.current.transition(next)?;
        self.history.push(next);
        Ok(next)
    }
}

fn state_held() -> &'static Mutex<LoaderStateMachine> {
    static STATE: OnceLock<Mutex<LoaderStateMachine>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(LoaderStateMachine::new()))
}

/// Moves the loader's own state, as the loader does. A mod uses [`set_state`], which refuses.
pub fn advance(next: LoaderState) -> Result<LoaderState, TransitionError> {
    state_held()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .advance(next)
}

/// The loader's state.
#[must_use]
pub fn get_state() -> LoaderState {
    state_held()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .current()
}

/// Every state the loader has held, oldest first.
#[must_use]
pub fn get_history() -> Vec<LoaderState> {
    state_held()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .history()
        .to_vec()
}

/// A mod asking to drive the loader's state. Refused: the state belongs to the loader.
pub fn set_state(_next: LoaderState) -> bool {
    refuse("state", "a mod tried to set the loader's state");
    false
}

/// Puts the loader's state back to starting.
pub fn reset_state() {
    *state_held()
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = LoaderStateMachine::new();
}

/// Whether the loader has reached [`LoaderState::Ready`].
#[must_use]
pub fn is_ready() -> bool {
    get_state() == LoaderState::Ready
}

static SAFE_MODE: AtomicBool = AtomicBool::new(false);
static RELOAD_REQUESTED: AtomicBool = AtomicBool::new(false);
static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Whether the loader is in safe mode.
#[must_use]
pub fn is_safe_mode() -> bool {
    SAFE_MODE.load(Ordering::SeqCst)
}

/// A mod asking to set safe mode. Refused: safe mode is the loader's decision.
pub fn set_safe_mode(_enabled: bool) -> bool {
    refuse("safe_mode", "a mod tried to set safe mode");
    false
}

/// Records that a reload was asked for, for the loader to act on.
pub fn request_reload() -> bool {
    RELOAD_REQUESTED.store(true, Ordering::SeqCst);
    true
}

/// Records that a shutdown was asked for, for the loader to act on.
pub fn request_shutdown() -> bool {
    SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
    true
}

#[must_use]
pub fn reload_requested() -> bool {
    RELOAD_REQUESTED.load(Ordering::SeqCst)
}

#[must_use]
pub fn shutdown_requested() -> bool {
    SHUTDOWN_REQUESTED.load(Ordering::SeqCst)
}

/// One phase's work and how long it took, as the caller timed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhaseTiming {
    pub phase: Phase,
    pub what: String,
    pub duration: Duration,
}

/// What the loader did on the way up, and how long each step took.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PhaseReport {
    timings: Vec<PhaseTiming>,
}

impl PhaseReport {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records one phase with the duration the caller measured, so a report is exact.
    pub fn record(&mut self, phase: Phase, what: &str, duration: Duration) -> &mut Self {
        self.timings.push(PhaseTiming {
            phase,
            what: what.to_owned(),
            duration,
        });
        self
    }

    #[must_use]
    pub fn timings(&self) -> &[PhaseTiming] {
        &self.timings
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.timings.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.timings.is_empty()
    }

    /// Every phase's duration added up.
    #[must_use]
    pub fn total(&self) -> Duration {
        self.timings
            .iter()
            .fold(Duration::ZERO, |sum, timing| sum + timing.duration)
    }

    /// One phase's durations added up across every time it ran.
    #[must_use]
    pub fn phase_time(&self, phase: Phase) -> Duration {
        self.timings
            .iter()
            .filter(|timing| timing.phase == phase)
            .fold(Duration::ZERO, |sum, timing| sum + timing.duration)
    }

    /// The longest step, or `None` for an empty report.
    #[must_use]
    pub fn slowest(&self) -> Option<&PhaseTiming> {
        self.timings.iter().max_by_key(|timing| timing.duration)
    }
}

fn report_held(slot: &'static OnceLock<Mutex<PhaseReport>>) -> &'static Mutex<PhaseReport> {
    slot.get_or_init(|| Mutex::new(PhaseReport::new()))
}

/// The loader's startup report, filled as it starts.
#[must_use]
pub fn get_startup_report() -> PhaseReport {
    static STARTUP: OnceLock<Mutex<PhaseReport>> = OnceLock::new();
    report_held(&STARTUP)
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
}

/// The loader's report of the last mod load.
#[must_use]
pub fn get_load_report() -> PhaseReport {
    static LOAD: OnceLock<Mutex<PhaseReport>> = OnceLock::new();
    report_held(&LOAD)
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
}

/// The lines the loader has logged as errors.
#[must_use]
pub fn get_errors() -> Vec<String> {
    errors_held()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
}

/// The lines the loader has logged as warnings.
#[must_use]
pub fn get_warnings() -> Vec<String> {
    warnings_held()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
}

fn errors_held() -> &'static Mutex<Vec<String>> {
    static ERRORS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    ERRORS.get_or_init(|| Mutex::new(Vec::new()))
}

fn warnings_held() -> &'static Mutex<Vec<String>> {
    static WARNINGS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    WARNINGS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Publishes an extension point on the interop registry.
pub fn register_extension_point(point: ExtensionPoint) -> bool {
    crate::interop::register_extension_point(point)
}

/// Registers a compatibility layer on the interop registry.
pub fn register_compatibility_layer(adapter: CompatibilityAdapter) -> bool {
    crate::interop::register_compatibility_adapter(adapter)
}

/// A mod asking to change the loader's own configuration. Refused: a mod may read it only.
pub fn set_config(_key: &str, _value: &str) -> bool {
    refuse("config", "a mod tried to change the loader's configuration");
    false
}

fn refuse(what: &str, message: &str) {
    crate::warn!("{message}: {}", why_unknown(what));
}

/// Why a mod may not touch one of the loader's own things.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "config" => {
            "the loader's configuration belongs to the loader; a mod may read it and must not \
             write it"
                .to_owned()
        }
        "state" => {
            "the loader's state belongs to the loader; a mod may read it and must not change it"
                .to_owned()
        }
        "phase" => {
            "the loader's phase is driven by the loader; a mod may read it and must not dispatch \
             it"
            .to_owned()
        }
        "safe_mode" => {
            "safe mode is the loader's decision; a mod may read it and must not set it".to_owned()
        }
        _ => format!("{what} belongs to the loader, and a mod may not change it"),
    }
}

/// Every loader-owned thing a mod may read but not write.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["config", "state", "phase", "safe_mode"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn rooted(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("cinnaloader-loader-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        fs::set_root(root.clone());
        root
    }

    #[test]
    fn a_config_round_trips_through_its_file() {
        let _serial = serial();
        let root = rooted("round-trip");
        let config = LoaderConfig {
            mod_directories: vec![PathBuf::from("mods"), PathBuf::from("extra")],
            disabled_mods: vec!["bad".to_owned(), "worse".to_owned()],
            load_order: vec!["core".to_owned()],
            log_level: LogLevel::Debug,
            dev: true,
        };
        config.write().expect("the loader's config writes");
        let read = LoaderConfig::read().expect("and reads back");
        assert_eq!(read, config);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_unknown_config_key_is_refused_by_name() {
        let _serial = serial();
        let root = rooted("unknown");
        fs::write_text(CONFIG_PATH, "mod_directories = mods\nmystery = 1\n").unwrap();
        assert_eq!(
            LoaderConfig::read(),
            Err(ConfigError::UnknownKey("mystery".to_owned()))
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_wrongly_typed_config_value_is_refused_with_both_kinds() {
        let _serial = serial();
        let mut config = LoaderConfig::defaults();
        assert_eq!(
            config.set("log_level", "true"),
            Err(ConfigError::WrongType {
                key: "log_level".to_owned(),
                expected: FieldKind::LogLevel,
                found: FieldKind::Flag,
            })
        );
        assert_eq!(
            config.set("dev", "info"),
            Err(ConfigError::WrongType {
                key: "dev".to_owned(),
                expected: FieldKind::Flag,
                found: FieldKind::LogLevel,
            })
        );
        assert_eq!(
            config.set("dev", "loud"),
            Err(ConfigError::WrongType {
                key: "dev".to_owned(),
                expected: FieldKind::Flag,
                found: FieldKind::Text,
            })
        );
        assert_eq!(
            config.log_level,
            LogLevel::Info,
            "a refused value is not stored"
        );
    }

    #[test]
    fn a_service_resolves_a_capability_and_names_one_it_lacks() {
        let _serial = serial();
        reset_services();
        let manager = get_service(PERMISSION_MANAGER).expect("the loader publishes its managers");
        assert_eq!(
            manager.supports(&ServiceRequirement::new("permissions", "1.0.0")),
            Ok(())
        );
        let logger = get_service(LOGGER).expect("and a logger");
        assert_eq!(
            logger.supports(&ServiceRequirement::new("permissions", "1.0.0")),
            Err(ServiceError::MissingCapability {
                service: LOGGER.to_owned(),
                capability: "permissions".to_owned(),
            })
        );
        assert_eq!(
            manager.supports(&ServiceRequirement::new("permissions", "2.0.0")),
            Err(ServiceError::VersionTooOld {
                service: PERMISSION_MANAGER.to_owned(),
                required: "2.0.0".to_owned(),
                offered: "1.0.0".to_owned(),
            })
        );
    }

    #[test]
    fn the_registry_lists_the_loaders_own_services_and_takes_one_once() {
        let _serial = serial();
        reset_services();
        assert_eq!(services().len(), 10);
        assert!(service_names().contains(&MOD_MANAGER.to_owned()));
        assert!(get_mod_manager().is_some());
        assert!(get_permission_manager().is_some());
        assert!(get_capabilities().contains(&"permissions".to_owned()));
        assert!(register_service(GlobalService::new(
            "hello:probe",
            "2.0.0",
            "hello"
        )));
        assert!(
            !register_service(GlobalService::new("hello:probe", "3.0.0", "hello")),
            "a name is taken once"
        );
        assert_eq!(
            get_service("hello:probe").map(|service| service.version.to_string()),
            Some("2.0.0".to_owned())
        );
        assert_eq!(get_service("nothing"), None);
        assert!(unregister_service("hello:probe"));
        assert!(!unregister_service("hello:probe"));
    }

    #[test]
    fn the_state_machine_transitions_and_refuses_an_illegal_one() {
        let _serial = serial();
        let mut machine = LoaderStateMachine::new();
        assert_eq!(machine.current(), LoaderState::Starting);
        assert_eq!(
            machine.advance(LoaderState::Loading),
            Ok(LoaderState::Loading)
        );
        assert_eq!(
            machine.advance(LoaderState::Failed),
            Ok(LoaderState::Failed)
        );
        assert_eq!(
            machine.advance(LoaderState::Loading),
            Ok(LoaderState::Loading)
        );
        assert_eq!(machine.advance(LoaderState::Ready), Ok(LoaderState::Ready));
        assert_eq!(
            machine.history().to_vec(),
            vec![
                LoaderState::Starting,
                LoaderState::Loading,
                LoaderState::Failed,
                LoaderState::Loading,
                LoaderState::Ready,
            ]
        );
        assert_eq!(
            LoaderState::Starting.transition(LoaderState::Ready),
            Err(TransitionError {
                from: LoaderState::Starting,
                to: LoaderState::Ready,
            })
        );
        assert!(!LoaderState::Ready.can_transition_to(LoaderState::Failed));
    }

    #[test]
    fn a_phase_report_sums_caller_supplied_durations() {
        let mut report = PhaseReport::new();
        assert!(report.is_empty());
        report.record(Phase::Load, "reading manifests", Duration::from_millis(5));
        report.record(Phase::Initialize, "wiring mods", Duration::from_millis(3));
        report.record(Phase::Initialize, "a second mod", Duration::from_millis(2));
        assert_eq!(report.len(), 3);
        assert_eq!(report.total(), Duration::from_millis(10));
        assert_eq!(
            report.phase_time(Phase::Initialize),
            Duration::from_millis(5)
        );
        assert_eq!(report.phase_time(Phase::Tick), Duration::ZERO);
        assert_eq!(
            report.slowest().map(|timing| timing.what.clone()),
            Some("reading manifests".to_owned())
        );
    }

    #[test]
    fn a_mod_cannot_write_the_loaders_config_or_state() {
        let _serial = serial();
        assert!(!set_config("log_level", "trace"));
        assert!(!set_state(LoaderState::Ready));
        assert!(!set_safe_mode(true));
        assert!(!is_safe_mode());
        assert_eq!(get_state(), LoaderState::Starting);
        assert!(get_config().get("log_level").is_some());
        assert_eq!(refusals().len(), 4);
        assert!(why_unknown("config").contains("must not write"));
        assert!(why_unknown("state").contains("must not change"));
        assert_eq!(
            why_unknown("safe_mode"),
            "safe mode is the loader's decision; a mod may read it and must not set it"
        );
        assert!(get_startup_report().is_empty());
        assert!(get_load_report().is_empty());
    }
}
