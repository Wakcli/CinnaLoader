//! Loads Cinnamods mod cdylibs and drives them through the C ABI.
//!
//! The loader owns one shared [`HostApi`]; each mod cdylib resolves the entry points
//! by name, gets the table in `cinnaloader_init`, and is then advanced from a tick.

#[cfg(feature = "gui")]
pub mod gui;

use std::{
    ffi::{c_char, CStr, CString},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        Mutex, OnceLock,
    },
};

use anyhow::{bail, Context, Result};
use cinnaloader::deps::{DependencyResolver, GraphNode};
use cinnaloader::lifecycle::Phase;
use cinnaloader::modinfo::{HostFacts, ModEntrypoint, ModInfo, ModManifest, State};
use cinnaloader::HostApi;
use libloading::{Library, Symbol};

type AbiFn = extern "C" fn() -> u32;
type InitFn = unsafe extern "C" fn(*const HostApi) -> i32;
type FrameFn = extern "C" fn(u32) -> i32;
/// `fn(out, capacity) -> i32`, the manifest a mod declares before it is loaded.
type ManifestFn = unsafe extern "C" fn(*mut c_char, usize) -> i32;
type PhaseFn = extern "C" fn(u32) -> i32;
type ShutdownFn = extern "C" fn() -> i32;

/// One mod, kept alive by the library handle beside it.
struct Loaded {
    name: String,
    _path: PathBuf,
    /// Held so the mod's code stays mapped until after `shutdown`.
    _library: Library,
    /// This mod's own view of the host, so `mod_name` names the mod and not the loader.
    _name: CString,
    /// Kept alive for the pointers the mod captured; never read here.
    _api: Box<HostApi>,
    frame: Option<FrameFn>,
    phase: Option<PhaseFn>,
    shutdown: Option<ShutdownFn>,
}

/// Loads mod libraries from a directory and ticks them.
pub struct Host {
    dir: CString,
    mods: Vec<Loaded>,
}

// SAFETY: every mod's `api.mods_dir` points into this struct's own `dir`, which never
// moves after construction, so the host may be moved to another thread (the tick thread).
unsafe impl Send for Host {}

/// The loader's own `cinnaloader_*` log sink; writes to stderr.
unsafe extern "C" fn host_log(level: u32, message: *const c_char) {
    let text = if message.is_null() {
        String::new()
    } else {
        // SAFETY: the SDK passes a live NUL-terminated string for the duration of the call.
        unsafe { std::ffi::CStr::from_ptr(message) }
            .to_string_lossy()
            .into_owned()
    };
    let tag = match level {
        0 => "error",
        1 => "warn",
        _ => "info",
    };
    eprintln!("[cinnaloader/{tag}] {text}");
}

/// Overlay lines are process-wide so the external overlay can read them.
static OVERLAY: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
const MAX_OVERLAY_LINES: usize = 64;

fn overlay_buffer() -> &'static Mutex<Vec<String>> {
    OVERLAY.get_or_init(|| Mutex::new(Vec::new()))
}

/// A mod's contribution to the loader's on-screen overlay.
unsafe extern "C" fn host_overlay_line(message: *const c_char) {
    if message.is_null() {
        return;
    }
    // SAFETY: the SDK passes a live NUL-terminated string for the duration of the call.
    let text = unsafe { std::ffi::CStr::from_ptr(message) }
        .to_string_lossy()
        .into_owned();
    if let Ok(mut lines) = overlay_buffer().lock() {
        if lines.len() >= MAX_OVERLAY_LINES {
            lines.remove(0);
        }
        lines.push(text);
    }
}

/// The current overlay lines, for an overlay view to render.
#[must_use]
pub fn overlay_lines() -> Vec<String> {
    overlay_buffer()
        .lock()
        .map(|lines| lines.clone())
        .unwrap_or_default()
}

/// Pending ClickGUI toggle request, raised by a mod's `toggle_gui` call.
static GUI_TOGGLE: AtomicBool = AtomicBool::new(false);

unsafe extern "C" fn host_toggle_gui() {
    GUI_TOGGLE.store(true, Ordering::SeqCst);
}

/// Host-side raise of the ClickGUI toggle (for the CLI's own keybind).
pub fn request_gui_toggle() {
    GUI_TOGGLE.store(true, Ordering::SeqCst);
}

/// Consumes a pending ClickGUI toggle, returning whether one was requested.
#[must_use]
pub fn take_gui_toggle() -> bool {
    GUI_TOGGLE.swap(false, Ordering::SeqCst)
}

/// Names of the mods loaded by the process's first [`Host::load_dir`], for the ClickGUI.
static MOD_NAMES: OnceLock<Vec<String>> = OnceLock::new();

/// The loaded mod names, process-wide.
#[must_use]
pub fn mod_names() -> Vec<String> {
    MOD_NAMES.get().cloned().unwrap_or_default()
}

/// Names the menu has switched off; their `frame` is skipped while disabled.
static DISABLED: OnceLock<Mutex<Vec<String>>> = OnceLock::new();

fn disabled() -> &'static Mutex<Vec<String>> {
    DISABLED.get_or_init(|| Mutex::new(Vec::new()))
}

/// Whether the mod's `frame` still runs.
#[must_use]
pub fn mod_enabled(name: &str) -> bool {
    disabled()
        .lock()
        .map(|names| !names.iter().any(|disabled| disabled == name))
        .unwrap_or(true)
}

/// Enables or disables a loaded mod by name.
pub fn set_mod_enabled(name: &str, enabled: bool) {
    if let Ok(mut names) = disabled().lock() {
        names.retain(|disabled| disabled != name);
        if !enabled {
            names.push(name.to_owned());
        }
    }
    // The mod is told, so its own event bus can say `mod_enabled` or `mod_disabled` rather
    // than a mod having to poll `is_enabled`.
    queue(
        if enabled { "enabled" } else { "disabled" },
        name,
        &kind_of(name),
        String::new(),
    );
}

/// The id a mod declared, for an event that names it.
fn kind_of(name: &str) -> String {
    records()
        .lock()
        .ok()
        .and_then(|records| {
            records
                .iter()
                .find(|record| record.name == name)
                .map(|record| record.manifest.id.clone())
        })
        .unwrap_or_else(|| name.to_owned())
}

/// What a mod declared about itself, and where its library came from.
#[derive(Clone)]
struct ModRecord {
    name: String,
    manifest: ModManifest,
    library: Option<PathBuf>,
    /// True while the mod's `load` is running.
    loading: bool,
}

static RECORDS: OnceLock<Mutex<Vec<ModRecord>>> = OnceLock::new();

/// Mods the loader switched off after a panic, so one bad mod cannot take the rest down.
static FAILED: OnceLock<Mutex<Vec<String>>> = OnceLock::new();

fn failed() -> &'static Mutex<Vec<String>> {
    FAILED.get_or_init(|| Mutex::new(Vec::new()))
}

/// The process's first [`Host`], for the client's frame boundaries.
static HOST: OnceLock<Mutex<Option<Host>>> = OnceLock::new();

fn host_slot() -> &'static Mutex<Option<Host>> {
    HOST.get_or_init(|| Mutex::new(None))
}

/// Keeps the host where the frame hook can reach it. The preload calls this instead of
/// keeping the host on its own thread.
pub fn hand_over(host: Host) {
    if let Ok(mut slot) = host_slot().lock() {
        *slot = Some(host);
    }
}

/// The host's observation of the running client, process-wide, as `key<TAB>value` lines.
static OBSERVATION: OnceLock<Mutex<String>> = OnceLock::new();

fn observation_slot() -> &'static Mutex<String> {
    OBSERVATION.get_or_init(|| Mutex::new(String::new()))
}

/// Replaces the host's observation of the running client, as `key<TAB>value` lines.
pub fn set_observation(text: &str) {
    if let Ok(mut observation) = observation_slot().lock() {
        observation.clear();
        observation.push_str(text);
    }
}

/// Writes the current observation NUL-terminated, returning 1 when there was one to write.
unsafe extern "C" fn host_observe(out: *mut c_char, capacity: usize) -> i32 {
    let text = observation_slot()
        .lock()
        .map(|observation| observation.clone())
        .unwrap_or_default();
    write_text((!text.is_empty()).then_some(text), out, capacity)
}

/// Frames the host has been advanced, for the observation baseline.
static FRAMES: AtomicU64 = AtomicU64::new(0);
/// Mod ticks delivered, for the observation baseline.
static TICKS: AtomicU64 = AtomicU64::new(0);
/// The last phase the loader walked, for the observation baseline.
static PHASE: AtomicU32 = AtomicU32::new(u32::MAX);

/// Publishes the loader's own view of the client, so a mod has something real to read before
/// any client read exists. One frame is one call here, from the client's frame hook or the
/// CLI's tick loop.
fn note_baseline(mods: usize) {
    let frames = FRAMES.fetch_add(1, Ordering::Relaxed) + 1;
    let ticks = TICKS.load(Ordering::Relaxed);
    let phase = match Phase::from_code(PHASE.load(Ordering::Relaxed)) {
        Some(phase) => phase.name(),
        None => "tick",
    };
    set_observation(&format!(
        "frame\t{frames}\nticks\t{ticks}\nmods\t{mods}\nphase\t{phase}"
    ));
}

/// One client frame: the render phases around one tick. The preload's frame hook calls this
/// each frame, so mods tick at the client's rate and see frame boundaries.
pub fn frame(dt_millis: u32) {
    walk_phase(Phase::RenderStart);
    if let Ok(mut slot) = host_slot().lock() {
        if let Some(host) = slot.as_mut() {
            host.tick(dt_millis);
        }
    }
    walk_phase(Phase::RenderEnd);
}

/// Hands one phase to every mod. The client's frame hook drives the render phases with it.
pub fn walk_phase(phase: Phase) {
    PHASE.store(phase.code(), Ordering::Relaxed);
    if let Ok(mut slot) = host_slot().lock() {
        if let Some(host) = slot.as_mut() {
            host.walk(phase);
        }
    }
}

/// Whether the mod was switched off after a panic.
#[must_use]
pub fn mod_failed(name: &str) -> bool {
    failed()
        .lock()
        .map(|names| names.iter().any(|failed| failed == name))
        .unwrap_or(false)
}

fn mark_failed(name: &str) {
    if let Ok(mut names) = failed().lock() {
        if !names.iter().any(|failed| failed == name) {
            eprintln!("[cinnaloader] {name} panicked and is switched off");
            names.push(name.to_owned());
        }
    }
}

/// Records a mod the loader would not load, so the panel can say why it is not running.
fn mark_refused(name: &str) {
    if let Ok(mut names) = failed().lock() {
        if !names.iter().any(|failed| failed == name) {
            names.push(name.to_owned());
        }
    }
}

fn records() -> &'static Mutex<Vec<ModRecord>> {
    RECORDS.get_or_init(|| Mutex::new(Vec::new()))
}

/// The loader's own version, reported to mods that gate on it.
const LOADER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// What the last resolution decided, for the graph report.
static ORDER: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
static PROBLEMS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
static CYCLES: OnceLock<Mutex<Vec<Vec<String>>>> = OnceLock::new();

fn last_order() -> Vec<String> {
    ORDER.get().and_then(|order| order.lock().ok().map(|order| order.clone())).unwrap_or_default()
}

fn last_problems() -> Vec<String> {
    PROBLEMS
        .get()
        .and_then(|problems| problems.lock().ok().map(|problems| problems.clone()))
        .unwrap_or_default()
}

fn last_cycles() -> Vec<Vec<String>> {
    CYCLES
        .get()
        .and_then(|cycles| cycles.lock().ok().map(|cycles| cycles.clone()))
        .unwrap_or_default()
}

/// Records what a resolution decided.
fn record_resolution(order: Vec<String>, problems: Vec<String>, cycles: Vec<Vec<String>>) {
    if let Ok(mut slot) = ORDER.get_or_init(|| Mutex::new(Vec::new())).lock() {
        *slot = order;
    }
    if let Ok(mut slot) = PROBLEMS.get_or_init(|| Mutex::new(Vec::new())).lock() {
        *slot = problems;
    }
    if let Ok(mut slot) = CYCLES.get_or_init(|| Mutex::new(Vec::new())).lock() {
        *slot = cycles;
    }
}

/// What this loader can provide, so a mod can ask before offering a feature: one name per
/// capability this client can really be given.
fn loader_features() -> Vec<&'static str> {
    cinnaloader::capability::ALL
        .iter()
        .filter(|capability| capability.is_supported())
        .map(|capability| capability.as_str())
        .collect()
}

/// The game and process facts every mod sees. The preload fills in what it can read from
/// the client; the rest are derived here.
static FACTS: OnceLock<HostFacts> = OnceLock::new();

/// Records what the client is. Called once, by the preload, once it has looked around.
pub fn set_host_facts(facts: HostFacts) {
    let _ = FACTS.set(facts);
}

/// The loader, game and process facts.
#[must_use]
pub fn host_facts() -> HostFacts {
    FACTS
        .get()
        .cloned()
        .unwrap_or_else(|| HostFacts {
            game_version: String::new(),
            game_build: String::new(),
            game_edition: "bedrock".to_owned(),
            game_platform: std::env::consts::OS.to_owned(),
            game_protocol: None,
            api_version: cinnaloader::ABI_VERSION,
            loader_version: LOADER_VERSION.to_owned(),
            features: loader_features().into_iter().map(str::to_owned).collect(),
            launch_address: None,
            window: None,
        })
}

/// Records a mod's manifest, or updates the one held under that name.
fn remember(name: &str, manifest: ModManifest, library: Option<PathBuf>) {
    if let Ok(mut records) = records().lock() {
        match records.iter_mut().find(|record| record.name == name) {
            Some(record) => {
                record.manifest = manifest;
                record.library = library.or_else(|| record.library.clone());
            }
            None => records.push(ModRecord {
                name: name.to_owned(),
                manifest,
                library,
                loading: false,
            }),
        }
    }
}

/// Marks a mod as loading, so a report asked for during `load` says so.
fn entering_load(name: &str) {
    if let Ok(mut records) = records().lock() {
        match records.iter_mut().find(|record| record.name == name) {
            Some(record) => record.loading = true,
            None => records.push(ModRecord {
                name: name.to_owned(),
                manifest: ModManifest::new(name, name, "0"),
                library: None,
                loading: true,
            }),
        }
    }
}

fn finished_loading(name: &str) {
    if let Ok(mut records) = records().lock() {
        let record = records.iter_mut().find(|record| record.name == name);
        if let Some(record) = record {
            record.loading = false;
        }
    }
}

/// The trust level a mod is held to: what it asked for, never above what this loader gives
/// without the user saying otherwise.
fn clamped_trust(name: &str) -> cinnaloader::permissions::TrustLevel {
    use cinnaloader::permissions::TrustLevel;
    declared_manifest(name).trust.min(TrustLevel::Normal)
}

/// The manifest a mod declared, or a bare one when it declared nothing.
fn declared_manifest(name: &str) -> ModManifest {
    records()
        .lock()
        .ok()
        .and_then(|records| {
            records
                .iter()
                .find(|record| record.name == name)
                .map(|record| record.manifest.clone())
        })
        .unwrap_or_else(|| ModManifest::new(name, name, "0"))
}

/// The loader's view of one mod: its declaration plus what only the loader knows.
fn mod_info(name: &str) -> ModInfo {
    let held = records().lock().ok().and_then(|records| {
        records
            .iter()
            .find(|record| record.name == name)
            .map(|record| (record.library.clone(), record.loading))
    });
    let (library, loading) = held.unwrap_or((None, false));
    let enabled = mod_enabled(name);
    let state = if mod_failed(name) {
        State::Failed
    } else if loading {
        State::Loading
    } else if enabled {
        State::Loaded
    } else {
        State::Disabled
    };
    ModInfo {
        manifest: declared_manifest(name),
        directory: config_dir().map(|dir| dir.join(name)),
        mod_name: name.to_owned(),
        state,
        enabled,
        reloading: false,
        library,
        entrypoint: ModEntrypoint::default(),
        loader_version: LOADER_VERSION.to_owned(),
        // What the loader enforces, which is never more than normal unless the user says so.
        trust: clamped_trust(name),
    }
}

/// The mod's identity: its library name, without the `lib` a shared object carries.
fn name_of(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "mod".into());
    let shared = matches!(path.extension().and_then(|ext| ext.to_str()), Some("so" | "dylib"));
    if shared {
        stem.strip_prefix("lib").unwrap_or(&stem).to_owned()
    } else {
        stem
    }
}

unsafe extern "C" fn host_declare(text: *const c_char) -> i32 {
    let Some(mod_name) = current_mod() else {
        eprintln!("[cinnaloader] a manifest was declared outside a mod's load");
        return 0;
    };
    let text = unsafe { c_text(text) };
    if text.len() > cinnaloader::modinfo::MAX_BYTES {
        eprintln!(
            "[cinnaloader] {mod_name}: manifest is {} bytes, over the {}-byte limit",
            text.len(),
            cinnaloader::modinfo::MAX_BYTES
        );
        return 0;
    }
    let manifest = ModManifest::decode(&text);
    let authors = if manifest.authors.is_empty() {
        String::new()
    } else {
        format!(", by {}", manifest.authors.join(", "))
    };
    let tags = if manifest.tags.is_empty() {
        String::new()
    } else {
        format!(", tags {}", manifest.tags.join(" "))
    };
    eprintln!(
        "[cinnaloader] {mod_name}: {} {} ({}{authors}{tags})",
        manifest.name,
        manifest.version,
        manifest.environment_name(),
    );
    remember(&mod_name, manifest, None);
    1
}

unsafe extern "C" fn host_report(out: *mut c_char, capacity: usize) -> i32 {
    let Some(mod_name) = current_mod() else {
        return 0;
    };
    let mut text = mod_info(&mod_name).encode();
    text.push_str(&host_facts().encode());
    write_text(Some(text), out, capacity)
}

/// What kind of control a setting is, and so which panel control draws it.
#[derive(Clone, Copy, PartialEq)]
pub enum SettingKind {
    Toggle,
    Slider,
    Choice,
    Keybind,
    Button,
}

/// One setting a mod registered, plus its current value.
pub struct Setting {
    /// Scoped as `mod:id`, so two mods may both use the same short id.
    pub id: String,
    /// The mod that owns it, for change delivery and config files.
    pub mod_name: String,
    pub label: String,
    pub category: String,
    pub kind: SettingKind,
    pub on: bool,
    pub number: f64,
    pub min: f64,
    pub max: f64,
    pub step: f64,
    pub index: u32,
    pub options: Vec<String>,
    pub key: String,
}

impl Setting {
    /// The value as it is written to the mod's config file.
    fn serialised(&self) -> String {
        match self.kind {
            SettingKind::Toggle => u8::from(self.on).to_string(),
            SettingKind::Slider => self.number.to_string(),
            SettingKind::Choice => self.index.to_string(),
            SettingKind::Keybind => self.key.clone(),
            SettingKind::Button => String::new(),
        }
    }

    fn apply(&mut self, value: &str) {
        match self.kind {
            SettingKind::Toggle => self.on = value == "1" || value == "true",
            SettingKind::Slider => {
                if let Ok(number) = value.parse::<f64>() {
                    self.number = number.clamp(self.min, self.max);
                }
            }
            SettingKind::Choice => {
                if let Ok(index) = value.parse::<u32>() {
                    self.index = index.min(self.options.len().saturating_sub(1) as u32);
                }
            }
            SettingKind::Keybind => {
                if !value.is_empty() {
                    self.key = value.to_owned();
                }
            }
            SettingKind::Button => {}
        }
    }
}

/// Something the mod should react to, drained from its queue by the SDK each tick.
pub struct Event {
    pub mod_name: String,
    pub kind: &'static str,
    pub id: String,
    pub value: String,
}

/// The registry and dependency graph as text, for a mod that asks about the others.
unsafe extern "C" fn host_graph(out: *mut c_char, capacity: usize) -> i32 {
    let mut text = String::new();
    let mut held = records().lock().map(|records| records.clone()).unwrap_or_default();
    held.sort_by(|left, right| left.name.cmp(&right.name));
    for record in held {
        let info = mod_info(&record.name);
        text.push_str(&format!(
            "mod={}|{}|{}|{}|{}\n",
            record.name,
            record.manifest.id,
            record.manifest.version,
            info.state,
            u8::from(info.enabled)
        ));
        for dependency in &record.manifest.dependencies {
            text.push_str(&format!(
                "depends={}|{}|{}|{}\n",
                record.name, dependency.id, dependency.requirement, dependency.kind
            ));
        }
        for id in &record.manifest.provides {
            text.push_str(&format!("provides={}|{id}\n", record.name));
        }
        for capability in &record.manifest.capabilities {
            text.push_str(&format!("modcap={}|{}\n", record.name, capability.as_str()));
        }
        for permission in &record.manifest.permissions {
            text.push_str(&format!("modperm={}|{}\n", record.name, permission.as_str()));
        }
    }
    for capability in cinnaloader::capability::ALL {
        text.push_str(&format!(
            "capability={}|{}\n",
            capability.as_str(),
            u8::from(capability.is_supported())
        ));
    }
    for (index, name) in last_order().iter().enumerate() {
        text.push_str(&format!("order={name}|{index}\n"));
    }
    for problem in last_problems() {
        text.push_str(&format!("error={}\n", problem.replace('\n', " ")));
    }
    for cycle in last_cycles() {
        text.push_str(&format!("cycle={}\n", cycle.join(",")));
    }
    write_text(Some(text), out, capacity)
}

/// Verbs a mod may ask the loader to run on another mod.
const CMD_ENABLE: u32 = 0;
const CMD_DISABLE: u32 = 1;
const CMD_IS_ENABLED: u32 = 2;
const CMD_IS_LOADED: u32 = 3;

unsafe extern "C" fn host_mods_command(verb: u32, id: *const c_char) -> i32 {
    let id = unsafe { c_text(id) };
    let loaded = records().lock().map(|records| {
        records
            .iter()
            .any(|record| record.name == id || record.manifest.id == id)
    });
    let loaded = loaded.unwrap_or(false);
    match verb {
        CMD_ENABLE => {
            set_mod_enabled(&id, true);
            1
        }
        CMD_DISABLE => {
            set_mod_enabled(&id, false);
            1
        }
        CMD_IS_ENABLED => i32::from(loaded && mod_enabled(&id)),
        CMD_IS_LOADED => i32::from(loaded),
        _ => -1,
    }
}

static SETTINGS: OnceLock<Mutex<Vec<Setting>>> = OnceLock::new();
static EVENTS: OnceLock<Mutex<Vec<Event>>> = OnceLock::new();
/// The mods directory, for each mod's config file.
static MODS_DIR: OnceLock<PathBuf> = OnceLock::new();
/// Keys pressed as of the last poll, for keybind edges.
static KEYS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();

fn settings_registry() -> &'static Mutex<Vec<Setting>> {
    SETTINGS.get_or_init(|| Mutex::new(Vec::new()))
}

fn event_queue() -> &'static Mutex<Vec<Event>> {
    EVENTS.get_or_init(|| Mutex::new(Vec::new()))
}

fn pressed_keys() -> &'static Mutex<Vec<String>> {
    KEYS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Runs `f` on one setting by id.
fn with_setting<T>(id: &str, f: impl FnOnce(&mut Setting) -> T) -> Option<T> {
    let mut settings = settings_registry().lock().ok()?;
    let setting = settings.iter_mut().find(|setting| setting.id == id)?;
    Some(f(setting))
}

/// Which mod's code is running, so the host can attribute what it registers and deliver
/// that mod's events. Set around each `init`, `frame` and `shutdown` call.
static CURRENT_MOD: Mutex<Option<String>> = Mutex::new(None);

fn enter_mod(name: &str) {
    if let Ok(mut current) = CURRENT_MOD.lock() {
        *current = Some(name.to_owned());
    }
}

fn leave_mod() {
    if let Ok(mut current) = CURRENT_MOD.lock() {
        *current = None;
    }
}

fn current_mod() -> Option<String> {
    CURRENT_MOD.lock().ok().and_then(|current| current.clone())
}

fn queue(kind: &'static str, mod_name: &str, id: &str, value: String) {
    if let Ok(mut events) = event_queue().lock() {
        events.push(Event {
            mod_name: mod_name.to_owned(),
            kind,
            id: id.to_owned(),
            value,
        });
    }
}

/// Registers a setting for the mod whose `load` is running, returning 1 or 0.
#[allow(clippy::too_many_arguments)]
fn register(
    kind: SettingKind,
    id: &str,
    label: &str,
    category: &str,
    options: Vec<String>,
) -> i32 {
    let Some(mod_name) = current_mod() else {
        eprintln!("[cinnaloader] a setting was registered outside a mod's load");
        return 0;
    };
    let Ok(mut settings) = settings_registry().lock() else {
        return 0;
    };
    if settings.iter().any(|setting| setting.id == id) {
        eprintln!("[cinnaloader] setting {id} is already registered");
        return 0;
    }
    eprintln!("[cinnaloader] {mod_name} registered setting {id}");
    settings.push(Setting {
        id: id.to_owned(),
        mod_name,
        label: label.to_owned(),
        category: category.to_owned(),
        kind,
        on: false,
        number: 0.0,
        min: 0.0,
        max: 1.0,
        step: 0.1,
        index: 0,
        options,
        key: String::new(),
    });
    1
}

/// The text of a NUL-terminated argument, or an empty string.
unsafe fn c_text(ptr: *const c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    // SAFETY: the SDK passes a live NUL-terminated string for the duration of the call.
    unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned()
}

unsafe extern "C" fn host_feature(
    id: *const c_char,
    label: *const c_char,
    category: *const c_char,
    default_on: u32,
) -> i32 {
    let (id, label, category) = unsafe { (c_text(id), c_text(label), c_text(category)) };
    let code = register(SettingKind::Toggle, &id, &label, &category, Vec::new());
    if code == 1 {
        // A default is not a user change, so it is applied without queueing an event.
        with_setting(&id, |setting| setting.on = default_on != 0);
    }
    code
}

unsafe extern "C" fn host_slider(
    id: *const c_char,
    label: *const c_char,
    category: *const c_char,
    value: f64,
    min: f64,
    max: f64,
    step: f64,
) -> i32 {
    let (id, label, category) = unsafe { (c_text(id), c_text(label), c_text(category)) };
    if !min.is_finite() || !max.is_finite() || min >= max || !step.is_finite() || step <= 0.0 {
        return 0;
    }
    let code = register(SettingKind::Slider, &id, &label, &category, Vec::new());
    if code == 1 {
        with_setting(&id, |setting| {
            setting.min = min;
            setting.max = max;
            setting.step = step;
            setting.number = value.clamp(min, max);
        });
    }
    code
}

unsafe extern "C" fn host_choice(
    id: *const c_char,
    label: *const c_char,
    category: *const c_char,
    options: *const c_char,
    default_index: u32,
) -> i32 {
    let (id, label, category, options) =
        unsafe { (c_text(id), c_text(label), c_text(category), c_text(options)) };
    let options: Vec<String> = options.split('\n').map(str::to_owned).collect();
    if options.is_empty() || options.len() > 8 {
        return 0;
    }
    let code = register(SettingKind::Choice, &id, &label, &category, options);
    if code == 1 {
        with_setting(&id, |setting| {
            setting.index = default_index.min(setting.options.len() as u32 - 1);
        });
    }
    code
}

unsafe extern "C" fn host_keybind(
    id: *const c_char,
    label: *const c_char,
    category: *const c_char,
    default_key: *const c_char,
) -> i32 {
    let (id, label, category, key) =
        unsafe { (c_text(id), c_text(label), c_text(category), c_text(default_key)) };
    let code = register(SettingKind::Keybind, &id, &label, &category, Vec::new());
    if code == 1 {
        with_setting(&id, |setting| setting.key = key.clone());
    }
    code
}

unsafe extern "C" fn host_button(
    id: *const c_char,
    label: *const c_char,
    category: *const c_char,
) -> i32 {
    let (id, label, category) = unsafe { (c_text(id), c_text(label), c_text(category)) };
    register(SettingKind::Button, &id, &label, &category, Vec::new())
}

unsafe extern "C" fn host_enabled(id: *const c_char) -> i32 {
    let id = unsafe { c_text(id) };
    settings_registry()
        .lock()
        .ok()
        .and_then(|settings| {
            settings
                .iter()
                .find(|setting| setting.id == id && setting.kind == SettingKind::Toggle)
                .map(|setting| i32::from(setting.on))
        })
        .unwrap_or(-1)
}

unsafe extern "C" fn host_number(id: *const c_char, value: f64) -> f64 {
    let id = unsafe { c_text(id) };
    let Ok(mut settings) = settings_registry().lock() else {
        return 0.0;
    };
    let Some(setting) = settings.iter_mut().find(|setting| setting.id == id) else {
        return 0.0;
    };
    if value.is_finite() && setting.kind == SettingKind::Slider {
        setting.number = value.clamp(setting.min, setting.max);
    }
    setting.number
}

unsafe extern "C" fn host_set_number(id: *const c_char, value: f64) -> i32 {
    let id = unsafe { c_text(id) };
    if !value.is_finite() || set_setting_value(&id, value.to_string()).is_none() {
        return 0;
    }
    1
}

unsafe extern "C" fn host_choice_index(id: *const c_char) -> i32 {
    let id = unsafe { c_text(id) };
    settings_registry()
        .lock()
        .ok()
        .and_then(|settings| {
            settings
                .iter()
                .find(|setting| setting.id == id && setting.kind == SettingKind::Choice)
                .map(|setting| setting.index as i32)
        })
        .unwrap_or(-1)
}

unsafe extern "C" fn host_key_name(id: *const c_char, out: *mut c_char, capacity: usize) -> i32 {
    let id = unsafe { c_text(id) };
    write_text(
        settings_registry()
            .lock()
            .ok()
            .and_then(|settings| {
                settings
                    .iter()
                    .find(|setting| setting.id == id && setting.kind == SettingKind::Keybind)
                    .map(|setting| setting.key.clone())
            }),
        out,
        capacity,
    )
}

unsafe extern "C" fn host_set_enabled(id: *const c_char, on: u32) -> i32 {
    let id = unsafe { c_text(id) };
    set_feature_enabled(&id, on != 0);
    1
}

unsafe extern "C" fn host_take_event(out: *mut c_char, capacity: usize) -> i32 {
    let Some(mod_name) = current_mod() else {
        return 0;
    };
    let Ok(mut events) = event_queue().lock() else {
        return 0;
    };
    let Some(index) = events.iter().position(|event| event.mod_name == mod_name) else {
        return 0;
    };
    let event = events.remove(index);
    let line = format!("{}\t{}\t{}", event.kind, event.id, event.value);
    write_text(Some(line), out, capacity)
}

unsafe extern "C" fn host_config(key: *const c_char, out: *mut c_char, capacity: usize) -> i32 {
    let key = unsafe { c_text(key) };
    let Some(mod_name) = current_mod() else {
        return 0;
    };
    let full = format!("!{key}");
    let value = read_config(&mod_name)
        .into_iter()
        .find(|(saved, _)| saved == &full)
        .map(|(_, value)| value);
    write_text(value, out, capacity)
}

unsafe extern "C" fn host_set_config(key: *const c_char, value: *const c_char) -> i32 {
    let (key, value) = unsafe { (c_text(key), c_text(value)) };
    let Some(mod_name) = current_mod() else {
        return 0;
    };
    let mut values: Vec<(String, String)> = read_config(&mod_name)
        .into_iter()
        .filter(|(saved, _)| saved.starts_with('!'))
        .collect();
    let full = format!("!{key}");
    // A hand-edited file may repeat a key; keep exactly one.
    values.retain(|(saved, _)| saved != &full);
    values.push((full, value));
    save_config(&mod_name, &values);
    1
}


/// Writes `value` into a caller buffer, returning 1 when it fit and 0 otherwise.
fn write_text(value: Option<String>, out: *mut c_char, capacity: usize) -> i32 {
    let Some(value) = value else {
        return 0;
    };
    let bytes = value.as_bytes();
    if out.is_null() || capacity == 0 || bytes.len() + 1 > capacity {
        return 0;
    }
    // SAFETY: the caller provided `capacity` writable bytes and the value fits with its NUL.
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), out as *mut u8, bytes.len());
        *out.add(bytes.len()) = 0;
    }
    1
}

/// Every registered setting, for the ClickGUI.
#[must_use]
pub fn settings() -> Vec<Setting> {
    settings_registry()
        .lock()
        .map(|settings| {
            settings
                .iter()
                .map(|setting| Setting {
                    id: setting.id.clone(),
                    mod_name: setting.mod_name.clone(),
                    label: setting.label.clone(),
                    category: setting.category.clone(),
                    kind: setting.kind,
                    on: setting.on,
                    number: setting.number,
                    min: setting.min,
                    max: setting.max,
                    step: setting.step,
                    index: setting.index,
                    options: setting.options.clone(),
                    key: setting.key.clone(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Whether a scoped toggle id is on. Unknown ids read as off.
#[must_use]
pub fn feature_enabled(id: &str) -> bool {
    settings_registry()
        .lock()
        .map(|settings| {
            settings
                .iter()
                .any(|setting| setting.id == id && setting.on)
        })
        .unwrap_or(false)
}

/// Sets a toggle and queues the change for its mod.
pub fn set_feature_enabled(id: &str, enabled: bool) {
    let Ok(mut settings) = settings_registry().lock() else {
        return;
    };
    let Some(setting) = settings.iter_mut().find(|setting| setting.id == id) else {
        return;
    };
    if setting.on == enabled {
        return;
    }
    setting.on = enabled;
    let (mod_name, id) = (setting.mod_name.clone(), setting.id.clone());
    drop(settings);
    queue("toggle", &mod_name, &id, u8::from(enabled).to_string());
    save_setting(&mod_name);
}

/// Sets any setting's value from its config text form, queueing the matching event.
fn set_setting_value(id: &str, value: String) -> Option<String> {
    let Ok(mut settings) = settings_registry().lock() else {
        return None;
    };
    let setting = settings.iter_mut().find(|setting| setting.id == id)?;
    let before = setting.serialised();
    setting.apply(&value);
    let after = setting.serialised();
    let (mod_name, kind) = (setting.mod_name.clone(), setting.kind);
    drop(settings);
    if before == after && !matches!(kind, SettingKind::Button) {
        return Some(after);
    }
    let kind = match kind {
        SettingKind::Toggle => "toggle",
        SettingKind::Slider => "number",
        SettingKind::Choice => "choice",
        SettingKind::Keybind => "keychange",
        SettingKind::Button => "press",
    };
    queue(kind, &mod_name, id, after.clone());
    save_setting(&mod_name);
    Some(after)
}

/// Applies a ClickGUI interaction on a setting, as the menu's key loop does.
pub fn press_setting(id: &str) {
    let Ok(settings) = settings_registry().lock() else {
        return;
    };
    let Some(setting) = settings.iter().find(|setting| setting.id == id) else {
        return;
    };
    let (kind, on, index, options) = (
        setting.kind,
        setting.on,
        setting.index,
        setting.options.len(),
    );
    drop(settings);
    match kind {
        SettingKind::Toggle => set_feature_enabled(id, !on),
        SettingKind::Choice if options > 0 => {
            let _ = set_setting_value(id, ((index as usize + 1) % options).to_string());
        }
        SettingKind::Button => {
            let _ = set_setting_value(id, String::new());
        }
        _ => {}
    }
}

/// Records the keys held now: a keybind's press and release become events.
pub fn update_keys(pressed: &[String]) {
    let Ok(mut held) = pressed_keys().lock() else {
        return;
    };
    let mut events = Vec::new();
    for setting in settings() {
        if setting.kind != SettingKind::Keybind || setting.key.is_empty() {
            continue;
        }
        let key = setting.key.to_ascii_lowercase();
        let now = pressed.iter().any(|held| held == &key);
        let before = held.iter().any(|held| held == &key);
        if now != before {
            events.push((setting.mod_name.clone(), setting.id.clone(), now));
        }
    }
    *held = pressed.to_vec();
    drop(held);
    for (mod_name, id, down) in events {
        queue("keybind", &mod_name, &id, u8::from(down).to_string());
    }
}

/// The mods directory each mod's config file lives in.
pub fn set_config_dir(dir: &Path) {
    let _ = MODS_DIR.set(dir.to_owned());
}

pub fn config_dir() -> Option<PathBuf> {
    MODS_DIR.get().cloned()
}

fn config_path(mod_name: &str) -> Option<PathBuf> {
    Some(MODS_DIR.get()?.join(format!("{mod_name}.cfg")))
}

/// The mod's private key/value pairs from its config file.
fn read_config(mod_name: &str) -> Vec<(String, String)> {
    let Some(path) = config_path(mod_name) else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| {
            let (key, value) = line.split_once('\t')?;
            Some((key.to_owned(), value.replace("\\n", "\n")))
        })
        .collect()
}

fn save_config(mod_name: &str, values: &[(String, String)]) {
    let Some(path) = config_path(mod_name) else {
        return;
    };
    let mut out = String::new();
    for setting in settings().iter().filter(|s| s.mod_name == mod_name) {
        out.push_str(&format!("{}\t{}\n", setting.id, setting.serialised()));
    }
    for (key, value) in values {
        // The key already carries the `!` that marks it as a private value.
        out.push_str(&format!("{key}\t{}\n", value.replace('\n', "\\n")));
    }
    if let Err(error) = std::fs::write(&path, out) {
        eprintln!("[cinnaloader] cannot write {}: {error}", path.display());
    }
}

/// Writes one mod's settings back to its config file, keeping its private values.
fn save_setting(mod_name: &str) {
    let values: Vec<(String, String)> = read_config(mod_name)
        .into_iter()
        .filter(|(key, _)| key.starts_with('!'))
        .collect();
    save_config(mod_name, &values);
}

/// Applies a mod's saved settings once it has registered them.
fn load_settings(mod_name: &str) {
    let saved = read_config(mod_name);
    let Ok(mut settings) = settings_registry().lock() else {
        return;
    };
    for setting in settings.iter_mut().filter(|s| s.mod_name == mod_name) {
        if let Some((_, value)) = saved.iter().find(|(key, _)| key == &setting.id) {
            setting.apply(value);
        }
    }
}


impl Host {
    /// Loads every mod library under `dir`. A mod that fails to load is reported and
    /// skipped, so one bad file does not take the others down.
    pub fn load_dir(dir: &Path) -> Result<Self> {
        let dir_c = CString::new(dir.to_string_lossy().as_bytes())
            .context("mods directory is not a valid path")?;
        set_config_dir(dir);
        let mut host = Self {
            dir: dir_c,
            mods: Vec::new(),
        };
        // Nothing of a mod runs before the order is decided, so a dependency is always
        // loaded ahead of whatever needs it.
        let mut candidates = Vec::new();
        for entry in std::fs::read_dir(dir)
            .with_context(|| format!("reading mods directory {}", dir.display()))?
        {
            let path = entry?.path();
            if !is_library(&path) {
                continue;
            }
            match discover(&path) {
                Ok(candidate) => candidates.push(candidate),
                Err(error) => eprintln!("[cinnaloader] skipped {}: {error:#}", path.display()),
            }
        }
        let mut resolver = DependencyResolver::new();
        for candidate in &candidates {
            resolver.add(GraphNode::from_manifest(&candidate.name, &candidate.manifest));
            remember(
                &candidate.name,
                candidate.manifest.clone(),
                Some(candidate.path.clone()),
            );
        }
        let graph = resolver.resolve();
        record_resolution(
            graph.order.clone(),
            graph.errors.iter().map(ToString::to_string).collect(),
            graph.cycles.iter().map(|cycle| cycle.cycle.clone()).collect(),
        );
        for error in &graph.errors {
            eprintln!("[cinnaloader] {error}");
        }
        for cycle in &graph.cycles {
            eprintln!("[cinnaloader] {cycle}");
        }
        let mut refused: Vec<String> = graph.errors.iter().map(|error| error.mod_name.clone()).collect();
        refused.extend(graph.cycles.iter().flat_map(|cycle| cycle.cycle.clone()));
        for name in &refused {
            mark_refused(name);
        }
        for name in &graph.order {
            let Some(candidate) = candidates.iter().find(|entry| &entry.name == name) else {
                continue;
            };
            match host.load_one(&candidate.path) {
                Ok(name) => eprintln!("[cinnaloader] loaded {name} ({})", candidate.path.display()),
                Err(error) => {
                    eprintln!("[cinnaloader] failed {}: {error:#}", candidate.path.display());
                    mark_refused(&candidate.name);
                }
            }
        }
        let _ = MOD_NAMES.set(host.mods.iter().map(|module| module.name.clone()).collect());
        // Now that every mod is loaded, walk the phases that only make sense afterwards.
        for phase in [Phase::Initialize, Phase::PostInitialize, Phase::Ready] {
            host.walk(phase);
        }
        Ok(host)
    }

    /// The names of the loaded mods, in load order.
    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        self.mods.iter().map(|m| m.name.as_str()).collect()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.mods.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.mods.is_empty()
    }

    /// Advances every enabled mod's frame by `dt_millis`. A mod that panics is switched
    /// off and skipped from then on, so the others keep running.
    pub fn tick(&mut self, dt_millis: u32) {
        note_baseline(self.mods.len());
        for module in &self.mods {
            if !mod_enabled(&module.name) || mod_failed(&module.name) {
                continue;
            }
            if let Some(frame) = module.frame {
                enter_mod(&module.name);
                let code = frame(dt_millis);
                leave_mod();
                TICKS.fetch_add(1, Ordering::Relaxed);
                if code != 0 {
                    mark_failed(&module.name);
                }
            }
        }
    }

    /// Hands one lifecycle phase to every mod that answers it.
    fn walk(&mut self, phase: Phase) {
        for module in &self.mods {
            if !mod_enabled(&module.name) || mod_failed(&module.name) {
                continue;
            }
            let Some(phase_fn) = module.phase else {
                continue;
            };
            enter_mod(&module.name);
            let code = phase_fn(phase.code());
            leave_mod();
            if code != 0 {
                mark_failed(&module.name);
            }
        }
    }

    fn load_one(&mut self, path: &Path) -> Result<String> {
        let name = name_of(path);
        // SAFETY: loading arbitrary native code is the point of a mod loader; the
        // directory is user-controlled.
        let library =
            unsafe { Library::new(path) }.with_context(|| format!("opening {}", path.display()))?;
        let abi: Symbol<AbiFn> = unsafe { library.get(b"cinnaloader_abi_version\0") }
            .context("not a cinnaloader mod: missing `cinnaloader_abi_version`")?;
        if abi() != cinnaloader::ABI_VERSION {
            bail!(
                "ABI mismatch: mod built for {}, loader is {}",
                abi(),
                cinnaloader::ABI_VERSION
            );
        }
        let init: Symbol<InitFn> = unsafe { library.get(b"cinnaloader_init\0") }
            .context("not a cinnaloader mod: missing `cinnaloader_init`")?;
        let name_c = CString::new(name.as_str()).context("mod name is not a valid path")?;
        let api = Box::new(host_api(self.dir.as_ptr(), name_c.as_ptr()));
        // The discovery pass already recorded what this mod declared, and overwriting that
        // here would throw the version and dependencies away; only a load that skipped
        // discovery, as a test does, needs a placeholder.
        let unknown = records()
            .lock()
            .map(|records| !records.iter().any(|record| record.name == name))
            .unwrap_or(false);
        if unknown {
            // Every path a mod works with is relative to its own directory, and this is the one
        // place that decides what that is.
        if let Some(directory) = config_dir() {
            cinnaloader::fs::set_root(directory.join(&name));
        }
        remember(&name, ModManifest::new(&name, &name, "0"), Some(path.to_owned()));
        }
        entering_load(&name);
        enter_mod(&name);
        // SAFETY: the table outlives every mod call; every callback is `'static`.
        let code = unsafe { init(api.as_ref() as *const HostApi) };
        leave_mod();
        finished_loading(&name);
        if code != 0 {
            bail!("mod init failed with code {code}");
        }
        // The mod has declared its manifest by now, so its requirements can be checked.
        // A mod asking to be trusted is exactly the mod not to trust: the level is clamped,
        // and the attempt is written down.
        let claimed = declared_manifest(&name).trust;
        if claimed > cinnaloader::permissions::TrustLevel::Normal {
            eprintln!(
                "[cinnaloader] {name} asked for trust {}, which is the user's to give: clamped to normal",
                claimed.as_str()
            );
        }
        let report = cinnaloader::compat::check(&declared_manifest(&name), &host_facts());
        if !report.is_compatible() {
            bail!("incompatible: {}", report.failures().join("; "));
        }
        // The mod has registered its settings by now, so its saved values can apply.
        load_settings(&name);
        let frame = unsafe { library.get(b"cinnaloader_frame\0") }
            .ok()
            .map(|symbol| *symbol);
        let phase = unsafe { library.get(b"cinnaloader_phase\0") }
            .ok()
            .map(|symbol| *symbol);
        let shutdown = unsafe { library.get(b"cinnaloader_shutdown\0") }
            .ok()
            .map(|symbol| *symbol);
        self.mods.push(Loaded {
            name: name.clone(),
            _path: path.to_owned(),
            _library: library,
            _name: name_c,
            _api: api,
            frame,
            phase,
            shutdown,
        });
        Ok(name)
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        for module in &self.mods {
            if let Some(shutdown) = module.shutdown {
                enter_mod(&module.name);
                shutdown();
                leave_mod();
            }
        }
        let _ = &self.dir;
    }
}

/// One mod found in the directory, before any of it has run.
struct Candidate {
    name: String,
    manifest: ModManifest,
    path: PathBuf,
}

/// Reads what a library says about itself without running any of it: the ABI it was built
/// for and the manifest it declared at compile time.
fn discover(path: &Path) -> Result<Candidate> {
    let name = name_of(path);
    // SAFETY: loading arbitrary native code is the point of a mod loader; the directory is
    // user-controlled.
    let library =
        unsafe { Library::new(path) }.with_context(|| format!("opening {}", path.display()))?;
    let abi: Symbol<AbiFn> = unsafe { library.get(b"cinnaloader_abi_version\0") }
        .context("not a cinnaloader mod: missing `cinnaloader_abi_version`")?;
    if abi() != cinnaloader::ABI_VERSION {
        bail!(
            "ABI mismatch: mod built for {}, loader is {}",
            abi(),
            cinnaloader::ABI_VERSION
        );
    }
    let manifest = match unsafe { library.get::<ManifestFn>(b"cinnaloader_manifest\0") } {
        Ok(declared) => {
            let mut buffer = vec![0 as c_char; cinnaloader::modinfo::MAX_BYTES];
            // SAFETY: the mod writes at most `buffer.len()` bytes, and writes less when its
            // manifest does not fit.
            let written = unsafe { declared(buffer.as_mut_ptr(), buffer.len()) };
            if written <= 0 {
                ModManifest::new(&name, &name, "0")
            } else {
                // SAFETY: the mod NUL-terminated what it wrote.
                let text = unsafe { CStr::from_ptr(buffer.as_ptr()) }.to_string_lossy();
                ModManifest::decode(&text)
            }
        }
        Err(_) => ModManifest::new(&name, &name, "0"),
    };
    Ok(Candidate {
        name,
        manifest,
        path: path.to_owned(),
    })
}

/// The callback table one mod is handed. Every function is `'static`, so the table lives as
/// long as the mod does.
fn host_api(dir: *const c_char, name: *const c_char) -> HostApi {
    HostApi {
        abi_version: cinnaloader::ABI_VERSION,
        log: Some(host_log),
        mods_dir: dir,
        overlay_line: Some(host_overlay_line),
        toggle_gui: Some(host_toggle_gui),
        mod_name: name,
        feature: Some(host_feature),
        slider: Some(host_slider),
        choice: Some(host_choice),
        keybind: Some(host_keybind),
        button: Some(host_button),
        enabled: Some(host_enabled),
        set_enabled: Some(host_set_enabled),
        number: Some(host_number),
        set_number: Some(host_set_number),
        choice_index: Some(host_choice_index),
        key_name: Some(host_key_name),
        take_event: Some(host_take_event),
        config: Some(host_config),
        set_config: Some(host_set_config),
        declare: Some(host_declare),
        report: Some(host_report),
        graph: Some(host_graph),
        mods_command: Some(host_mods_command),
        observe: Some(host_observe),
    }
}

fn is_library(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
        return false;
    };
    matches!(ext, "so" | "dylib" | "dll")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The registries are process-wide, so tests that touch them run one at a time.
    static SERIAL: Mutex<()> = Mutex::new(());

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        SERIAL.lock().unwrap_or_else(|error| error.into_inner())
    }

    #[test]
    fn a_directory_without_mods_is_empty_not_an_error() {
        let _serial = serial();
        let dir = std::env::temp_dir().join(format!("cinnaloader-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let host = Host::load_dir(&dir).unwrap();
        assert!(host.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn non_library_files_are_ignored() {
        let _serial = serial();
        let dir = std::env::temp_dir().join(format!("cinnaloader-noise-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("readme.txt"), b"not a mod").unwrap();
        let host = Host::load_dir(&dir).unwrap();
        assert!(host.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_mod_can_be_disabled_and_enabled_by_name() {
        let _serial = serial();
        let name = format!("cinnaloader-toggle-{}", std::process::id());
        assert!(mod_enabled(&name), "mods start enabled");
        let _ = take_queued(&name);
        set_mod_enabled(&name, false);
        assert!(!mod_enabled(&name));
        // The mod is told, so its own bus can say `mod_disabled` without polling.
        assert_eq!(take_queued(&name), vec![format!("disabled\t{name}\t")]);
        set_mod_enabled(&name, true);
        assert!(mod_enabled(&name));
        assert_eq!(take_queued(&name), vec![format!("enabled\t{name}\t")]);
    }

    /// Drains the events queued for one mod, as the mod's own tick would.
    fn take_queued(name: &str) -> Vec<String> {
        enter_mod(name);
        let mut buffer = [0 as c_char; 256];
        let mut seen = Vec::new();
        while unsafe { host_take_event(buffer.as_mut_ptr(), buffer.len()) } == 1 {
            // SAFETY: the host wrote a NUL-terminated event line.
            seen.push(unsafe { CStr::from_ptr(buffer.as_ptr()) }.to_string_lossy().into_owned());
        }
        leave_mod();
        seen
    }

    #[test]
    fn a_feature_is_scoped_queued_and_drained() {
        let _serial = serial();
        let name = format!("cinnaloader-feature-{}", std::process::id());
        let scoped = CString::new(format!("{name}:sprint")).unwrap();
        let label = CString::new("Sprint").unwrap();
        let category = CString::new("Movement").unwrap();

        enter_mod(&name);
        let registered =
            unsafe { host_feature(scoped.as_ptr(), label.as_ptr(), category.as_ptr(), 1) };
        let duplicate = unsafe { host_feature(scoped.as_ptr(), label.as_ptr(), category.as_ptr(), 0) };
        leave_mod();
        assert_eq!(registered, 1, "a setting registers for the mod that is loading");
        assert_eq!(duplicate, 0, "a taken id is refused");
        assert!(settings().iter().any(|setting| setting.id == scoped.to_string_lossy()));
        assert!(feature_enabled(&scoped.to_string_lossy()));

        set_feature_enabled(&scoped.to_string_lossy(), false);
        assert!(!feature_enabled(&scoped.to_string_lossy()));

        enter_mod(&name);
        let mut buffer = [0 as c_char; 128];
        assert_eq!(unsafe { host_take_event(buffer.as_mut_ptr(), buffer.len()) }, 1);
        // SAFETY: the host wrote a NUL-terminated event line.
        let taken = unsafe { CStr::from_ptr(buffer.as_ptr()) }.to_str().unwrap();
        let mut parts = taken.split('\t');
        assert_eq!(parts.next(), Some("toggle"));
        assert_eq!(parts.next(), Some(scoped.to_string_lossy().as_ref()));
        assert_eq!(parts.next(), Some("0"));
        assert_eq!(unsafe { host_take_event(buffer.as_mut_ptr(), buffer.len()) }, 0);
        leave_mod();
    }

    #[test]
    fn a_slider_clamps_and_reports_its_value() {
        let _serial = serial();
        let name = format!("cinnaloader-slider-{}", std::process::id());
        let scoped = format!("{name}:speed");
        let id = CString::new(scoped.clone()).unwrap();
        let label = CString::new("Speed").unwrap();
        let category = CString::new("Movement").unwrap();
        enter_mod(&name);
        assert_eq!(
            unsafe { host_slider(id.as_ptr(), label.as_ptr(), category.as_ptr(), 2.0, 0.0, 10.0, 0.5) },
            1
        );
        leave_mod();
        assert_eq!(unsafe { host_number(id.as_ptr(), f64::NAN) }, 2.0);
        set_setting_value(&scoped, "99".to_owned());
        assert_eq!(unsafe { host_number(id.as_ptr(), f64::NAN) }, 10.0, "clamped to max");
        set_setting_value(&scoped, "-5".to_owned());
        assert_eq!(unsafe { host_number(id.as_ptr(), f64::NAN) }, 0.0, "clamped to min");
    }

    #[test]
    fn a_declared_manifest_comes_back_with_the_loaders_facts() {
        let _serial = serial();
        let name = format!("cinnaloader-manifest-{}", std::process::id());
        entering_load(&name);
        enter_mod(&name);
        let text = CString::new(
            "v=1\nid=demo\nname=Demo\nversion=2.1.0\nauthor=me\ntag=test\nlicense=MIT\n",
        )
        .unwrap();
        assert_eq!(unsafe { host_declare(text.as_ptr()) }, 1);
        let mut buffer = [0 as c_char; cinnaloader::modinfo::MAX_BYTES * 2];
        assert_eq!(unsafe { host_report(buffer.as_mut_ptr(), buffer.len()) }, 1);
        leave_mod();
        finished_loading(&name);

        // SAFETY: the loader wrote a NUL-terminated report.
        let info = ModInfo::decode(unsafe { CStr::from_ptr(buffer.as_ptr()) }.to_str().unwrap());
        assert_eq!(info.manifest.name, "Demo");
        assert_eq!(info.manifest.version.major(), Some(2));
        assert_eq!(info.manifest.authors, vec!["me".to_owned()]);
        assert_eq!(info.manifest.license.as_deref(), Some("MIT"));
        assert_eq!(info.mod_name, name, "the loader keeps its own identity for the mod");
        assert_eq!(info.state, State::Loading, "asked during load");
        assert_eq!(info.loader_version, LOADER_VERSION);

        set_mod_enabled(&name, false);
        enter_mod(&name);
        assert_eq!(unsafe { host_report(buffer.as_mut_ptr(), buffer.len()) }, 1);
        leave_mod();
        // SAFETY: the loader wrote a NUL-terminated report.
        let info = ModInfo::decode(unsafe { CStr::from_ptr(buffer.as_ptr()) }.to_str().unwrap());
        assert_eq!(info.state, State::Disabled);
        assert!(!info.enabled);
        set_mod_enabled(&name, true);
    }

    #[test]
    fn a_failed_mod_is_reported_but_still_enabled() {
        let _serial = serial();
        let name = format!("cinnaloader-failed-{}", std::process::id());
        remember(&name, ModManifest::new(&name, &name, "1.0"), None);
        assert!(!mod_failed(&name));
        mark_failed(&name);
        assert!(mod_failed(&name), "a panic switches the mod off");
        let info = mod_info(&name);
        assert_eq!(info.state, State::Failed);
        assert!(info.enabled, "failed is its own state, not a user toggle");
    }

    #[test]
    fn a_mod_identity_drops_the_lib_prefix() {
        assert_eq!(name_of(Path::new("/mods/libhello_mod.so")), "hello_mod");
        assert_eq!(name_of(Path::new("/mods/hello.dylib")), "hello");
        assert_eq!(name_of(Path::new("/mods/hello.dll")), "hello");
    }

    #[test]
    fn a_saved_setting_is_applied_when_the_mod_registers_it() {
        let _serial = serial();
        let name = format!("cinnaloader-persist-{}", std::process::id());
        let scoped = format!("{name}:speed");
        let dir = std::env::temp_dir().join(format!("cinnaloader-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        set_config_dir(&dir);
        let dir = config_dir().expect("a config directory");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(format!("{name}.cfg"));
        std::fs::write(&file, format!("{scoped}\t7.5\n!greeting\thello\n")).unwrap();

        let id = CString::new(scoped.clone()).unwrap();
        let label = CString::new("Speed").unwrap();
        let category = CString::new("Movement").unwrap();
        enter_mod(&name);
        let registered =
            unsafe { host_slider(id.as_ptr(), label.as_ptr(), category.as_ptr(), 1.0, 0.0, 10.0, 0.5) };
        let config_value = {
            let key = CString::new("greeting").unwrap();
            let mut buffer = [0 as c_char; 64];
            let ok = unsafe { host_config(key.as_ptr(), buffer.as_mut_ptr(), buffer.len()) };
            (ok, unsafe { CStr::from_ptr(buffer.as_ptr()) }.to_string_lossy().into_owned())
        };
        leave_mod();
        assert_eq!(registered, 1);
        assert_eq!(config_value, (1, "hello".to_owned()));

        load_settings(&name);
        assert_eq!(
            unsafe { host_number(id.as_ptr(), f64::NAN) },
            7.5,
            "the saved value replaces the default"
        );
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn a_keybind_queues_press_and_release_once() {
        let _serial = serial();
        let name = format!("cinnaloader-key-{}", std::process::id());
        let scoped = format!("{name}:panic");
        let id = CString::new(scoped).unwrap();
        let label = CString::new("Panic").unwrap();
        let category = CString::new("Debug").unwrap();
        let key = CString::new("P").unwrap();
        enter_mod(&name);
        assert_eq!(
            unsafe { host_keybind(id.as_ptr(), label.as_ptr(), category.as_ptr(), key.as_ptr()) },
            1
        );
        leave_mod();

        update_keys(&["p".to_owned()]);
        update_keys(&["p".to_owned()]);
        update_keys(&[]);
        enter_mod(&name);
        let mut buffer = [0 as c_char; 128];
        let mut seen = Vec::new();
        while unsafe { host_take_event(buffer.as_mut_ptr(), buffer.len()) } == 1 {
            // SAFETY: the host wrote a NUL-terminated event line.
            seen.push(unsafe { CStr::from_ptr(buffer.as_ptr()) }.to_string_lossy().into_owned());
        }
        leave_mod();
        assert_eq!(seen.len(), 2, "one press and one release: {seen:?}");
        assert!(
            seen[0].starts_with("keybind\t"),
            "the SDK matches on this kind: {seen:?}"
        );
        assert!(seen[0].ends_with("\t1"));
        assert!(seen[1].ends_with("\t0"));
    }

    #[test]
    fn rebinding_a_key_queues_keychange_not_a_press() {
        let _serial = serial();
        let name = format!("cinnaloader-keychange-{}", std::process::id());
        let scoped = format!("{name}:panic");
        let id = CString::new(scoped.clone()).unwrap();
        let label = CString::new("Panic").unwrap();
        let category = CString::new("Debug").unwrap();
        let key = CString::new("P").unwrap();
        enter_mod(&name);
        assert_eq!(
            unsafe { host_keybind(id.as_ptr(), label.as_ptr(), category.as_ptr(), key.as_ptr()) },
            1
        );
        leave_mod();

        let _ = set_setting_value(&scoped, "G".to_owned());
        enter_mod(&name);
        let mut buffer = [0 as c_char; 128];
        let mut seen = Vec::new();
        while unsafe { host_take_event(buffer.as_mut_ptr(), buffer.len()) } == 1 {
            // SAFETY: the host wrote a NUL-terminated event line.
            seen.push(unsafe { CStr::from_ptr(buffer.as_ptr()) }.to_string_lossy().into_owned());
        }
        leave_mod();
        assert_eq!(seen.len(), 1, "a rebind is one event: {seen:?}");
        assert!(seen[0].starts_with("keychange\t"), "{seen:?}");
        assert!(seen[0].ends_with("\tG"), "{seen:?}");
    }

    #[test]
    fn the_capability_table_the_loader_writes_is_what_the_sdk_answers_with() {
        let _serial = serial();
        remember(
            "core_lib",
            ModManifest::new("core_lib", "Core", "1.2")
                .capability(cinnaloader::capability::Capability::Render),
            None,
        );
        let dir = CString::new("/tmp").unwrap();
        let name = CString::new("core_lib").unwrap();
        let api = host_api(dir.as_ptr(), name.as_ptr());
        // SAFETY: the table outlives the calls below and every slot is a loader callback.
        assert_eq!(unsafe { cinnaloader::install(&api) }, 0);

        use cinnaloader::capability::{self as capabilities, Capability as Cap};
        assert!(capabilities::supported(Cap::Render));
        assert!(capabilities::supported(Cap::FilesystemRead));
        assert!(!capabilities::supported(Cap::WorldWrite));
        assert_eq!(
            capabilities::why_unsupported(Cap::WorldWrite),
            "needs hooks into the client's world and entity state"
        );
        assert_eq!(
            cinnaloader::mods::capabilities_of("core_lib"),
            vec!["render".to_owned()]
        );
        assert_eq!(capabilities::provided_by_id(Cap::Render), vec!["core_lib"]);
        assert_eq!(capabilities::list().len(), 38);
    }

    #[test]
    fn the_graph_the_loader_writes_is_the_graph_the_sdk_reads() {
        let _serial = serial();
        remember(
            "core_lib",
            ModManifest::new("core_lib", "Core", "1.2")
                .provides("core_lib")
                .depends_on("hello_mod"),
            None,
        );
        remember(
            "hello_mod",
            ModManifest::new("hello_mod", "Hello", "0.1")
                .depends_on_versioned("core_lib", ">=1.0"),
            None,
        );
        record_resolution(
            vec!["core_lib".to_owned(), "hello_mod".to_owned()],
            vec!["hello_mod needs missing_lib, which is not installed".to_owned()],
            vec![vec!["a".to_owned(), "b".to_owned()]],
        );
        let dir = CString::new("/tmp").unwrap();
        let name = CString::new("hello_mod").unwrap();
        let api = host_api(dir.as_ptr(), name.as_ptr());
        // SAFETY: the table is alive for the rest of the test and every slot is a real
        // loader callback.
        assert_eq!(unsafe { cinnaloader::install(&api) }, 0);

        let registry = cinnaloader::mods::registry();
        let core = cinnaloader::mods::get_by_id("core_lib").expect("core_lib is installed");
        assert_eq!(core.version.to_string(), "1.2");
        assert_eq!(core.name, "core_lib");
        assert_eq!(registry.order, vec!["core_lib", "hello_mod"]);
        assert_eq!(
            cinnaloader::mods::get_dependencies("hello_mod"),
            vec!["core_lib"]
        );
        assert_eq!(
            cinnaloader::mods::get_dependents("core_lib"),
            vec!["hello_mod"]
        );
        assert!(cinnaloader::mods::get_load_errors()[0].contains("missing_lib"));
        assert_eq!(registry.cycles.len(), 1);
        assert!(cinnaloader::mods::explain_load_order()[1].starts_with("hello_mod after"));
        assert!(!cinnaloader::mods::validate_dependency_graph().is_empty());
    }

    #[test]
    fn the_live_observation_the_loader_writes_is_what_the_sdk_reads() {
        let _serial = serial();
        set_observation("frame\t7\nticks\t9\nmods\t1\nphase\ttick");
        let dir = CString::new("/tmp").unwrap();
        let name = CString::new("hello_mod").unwrap();
        let api = host_api(dir.as_ptr(), name.as_ptr());
        // SAFETY: the table outlives the calls below and every slot is a loader callback.
        assert_eq!(unsafe { cinnaloader::install(&api) }, 0);

        let observation = cinnaloader::live::observation();
        assert!(observation.contains("frame\t7"), "{observation:?}");
        assert_eq!(cinnaloader::live::value("frame"), Some("7".to_owned()));
        assert_eq!(cinnaloader::live::value("missing"), None);
        assert_eq!(cinnaloader::live::u64("frame"), Some(7));
        assert_eq!(cinnaloader::live::text("phase"), Some("tick".to_owned()));
        assert_eq!(cinnaloader::live::f32("missing"), None);
    }
}
