//! Hot reload and development mode: what a mod may reload of its own, and what it may not.
//!
//! The half that is real is a mod's own files. A path a mod owns is fingerprinted — size,
//! mtime and a content hash — and a change is a fingerprint that differs from the one stored,
//! so asking twice with nothing changed reports nothing. From the changed paths a
//! [`ReloadPlan`] says which entries are reloadable and which are blocked, with a reason per
//! entry; a [`ReloadRequest`] queue runs them through queued → running → done/failed with a
//! recorded outcome; and a mod's own reload listeners fire in registration order.
//!
//! The half that is refused is the client's. Reloading the client, its resources or its
//! classes, and watching the client's own files, are things this host installs no hook for,
//! and [`why_unknown`] says why at each entry point that asks.
//!
//! ```
//! use cinnaloader::dev;
//!
//! dev::enable();
//! assert!(dev::is_enabled());
//! let plan = dev::plan_reload(&["assets/lang/en_us.json".to_owned()]);
//! assert_eq!(plan.reloadable.len(), 1);
//! assert!(plan.blocked.is_empty());
//! dev::disable();
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

/// What a watched path looked like the last time it was checked: size, mtime and a hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint {
    pub path: String,
    pub size: u64,
    pub modified: Option<SystemTime>,
    /// FNV-1a over the contents, as [`crate::fs::hash_bytes`].
    pub hash: u64,
}

/// What a changed path would reload, which is also what decides whether it can.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReloadKind {
    Mod,
    Config,
    Resources,
    Scripts,
    Registries,
    Shaders,
    Ui,
    World,
    Client,
    Server,
}

impl ReloadKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mod => "mod",
            Self::Config => "config",
            Self::Resources => "resources",
            Self::Scripts => "scripts",
            Self::Registries => "registry",
            Self::Shaders => "shaders",
            Self::Ui => "ui",
            Self::World => "world",
            Self::Client => "client",
            Self::Server => "server",
        }
    }

    /// The name a queued request carries, and what a duplicate is compared against.
    #[must_use]
    pub fn target(self) -> String {
        match self {
            Self::Mod => crate::metadata::id(),
            Self::Config => "config".to_owned(),
            Self::Resources => "assets".to_owned(),
            Self::Scripts => "scripts".to_owned(),
            Self::Registries => "registries".to_owned(),
            Self::Shaders => "shaders".to_owned(),
            Self::Ui => "ui".to_owned(),
            Self::World => "world".to_owned(),
            Self::Client => "client".to_owned(),
            Self::Server => "server".to_owned(),
        }
    }

    /// The kind of refusal this would be, when it cannot be reloaded at all.
    #[must_use]
    fn refusal(self) -> Option<&'static str> {
        match self {
            Self::Shaders => Some("client_resources"),
            Self::Ui => Some("client_classes"),
            Self::World | Self::Client | Self::Server => Some("client"),
            _ => None,
        }
    }
}

impl fmt::Display for ReloadKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Which file a path looks like, which is what a reload plan keys on.
#[must_use]
pub fn kind_of(path: &str) -> ReloadKind {
    let lower = path.to_ascii_lowercase();
    let first = lower.split('/').next().unwrap_or_default();
    if lower.starts_with("client") {
        return ReloadKind::Client;
    }
    if first == "assets" || first == "resources" {
        return ReloadKind::Resources;
    }
    if first == "config" || lower.ends_with(".toml") || lower.ends_with(".cfg") || lower.ends_with(".ini") {
        return ReloadKind::Config;
    }
    if first == "scripts"
        || lower.ends_with(".lua")
        || lower.ends_with(".wasm")
        || lower.ends_with(".js")
    {
        return ReloadKind::Scripts;
    }
    if first == "shaders"
        || lower.ends_with(".frag")
        || lower.ends_with(".vert")
        || lower.ends_with(".glsl")
    {
        return ReloadKind::Shaders;
    }
    if first == "ui" || lower.ends_with(".gui") {
        return ReloadKind::Ui;
    }
    if first == "data" || first == "registries" || lower.ends_with(".registry") {
        return ReloadKind::Registries;
    }
    ReloadKind::Mod
}

/// One changed path that can be reloaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedReload {
    pub path: String,
    pub kind: ReloadKind,
    pub target: String,
    pub reason: String,
}

/// One changed path that cannot, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockedReload {
    pub path: String,
    pub kind: ReloadKind,
    pub target: String,
    pub reason: String,
}

/// What a set of changed paths would do if it were reloaded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReloadPlan {
    pub reloadable: Vec<PlannedReload>,
    pub blocked: Vec<BlockedReload>,
}

impl ReloadPlan {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.reloadable.is_empty() && self.blocked.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.reloadable.len() + self.blocked.len()
    }

    /// The kinds that could reload, which is what `reload_now` matches a listener against.
    #[must_use]
    pub fn reloadable_kinds(&self) -> Vec<ReloadKind> {
        let mut kinds: Vec<ReloadKind> = self.reloadable.iter().map(|entry| entry.kind).collect();
        kinds.sort();
        kinds.dedup();
        kinds
    }
}

/// Where a request is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueState {
    Queued,
    Running,
    Done,
    Failed,
}

impl QueueState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }
}

impl fmt::Display for QueueState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One reload request, and what happened to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReloadRequest {
    pub id: u64,
    pub target: String,
    pub kind: ReloadKind,
    pub state: QueueState,
    pub outcome: Option<String>,
}

struct Listener {
    name: String,
    kind: ReloadKind,
    run: Box<dyn FnMut() + Send>,
}

type ConsoleCommand = Box<dyn FnMut(&[String]) -> String + Send>;

#[derive(Default)]
struct Dev {
    enabled: Option<bool>,
    watched: BTreeMap<String, Option<Fingerprint>>,
    listeners: Vec<Listener>,
    queue: Vec<ReloadRequest>,
    next_request: u64,
    last_reload: Option<SystemTime>,
    notifications: Vec<String>,
    log_level: u32,
    features: BTreeMap<String, bool>,
    commands: BTreeMap<String, ConsoleCommand>,
}

impl fmt::Debug for Dev {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Dev")
            .field("enabled", &self.enabled)
            .field("watched", &self.watched.len())
            .field("listeners", &self.listeners.len())
            .field("queue", &self.queue.len())
            .field("log_level", &self.log_level)
            .field("notifications", &self.notifications.len())
            .field("features", &self.features.len())
            .field("commands", &self.commands.len())
            .finish()
    }
}

fn dev() -> &'static Mutex<Dev> {
    static DEV: OnceLock<Mutex<Dev>> = OnceLock::new();
    DEV.get_or_init(|| Mutex::new(Dev::default()))
}

fn with<T>(body: impl FnOnce(&mut Dev) -> T) -> T {
    let mut guard = dev().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn env_truthy(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| {
        matches!(
            value.to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

/// Whether development mode is on. Defaults to `CINNALOADER_DEV`, then off.
#[must_use]
pub fn is_enabled() -> bool {
    with(|dev| dev.enabled.unwrap_or_else(|| env_truthy("CINNALOADER_DEV")))
}

/// Turns development mode on.
pub fn enable() {
    with(|dev| dev.enabled = Some(true));
}

/// Turns it off.
pub fn disable() {
    with(|dev| dev.enabled = Some(false));
}

/// A file's fingerprint, or `None` when it is not there or is a directory.
#[must_use]
pub fn fingerprint(path: &str) -> Option<Fingerprint> {
    let metadata = crate::fs::metadata(path).ok()?;
    if !metadata.is_file {
        return None;
    }
    Some(Fingerprint {
        path: path.to_owned(),
        size: metadata.size,
        modified: metadata.modified,
        hash: crate::fs::hash(path)?,
    })
}

/// Starts watching paths inside the mod's own directory, and answers how many it took.
///
/// A path that resolves outside the mod's directory is refused: watching the client's own
/// files needs the client's file watcher, which this host does not install.
pub fn watch_files(paths: &[&str]) -> usize {
    with(|dev| {
        let mut added = 0;
        for path in paths {
            if crate::fs::resolve(path).is_err() {
                crate::warn!(
                    "dev watch refused {path}: {}",
                    why_unknown("client_watcher")
                );
                continue;
            }
            dev.watched.insert((*path).to_owned(), fingerprint(path));
            added += 1;
        }
        added
    })
}

/// Stops watching paths, and answers how many it stopped.
pub fn unwatch_files(paths: &[&str]) -> usize {
    with(|dev| {
        let mut removed = 0;
        for path in paths {
            removed += usize::from(dev.watched.remove(*path).is_some());
        }
        removed
    })
}

/// Watches every file under the mod's own directory.
pub fn watch_mod_directory() -> usize {
    let Some(root) = crate::fs::root() else {
        crate::warn!(
            "dev watch refused the mod directory: {}",
            why_unknown("client_watcher")
        );
        return 0;
    };
    let files = crate::fs::walk("").unwrap_or_default();
    with(|dev| {
        let mut added = 0;
        for file in files {
            if !file.is_file() {
                continue;
            }
            let Ok(relative) = file.strip_prefix(&root) else {
                continue;
            };
            let relative = relative.to_string_lossy().replace('\\', "/");
            dev.watched.insert(relative.clone(), fingerprint(&relative));
            added += 1;
        }
        added
    })
}

/// Every watched path whose fingerprint differs from the one stored, in name order.
///
/// The stored fingerprint is replaced as each change is reported, so calling this again with
/// nothing changed reports nothing.
#[must_use]
pub fn get_changed_files() -> Vec<String> {
    with(|dev| {
        let mut changed = Vec::new();
        for (path, last) in &mut dev.watched {
            let current = fingerprint(path);
            if current != *last {
                *last = current;
                changed.push(path.clone());
            }
        }
        changed
    })
}

fn frozen_registry() -> Option<String> {
    crate::registry::registered()
        .into_iter()
        .find(|name| crate::registry::is_frozen(Some(name)))
}

/// Works out what a set of changed paths would reload, and what it cannot.
#[must_use]
pub fn plan_reload(changed: &[String]) -> ReloadPlan {
    let frozen = frozen_registry();
    let mut plan = ReloadPlan::default();
    for path in changed {
        let kind = kind_of(path);
        let target = kind.target();
        match kind.refusal() {
            Some(refusal) => plan.blocked.push(BlockedReload {
                path: path.clone(),
                kind,
                target,
                reason: why_unknown(refusal),
            }),
            None => match kind {
                ReloadKind::Registries => match &frozen {
                    Some(name) => plan.blocked.push(BlockedReload {
                        path: path.clone(),
                        kind,
                        target,
                        reason: format!(
                            "the '{name}' registry is frozen, so nothing can be re-registered into it"
                        ),
                    }),
                    None => plan.reloadable.push(PlannedReload {
                        path: path.clone(),
                        kind,
                        target,
                        reason: "the registries are not frozen, so this can be rebuilt".to_owned(),
                    }),
                },
                _ => plan.reloadable.push(PlannedReload {
                    path: path.clone(),
                    kind,
                    target,
                    reason: format!("a mod's own {} is read from disk and re-registered", kind),
                }),
            },
        }
    }
    plan
}

/// Queues a reload request, and answers its id, or `None` when the same target is pending.
#[must_use]
pub fn enqueue(kind: ReloadKind) -> Option<u64> {
    with(|dev| {
        let target = kind.target();
        let pending = dev.queue.iter().any(|request| {
            request.target == target
                && matches!(request.state, QueueState::Queued | QueueState::Running)
        });
        if pending {
            return None;
        }
        dev.next_request += 1;
        let id = dev.next_request;
        dev.queue.push(ReloadRequest {
            id,
            target,
            kind,
            state: QueueState::Queued,
            outcome: None,
        });
        Some(id)
    })
}

/// Moves the first queued request to running, and answers its id.
#[must_use]
pub fn start_next() -> Option<u64> {
    with(|dev| {
        let request = dev.queue.iter_mut().find(|request| request.state == QueueState::Queued)?;
        request.state = QueueState::Running;
        Some(request.id)
    })
}

/// Finishes a running request, recording whether it succeeded and why.
#[must_use]
pub fn finish(id: u64) -> bool {
    with(|dev| {
        let Some(index) = dev
            .queue
            .iter()
            .position(|request| request.id == id && request.state == QueueState::Running)
        else {
            return false;
        };
        let (kind, target) = {
            let request = &dev.queue[index];
            (request.kind, request.target.clone())
        };
        let (state, outcome) = evaluate(kind, &target);
        dev.queue[index].state = state;
        dev.queue[index].outcome = Some(outcome);
        dev.last_reload = Some(SystemTime::now());
        true
    })
}

fn evaluate(kind: ReloadKind, target: &str) -> (QueueState, String) {
    match kind.refusal() {
        Some(refusal) => (QueueState::Failed, why_unknown(refusal)),
        None => match kind {
            ReloadKind::Registries => match frozen_registry() {
                Some(name) => (
                    QueueState::Failed,
                    format!("the '{name}' registry is frozen, so {target} cannot be rebuilt"),
                ),
                None => (QueueState::Done, format!("{target} rebuilt")),
            },
            _ => (QueueState::Done, format!("{target} reloaded")),
        },
    }
}

/// Runs every queued request to a finish, and answers how many ran.
#[must_use]
pub fn run_queue() -> usize {
    let mut ran = 0;
    while let Some(id) = start_next() {
        let _ = finish(id);
        ran += 1;
    }
    ran
}

/// How many requests are queued or running.
#[must_use]
pub fn pending_count() -> usize {
    with(|dev| {
        dev.queue
            .iter()
            .filter(|request| matches!(request.state, QueueState::Queued | QueueState::Running))
            .count()
    })
}

/// Every request, in the order it was queued.
#[must_use]
pub fn get_reload_status() -> Vec<ReloadRequest> {
    with(|dev| dev.queue.clone())
}

/// The outcome of every request that failed.
#[must_use]
pub fn get_reload_errors() -> Vec<String> {
    with(|dev| {
        dev.queue
            .iter()
            .filter(|request| request.state == QueueState::Failed)
            .filter_map(|request| request.outcome.clone())
            .collect()
    })
}

/// When the last request finished, or `None` before any has.
#[must_use]
pub fn get_last_reload_time() -> Option<SystemTime> {
    with(|dev| dev.last_reload)
}

/// Queues a reload of this mod's own state.
#[must_use]
pub fn reload_mod() -> Option<u64> {
    enqueue(ReloadKind::Mod)
}

/// Queues a reload of the mod's own config.
#[must_use]
pub fn reload_config() -> Option<u64> {
    enqueue(ReloadKind::Config)
}

/// Queues a reload of the mod's own resources.
#[must_use]
pub fn reload_resources() -> Option<u64> {
    enqueue(ReloadKind::Resources)
}

/// Queues a reload of the mod's own scripts.
#[must_use]
pub fn reload_scripts() -> Option<u64> {
    enqueue(ReloadKind::Scripts)
}

/// Queues a rebuild of the registries.
#[must_use]
pub fn rebuild_registries() -> Option<u64> {
    enqueue(ReloadKind::Registries)
}

/// Queues every reload this host can actually do, and answers how many it queued.
pub fn reload_all() -> usize {
    [ReloadKind::Mod, ReloadKind::Config, ReloadKind::Resources, ReloadKind::Scripts]
        .into_iter()
        .filter(|kind| enqueue(*kind).is_some())
        .count()
}

/// A listener that runs when its kind of thing can reload.
pub fn add_reload_listener(
    name: &str,
    kind: ReloadKind,
    run: impl FnMut() + Send + 'static,
) -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    with(|dev| {
        dev.listeners.push(Listener {
            name: name.to_owned(),
            kind,
            run: Box::new(run),
        });
    });
    id
}

/// Removes a listener by name, and answers whether there was one.
pub fn remove_reload_listener(name: &str) -> bool {
    with(|dev| {
        let before = dev.listeners.len();
        dev.listeners.retain(|listener| listener.name != name);
        dev.listeners.len() != before
    })
}

/// The listeners in place, by name, in the order they will run.
#[must_use]
pub fn get_reload_listeners() -> Vec<String> {
    with(|dev| dev.listeners.iter().map(|listener| listener.name.clone()).collect())
}

/// Reads the changed files, works out what can reload, and calls exactly those listeners.
///
/// Returns how many ran, in registration order. With nothing changed it calls none.
pub fn reload_now() -> usize {
    let changed = get_changed_files();
    let plan = plan_reload(&changed);
    let kinds = plan.reloadable_kinds();
    with(|dev| {
        let mut ran = 0;
        for listener in &mut dev.listeners {
            if kinds.contains(&listener.kind) {
                (listener.run)();
                ran += 1;
            }
        }
        ran
    })
}

/// What a reload would drop: the state this host tracks, and what it cannot put back.
#[must_use]
pub fn what_would_be_lost() -> Vec<String> {
    with(|dev| {
        let mut lost = Vec::new();
        if !dev.listeners.is_empty() {
            lost.push(format!(
                "{} reload listener(s), which a reload clears and the mod must re-register",
                dev.listeners.len()
            ));
        }
        if !dev.watched.is_empty() {
            lost.push(format!(
                "{} watched fingerprint(s), reset when the watch list is rebuilt",
                dev.watched.len()
            ));
        }
        if let Some(name) = frozen_registry() {
            lost.push(format!(
                "every registration in the frozen '{name}' registry, which cannot be re-registered"
            ));
        }
        if lost.is_empty() {
            lost.push("nothing this host tracks is lost by a reload".to_owned());
        }
        lost
    })
}

/// The symbols this mod was built from, for the reload report.
#[must_use]
pub fn get_build_info() -> Vec<(String, String)> {
    vec![
        ("mod".to_owned(), crate::metadata::id()),
        ("version".to_owned(), crate::metadata::version().to_string()),
        (
            "build".to_owned(),
            crate::metadata::build_number()
                .map(|number| number.to_string())
                .unwrap_or_default(),
        ),
        (
            "commit".to_owned(),
            crate::metadata::git_commit().unwrap_or_default(),
        ),
        ("development".to_owned(), is_enabled().to_string()),
    ]
}

/// The compiler and ABI this mod was built against.
#[must_use]
pub fn get_compile_info() -> Vec<(String, String)> {
    vec![
        (
            "profile".to_owned(),
            if cfg!(debug_assertions) { "debug" } else { "release" }.to_owned(),
        ),
        ("abi".to_owned(), crate::ABI_VERSION.to_string()),
        ("sdk".to_owned(), env!("CARGO_PKG_VERSION").to_owned()),
        (
            "debug_assertions".to_owned(),
            cfg!(debug_assertions).to_string(),
        ),
    ]
}

/// Logs a development message and records it.
pub fn notify(message: &str) -> bool {
    crate::info!("{message}");
    with(|dev| {
        dev.notifications.push(message.to_owned());
        true
    })
}

/// The path to the loader's log, which this host does not write to a file.
#[must_use]
pub fn open_log() -> Option<PathBuf> {
    crate::info!("dev open_log: this host writes no log file");
    None
}

/// The mod's own directory.
#[must_use]
pub fn open_mod_directory() -> Option<PathBuf> {
    crate::fs::root()
}

/// The mod's own config directory.
#[must_use]
pub fn open_config_directory() -> Option<PathBuf> {
    crate::metadata::config_directory()
}

/// The path to the last crash report, which this host does not write to a file.
#[must_use]
pub fn open_crash_report() -> Option<PathBuf> {
    crate::info!("dev open_crash_report: this host writes no crash file");
    None
}

/// Registers a command a caller may run by name. `false` when the name is empty or taken.
pub fn register_console_command(
    name: &str,
    run: impl FnMut(&[String]) -> String + Send + 'static,
) -> bool {
    if name.trim().is_empty() {
        return false;
    }
    with(|dev| {
        if dev.commands.contains_key(name) {
            return false;
        }
        dev.commands.insert(name.to_owned(), Box::new(run));
        true
    })
}

/// Runs a registered command, or `None` when there is no such command.
#[must_use]
pub fn run_command(name: &str, args: &[&str]) -> Option<String> {
    let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
    with(|dev| dev.commands.get_mut(name).map(|run| run(&args)))
}

/// Sets the level the loader logs at, and answers whether it recognised the name.
#[must_use]
pub fn set_log_level(name: &str) -> bool {
    let level = match name.to_ascii_lowercase().as_str() {
        "error" => crate::level::ERROR,
        "warn" => crate::level::WARN,
        "info" => crate::level::INFO,
        _ => return false,
    };
    with(|dev| {
        dev.log_level = level;
        true
    })
}

/// Flips a feature flag, making it when it is not there, and answers its new value.
pub fn toggle_feature(name: &str) -> bool {
    with(|dev| {
        let flag = dev.features.entry(name.to_owned()).or_insert(false);
        *flag = !*flag;
        *flag
    })
}

/// Sets a feature flag, and answers whether that changed it.
pub fn set_feature_flag(name: &str, on: bool) -> bool {
    with(|dev| {
        let previous = dev.features.get(name).copied().unwrap_or(false);
        dev.features.insert(name.to_owned(), on);
        previous != on
    })
}

/// Every feature flag, by name.
#[must_use]
pub fn get_feature_flags() -> Vec<(String, bool)> {
    with(|dev| dev.features.iter().map(|(name, on)| (name.clone(), *on)).collect())
}

/// Why something a mod might ask for has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "client" => {
            "reloading the client means replacing its code and its state, which only the \
             client's own launcher can do"
                .to_owned()
        }
        "client_resources" => {
            "the client's textures and shaders live in registries it froze at startup, and a \
             mod cannot reach them"
                .to_owned()
        }
        "client_classes" => {
            "hot-swapping the client's classes needs its own class loader, which this host \
             does not have"
                .to_owned()
        }
        "client_watcher" => {
            "watching the client's own files needs its file watcher; this host polls only the \
             mod's own directory"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reload.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["client", "client_resources", "client_classes", "client_watcher"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

/// Reloading the world means reloading the client's. `false`.
#[must_use]
pub fn restart_world() -> bool {
    crate::warn!("a mod asked this host to restart the world: {}", why_unknown("client"));
    false
}

/// Reloading the client. `false`.
#[must_use]
pub fn restart_client() -> bool {
    crate::warn!("a mod asked this host to restart the client: {}", why_unknown("client"));
    false
}

/// Reloading the server means reloading the client's. `false`.
#[must_use]
pub fn restart_server() -> bool {
    crate::warn!("a mod asked this host to restart the server: {}", why_unknown("client"));
    false
}

/// Recompiling the client's shaders. `false`.
#[must_use]
pub fn recompile_shaders() -> bool {
    crate::warn!(
        "a mod asked this host to recompile shaders: {}",
        why_unknown("client_resources")
    );
    false
}

/// Rebuilding the client's UI. `false`.
#[must_use]
pub fn rebuild_ui() -> bool {
    crate::warn!(
        "a mod asked this host to rebuild the UI: {}",
        why_unknown("client_classes")
    );
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with(|dev| *dev = Dev::default());
    }

    fn rooted(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "cinnaloader-dev-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        crate::fs::set_root(root.clone());
        root
    }

    #[test]
    fn a_fingerprint_is_the_same_until_the_file_changes() {
        let _serial = serial();
        fresh();
        let root = rooted("fingerprint");
        crate::fs::write_text("notes.txt", "one").unwrap();
        assert_eq!(watch_files(&["notes.txt"]), 1);
        assert!(get_changed_files().is_empty(), "nothing changed yet");
        let before = fingerprint("notes.txt").unwrap();
        assert_eq!(fingerprint("notes.txt").unwrap(), before);
        assert!(get_changed_files().is_empty(), "asking twice reports nothing");

        crate::fs::write_text("notes.txt", "two").unwrap();
        assert_ne!(fingerprint("notes.txt").unwrap(), before);
        assert_eq!(get_changed_files(), vec!["notes.txt".to_owned()]);
        assert!(get_changed_files().is_empty(), "reported once, then quiet");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn watching_and_unwatching_paths_tracks_the_list() {
        let _serial = serial();
        fresh();
        let root = rooted("watch");
        crate::fs::write_text("a.txt", "a").unwrap();
        crate::fs::write_text("nested/b.txt", "b").unwrap();
        assert_eq!(watch_files(&["a.txt", "b.txt"]), 2);
        assert_eq!(unwatch_files(&["a.txt"]), 1);
        assert_eq!(unwatch_files(&["a.txt"]), 0);
        assert_eq!(watch_mod_directory(), 2, "a.txt and nested/b.txt");
        assert!(get_changed_files().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_plan_says_what_can_reload_and_why_the_rest_cannot() {
        let _serial = serial();
        fresh();
        crate::registry::unfreeze(None);
        let changed = [
            "assets/textures/gui/menu.png".to_owned(),
            "config/settings.toml".to_owned(),
            "scripts/tick.lua".to_owned(),
            "data/recipes.json".to_owned(),
            "shaders/terrain.frag".to_owned(),
            "client/version.json".to_owned(),
        ];
        let plan = plan_reload(&changed);
        assert_eq!(plan.reloadable.len(), 4, "{plan:?}");
        assert_eq!(plan.blocked.len(), 2, "{plan:?}");
        for entry in &plan.reloadable {
            assert!(!entry.reason.is_empty());
        }
        for entry in &plan.blocked {
            assert!(!entry.reason.is_empty());
        }
        assert!(plan
            .blocked
            .iter()
            .any(|entry| entry.kind == ReloadKind::Shaders && entry.reason.contains("froze")));
        assert!(plan
            .blocked
            .iter()
            .any(|entry| entry.kind == ReloadKind::Client));

        assert!(crate::registry::freeze(None));
        let plan = plan_reload(&changed);
        assert!(plan
            .blocked
            .iter()
            .any(|entry| entry.kind == ReloadKind::Registries && entry.reason.contains("frozen")));
        assert_eq!(plan.reloadable.len(), 3, "the registry path is now blocked: {plan:?}");
        crate::registry::unfreeze(None);
    }

    #[test]
    fn the_queue_refuses_a_duplicate_and_moves_through_its_states() {
        let _serial = serial();
        fresh();
        crate::registry::unfreeze(None);
        let id = reload_mod().expect("a first request for the mod");
        assert!(reload_mod().is_none(), "a duplicate target is refused");
        assert_eq!(pending_count(), 1);
        assert_eq!(start_next(), Some(id));
        assert_eq!(get_reload_status()[0].state, QueueState::Running);
        assert!(finish(id));
        assert_eq!(get_reload_status()[0].state, QueueState::Done);
        assert_eq!(
            get_reload_status()[0].outcome.as_deref().map(|outcome| outcome.ends_with("reloaded")),
            Some(true)
        );
        assert_eq!(pending_count(), 0);
        assert!(get_last_reload_time().is_some());
        assert!(!finish(id), "a finish with no running request is refused");
    }

    #[test]
    fn a_registry_rebuild_fails_while_the_registry_is_frozen() {
        let _serial = serial();
        fresh();
        assert!(crate::registry::freeze(None));
        let id = rebuild_registries().expect("a registry request");
        assert_eq!(run_queue(), 1);
        let status = get_reload_status();
        assert_eq!(status[0].id, id);
        assert_eq!(status[0].state, QueueState::Failed);
        assert!(get_reload_errors()[0].contains("frozen"));
        crate::registry::unfreeze(None);
        assert!(rebuild_registries().is_some());
        assert_eq!(run_queue(), 1);
        assert_eq!(get_reload_status().last().unwrap().state, QueueState::Done);
    }

    #[test]
    fn listeners_run_in_order_and_only_for_what_can_reload() {
        let _serial = serial();
        fresh();
        crate::registry::unfreeze(None);
        let root = rooted("listeners");
        let order = Arc::new(Mutex::new(Vec::new()));
        let into = Arc::clone(&order);
        add_reload_listener("first", ReloadKind::Resources, move || {
            into.lock().unwrap().push("first".to_owned());
        });
        let into = Arc::clone(&order);
        add_reload_listener("second", ReloadKind::Resources, move || {
            into.lock().unwrap().push("second".to_owned());
        });
        let into = Arc::clone(&order);
        add_reload_listener("ignored", ReloadKind::Config, move || {
            into.lock().unwrap().push("ignored".to_owned());
        });
        assert_eq!(
            get_reload_listeners(),
            vec!["first".to_owned(), "second".to_owned(), "ignored".to_owned()]
        );

        crate::fs::write_text("assets/lang/en_us.json", "{\"a\":1}").unwrap();
        watch_files(&["assets/lang/en_us.json"]);
        assert_eq!(reload_now(), 0, "nothing changed, so no listener runs");
        crate::fs::write_text("assets/lang/en_us.json", "{\"a\":2}").unwrap();
        assert_eq!(reload_now(), 2, "the two resource listeners, in order");
        assert_eq!(*order.lock().unwrap(), vec!["first".to_owned(), "second".to_owned()]);
        assert!(remove_reload_listener("second"));
        assert_eq!(get_reload_listeners(), vec!["first".to_owned(), "ignored".to_owned()]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_flag_and_the_refused_entry_points_say_why() {
        let _serial = serial();
        fresh();
        disable();
        assert!(!is_enabled());
        enable();
        assert!(is_enabled());
        disable();
        assert!(!is_enabled());

        assert!(!restart_world());
        assert!(!restart_client());
        assert!(!restart_server());
        assert!(!recompile_shaders());
        assert!(!rebuild_ui());
        let refusals = refusals();
        assert_eq!(refusals.len(), 4);
        for (what, why) in refusals {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
    }

    #[test]
    fn feature_flags_console_commands_and_the_loss_report() {
        let _serial = serial();
        fresh();
        assert!(!get_feature_flags().iter().any(|(name, _)| name == "hot"));
        assert!(toggle_feature("hot"), "a new flag toggles on");
        assert!(!toggle_feature("hot"), "and off again");
        assert!(set_feature_flag("cold", true), "a new value is a change");
        assert!(!set_feature_flag("cold", true), "the same value is not");
        assert_eq!(
            get_feature_flags(),
            vec![("cold".to_owned(), true), ("hot".to_owned(), false)]
        );

        assert!(register_console_command("echo", |args| args.join(" ")));
        assert!(!register_console_command("echo", |_| String::new()), "no duplicate name");
        assert_eq!(run_command("echo", &["hi", "there"]), Some("hi there".to_owned()));
        assert_eq!(run_command("missing", &[]), None);
        assert!(set_log_level("warn"));
        assert!(!set_log_level("nonsense"));
        assert!(notify("a dev note"));

        add_reload_listener("reload", ReloadKind::Mod, || {});
        let lost = what_would_be_lost();
        assert!(lost.iter().any(|line| line.contains("listener")), "{lost:?}");
        assert!(!get_build_info().is_empty());
        assert!(get_compile_info().iter().any(|(key, _)| key == "abi"));
    }
}
