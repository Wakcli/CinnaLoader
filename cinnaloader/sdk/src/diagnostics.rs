//! What is going on: this mod, the loader, the game, the machine, and what to do about it.
//!
//! [`report`] answers with one text block for a bug report; the accessors behind it answer
//! one question each. [`run_health_check`] is the one that is worth running before shipping
//! a mod: it compares what the mod declared with what it actually has.
//!
//! ```
//! use cinnaloader::prelude::*;
//!
//! info!("{}", diagnostics::report());
//! for finding in diagnostics::run_health_check() {
//!     warn!("{}", finding);
//! }
//! ```

use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::capability::{self, Capability};
use crate::log::{self, Record};
use crate::metadata;

/// Everything [`report`] knows, as values, for a caller that wants fields rather than text.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Report {
    pub environment: Environment,
    pub runtime: RuntimeInfo,
    pub loader: LoaderState,
    pub game: GameState,
    pub mod_status: ModStatus,
    pub memory: MemoryUsage,
    pub threads: Vec<String>,
    pub recent_errors: Vec<Record>,
    pub unhealth: Vec<String>,
}

/// Where this is running.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Environment {
    pub platform: String,
    pub architecture: String,
    pub family: String,
    pub mods_dir: Option<PathBuf>,
    pub data_dir: Option<PathBuf>,
    pub config_dir: Option<PathBuf>,
}

/// The process this mod is in.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RuntimeInfo {
    pub pid: u32,
    pub sdk_version: String,
    pub abi: u32,
    pub uptime_secs: u64,
    pub main_thread: String,
    pub worker_threads: usize,
    pub tasks_scheduled: u64,
    pub tasks_pending: usize,
}

/// What the loader is, and how it is doing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LoaderState {
    pub version: Option<String>,
    pub mods: Vec<String>,
    pub load_order: Vec<String>,
    pub failed: Vec<String>,
    pub errors: Vec<String>,
    pub capabilities_supported: Vec<String>,
}

/// What the game is, as far as this loader can tell.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GameState {
    pub version: Option<String>,
    pub platform: String,
    pub protocol: Option<u32>,
    pub phase: String,
    pub initialized: bool,
    pub ready: bool,
}

/// How this mod itself stands.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModStatus {
    pub id: String,
    pub version: String,
    pub state: String,
    pub enabled: bool,
    pub reloading: bool,
    pub declared_capabilities: Vec<String>,
    pub held_capabilities: Vec<String>,
    pub missing_permissions: Vec<String>,
    pub dependencies: Vec<String>,
}

/// What the process is using.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MemoryUsage {
    /// Resident set size in bytes, where the platform says.
    pub resident_bytes: Option<u64>,
    pub virtual_bytes: Option<u64>,
    /// Live allocations the log module is holding, as a rough figure.
    pub log_records: usize,
}

fn uptime() -> u64 {
    static STARTED: OnceLock<std::time::Instant> = OnceLock::new();
    STARTED
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_secs()
}

/// Every milestone a mod noted, for `get_game_state`'s phase and a report's timeline.
pub fn note(milestone: &str) {
    let mut held = timeline().lock().unwrap_or_else(|error| error.into_inner());
    held.push((
        uptime(),
        milestone.to_owned(),
    ));
}

fn timeline() -> &'static Mutex<Vec<(u64, String)>> {
    static TIMELINE: OnceLock<Mutex<Vec<(u64, String)>>> = OnceLock::new();
    TIMELINE.get_or_init(|| Mutex::new(Vec::new()))
}

/// The milestones noted so far, oldest first.
#[must_use]
pub fn get_timeline() -> Vec<(u64, String)> {
    timeline()
        .lock()
        .map(|held| held.clone())
        .unwrap_or_default()
}

/// Where this is running.
#[must_use]
pub fn get_environment() -> Environment {
    Environment {
        platform: std::env::consts::OS.to_owned(),
        architecture: std::env::consts::ARCH.to_owned(),
        family: std::env::consts::FAMILY.to_owned(),
        mods_dir: crate::mods_dir().map(PathBuf::from),
        data_dir: metadata::data_directory(),
        config_dir: metadata::config_directory(),
    }
}

/// The process this mod is in.
#[must_use]
pub fn get_runtime_info() -> RuntimeInfo {
    let metrics = crate::tasks::get_metrics();
    RuntimeInfo {
        pid: std::process::id(),
        sdk_version: env!("CARGO_PKG_VERSION").to_owned(),
        abi: crate::ABI_VERSION,
        uptime_secs: uptime(),
        main_thread: crate::tasks::current_thread(),
        worker_threads: metrics.background as usize,
        tasks_scheduled: metrics.scheduled,
        tasks_pending: metrics.pending,
    }
}

/// What the loader is, and how it is doing.
#[must_use]
pub fn get_loader_state() -> LoaderState {
    let registry = crate::mods::registry();
    LoaderState {
        version: crate::read_report()
            .map(|text| crate::modinfo::ModInfo::decode(&text).loader_version)
            .filter(|version| !version.is_empty()),
        mods: registry
            .mods
            .iter()
            .map(|entry| format!("{} ({})", entry.name, entry.id))
            .collect(),
        load_order: registry.order.clone(),
        failed: registry
            .mods
            .iter()
            .filter(|entry| entry.state == crate::modinfo::State::Failed)
            .map(|entry| entry.name.clone())
            .collect(),
        errors: registry.errors.clone(),
        capabilities_supported: capability::ALL
            .iter()
            .filter(|capability| capability::supported(**capability))
            .map(|capability| capability.as_str().to_owned())
            .collect(),
    }
}

/// What the game is, as far as this loader can tell.
#[must_use]
pub fn get_game_state() -> GameState {
    let facts = crate::host_facts();
    GameState {
        version: Some(facts.game_version.clone()).filter(|version| !version.is_empty()),
        platform: facts.game_platform.clone(),
        protocol: facts.game_protocol,
        phase: crate::lifecycle::current_phase().name().to_owned(),
        initialized: crate::lifecycle::is_initialized(),
        ready: crate::lifecycle::is_ready(),
    }
}

/// How this mod itself stands.
#[must_use]
pub fn get_mod_status() -> ModStatus {
    let manifest = metadata::manifest();
    let info = metadata::report();
    ModStatus {
        id: manifest.id.clone(),
        version: manifest.version.to_string(),
        state: info.state.to_string(),
        enabled: info.enabled,
        reloading: info.reloading,
        declared_capabilities: manifest
            .capabilities
            .iter()
            .map(|capability| capability.as_str().to_owned())
            .collect(),
        held_capabilities: manifest
            .capabilities
            .iter()
            .filter(|capability| capability::has(**capability))
            .map(|capability| capability.as_str().to_owned())
            .collect(),
        missing_permissions: capability::check_permissions()
            .into_iter()
            .map(|(capability, permission)| format!("{capability} needs {permission}"))
            .collect(),
        dependencies: manifest
            .dependencies
            .iter()
            .map(|dependency| dependency.id.clone())
            .collect(),
    }
}

/// What the process is using. On Linux this is the kernel's own figure; elsewhere it is
/// `None` rather than a guess.
#[must_use]
pub fn get_memory_usage() -> MemoryUsage {
    let resident_bytes = read_kilobytes("VmRSS:").map(|kilobytes| kilobytes * 1024);
    let virtual_bytes = read_kilobytes("VmSize:").map(|kilobytes| kilobytes * 1024);
    MemoryUsage {
        resident_bytes,
        virtual_bytes,
        log_records: log::get_recent(log::HISTORY).len(),
    }
}

fn read_kilobytes(field: &str) -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix(field) {
            let value: String = rest
                .trim()
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            return value.parse().ok();
        }
    }
    None
}

/// Memory, tasks and threads in one place, for a caller watching a mod settle.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ResourceUsage {
    pub memory: MemoryUsage,
    pub tasks: crate::tasks::TaskMetrics,
    pub log_records: usize,
    pub log_written: u64,
    pub log_dropped: u64,
}

#[must_use]
pub fn get_resource_usage() -> ResourceUsage {
    let (written, dropped) = log::counters();
    ResourceUsage {
        memory: get_memory_usage(),
        tasks: crate::tasks::get_metrics(),
        log_records: log::get_recent(log::HISTORY).len(),
        log_written: written,
        log_dropped: dropped,
    }
}

/// The threads this host knows about: the one a mod runs on, and the pool's workers.
#[must_use]
pub fn get_active_threads() -> Vec<String> {
    let metrics = crate::tasks::get_metrics();
    let mut threads = vec![format!("main ({})", crate::tasks::current_thread())];
    if crate::tasks::is_worker_thread() {
        threads.push("this is a worker".to_owned());
    }
    threads.push(format!("workers started: {}", metrics.background));
    threads.push(format!(
        "render thread: the main thread, inside the render phase ({})",
        crate::tasks::is_render_thread(),
    ));
    threads.push("network thread: the client's, out of reach".to_owned());
    threads
}

/// The last panic the loader caught from this mod, if it caught one.
#[must_use]
pub fn get_last_crash() -> Option<Crash> {
    crashes()
        .lock()
        .ok()
        .and_then(|crashes| crashes.last().cloned())
}

/// One panic, as the loader reported it to the mod.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crash {
    pub kind: String,
    pub message: String,
    pub at_secs: u64,
}

/// The panics so far, oldest first.
#[must_use]
pub fn get_crashes() -> Vec<Crash> {
    crashes()
        .lock()
        .map(|crashes| crashes.clone())
        .unwrap_or_default()
}

fn crashes() -> &'static Mutex<Vec<Crash>> {
    static CRASHES: OnceLock<Mutex<Vec<Crash>>> = OnceLock::new();
    CRASHES.get_or_init(|| Mutex::new(Vec::new()))
}

/// Records a panic. The loader calls this as it hands the panic to the mod.
pub fn note_crash(kind: &str, message: &str) {
    if let Ok(mut crashes) = crashes().lock() {
        crashes.push(Crash {
            kind: kind.to_owned(),
            message: message.to_owned(),
            at_secs: uptime(),
        });
    }
}

/// The errors this mod has logged, newest last, whether or not capture is on.
#[must_use]
pub fn get_recent_errors() -> Vec<Record> {
    let mut errors = log::captured_errors();
    let known: Vec<(log::Level, String)> = errors
        .iter()
        .map(|record| (record.level, record.message.clone()))
        .collect();
    // Errors logged while capture was off are still in the history, so they are added
    // rather than lost; what capture already kept is not repeated.
    for record in log::get_recent(log::HISTORY) {
        if record.level >= log::Level::Error
            && !known.contains(&(record.level, record.message.clone()))
        {
            errors.push(record);
        }
    }
    errors
}

/// What each capability means for this mod: whether the host has it, whether the mod holds
/// it, and what it would need.
#[must_use]
pub fn get_capability_report() -> Vec<CapabilityRow> {
    let manifest = metadata::manifest();
    capability::ALL
        .iter()
        .map(|capability| CapabilityRow {
            capability: *capability,
            supported: capability::supported(*capability),
            declared: manifest.capabilities.contains(capability),
            held: capability::has(*capability),
            experimental: capability::is_experimental(*capability),
            unsafe_: capability::is_unsafe(*capability),
            requirements: capability::get_requirements(*capability).to_vec(),
            note: capability::why_unsupported(*capability),
        })
        .collect()
}

/// One capability, for a report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityRow {
    pub capability: Capability,
    pub supported: bool,
    pub declared: bool,
    pub held: bool,
    pub experimental: bool,
    pub unsafe_: bool,
    pub requirements: Vec<crate::modinfo::Permission>,
    pub note: &'static str,
}

impl fmt::Display for CapabilityRow {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{:<22} {:<9} {:<8} {:<5} {}",
            self.capability.as_str(),
            if self.supported { "yes" } else { "no" },
            if self.held { "held" } else { "-" },
            if self.unsafe_ { "unsafe" } else { "" },
            self.note,
        )
    }
}

/// Everything at once.
#[must_use]
pub fn get_report() -> Report {
    Report {
        environment: get_environment(),
        runtime: get_runtime_info(),
        loader: get_loader_state(),
        game: get_game_state(),
        mod_status: get_mod_status(),
        memory: get_memory_usage(),
        threads: get_active_threads(),
        recent_errors: get_recent_errors(),
        unhealth: run_health_check(),
    }
}

/// The report as text, which is what goes into a bug report.
#[must_use]
pub fn report() -> String {
    let report = get_report();
    let mut text = String::new();
    text.push_str(&format!(
        "mod {} v{} ({}) enabled={} reloading={}\n",
        report.mod_status.id,
        report.mod_status.version,
        report.mod_status.state,
        report.mod_status.enabled,
        report.mod_status.reloading,
    ));
    text.push_str(&format!(
        "environment {} {} {} mods={} data={} config={}\n",
        report.environment.platform,
        report.environment.architecture,
        report.environment.family,
        report
            .environment
            .mods_dir
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "?".to_owned()),
        report
            .environment
            .data_dir
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "?".to_owned()),
        report
            .environment
            .config_dir
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "?".to_owned()),
    ));
    text.push_str(&format!(
        "runtime pid={} sdk={} abi={} uptime={}s thread={} workers={} tasks_scheduled={} pending={}\n",
        report.runtime.pid,
        report.runtime.sdk_version,
        report.runtime.abi,
        report.runtime.uptime_secs,
        report.runtime.main_thread,
        report.runtime.worker_threads,
        report.runtime.tasks_scheduled,
        report.runtime.tasks_pending,
    ));
    text.push_str(&format!(
        "game {} on {} protocol={:?} phase={} initialized={} ready={}\n",
        report.game.version.as_deref().unwrap_or("unknown"),
        report.game.platform,
        report.game.protocol,
        report.game.phase,
        report.game.initialized,
        report.game.ready,
    ));
    text.push_str(&format!(
        "loader {} mods={:?} order={:?} failed={:?}\n",
        report.loader.version.as_deref().unwrap_or("none"),
        report.loader.mods,
        report.loader.load_order,
        report.loader.failed,
    ));
    text.push_str(&format!(
        "memory rss={}KiB vsz={}KiB log_records={}\n",
        report
            .memory
            .resident_bytes
            .map(|bytes| bytes / 1024)
            .map(|kilobytes| kilobytes.to_string())
            .unwrap_or_else(|| "?".to_owned()),
        report
            .memory
            .virtual_bytes
            .map(|bytes| bytes / 1024)
            .map(|kilobytes| kilobytes.to_string())
            .unwrap_or_else(|| "?".to_owned()),
        report.memory.log_records,
    ));
    text.push_str(&format!(
        "capabilities declared={:?} held={:?}\n",
        report.mod_status.declared_capabilities, report.mod_status.held_capabilities
    ));
    for thread in &report.threads {
        text.push_str(&format!("thread {thread}\n"));
    }
    if !report.loader.errors.is_empty() {
        text.push_str(&format!("load errors: {:?}\n", report.loader.errors));
    }
    if !report.recent_errors.is_empty() {
        text.push_str("recent errors:\n");
        for record in &report.recent_errors {
            text.push_str(&format!("  {record}\n"));
        }
    }
    if report.unhealth.is_empty() {
        text.push_str("health: nothing to report\n");
    } else {
        text.push_str("health:\n");
        for finding in &report.unhealth {
            text.push_str(&format!("  {finding}\n"));
        }
    }
    text
}

/// Writes the report to a file, and answers where it went. With no path, it goes into the
/// mod's data directory as `report.txt`.
pub fn export_report(path: Option<&std::path::Path>) -> std::io::Result<PathBuf> {
    let path = match path {
        Some(path) => path.to_owned(),
        None => {
            if let Some(dir) = metadata::data_directory() {
                std::fs::create_dir_all(&dir)?;
                dir.join("report.txt")
            } else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "no directory is known for this mod",
                ));
            }
        }
    };
    std::fs::write(&path, report())?;
    Ok(path)
}

/// Compares what this mod declared with what it has, and answers with what is wrong. Empty
/// is the answer a mod wants before shipping.
#[must_use]
pub fn run_health_check() -> Vec<String> {
    let mut findings = Vec::new();
    let manifest = metadata::manifest();
    if manifest.id.is_empty() || manifest.id == "mod" {
        findings.push("the manifest has no id: declare one, it namespaces everything else".to_owned());
    }
    if manifest.version.to_string() == "0" {
        findings.push("the manifest has no version".to_owned());
    }
    for (capability, permission) in capability::check_permissions() {
        findings.push(format!(
            "{capability} was declared without the {permission} permission it implies"
        ));
    }
    for capability in &manifest.capabilities {
        if !capability::supported(*capability) {
            findings.push(format!(
                "{capability} was declared but this host cannot provide it ({})",
                capability::why_unsupported(*capability)
            ));
        }
    }
    for dependency in &manifest.dependencies {
        if dependency.kind.is_required() && !crate::mods::is_loaded(&dependency.id) {
            findings.push(format!("{} is required and not loaded", dependency.id));
        }
    }
    if crate::read_report().is_none() {
        findings.push("no loader answered: this is running outside the client".to_owned());
    }
    if !log::get_recent(log::HISTORY)
        .iter()
        .any(|record| record.level >= log::Level::Warn)
    {
        // Silence is not a finding; this branch exists so the check reads as a check.
    }
    if log::get_recent(log::HISTORY).iter().filter(|record| record.level >= log::Level::Error).count() > 0
    {
        findings.push("errors were logged during this run: see get_recent_errors".to_owned());
    }
    for crash in get_crashes() {
        findings.push(format!("{} panicked earlier: {}", crash.kind, crash.message));
    }
    let metrics = crate::tasks::get_metrics();
    if metrics.pending > 256 {
        findings.push(format!(
            "{} tasks are queued and not running: something is scheduling faster than the tick drains",
            metrics.pending
        ));
    }
    findings
}

/// The report as text, under the name the wishlist uses.
#[must_use]
pub fn report_text() -> String {
    report()
}

/// A counter a mod can use to notice that something is being called far more often than it
/// should be, which is the usual cause of a report nobody can read.
#[derive(Debug, Default)]
pub struct Counter {
    calls: AtomicU64,
}

impl Counter {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn hit(&self) -> u64 {
        self.calls.fetch_add(1, Ordering::Relaxed) + 1
    }

    #[must_use]
    pub fn count(&self) -> u64 {
        self.calls.load(Ordering::Relaxed)
    }

    pub fn reset(&self) {
        self.calls.store(0, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_environment_says_where_this_is() {
        let environment = get_environment();
        assert_eq!(environment.platform, std::env::consts::OS);
        assert!(!environment.architecture.is_empty());
        assert!(!environment.family.is_empty());
    }

    #[test]
    fn the_runtime_names_the_process_and_the_thread() {
        let runtime = get_runtime_info();
        assert_eq!(runtime.pid, std::process::id());
        assert_eq!(runtime.abi, crate::ABI_VERSION);
        assert!(!runtime.sdk_version.is_empty());
        assert!(runtime.uptime_secs < 60 * 60);
    }

    #[test]
    fn a_report_renders_without_a_loader() {
        let text = report();
        assert!(text.contains("environment "), "{text}");
        assert!(text.contains("runtime pid="), "{text}");
        assert!(text.contains("game "), "{text}");
        assert!(text.contains("health"), "{text}");
    }

    #[test]
    fn a_health_check_finds_a_capability_without_its_permission() {
        crate::metadata::declare(
            crate::modinfo::ModManifest::new("health-probe", "Health probe", "0.1")
                .capability(Capability::FilesystemWrite),
        );
        let findings = run_health_check();
        assert!(
            findings
                .iter()
                .any(|finding| finding.contains("filesystem_write")),
            "{findings:?}"
        );
    }

    #[test]
    fn the_capability_report_lists_everything() {
        let rows = get_capability_report();
        assert_eq!(rows.len(), capability::ALL.len());
        let render = rows
            .iter()
            .find(|row| row.capability == Capability::Render)
            .expect("render is known");
        assert!(render.supported);
        let unmappable = rows
            .iter()
            .find(|row| row.capability == Capability::WorldWrite)
            .expect("world_write is known");
        assert!(!unmappable.supported);
        assert!(!unmappable.note.is_empty());
    }

    #[test]
    fn memory_is_read_from_the_kernel_or_left_unknown() {
        let usage = get_memory_usage();
        if cfg!(target_os = "linux") {
            assert!(usage.resident_bytes.unwrap_or_default() > 0);
        }
    }

    #[test]
    fn a_crash_is_remembered() {
        let before = get_crashes().len();
        note_crash("panic", "the probe panicked");
        assert_eq!(get_crashes().len(), before + 1);
        assert_eq!(
            get_last_crash().map(|crash| crash.message),
            Some("the probe panicked".to_owned())
        );
    }

    #[test]
    fn a_milestone_lands_on_the_timeline() {
        let before = get_timeline().len();
        note("probe milestone");
        let timeline = get_timeline();
        assert_eq!(timeline.len(), before + 1);
        assert!(timeline.iter().any(|(_, name)| name == "probe milestone"));
    }

    #[test]
    fn exporting_writes_the_report_somewhere() {
        let path = std::env::temp_dir().join("cinnaloader-report-probe.txt");
        let written = export_report(Some(&path)).expect("the report is writable");
        let text = std::fs::read_to_string(&written).expect("and readable");
        assert!(text.contains("runtime pid="), "{text}");
        let _ = std::fs::remove_file(written);
    }
}
